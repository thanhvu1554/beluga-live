//! End-to-end tests: a fake phone talks to the real server (mock brain) over WebSocket.

use base64::Engine;
use beluga_server::config::Config;
use beluga_server::http::start_with;
use beluga_server::voice::{FakeVoice, Voice};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CODE: &str = "424242";

struct Harness {
    addr: SocketAddr,
    dir: std::path::PathBuf,
}

fn temp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("beluga-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn harness_with(voice: Arc<Voice>, delay_ms: u64, capture_ms: u64) -> Harness {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    std::fs::write(dir.join("board").join("version.json"), r#"{"version":7}"#).unwrap();
    let cfg = Config {
        port: 0,
        mock: true,
        mock_delay_ms: delay_ms,
        capture_timeout_ms: capture_ms,
        data_dir: dir.join("data"),
        board_dir: dir.join("board"),
        tutor_dir: dir.join("tutor"),
        claude_bin: "claude".into(),
        claude_effort: None,
        claude_model_map: vec![],
        eleven: None,
        genvoice: None,
        tts_provider: "auto".into(),
        stt_provider: "auto".into(),
        tts_base_ms: 8000,
        tts_per_char_ms: 60,
        stt_base_ms: 8000,
        stt_per_sec_ms: 1500,
        fixed_pair_code: Some(CODE.into()),
        models_path: std::path::PathBuf::from("no-models.json"),
        secrets: Default::default(),
        compact_tokens: 120_000,
        util_in_mock: false,
        voices_path: std::path::PathBuf::from("no-voices.json"),
        voice_clips_dir: std::path::PathBuf::from("no-voices"),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_with(cfg, listener, voice).await;
    });
    Harness { addr, dir }
}

async fn harness() -> Harness {
    harness_with(Voice::none(), 5, 8000).await
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Phone {
    ws: Ws,
}

impl Phone {
    async fn connect(h: &Harness) -> Phone {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", h.addr)).await.unwrap();
        Phone { ws }
    }

    async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    /// Next JSON frame, or None on close / timeout.
    async fn next(&mut self) -> Option<Value> {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).ok(),
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(_))) => continue,
                Err(_) => return None,
            }
        }
    }

    async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(f) = self.next().await {
            let done = pred(&f);
            out.push(f);
            if done {
                return out;
            }
        }
        panic!("stream ended before condition; got {out:#?}");
    }

    async fn hello(&mut self, extra: Value) -> Value {
        let mut h = json!({"t":"hello","v":1,"device":"test-phone","settings":{"lang":"vi","model":"sonnet","rigor":"medium"}});
        for (k, v) in extra.as_object().unwrap() {
            h[k] = v.clone();
        }
        self.send(h).await;
        self.next().await.expect("ready")
    }
}

fn kinds(frames: &[Value]) -> Vec<String> {
    frames.iter().map(|f| f["t"].as_str().unwrap_or("?").to_string()).collect()
}

fn seqs(frames: &[Value]) -> Vec<u64> {
    frames.iter().filter_map(|f| f["seq"].as_u64()).collect()
}

fn is_turn_end(f: &Value) -> bool {
    f["t"] == "turn_end"
}

#[tokio::test]
async fn health_and_board_are_served() {
    let h = harness().await;
    let r: Value = reqwest::get(format!("http://{}/health", h.addr)).await.unwrap().json().await.unwrap();
    assert_eq!(r, json!({"ok": true, "v": 3, "boardVersion": 7}), "PROTOCOL v3: health reports v 3");
    let body = reqwest::get(format!("http://{}/board/version.json", h.addr)).await.unwrap().text().await.unwrap();
    assert!(body.contains("\"version\":7"));
}

#[tokio::test]
async fn wrong_pair_code_is_fatal_error() {
    let h = harness().await;
    let mut p = Phone::connect(&h).await;
    let f = p.hello(json!({"pairCode":"000000"})).await;
    assert_eq!(f["t"], "error");
    assert_eq!(f["code"], "bad_pair");
    assert_eq!(f["fatal"], true);
    let mut p2 = Phone::connect(&h).await;
    let f = p2.hello(json!({"pairToken":"not-a-token"})).await;
    assert_eq!(f["code"], "bad_pair");
}

