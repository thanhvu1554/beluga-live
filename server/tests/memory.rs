//! Memory, review offer, progress and the `[memory]` summary, end to end with the mock brain
//! (which echoes the attached context as speech) and a fake phone.

mod common;

use beluga_server::config::Config;
use beluga_server::http::start_with;
use beluga_server::voice::{FakeVoice, Voice};
use common::*;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;

const PAST: &str = "2026-01-01";
const FUTURE: &str = "2099-01-01";

struct Env {
    dir: PathBuf,
    addr: SocketAddr,
}

impl Env {
    fn tutor(&self) -> PathBuf {
        self.dir.join("tutor")
    }

    fn memory_file(&self, name: &str) -> String {
        std::fs::read_to_string(self.tutor().join("memory").join(format!("{name}.md"))).unwrap_or_default()
    }

    fn write_memory(&self, name: &str, text: &str) {
        let d = self.tutor().join("memory");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{name}.md")), text).unwrap();
    }
}

async fn serve(dir: &Path, voice: Arc<Voice>) -> SocketAddr {
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let cfg = Config {
        port: 0,
        mock: true,
        mock_delay_ms: 2,
        capture_timeout_ms: 8000,
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
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_with(cfg, listener, voice).await;
    });
    addr
}

async fn env() -> Env {
    let dir = temp_dir();
    let addr = serve(&dir, Voice::none()).await;
    Env { dir, addr }
}

/// A second server process over the same data and memory directories ("restart").
async fn restart(e: &Env) -> Env {
    Env { dir: e.dir.clone(), addr: serve(&e.dir, Voice::none()).await }
}

fn weak_line(topic: &str, count: u32, due: &str, note: &str) -> String {
    format!("- [yếu] {topic} | sai {count} | đúng 0 | bậc 0 | lần cuối {PAST} | ôn {due} | {note}\n")
}

fn says(frames: &[Value]) -> Vec<String> {
    frames.iter().filter(|f| f["t"] == "say").map(|f| f["text"].as_str().unwrap().to_string()).collect()
}

async fn rpc(addr: SocketAddr, sid: &str, name: &str, args: Value) -> Value {
    let r: Value = reqwest::Client::new()
        .post(format!("http://{addr}/mcp/{sid}"))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name": name, "arguments": args}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    r["result"].clone()
}

async fn say_turn(p: &mut Phone, text: &str) -> Vec<Value> {
    p.send(json!({"t":"user_text","text":text})).await;
    p.until(is_turn_end).await
}

// ------------------------------------------------------------------ memory_note

#[tokio::test]
async fn memory_note_tool_writes_files_and_merges_the_same_topic() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    let sid = p.hello(json!({})).await["sessionId"].as_str().unwrap().to_string();

    let r = rpc(e.addr, &sid, "memory_note", json!({"subject":"Toán","kind":"mistake","topic":"Xét dấu tam thức khi a âm","note":"quên đổi chiều"})).await;
    assert!(r["isError"].is_null(), "{r}");
    let out: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!((out["subject"].as_str(), out["action"].as_str()), (Some("toan"), Some("created")));
    assert!(out["nextReview"].as_str().unwrap().starts_with("20"));
    let toan = e.memory_file("toan");
    assert!(toan.contains("## Đang yếu\n- [yếu] Xét dấu tam thức khi a âm | sai 1 | đúng 0 | bậc 0 | lần cuối "), "{toan}");
    assert!(toan.contains("| quên đổi chiều"), "{toan}");

    // same topic, different case and spacing: same entry, count 2
    rpc(e.addr, &sid, "memory_note", json!({"subject":"toan","kind":"mistake","topic":"  xét DẤU tam   thức khi a âm","note":""})).await;
    let toan = e.memory_file("toan");
    assert_eq!(toan.lines().filter(|l| l.starts_with("- [yếu]")).count(), 1, "{toan}");
    assert!(toan.contains("| sai 2 | đúng 0 |"), "{toan}");

    // other kinds
    rpc(e.addr, &sid, "memory_note", json!({"subject":"vật lý","kind":"progress","topic":"Chuyển động thẳng đều","note":"xong"})).await;
    rpc(e.addr, &sid, "memory_note", json!({"subject":"toan","kind":"style","topic":"Hiểu nhanh hơn khi có ví dụ số"})).await;
    assert!(e.memory_file("ly").contains("## Đã học\n- [học] Chuyển động thẳng đều | ngày "));
    assert!(e.memory_file("profile").contains("## Cách giảng hợp\n- [cách] Hiểu nhanh hơn khi có ví dụ số | môn toan | ngày "));

    // two correct reviews in a row -> mastered
    for _ in 0..2 {
        rpc(e.addr, &sid, "memory_note", json!({"subject":"toan","kind":"mastered","topic":"Xét dấu tam thức khi a âm"})).await;
    }
    let toan = e.memory_file("toan");
    assert!(!toan.lines().any(|l| l.starts_with("- [yếu]")), "{toan}");
    assert!(toan.contains("## Đã vững\n- [vững] Xét dấu tam thức khi a âm | ngày "), "{toan}");
    assert!(toan.contains("| quên đổi chiều"), "the note survives the move: {toan}");

    // bad calls are errors the tutor can read, and write nothing
    for args in [json!({"subject":"toan","kind":"nope","topic":"x"}), json!({"subject":"toan","kind":"mistake","topic":"  "}), json!({})] {
        let r = rpc(e.addr, &sid, "memory_note", args).await;
        assert_eq!(r["isError"], true, "{r}");
    }
    assert!(!e.tutor().join("memory").join("chung.md").exists());
}

