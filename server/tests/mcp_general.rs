//! The general `/mcp` route (any Claude Code session on this PC talking to the phone) with a fake
//! phone, plus the guarantee that the tutor's per-session route only exposes `capture_photo`.

mod common;

use base64::Engine;
use beluga_server::config::Config;
use common::*;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;

async fn server(brain_delay_ms: u64) -> SocketAddr {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let cfg = Config {
        port: 0,
        mock: true,
        mock_delay_ms: brain_delay_ms,
        capture_timeout_ms: 8000,
        data_dir: dir.join("data"),
        board_dir: dir.join("board"),
        tutor_dir: dir.join("tutor"),
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
        fixed_pair_code: Some(CODE.into()),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = beluga_server::start(cfg, listener).await;
    });
    addr
}

/// JSON-RPC call to the general route; returns the `result` object.
async fn rpc(addr: SocketAddr, path: &str, method: &str, params: Value) -> Value {
    let r: Value = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    r
}

async fn tool(addr: SocketAddr, name: &str, args: Value) -> Value {
    rpc(addr, "/mcp", "tools/call", json!({"name": name, "arguments": args})).await["result"].clone()
}

fn text_json(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn is_error(result: &Value) -> bool {
    result["isError"] == true
}

#[tokio::test]
async fn protocol_basics_and_tool_lists() {
    let addr = server(5).await;
    let r = rpc(addr, "/mcp", "initialize", json!({"protocolVersion":"2025-06-18"})).await;
    assert_eq!(r["result"]["serverInfo"]["name"], "beluga");
    let r = rpc(addr, "/mcp", "tools/list", json!({})).await;
    let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["phone_status", "capture_photo", "speak", "present", "listen", "interrupt"]);
    for t in r["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
    }

    // The tutor's own per-session route sees only capture_photo and cannot call the others.
    let mut p = Phone::connect(addr).await;
    let ready = p.hello(json!({})).await;
    let path = format!("/mcp/{}", ready["sessionId"].as_str().unwrap());
    let r = rpc(addr, &path, "tools/list", json!({})).await;
    let tools = r["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "capture_photo");
    for name in ["speak", "present", "listen", "interrupt", "phone_status"] {
        let r = rpc(addr, &path, "tools/call", json!({"name": name, "arguments": {"text": "x", "lines": "x"}})).await;
        assert!(r.get("error").is_some(), "{name} must not be callable by the tutor: {r}");
    }
}

#[tokio::test]
async fn without_a_phone_tools_explain_and_status_says_disconnected() {
    let addr = server(5).await;
    let st = tool(addr, "phone_status", json!({})).await;
    assert!(!is_error(&st));
    assert_eq!(text_json(&st)["connected"], false);
    for (name, args) in [("speak", json!({"text":"hi"})), ("present", json!({"lines":"hi"})), ("listen", json!({})), ("interrupt", json!({})), ("capture_photo", json!({}))] {
        let r = tool(addr, name, args).await;
        assert!(is_error(&r), "{name}");
        assert!(r["content"][0]["text"].as_str().unwrap().contains("điện thoại"), "{name}: readable message");
    }
}

#[tokio::test]
async fn status_reports_the_newest_live_phone() {
    let addr = server(5).await;
    let mut a = Phone::connect(addr).await;
    let ra = a.hello(json!({})).await;
    let st = text_json(&tool(addr, "phone_status", json!({})).await);
    assert_eq!(st["connected"], true);
    assert_eq!(st["sessionId"], ra["sessionId"]);
    assert_eq!(st["state"], "idle");
    assert_eq!(st["tts"], "device");
    assert_eq!(st["stt"], "device");

    let mut b = Phone::connect(addr).await;
    let rb = b.hello(json!({})).await;
    assert_eq!(text_json(&tool(addr, "phone_status", json!({})).await)["sessionId"], rb["sessionId"]);

    drop(b);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let st = text_json(&tool(addr, "phone_status", json!({})).await);
    assert_eq!(st["sessionId"], ra["sessionId"], "falls back to the older live connection");
    drop(a);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(text_json(&tool(addr, "phone_status", json!({})).await)["connected"], false);
}

#[tokio::test]
async fn speak_and_present_go_through_the_real_parser_and_the_journal() {
    let addr = server(5).await;
    let mut p = Phone::connect(addr).await;
    let ready = p.hello(json!({})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();

    let r = tool(addr, "speak", json!({"text": "Xin chào cậu. @step không phải lệnh.\nCâu thứ hai."})).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(text_json(&r)["events"], 2 + 0);
    let f = p.until(is_turn_end).await;
    let says: Vec<&Value> = f.iter().filter(|x| x["t"] == "say").collect();
    assert_eq!(says.len(), 2);
    assert_eq!(says[0]["text"], "Xin chào cậu. @step không phải lệnh.");
    assert_eq!(says[0]["seq"], 1);
    assert_eq!(says[0]["turn"], 1);

    let lines = "Nhìn bảng nhé.\n@step {\"title\":\"Đưa về dạng chuẩn\"}\n@show {\"type\":\"math\",\"tex\":\"x^2-5x+6>0\"}\n@nonsense {}\n@ask {\"kind\":\"next\",\"prompt\":\"Tiếp?\"}\nbị bỏ sau ask";
    let r = tool(addr, "present", json!({"lines": lines})).await;
    assert!(!is_error(&r), "{r}");
    let info = text_json(&r);
    assert_eq!(info["events"], 4); // say, step, show, ask
    assert_eq!(info["turn"], 2);
    assert_eq!(info["dropped"].as_array().unwrap().len(), 1);
    assert!(info["dropped"][0].as_str().unwrap().contains("@nonsense"));
    let f = p.until(is_turn_end).await;
    let kinds: Vec<&str> = f.iter().map(|x| x["t"].as_str().unwrap()).filter(|t| *t != "state").collect();
    assert_eq!(kinds, ["say", "step", "show", "ask", "turn_end"]);
    let seqs: Vec<u64> = f.iter().filter_map(|x| x["seq"].as_u64()).collect();
    assert_eq!(seqs, [4, 5, 6, 7, 8], "seq continues after the first call (3 was its turn_end)");

    // They are in the session journal like any tutor event: reopening rebuilds the board.
    p.send(json!({"t":"open_session","sessionId":sid})).await;
    let fr = p.until(|x| x["t"] == "snapshot").await;
    let ev = fr.last().unwrap()["events"].as_array().unwrap();
    assert_eq!(ev.len(), 8);
    assert!(ev.iter().any(|e| e["t"] == "show" && e["block"]["type"] == "math"));
}

#[tokio::test]
async fn busy_tutor_blocks_speak_until_interrupt() {
    let addr = server(40).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"giảng bài dài"})).await;
    p.until(|f| f["t"] == "say").await;

    let r = tool(addr, "speak", json!({"text": "chen ngang"})).await;
    assert!(is_error(&r));
    assert!(r["content"][0]["text"].as_str().unwrap().contains("bận"));
    assert_eq!(text_json(&tool(addr, "phone_status", json!({})).await)["state"], "busy");

    let r = tool(addr, "interrupt", json!({})).await;
    assert!(!is_error(&r));
    assert_eq!(text_json(&r)["was"], "tutor_turn");
    p.until(|f| f["t"] == "turn_end" && f["interrupted"] == true).await;

    // The brain needs a moment to confirm the stop; then speaking works again.
    let mut ok = false;
    for _ in 0..30 {
        if !is_error(&tool(addr, "speak", json!({"text": "xong rồi"})).await) {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "speak should work after interrupt");
}