#[tokio::test]
async fn pair_code_yields_token_which_works_next_time() {
    let h = harness().await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    assert_eq!(ready["t"], "ready");
    assert_eq!(ready["v"], 1);
    assert_eq!(ready["tts"], "device");
    assert_eq!(ready["stt"], "device");
    assert_eq!(ready["boardVersion"], 7);
    let token = ready["pairToken"].as_str().unwrap().to_string();
    assert!(token.len() >= 16);
    let mut p2 = Phone::connect(&h).await;
    let ready2 = p2.hello(json!({"pairToken":token})).await;
    assert_eq!(ready2["t"], "ready");
    assert!(ready2.get("pairToken").is_none());
}

#[tokio::test]
async fn full_lesson_flow_in_seq_order() {
    let h = harness().await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();

    p.send(json!({"t":"user_text","text":"giảng bài này đi"})).await;
    let turn1 = p.until(is_turn_end).await;
    let content: Vec<String> = kinds(&turn1).into_iter().filter(|k| k != "state").collect();
    assert_eq!(content, ["user", "title", "say", "step", "show", "say", "ask", "turn_end"]);
    assert_eq!(seqs(&turn1), vec![1, 2, 3, 4, 5, 6, 7]);
    let echo = turn1.iter().find(|f| f["t"] == "user").unwrap();
    assert_eq!((echo["text"].as_str(), echo["via"].as_str(), echo["turn"].as_u64()), (Some("giảng bài này đi"), Some("text"), Some(1)));
    let title = turn1.iter().find(|f| f["t"] == "title").unwrap();
    let ttext = title["text"].as_str().unwrap();
    assert!(ttext.starts_with("Toán · Bất phương trình bậc hai · "), "{ttext}");
    assert_eq!(turn1.iter().find(|f| f["t"] == "ask").unwrap()["kind"], "next");
    assert_eq!(turn1.iter().find(|f| f["t"] == "show").unwrap()["block"]["type"], "math");
    assert!(turn1.contains(&json!({"t":"state","state":"thinking"})));

    // The student answers; the tutor finishes the lesson. seq and step index continue.
    p.send(json!({"t":"played","seq":7})).await;
    p.send(json!({"t":"user_text","text":"tìm nghiệm bằng delta"})).await;
    let turn2 = p.until(is_turn_end).await;
    let content: Vec<String> = kinds(&turn2).into_iter().filter(|k| k != "state").collect();
    assert_eq!(content, ["user", "say", "step", "show", "say", "done", "turn_end"]);
    assert_eq!(seqs(&turn2), vec![8, 9, 10, 11, 12, 13, 14]);
    let step = turn2.iter().find(|f| f["t"] == "step").unwrap();
    assert_eq!(step["index"], 1);
    assert_eq!(turn2.iter().find(|f| f["t"] == "show").unwrap()["step"], 1);
    assert!(turn2.iter().filter(|f| f["t"] == "say").all(|f| f["turn"] == 2));

    // Session list shows it, with title and date.
    p.send(json!({"t":"list_sessions"})).await;
    let list = p.until(|f| f["t"] == "sessions").await.pop().unwrap();
    let item = &list["items"][0];
    assert_eq!(item["sessionId"], sid);
    assert_eq!(item["title"], title["text"]);
    assert!(item["updatedAt"].as_str().unwrap().len() > 10);
}