#[tokio::test]
async fn memory_note_without_an_open_session_is_a_text_error() {
    let e = env().await;
    let r = rpc(e.addr, "no-such-session", "memory_note", json!({"subject":"toan","kind":"mistake","topic":"x"})).await;
    assert_eq!(r["isError"], true);
    assert!(!e.tutor().join("memory").exists());
}

// ------------------------------------------------------------------ [memory] summary

#[tokio::test]
async fn first_turn_gets_the_memory_summary_once_even_after_a_restart() {
    let e = env().await;
    e.write_memory("toan", &format!("# Toán\n\n## Đang yếu\n{}", weak_line("Xét dấu tam thức khi a âm", 3, FUTURE, "hay quên đổi chiều")));
    e.write_memory("profile", "# Hồ sơ\n\n## Cách giảng hợp\n- [cách] Hiểu nhanh hơn khi có ví dụ số | ngày 2026-11-01\n");
    let mut p = Phone::connect(e.addr).await;
    let ready = p.hello(json!({})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();

    let first = says(&say_turn(&mut p, "giảng bài").await);
    assert!(first.contains(&"Mock thấy memory.".to_string()), "{first:?}");
    assert!(first.iter().any(|s| s == "Mock nhớ: Cách giảng hợp: Hiểu nhanh hơn khi có ví dụ số"), "{first:?}");
    assert!(first.iter().any(|s| s.starts_with("Mock nhớ: toan · Xét dấu tam thức khi a âm · sai 3") && s.ends_with("hay quên đổi chiều")), "{first:?}");

    let second = says(&say_turn(&mut p, "tiếp").await);
    assert!(!second.iter().any(|s| s.starts_with("Mock")), "no second summary: {second:?}");

    // the server restarts, the phone reopens the same session: still no second summary
    let e2 = restart(&e).await;
    let mut p2 = Phone::connect(e2.addr).await;
    p2.hello(json!({"sessionId": sid, "lastSeq": 0})).await;
    p2.drain_quiet(200).await;
    let third = says(&say_turn(&mut p2, "tiếp nữa").await);
    assert!(!third.iter().any(|s| s.starts_with("Mock")), "resumed session: {third:?}");
}

#[tokio::test]
async fn nothing_known_means_no_memory_block() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    let s = says(&say_turn(&mut p, "chào").await);
    assert!(!s.iter().any(|s| s.starts_with("Mock")), "{s:?}");
}

// ------------------------------------------------------------------ review offer

fn due_memory() -> String {
    format!(
        "# Toán\n\n## Đang yếu\n{}{}",
        weak_line("Xét dấu tam thức khi a âm", 3, "2026-01-05", "hay quên đổi chiều"),
        weak_line("Điều kiện của căn thức", 1, "2026-01-02", ""),
    )
}

