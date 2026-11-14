//! Manual smoke test against the REAL Claude Code CLI (spends plan quota, so it is `#[ignore]`).
//!
//! Run: `BELUGA_TEST_JPEG=<path to a jpeg> cargo test --test real_claude -- --ignored --nocapture`
//! Optional: `BELUGA_TEST_MODEL` (default haiku).

use base64::Engine;
use beluga_server::config::Config;
use beluga_server::http::start_with;
use beluga_server::voice::Voice;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
#[ignore]
async fn real_claude_turn_with_capture_photo_tool() {
    let jpeg_path = std::env::var("BELUGA_TEST_JPEG").expect("BELUGA_TEST_JPEG");
    let jpeg = std::fs::read(jpeg_path).unwrap();
    let model = std::env::var("BELUGA_TEST_MODEL").unwrap_or_else(|_| "haiku".into());
    let root = beluga_server::config::find_repo_root();
    let tmp = std::env::temp_dir().join(format!("beluga-real-{}", uuid::Uuid::new_v4()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Config {
        port: 0,
        mock: false,
        mock_delay_ms: 0,
        capture_timeout_ms: 8000,
        data_dir: tmp.join("data"),
        board_dir: root.join("shared").join("board"),
        tutor_dir: root.join("tutor"),
        claude_bin: "claude".into(),
        claude_effort: std::env::var("BELUGA_EFFORT").ok(),
        eleven: None,
        genvoice: None,
        tts_provider: "auto".into(),
        stt_provider: "auto".into(),
        tts_base_ms: 8000,
        tts_per_char_ms: 60,
        stt_base_ms: 8000,
        stt_per_sec_ms: 1500,
        fixed_pair_code: Some("111111".into()),
    };
    tokio::spawn(async move {
        let _ = start_with(cfg, listener, Voice::none()).await;
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let send = |v: Value| Message::Text(v.to_string().into());
    ws.send(send(json!({"t":"hello","v":1,"pairCode":"111111","device":"real-test",
        "settings":{"lang":"vi","model":model,"rigor":"medium"}}))).await.unwrap();
    let started = std::time::Instant::now();
    let mut sent_first = false;
    let mut first_say_ms = None;
    let mut turn_ends = 0;
    let mut photo_sent = false;
    let deadline = Duration::from_secs(150);
    while let Ok(Some(Ok(m))) = tokio::time::timeout(deadline, ws.next()).await {
        let Message::Text(t) = m else { continue };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        let brief = v.to_string();
        println!("[{:>5}ms] {}", started.elapsed().as_millis(), brief.chars().take(260).collect::<String>());
        match v["t"].as_str().unwrap_or("") {
            "ready" if !sent_first => {
                sent_first = true;
                ws.send(send(json!({"t":"user_text","text":"Giảng giúp tớ câu trong ảnh đi (tớ chưa gửi ảnh, cậu tự chụp bằng tool)."}))).await.unwrap();
            }
            "capture_request" => {
                photo_sent = true;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
                ws.send(send(json!({"t":"photo","requestId":v["requestId"],"jpeg":b64,"w":480,"h":200}))).await.unwrap();
            }
            "say" if first_say_ms.is_none() => first_say_ms = Some(started.elapsed().as_millis()),
            "error" => panic!("server error: {v}"),
            "turn_end" => {
                turn_ends += 1;
                break;
            }
            _ => {}
        }
    }
    println!("first say after {:?} ms, photo_sent={photo_sent}", first_say_ms);
    assert!(turn_ends == 1, "no turn_end");
    assert!(photo_sent, "Claude never called capture_photo");
}

async fn real_server(data: &std::path::Path) -> std::net::SocketAddr {
    let root = beluga_server::config::find_repo_root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Config {
        port: 0,
        mock: false,
        mock_delay_ms: 0,
        capture_timeout_ms: 8000,
        data_dir: data.to_path_buf(),
        board_dir: root.join("shared").join("board"),
        tutor_dir: root.join("tutor"),
        claude_bin: "claude".into(),
        claude_effort: None,
        eleven: None,
        genvoice: None,
        tts_provider: "auto".into(),
        stt_provider: "auto".into(),
        tts_base_ms: 8000,
        tts_per_char_ms: 60,
        stt_base_ms: 8000,
        stt_per_sec_ms: 1500,
        fixed_pair_code: Some("111111".into()),
    };
    tokio::spawn(async move {
        let _ = start_with(cfg, listener, Voice::none()).await;
    });
    addr
}

/// Collect frames until `stop` returns true (or 120 s).
async fn drain(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    mut stop: impl FnMut(&Value) -> bool,
) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_secs(120), ws.next()).await {
        let Message::Text(t) = m else { continue };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        println!("  {}", v.to_string().chars().take(200).collect::<String>());
        let s = stop(&v);
        out.push(v);
        if s {
            break;
        }
    }
    out
}

#[tokio::test]
#[ignore]
async fn real_claude_interrupt_then_resume_after_restart() {
    let tmp = std::env::temp_dir().join(format!("beluga-real2-{}", uuid::Uuid::new_v4()));
    let send = |v: Value| Message::Text(v.to_string().into());
    let hello = |sid: Option<&str>| {
        let mut h = json!({"t":"hello","v":1,"pairCode":"111111","device":"t",
            "settings":{"lang":"vi","model":"haiku","rigor":"light"}});
        if let Some(s) = sid {
            h["sessionId"] = json!(s);
        }
        h
    };
    let addr = real_server(&tmp.join("data")).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
    ws.send(send(hello(None))).await.unwrap();
    let ready = drain(&mut ws, |f| f["t"] == "ready").await.pop().unwrap();
    let sid = ready["sessionId"].as_str().unwrap().to_string();

    println!("--- turn 1 (will be interrupted)");
    ws.send(send(json!({"t":"user_text","text":"Đếm từ một đến bốn mươi, mỗi số một dòng riêng, bằng chữ."}))).await.unwrap();
    let mut says = 0;
    let mut last = 0;
    let _ = drain(&mut ws, |f| {
        if f["t"] == "say" {
            says += 1;
            last = f["seq"].as_u64().unwrap();
        }
        says >= 3
    })
    .await;
    ws.send(send(json!({"t":"played","seq":last}))).await.unwrap();
    let t0 = std::time::Instant::now();
    ws.send(send(json!({"t":"interrupt"}))).await.unwrap();
    let fr = drain(&mut ws, |f| f["t"] == "turn_end").await;
    println!("interrupt -> turn_end in {} ms", t0.elapsed().as_millis());
    assert_eq!(fr.last().unwrap()["interrupted"], true);

    println!("--- turn 2 (same process, after interrupt)");
    ws.send(send(json!({"t":"user_text","text":"Cậu vừa đếm tới số mấy thì tớ ngắt? Trả lời một câu."}))).await.unwrap();
    let fr = drain(&mut ws, |f| f["t"] == "turn_end" && f["turn"] == 2).await;
    assert!(fr.iter().any(|f| f["t"] == "say"), "no answer after interrupt");
    drop(ws);

    println!("--- new server instance, same session: --resume");
    let addr2 = real_server(&tmp.join("data")).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr2}/ws")).await.unwrap();
    ws.send(send(hello(Some(&sid)))).await.unwrap();
    drain(&mut ws, |f| f["t"] == "ready").await;
    ws.send(send(json!({"t":"user_text","text":"Nhắc lại giúp tớ: lúc đầu tớ nhờ cậu đếm tới số mấy? Một câu thôi."}))).await.unwrap();
    let fr = drain(&mut ws, |f| f["t"] == "turn_end").await;
    let said: String = fr.iter().filter(|f| f["t"] == "say").map(|f| f["text"].as_str().unwrap().to_string()).collect::<Vec<_>>().join(" ");
    println!("resumed answer: {said}");
    assert!(said.contains("40") || said.contains("bốn mươi"), "resume lost context: {said}");
}
