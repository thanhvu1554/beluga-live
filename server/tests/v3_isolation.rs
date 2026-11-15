//! PROTOCOL v3 section 18, locked: every folder is its own knowledge stream. Two folders carry
//! distinctive markers in their memory, knowledge, exams, weekly summary and due review items; a
//! third marker sits in the legacy subject file. Nothing of folder A may reach anything about
//! folder B (and nothing of either folder reaches a session outside folders): the text sent to the
//! (mock) Claude, the review offer, `review`, `knowledge`, `exams`, `home`, `greeting`, the
//! STT-fix prompt and the review-quiz kickoff.

mod common;

use beluga_server::config::Config;
use beluga_server::http::start_with_util;
use beluga_server::util_llm::{FakeUtil, UtilReq};
use beluga_server::voice::Voice;
use common::*;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

const ALPHA: &str = "MARKER_ALPHA";
const BETA: &str = "MARKER_BETA";
const LEGACY: &str = "MARKER_LEGACY";
const PAST: &str = "2026-01-01";

type Log = Arc<Mutex<Vec<UtilReq>>>;

struct Env {
    dir: PathBuf,
    addr: SocketAddr,
    util_log: Log,
}

impl Env {
    fn data(&self) -> PathBuf {
        self.dir.join("data")
    }
    fn memory(&self) -> PathBuf {
        self.dir.join("tutor").join("memory")
    }
    fn read_memory(&self, name: &str) -> String {
        std::fs::read_to_string(self.memory().join(format!("{name}.md"))).unwrap_or_default()
    }
    /// Utility-model requests of one purpose, as one text (system + user).
    fn util_text(&self, purpose: &str) -> Vec<String> {
        self.util_log.lock().unwrap().iter().filter(|r| r.purpose == purpose).map(|r| format!("{}\n{}", r.system, r.user)).collect()
    }
}

async fn env() -> Env {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let util = FakeUtil::new(move |r| {
        l2.lock().unwrap().push(r.clone());
        Ok(match r.purpose {
            "stt_fix" => r.user.rsplit("Câu STT cần sửa: ").next().unwrap_or("").to_string(),
            "greeting" => r#"{"text":"Chào cậu nè","doodle":"whale_swim"}"#.into(),
            "title" => "Toán · Thử".into(),
            "folder_kind" => "toan".into(),
            _ => "ok".into(),
        })
    });
    let mut cfg = Config::for_test(&dir);
    cfg.mock_delay_ms = 1;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_with_util(cfg, listener, Voice::none(), util).await;
    });
    Env { dir, addr, util_log: log }
}

async fn v3(p: &mut Phone, extra: Value) -> Value {
    let mut h = json!({"v": 3});
    for (k, v) in extra.as_object().unwrap() {
        h[k] = v.clone();
    }
    p.hello(h).await
}

async fn rpc(addr: SocketAddr, sid: &str, name: &str, args: Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp/{sid}"))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name": name, "arguments": args}}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["result"]
        .clone()
}

fn weak(topic: &str, due: &str) -> String {
    format!("- [yếu] {topic} | sai 2 | đúng 0 | bậc 0 | lần cuối {PAST} | ôn {due} | ghi chú {topic}\n")
}

/// Folder memory, knowledge, an exam and a weekly summary, all carrying `marker`.
fn seed_folder(e: &Env, id: &str, marker: &str) {
    let mem = format!(
        "# Trí nhớ\n\n## Đang yếu\n{}{}\n## Đã học\n- [học] Bài {marker} | ngày 2026-11-10\n\n## Đã hiểu sau khi giảng lại\n- [hiểu] Hiểu {marker} | ngày 2026-11-10 | ví dụ {marker}\n",
        weak(&format!("Yếu {marker} một"), "2026-01-02"),
        weak(&format!("Yếu {marker} hai"), "2026-01-03"),
    );
    std::fs::create_dir_all(e.memory().join("folders")).unwrap();
    std::fs::write(e.memory().join("folders").join(format!("{id}.md")), mem).unwrap();
    let d = e.data().join("folders").join(id);
    std::fs::create_dir_all(d.join("exams")).unwrap();
    let k = json!({"v":1,"folderId": id,"subject":"toan","curriculum":[{"id":"c1","title": format!("Chương {marker}"),"topics":[
        {"id":"c1.t1","title": format!("Chủ đề {marker} đã học")},{"id":"c1.t2","title": format!("Chủ đề {marker} đang học")}]}],
        "schoolProgress":{"coveredTopicIds":["c1.t1"],"currentTopicId":"c1.t2","confidence":0.8}});
    std::fs::write(d.join("knowledge.json"), k.to_string()).unwrap();
    let exam_id = format!("exam-{}", marker.to_lowercase().replace('_', "-"));
    std::fs::write(
        d.join("exams").join(format!("{exam_id}.json")),
        json!({"examId": exam_id, "folderId": id, "title": format!("Đề {marker}"), "structureId": "s1", "createdAt": "2026-11-10T00:00:00+07:00", "questions": 3}).to_string(),
    )
    .unwrap();
    std::fs::write(d.join("weekly.json"), json!({"from":"2026-11-08","to":"2026-11-14","md": format!("Tuần {marker}")}).to_string()).unwrap();
}