#[tokio::test]
async fn reconnect_with_last_seq_gets_snapshot_and_turn_continues() {
    let h = harness_with(Voice::none(), 30, 8000).await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();
    let token = ready["pairToken"].as_str().unwrap().to_string();

    p.send(json!({"t":"user_text","text":"giảng bài dài"})).await;
    // Take three says, then the phone "loses the network".
    let mut got = Vec::new();
    while got.iter().filter(|f: &&Value| f["t"] == "say").count() < 3 {
        got.push(p.next().await.unwrap());
    }
    let last_seq = *seqs(&got).last().unwrap();
    drop(p);

    // The turn keeps running without a phone. Reconnect later.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut p2 = Phone::connect(&h).await;
    let ready2 = p2.hello(json!({"pairToken":token,"sessionId":sid,"lastSeq":last_seq})).await;
    assert_eq!(ready2["sessionId"], sid);
    let rest = p2.until(|f| f["t"] == "snapshot").await;
    let snap = rest.last().unwrap();
    let events = snap["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert_eq!(events[0]["seq"].as_u64().unwrap(), last_seq + 1);
    // every event strictly after lastSeq, contiguous
    let s: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(s, (last_seq + 1..last_seq + 1 + s.len() as u64).collect::<Vec<_>>());
    // Live frames after the snapshot (reconnected notice at least) and no duplicates of old seqs.
    let live = p2.until(|f| f["t"] == "notice" && f["code"] == "reconnected").await;
    assert!(live.last().is_some());
    // The rest of the turn arrives live, strictly after what the snapshot covered.
    let max_snap = *s.last().unwrap();
    let tail = p2.until(is_turn_end).await;
    assert!(seqs(&tail).iter().all(|q| *q > max_snap));

    // Opening the session fresh rebuilds the whole board (seq from 1).
    p2.send(json!({"t":"open_session","sessionId":sid})).await;
    let frames = p2.until(|f| f["t"] == "snapshot").await;
    assert!(frames.iter().any(|f| f["t"] == "ready"));
    let all = frames.last().unwrap()["events"].as_array().unwrap().clone();
    assert_eq!(all[0]["seq"], 1);
    assert!(all.iter().any(|e| e["t"] == "turn_end"));
    assert!(all.iter().all(|e| e["t"] != "say" || e["audio"] == false));
}

#[tokio::test]
async fn interrupt_stops_turn_and_next_turn_works() {
    let h = harness_with(Voice::none(), 40, 8000).await;
    let mut p = Phone::connect(&h).await;
    p.hello(json!({"pairCode":CODE})).await;
    p.send(json!({"t":"user_text","text":"giảng bài dài"})).await;
    let mut said = 0;
    let mut last_say = 0;
    while said < 3 {
        let f = p.next().await.unwrap();
        if f["t"] == "say" {
            said += 1;
            last_say = f["seq"].as_u64().unwrap();
        }
    }
    p.send(json!({"t":"played","seq":last_say})).await;
    p.send(json!({"t":"interrupt"})).await;
    let after = p.until(is_turn_end).await;
    let end = after.last().unwrap();
    assert_eq!(end["interrupted"], true);
    assert_eq!(end["turn"], 1);
    // No more turn-1 content leaks after turn_end.
    p.send(json!({"t":"user_text","text":"xong rồi, cho tớ đáp án"})).await;
    let next = p.until(|f| f["t"] == "turn_end" && f["turn"] == 2).await;
    for f in &next {
        if f["t"] == "say" || f["t"] == "step" || f["t"] == "show" || f["t"] == "done" {
            assert_eq!(f["turn"], 2, "{f}");
        }
    }
    assert!(next.iter().any(|f| f["t"] == "done"));
    assert!(next.last().unwrap().get("interrupted").is_none());
}

#[tokio::test]
async fn capture_request_photo_roundtrip_and_file_saved() {
    let h = harness().await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();
    p.send(json!({"t":"user_text","text":"chụp đề giúp tớ"})).await;
    let frames = p.until(|f| f["t"] == "capture_request").await;
    assert!(frames.iter().any(|f| f["t"] == "state" && f["state"] == "tool" && f["tool"] == "capture"));
    let rid = frames.last().unwrap()["requestId"].as_str().unwrap().to_string();
    let jpeg = base64::engine::general_purpose::STANDARD.encode([0xFFu8, 0xD8, 0xFF, 0xD9]);
    p.send(json!({"t":"photo","requestId":rid,"jpeg":jpeg,"w":1,"h":1})).await;
    let rest = p.until(is_turn_end).await;
    let says: Vec<&str> = rest.iter().filter(|f| f["t"] == "say").map(|f| f["text"].as_str().unwrap()).collect();
    assert!(says.contains(&"Tớ thấy đề rồi."), "{says:?}");
    let photos = h.dir.join("tutor").join("photos").join(&sid);
    let n = std::fs::read_dir(photos).unwrap().count();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn capture_times_out_with_text_error() {
    let h = harness_with(Voice::none(), 5, 150).await;
    let mut p = Phone::connect(&h).await;
    p.hello(json!({"pairCode":CODE})).await;
    p.send(json!({"t":"user_text","text":"chụp đề"})).await;
    p.until(|f| f["t"] == "capture_request").await;
    let rest = p.until(is_turn_end).await;
    let says: Vec<&str> = rest.iter().filter(|f| f["t"] == "say").map(|f| f["text"].as_str().unwrap()).collect();
    assert!(says.contains(&"Tớ chưa chụp được ảnh."), "{says:?}");
}

#[tokio::test]
async fn tts_fallback_when_probe_fails() {
    let voice = Arc::new(Voice::build(Some(Arc::new(FakeVoice { fail_status: Some(402), transcript: String::new() })), None, 3000));
    let h = harness_with(voice, 5, 8000).await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    assert_eq!(ready["tts"], "device");
    let notice = p.next().await.unwrap();
    assert_eq!(notice["t"], "notice");
    assert_eq!(notice["code"], "tts_fallback");
    p.send(json!({"t":"user_text","text":"hi"})).await;
    let frames = p.until(is_turn_end).await;
    assert!(frames.iter().filter(|f| f["t"] == "say").all(|f| f["audio"] == false));
    // Not announced a second time to a later phone.
    let mut p2 = Phone::connect(&h).await;
    p2.hello(json!({"pairCode":CODE})).await;
    p2.send(json!({"t":"list_sessions"})).await;
    assert_eq!(p2.next().await.unwrap()["t"], "sessions");
}

#[tokio::test]
async fn tts_audio_frames_follow_say_when_enabled() {
    let voice = Arc::new(Voice::build(Some(Arc::new(FakeVoice { fail_status: None, transcript: String::new() })), None, 3000));
    let h = harness_with(voice, 5, 8000).await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    assert_eq!(ready["tts"], "server");
    p.send(json!({"t":"user_text","text":"hi"})).await;
    let mut frames = p.until(is_turn_end).await;
    // audio frames may arrive after turn_end; collect a little more
    while let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(300), p.next()).await {
        frames.push(f);
    }
    let say_seqs: Vec<u64> = frames.iter().filter(|f| f["t"] == "say").map(|f| f["seq"].as_u64().unwrap()).collect();
    assert!(frames.iter().filter(|f| f["t"] == "say").all(|f| f["audio"] == true));
    for s in say_seqs {
        let a = frames.iter().find(|f| f["t"] == "audio" && f["seq"] == s).expect("audio for say");
        assert_eq!(a["mime"], "audio/mpeg");
        assert!(!a["data"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn server_stt_turns_audio_into_heard_and_a_turn() {
    let voice = Arc::new(Voice::build(
        None,
        Some(Arc::new(FakeVoice { fail_status: None, transcript: "giảng bài này đi".into() })),
        3000,
    ));
    let h = harness_with(voice, 5, 8000).await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    assert_eq!(ready["stt"], "server");
    p.send(json!({"t":"audio_start","mime":"audio/wav","sampleRate":16000})).await;
    p.ws.send(Message::Binary(vec![0u8; 100].into())).await.unwrap();
    p.send(json!({"t":"audio_end"})).await;
    let frames = p.until(is_turn_end).await;
    let heard = frames.iter().find(|f| f["t"] == "heard").expect("heard");
    assert_eq!(heard["text"], "giảng bài này đi");
}

#[tokio::test]
async fn mcp_route_serves_tool_over_http_on_loopback() {
    let h = harness().await;
    let mut p = Phone::connect(&h).await;
    let ready = p.hello(json!({"pairCode":CODE})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();
    let c = reqwest::Client::new();
    let url = format!("http://{}/mcp/{}", h.addr, sid);
    let r: Value = c
        .post(&url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["result"]["tools"][0]["name"], "capture_photo");

    // Call the tool while the phone answers the capture request.
    let call = tokio::spawn({
        let c = c.clone();
        let url = url.clone();
        async move {
            c.post(&url)
                .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"capture_photo","arguments":{}}}))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    });
    let req = p.until(|f| f["t"] == "capture_request").await.pop().unwrap();
    let jpeg = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
    p.send(json!({"t":"photo","requestId":req["requestId"],"jpeg":jpeg,"w":1,"h":1})).await;
    let r = call.await.unwrap();
    assert_eq!(r["result"]["content"][0]["type"], "image");
    assert_eq!(r["result"]["content"][0]["data"], jpeg);
}
