//! Manual check of protocol v2 against the REAL services (spends Claude plan quota and a few cents
//! of DeepSeek/OpenRouter, so it is `#[ignore]`). Keys come from `server/.env`.
//!
//! Run: `cargo test --test real_v2 -- --ignored --nocapture`
//! Optional: `BELUGA_TEST_JPEG=<path>` (a photo of a short math problem) for the attachment turn.
//!
//! One chat session: haiku -> DeepSeek -> OpenRouter -> back to haiku (same Claude session, resumed
//! across providers), a voice message corrected by the utility model, an auto title, usage, greeting.

mod common;

use beluga_server::config::Config;
use beluga_server::http::start_with;
use beluga_server::voice::Voice;
use common::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

async fn real_turn(p: &mut Phone, msg: Value) -> (Vec<Value>, u128) {
    let t0 = Instant::now();
    p.send(msg).await;
    let mut out = Vec::new();
    loop {
        let Some(f) = p.next_within(150_000).await else { panic!("no turn_end; got {out:#?}") };
        let end = f["t"] == "turn_end";
        out.push(f);
        if end {
            return (out, t0.elapsed().as_millis());
        }
    }
}

fn summary(label: &str, frames: &[Value], ms: u128) -> String {
    let chat: Vec<String> = frames.iter().filter(|f| f["t"] == "chat").map(|f| f["md"].as_str().unwrap_or("").replace('\n', " ⏎ ")).collect();
    let errors: Vec<String> = frames.iter().filter(|f| f["t"] == "error" || f["t"] == "notice").map(|f| f.to_string()).collect();
    let deltas = frames.iter().filter(|f| f["t"] == "chat_delta").count();
    println!("== {label}: {ms} ms, {deltas} deltas\n   chat: {chat:?}\n   errors/notices: {errors:?}");
    chat.join(" ")
}

#[tokio::test]
#[ignore]
async fn real_v2_chat_across_providers() {
    let _ = tracing_subscriber::fmt().with_env_filter("beluga_server=info").try_init();
    let root = beluga_server::config::find_repo_root();
    let tmp = std::env::temp_dir().join(format!("beluga-real-v2-{}", uuid::Uuid::new_v4()));
    copy_dir(&root.join("tutor").join(".claude"), &tmp.join("tutor").join(".claude"));
    std::fs::copy(root.join("tutor").join("CLAUDE.md"), tmp.join("tutor").join("CLAUDE.md")).unwrap();
    let mut cfg = Config::from_args(&[]);
    cfg.data_dir = tmp.join("data");
    cfg.tutor_dir = tmp.join("tutor");
    cfg.board_dir = root.join("shared").join("board");
    cfg.fixed_pair_code = Some(CODE.into());
    cfg.models_path = tmp.join("none.json");
    cfg.genvoice = None;
    cfg.eleven = None;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_with(cfg, listener, Voice::none()).await;
    });
    let mut p = Phone::connect(addr).await;
    let ready = p.hello(json!({"v": 2, "settings": {"lang": "vi", "model": "haiku", "rigor": "light", "mode": "chat"}})).await;
    let models: Vec<&str> = ready["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    println!("ready: mode={} model={} models={models:?}", ready["mode"], ready["model"]);
    let token = ready["pairToken"].as_str().unwrap().to_string();

    let (f, ms) = real_turn(&mut p, json!({"t":"user_text","text":"Chào cậu. 2+3 bằng mấy? Trả lời đúng một dòng."})).await;
    assert!(summary("haiku", &f, ms).contains('5'));
    let extra = p.drain_quiet(12_000).await;
    for x in &extra {
        if x["t"] == "title" || x["t"] == "usage" {
            println!("   after turn: {x}");
        }
    }

    let short = std::env::var("BELUGA_TEST_SHORT").is_ok(); // only the first turn and the image
    for (model, q, want) in [
        ("deepseek", "Còn 4+5? Một dòng thôi.", '9'),
        ("openrouter-haiku", "Còn 6+7? Một dòng thôi.", '3'),
        ("haiku", "Phép tính đầu tiên tớ hỏi là gì? Một dòng.", '2'),
    ] {
        if short || !models.contains(&model) {
            println!("== {model}: not configured, skipped");
            continue;
        }
        p.send(json!({"t":"settings","model": model})).await;
        let (f, ms) = real_turn(&mut p, json!({"t":"user_text","text": q})).await;
        let s = summary(model, &f, ms);
        assert!(f.iter().all(|x| x["t"] != "error"), "{model}: {f:#?}");
        assert!(s.contains(want), "{model}: {s}");
    }

    // speech corrected by the utility model
    p.send(json!({"t":"settings","model":"haiku"})).await;
    if !short {
        let (f, ms) = real_turn(&mut p, json!({"t":"user_text","text":"tam thứt bật hai là gì, nói một câu thôi","via":"voice"})).await;
        let heard = f.iter().find(|x| x["t"] == "heard");
        println!("   heard: {heard:?}");
        summary("voice", &f, ms);
    }

    if let Ok(path) = std::env::var("BELUGA_TEST_JPEG") {
        let jpeg = std::fs::read(path).unwrap();
        let up: Value = reqwest::Client::new()
            .post(format!("http://{addr}/upload?name=de.jpg"))
            .bearer_auth(&token)
            .header("Content-Type", "image/jpeg")
            .body(jpeg)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let (f, ms) = real_turn(&mut p, json!({"t":"user_text","text":"Đề trong ảnh là gì? Chép lại một dòng.","files":[up["fileId"]]})).await;
        summary("image (haiku)", &f, ms);
    }

    p.send(json!({"t":"get_usage"})).await;
    let u = p.until(|x| x["t"] == "usage").await.pop().unwrap();
    println!("usage: {u}");
    p.send(json!({"t":"get_greeting","localTime":"2026-11-15T21:00"})).await;
    println!("greeting: {}", p.until(|x| x["t"] == "greeting").await.pop().unwrap());
    println!("usage.jsonl:\n{}", std::fs::read_to_string(tmp.join("data").join("usage.jsonl")).unwrap_or_default());
    let _ = Duration::from_secs(0);
}