async fn create_folder(p: &mut Phone, name: &str, about: &str) -> String {
    p.send(json!({"t":"create_folder","name": name,"about": about,"subject":"toan"})).await;
    let id = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();
    p.until(|x| x["t"] == "folders").await;
    id
}

async fn ask(p: &mut Phone, msg: Value, t: &str) -> Value {
    p.send(msg).await;
    p.until(|x| x["t"] == t).await.pop().unwrap()
}

/// Open a new chat session (in a folder or not); returns its id and every frame up to the
/// server's own first turn (the review offer) or a short quiet time.
async fn open(p: &mut Phone, folder: Option<&str>) -> (String, Vec<Value>) {
    let mut msg = json!({"t":"new_session","mode":"chat"});
    if let Some(f) = folder {
        msg["folderId"] = json!(f);
    }
    p.send(msg).await;
    let ready = p.until(|x| x["t"] == "ready").await.pop().unwrap();
    let rest = p.drain_quiet(300).await;
    (ready["sessionId"].as_str().unwrap().to_string(), rest)
}

/// One chat turn; the mock echoes every line of the text the server sent to Claude ("#raw").
async fn raw_turn(p: &mut Phone, text: &str) -> String {
    p.send(json!({"t":"user_text","text": format!("#raw {text}")})).await;
    let f = p.until(is_turn_end).await;
    f.iter().filter(|x| x["t"] == "chat").map(|x| x["md"].as_str().unwrap()).collect::<Vec<_>>().join("\n")
}

fn s(v: &Value) -> String {
    v.to_string()
}

struct World {
    e: Env,
    p: Phone,
    a: String,
    b: String,
}

async fn world() -> World {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    v3(&mut p, json!({})).await;
    let a = create_folder(&mut p, "Folder Alpha", &format!("mô tả {ALPHA}")).await;
    let b = create_folder(&mut p, "Folder Beta", &format!("mô tả {BETA}")).await;
    seed_folder(&e, &a, ALPHA);
    seed_folder(&e, &b, BETA);
    std::fs::write(e.memory().join("toan.md"), format!("# Toán\n\n## Đang yếu\n{}", weak(&format!("Yếu {LEGACY}"), "2026-01-02"))).unwrap();
    std::fs::write(e.memory().join("profile.md"), "# Hồ sơ\n\n## Cách giảng hợp\n- [cách] Thích ví dụ số | ngày 2026-11-01\n").unwrap();
    World { e, p, a, b }
}