#[tokio::test]
async fn new_session_with_a_due_item_starts_with_a_review_offer_from_the_server() {
    let dir = temp_dir();
    let voice = Arc::new(Voice::build(Some(Arc::new(FakeVoice { fail_status: None, transcript: String::new() })), None, 3000));
    let addr = serve(&dir, voice).await;
    let e = Env { dir, addr };
    e.write_memory("toan", &due_memory());
    e.write_memory("hoa", &format!("# Hóa\n\n## Đang yếu\n{}", weak_line("Cân bằng phương trình", 1, "2026-01-03", "")));

    let mut p = Phone::connect(e.addr).await;
    let ready = p.hello(json!({})).await;
    let sid = ready["sessionId"].as_str().unwrap().to_string();
    let frames = p.until(is_turn_end).await;
    let kinds: Vec<&str> = frames.iter().map(|f| f["t"].as_str().unwrap()).filter(|t| *t != "state" && *t != "audio").collect();
    assert_eq!(kinds, ["say", "step", "show", "turn_end"], "{frames:#?}");

    // the item that is overdue the longest: "Điều kiện của căn thức" (due 01-02)
    let say = frames.iter().find(|f| f["t"] == "say").unwrap();
    assert_eq!(say["text"], "Lần trước cậu hay nhầm điều kiện của căn thức, ôn một câu không?");
    assert!(!say["text"].as_str().unwrap().contains('!'));
    assert_eq!(say["audio"], true, "the invitation is voiced like any say");
    let block = &frames.iter().find(|f| f["t"] == "show").unwrap()["block"];
    assert_eq!(block["type"], "review_offer");
    assert_eq!((block["topic"].as_str(), block["subject"].as_str(), block["lastSeen"].as_str()), (Some("Điều kiện của căn thức"), Some("toan"), Some(PAST)));
    assert!(block["fallback"].as_str().unwrap().contains("Điều kiện của căn thức"));
    assert!(block["id"].as_str().is_some());
    // seq numbers are contiguous and the audio for the say arrives
    let seqs: Vec<u64> = frames.iter().filter_map(|f| f["seq"].as_u64()).collect();
    assert!(seqs.windows(2).all(|w| w[1] > w[0]));
    let audio = p.drain_quiet(300).await;
    assert!(audio.iter().chain(frames.iter()).any(|f| f["t"] == "audio" && f["seq"] == say["seq"]));

    // the session is not listed until the student actually says something
    p.send(json!({"t":"list_sessions"})).await;
    let list = p.until(|f| f["t"] == "sessions").await;
    assert_eq!(list.last().unwrap()["items"].as_array().unwrap().len(), 0);

    // the next user turn carries the review note, the one after does not
    let t1 = says(&say_turn(&mut p, "ừ ôn đi").await);
    assert!(t1.iter().any(|s| s == "Mock ôn lại: toan · Điều kiện của căn thức"), "{t1:?}");
    let t2 = says(&say_turn(&mut p, "tiếp").await);
    assert!(!t2.iter().any(|s| s.starts_with("Mock ôn lại")), "{t2:?}");

    // reconnecting to the same session (snapshot) does not offer a second time
    let mut p2 = Phone::connect(e.addr).await;
    p2.hello(json!({"sessionId": sid, "lastSeq": 0})).await;
    let again = p2.drain_quiet(300).await;
    assert_eq!(again.iter().filter(|f| f["t"] == "snapshot").flat_map(|f| f["events"].as_array().unwrap()).filter(|x| x["t"] == "show" && x["block"]["type"] == "review_offer").count(), 1);
    assert!(again.iter().filter(|f| f["t"] == "show").count() == 0, "no new live offer: {again:#?}");

    // a brand-new session offers again
    let mut p3 = Phone::connect(e.addr).await;
    p3.hello(json!({})).await;
    let f3 = p3.until(is_turn_end).await;
    assert_eq!(says(&f3).len(), 1);
}