#[tokio::test]
async fn interrupt_with_nothing_running_cuts_queued_playback() {
    let addr = server(5).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({})).await;
    tool(addr, "speak", json!({"text": "Một câu."})).await;
    p.until(is_turn_end).await;
    let r = tool(addr, "interrupt", json!({})).await;
    assert_eq!(text_json(&r)["was"], "queued_playback");
    let f = p.until(is_turn_end).await;
    assert_eq!(f.last().unwrap()["interrupted"], true);
}

#[tokio::test]
async fn listen_captures_the_next_utterance_and_hides_it_from_the_tutor() {
    let addr = server(5).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({})).await;

    let call = tokio::spawn(async move { tool(addr, "listen", json!({"timeout_s": 10})).await });
    // wait until the server is really listening
    for _ in 0..50 {
        if text_json(&tool(addr, "phone_status", json!({})).await)["state"] == "listening" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    p.send(json!({"t":"user_text","text":"đáp án là năm"})).await;
    let r = call.await.unwrap();
    assert_eq!(text_json(&r)["heard"], "đáp án là năm");
    let quiet = p.drain_quiet(400).await;
    assert!(quiet.iter().all(|f| f["t"] != "say" && f["t"] != "state"), "tutor must not see it: {quiet:?}");

    // Next utterance (no listen waiting) goes to the tutor as usual.
    p.send(json!({"t":"user_text","text":"hello"})).await;
    let f = p.until(is_turn_end).await;
    assert!(f.iter().any(|x| x["t"] == "say"));
}

#[tokio::test]
async fn listen_times_out_with_a_text_answer_and_later_speech_goes_to_the_tutor() {
    let addr = server(5).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({})).await;
    let r = tool(addr, "listen", json!({"timeout_s": 0.3})).await;
    assert!(!is_error(&r));
    let v = text_json(&r);
    assert!(v["heard"].is_null());
    assert!(v["text"].as_str().unwrap().contains("Không có ai nói"));
    // The abandoned listen must not swallow the next utterance.
    p.send(json!({"t":"user_text","text":"hello"})).await;
    let f = p.until(is_turn_end).await;
    assert!(f.iter().any(|x| x["t"] == "say"));
}

#[tokio::test]
async fn second_listen_replaces_the_first() {
    let addr = server(5).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({})).await;
    let first = tokio::spawn(async move { tool(addr, "listen", json!({"timeout_s": 5})).await });
    for _ in 0..50 {
        if text_json(&tool(addr, "phone_status", json!({})).await)["state"] == "listening" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let second = tokio::spawn(async move { tool(addr, "listen", json!({"timeout_s": 5})).await });
    let r1 = first.await.unwrap();
    assert!(is_error(&r1), "the older listen is cancelled");
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.send(json!({"t":"user_text","text":"một"})).await;
    assert_eq!(text_json(&second.await.unwrap())["heard"], "một");
}

#[tokio::test]
async fn capture_photo_returns_image_content() {
    let addr = server(5).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({})).await;
    let call = tokio::spawn(async move { tool(addr, "capture_photo", json!({})).await });
    let req = p.until(|f| f["t"] == "capture_request").await.pop().unwrap();
    let jpeg = base64::engine::general_purpose::STANDARD.encode([0xFFu8, 0xD8, 1, 2, 0xFF, 0xD9]);
    p.send(json!({"t":"photo","requestId":req["requestId"],"jpeg":jpeg,"w":2,"h":2})).await;
    let r = call.await.unwrap();
    assert_eq!(r["content"][0]["type"], "image");
    assert_eq!(r["content"][0]["mimeType"], "image/jpeg");
    assert_eq!(r["content"][0]["data"], jpeg);
}