#[tokio::test]
async fn nothing_of_folder_a_reaches_folder_b_or_a_session_outside_folders() {
    let World { e, mut p, a, b } = world().await;

    // --- a session in B: the server's review offer, then the text Claude receives
    let (sid_b, offer) = open(&mut p, Some(&b)).await;
    let offer_s = offer.iter().map(s).collect::<String>();
    assert!(offer.iter().any(|x| x["t"] == "show" && x["block"]["type"] == "review_offer"), "{offer:#?}");
    assert!(offer_s.contains(BETA) && !offer_s.contains(ALPHA) && !offer_s.contains(LEGACY), "{offer_s}");
    let show = offer.iter().find(|x| x["t"] == "show").unwrap();
    assert_eq!(show["block"]["folderName"], "Folder Beta");

    let text_b = raw_turn(&mut p, "học tiếp").await;
    assert!(text_b.contains("Mock raw: [folder: Folder Beta | mô tả MARKER_BETA | kiểu dạy: toan]"), "{text_b}");
    assert!(text_b.contains(&format!("Yếu {BETA} một")), "B's weak items: {text_b}");
    assert!(text_b.contains(&format!("Chủ đề {BETA} đang học")), "B's school progress: {text_b}");
    assert!(text_b.contains(&format!("[ôn lại: toan · Yếu {BETA} một")), "the answered offer is B's: {text_b}");
    assert!(text_b.contains("Thích ví dụ số"), "profile.md is shared: {text_b}");
    assert!(!text_b.contains(ALPHA), "A leaked into B's context: {text_b}");
    assert!(!text_b.contains(LEGACY), "a folder session never reads the subject file: {text_b}");

    // --- STT correction for B: only B's folder and curriculum
    p.send(json!({"t":"user_text","text":"tam thứt bật hai","via":"voice"})).await;
    p.until(is_turn_end).await;
    let fix = e.util_text("stt_fix");
    assert!(!fix.is_empty());
    let fix = fix.last().unwrap();
    assert!(fix.contains("Folder Beta") && fix.contains(&format!("Chủ đề {BETA} đã học")), "{fix}");
    assert!(!fix.contains(ALPHA) && !fix.contains("Folder Alpha"), "{fix}");

    // --- memory_note in B naming another subject lands in B's file only
    let a_before = e.read_memory(&format!("folders/{a}"));
    let toan_before = e.read_memory("toan");
    let r = rpc(e.addr, &sid_b, "memory_note", json!({"subject":"toan","kind":"mistake","topic":"Lỗi mới của Beta","note":"ghi ở B"})).await;
    assert!(r["isError"].is_null(), "{r}");
    let r = rpc(e.addr, &sid_b, "memory_note", json!({"subject":"van","kind":"progress","topic":"Bài văn ghi ở B"})).await;
    assert!(r["isError"].is_null(), "{r}");
    let b_file = e.read_memory(&format!("folders/{b}"));
    assert!(b_file.contains("- [yếu] Lỗi mới của Beta | sai 1") && b_file.contains("- [học] Bài văn ghi ở B"), "{b_file}");
    assert_eq!(e.read_memory(&format!("folders/{a}")), a_before, "A's file untouched");
    assert_eq!(e.read_memory("toan"), toan_before, "subject file untouched");
    assert!(!e.memory().join("van.md").exists(), "no subject file is made from inside a folder");

    // --- B's screens
    for (msg, t) in [
        (json!({"t":"get_review","folderId": b}), "review"),
        (json!({"t":"get_knowledge","folderId": b}), "knowledge"),
        (json!({"t":"list_exams","folderId": b}), "exams"),
        (json!({"t":"get_home","folderId": b}), "home"),
        (json!({"t":"get_greeting","localTime":"2026-11-15T20:00","folderId": b}), "greeting"),
    ] {
        let v = ask(&mut p, msg, t).await;
        let txt = s(&v);
        assert!(!txt.contains(ALPHA) && !txt.contains(LEGACY) && !txt.contains("Folder Alpha"), "{t} of B leaks: {txt}");
        assert!(t == "greeting" || txt.contains(BETA) || txt.contains(&b), "{t} of B is about B: {txt}");
    }
    let g = ask(&mut p, json!({"t":"get_greeting","localTime":"2026-11-15T20:30","folderId": b}), "greeting").await;
    assert!(g["sub"].as_str().unwrap().ends_with("trong Folder Beta"), "{g}");
    let gprompt = e.util_text("greeting");
    assert!(gprompt.iter().all(|t| !t.contains(ALPHA) && !t.contains("Folder Alpha")), "{gprompt:?}");
    let home_b = ask(&mut p, json!({"t":"get_home","folderId": b}), "home").await;
    assert_eq!(home_b["folderId"], b.as_str());
    assert_eq!(home_b["continue"]["sessionId"], sid_b.as_str());
    assert_eq!(home_b["due"].as_array().unwrap().len(), 1);
    assert_eq!((home_b["due"][0]["folderName"].as_str(), home_b["due"][0]["count"].as_u64()), (Some("Folder Beta"), Some(2)), "the two seeded items; the new note is due tomorrow");
    assert_eq!(home_b["quick"].as_array().unwrap().len(), 1);

    // --- the review quiz of B
    p.send(json!({"t":"new_session","folderId": b,"mode":"chat","kickoff":"review_quiz"})).await;
    let quiz = p.until(is_turn_end).await;
    let qtext = quiz.iter().map(s).collect::<String>();
    assert!(qtext.contains(BETA) && !qtext.contains(ALPHA) && !qtext.contains(LEGACY), "{qtext}");

    // --- a session outside any folder: no offer, no folder data at all
    let (_, before) = open(&mut p, None).await;
    assert!(before.iter().all(|x| x["t"] != "say" && x["t"] != "show" && x["t"] != "chat"), "no self-initiated offer outside folders: {before:#?}");
    let text_none = raw_turn(&mut p, "chào").await;
    assert!(!text_none.contains(ALPHA) && !text_none.contains(BETA) && !text_none.contains("[folder"), "{text_none}");
    assert!(text_none.contains(LEGACY) && text_none.contains("Thích ví dụ số"), "profile + subject file: {text_none}");

    // --- the review quiz always needs a folder
    let err = ask(&mut p, json!({"t":"new_session","mode":"chat","kickoff":"review_quiz"}), "error").await;
    assert_eq!(err["code"], "no_folder");
}