#[tokio::test]
async fn offer_follows_the_language_setting_and_nothing_due_means_no_offer() {
    let e = env().await;
    e.write_memory("anh-ssat", &format!("# Anh\n\n## Đang yếu\n{}", weak_line("C++ pointer", 2, "2026-01-02", "")));
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({"settings":{"lang":"en","model":"sonnet","rigor":"medium"}})).await;
    let f = p.until(is_turn_end).await;
    assert_eq!(says(&f), ["Last time you kept mixing up C++ pointer. Want a quick review question?"], "acronyms keep their case");
    assert_eq!(f.iter().find(|x| x["t"] == "step").unwrap()["title"], "Review");

    let e2 = env().await;
    e2.write_memory("toan", &format!("# Toán\n\n## Đang yếu\n{}", weak_line("Chưa tới hạn", 1, FUTURE, "")));
    e2.write_memory("ly", "# Lý\n\n## Đã vững\n- [vững] Đã vững rồi | ngày 2026-01-01\n");
    let mut q = Phone::connect(e2.addr).await;
    q.hello(json!({})).await;
    assert!(q.drain_quiet(300).await.iter().all(|f| f["t"] != "say"), "no offer for items that are not due");
}

#[tokio::test]
async fn an_unanswered_offer_survives_a_restart() {
    let e = env().await;
    e.write_memory("toan", &due_memory());
    let mut p = Phone::connect(e.addr).await;
    let sid = p.hello(json!({})).await["sessionId"].as_str().unwrap().to_string();
    p.until(is_turn_end).await;
    drop(p);
    let e2 = restart(&e).await;
    let mut q = Phone::connect(e2.addr).await;
    q.hello(json!({"sessionId": sid, "lastSeq": 0})).await;
    q.drain_quiet(200).await;
    let t = says(&say_turn(&mut q, "ok").await);
    assert!(t.iter().any(|s| s == "Mock ôn lại: toan · Điều kiện của căn thức"), "{t:?}");
}

#[tokio::test]
async fn offer_note_includes_the_item_note_and_the_session_gets_the_summary_too() {
    let e = env().await;
    e.write_memory("toan", &format!("# Toán\n\n## Đang yếu\n{}", weak_line("Xét dấu tam thức khi a âm", 3, "2026-01-05", "hay quên đổi chiều")));
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.until(is_turn_end).await;
    let t = says(&say_turn(&mut p, "ừ").await);
    assert!(t.iter().any(|s| s == "Mock ôn lại: toan · Xét dấu tam thức khi a âm · hay quên đổi chiều"), "{t:?}");
    assert!(t.iter().any(|s| s == "Mock thấy memory."), "the first user turn still gets the summary: {t:?}");
}

// ------------------------------------------------------------------ progress

#[tokio::test]
async fn get_progress_returns_the_progress_frame() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"get_progress"})).await;
    let empty = p.until(|f| f["t"] == "progress").await.pop().unwrap();
    assert_eq!(empty["subjects"], json!([]));
    assert_eq!(empty["style"], json!([]));
    assert!(empty["updatedAt"].as_str().unwrap().contains('T'));

    e.write_memory(
        "toan",
        &format!(
            "# Toán\n\n## Đang yếu\n{}{}\n## Đã vững\n- [vững] Giải phương trình bậc hai | ngày 2026-11-10\n\n## Đã học\n- [học] Hàm số bậc hai | ngày 2026-11-12 | xong\n- [học] Bất phương trình | ngày 2026-11-14\n",
            weak_line("B", 1, "2026-01-09", "ghi chú B"),
            weak_line("A", 4, "2026-01-02", ""),
        ),
    );
    e.write_memory("profile", "# Hồ sơ\n\n## Cách giảng hợp\n- [cách] Ví dụ số | ngày 2026-11-01\n- Dòng lạ không theo khuôn\n");
    p.send(json!({"t":"get_progress"})).await;
    let pr = p.until(|f| f["t"] == "progress").await.pop().unwrap();
    let subs = pr["subjects"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    let s = &subs[0];
    assert_eq!((s["id"].as_str(), s["name"].as_str()), (Some("toan"), Some("Toán")));
    assert_eq!(s["weak"][0], json!({"topic":"A","note":"","count":4,"lastSeen":PAST,"due":"2026-01-02"}));
    assert_eq!(s["weak"][1]["topic"], "B");
    assert_eq!(s["mastered"], json!([{"topic":"Giải phương trình bậc hai","date":"2026-11-10"}]));
    assert_eq!(s["recent"], json!([{"topic":"Bất phương trình","date":"2026-11-14"},{"topic":"Hàm số bậc hai","date":"2026-11-12"}]));
    assert_eq!(pr["style"], json!(["Ví dụ số"]));
}