#[tokio::test]
async fn home_and_greeting_outside_folders_always_name_the_folder() {
    let World { e: _e, mut p, a, b } = world().await;
    let (sid_a, _) = open(&mut p, Some(&a)).await;
    raw_turn(&mut p, "bài trong A").await;

    let h = ask(&mut p, json!({"t":"get_home"}), "home").await;
    assert!(h["folderId"].is_null());
    assert_eq!(h["continue"]["sessionId"], sid_a.as_str());
    assert_eq!((h["continue"]["folderId"].as_str(), h["continue"]["folderName"].as_str()), (Some(a.as_str()), Some("Folder Alpha")));
    let due = h["due"].as_array().unwrap();
    assert_eq!(due.len(), 2, "{h}");
    for d in due {
        let (id, name) = (d["folderId"].as_str().unwrap(), d["folderName"].as_str().unwrap());
        assert_eq!(name, if id == a { "Folder Alpha" } else { "Folder Beta" });
        let marker = if id == a { ALPHA } else { BETA };
        assert!(d["topics"].as_array().unwrap().iter().all(|t| t.as_str().unwrap().contains(marker)), "topics stay with their folder: {d}");
        assert_eq!(d["kind"], "toan");
    }
    assert!(h["quick"].as_array().unwrap().iter().all(|q| q["folderName"].as_str().map(|n| n.starts_with("Folder ")).unwrap_or(false)));
    assert!(h["quick"].as_array().unwrap().iter().any(|q| q["folderId"] == b.as_str()));

    let g = ask(&mut p, json!({"t":"get_greeting","localTime":"2026-11-15T20:00"}), "greeting").await;
    let sub = g["sub"].as_str().unwrap();
    assert!(sub.ends_with("trong Folder Alpha") || sub.ends_with("trong Folder Beta"), "{sub}");
    assert!(!sub.contains("Toán"), "never a subject name instead of the folder's: {sub}");

    // unknown folder: an error, never another folder's data
    let err = ask(&mut p, json!({"t":"get_home","folderId":"nope"}), "error").await;
    assert_eq!(err["code"], "no_folder");
}

#[tokio::test]
async fn a_moved_session_gets_the_new_folders_context_and_its_old_notes_stay_behind() {
    let World { e, mut p, a, b } = world().await;
    let (sid, _) = open(&mut p, Some(&a)).await;
    let first = raw_turn(&mut p, "trong A").await;
    assert!(first.contains("[folder: Folder Alpha") && first.contains(ALPHA), "{first}");
    rpc(e.addr, &sid, "memory_note", json!({"kind":"mistake","topic":"Ghi khi còn ở A"})).await;
    let second = raw_turn(&mut p, "vẫn A").await;
    assert!(!second.contains("[folder") && !second.contains("[memory]"), "context goes in once: {second}");

    p.send(json!({"t":"move_session","sessionId": sid,"folderId": b})).await;
    p.until(|x| x["t"] == "sessions").await;
    let moved = raw_turn(&mut p, "giờ ở B").await;
    assert!(moved.contains("Mock raw: [folder: Folder Beta | mô tả MARKER_BETA | kiểu dạy: toan]"), "{moved}");
    assert!(moved.contains(&format!("Yếu {BETA} một")), "B's memory is attached: {moved}");
    assert!(!moved.contains(ALPHA), "{moved}");
    let again = raw_turn(&mut p, "tiếp ở B").await;
    assert!(!again.contains("[folder") && !again.contains("[memory]"), "{again}");

    rpc(e.addr, &sid, "memory_note", json!({"kind":"mistake","topic":"Ghi sau khi sang B"})).await;
    let fa = e.read_memory(&format!("folders/{a}"));
    let fb = e.read_memory(&format!("folders/{b}"));
    assert!(fa.contains("Ghi khi còn ở A") && !fa.contains("Ghi sau khi sang B"), "{fa}");
    assert!(fb.contains("Ghi sau khi sang B") && !fb.contains("Ghi khi còn ở A"), "{fb}");

    // out of every folder: told once, then the no-folder memory (no folder data)
    p.send(json!({"t":"move_session","sessionId": sid,"folderId": null})).await;
    p.until(|x| x["t"] == "sessions").await;
    let out = raw_turn(&mut p, "ra ngoài").await;
    assert!(out.contains("Mock raw: [folder: không có"), "{out}");
    assert!(!out.contains(ALPHA) && !out.contains(BETA), "{out}");
}

#[tokio::test]
async fn a_pending_offer_does_not_follow_a_session_into_another_folder() {
    let World { e: _e, mut p, a, b } = world().await;
    let (sid, offer) = open(&mut p, Some(&a)).await;
    assert!(offer.iter().any(|x| x["t"] == "show" && x["block"]["type"] == "review_offer"));
    p.send(json!({"t":"move_session","sessionId": sid,"folderId": b})).await;
    p.until(|x| x["t"] == "sessions").await;
    let t = raw_turn(&mut p, "ừ ôn đi").await;
    assert!(!t.contains(ALPHA), "the offer from A is dropped after the move: {t}");
}
