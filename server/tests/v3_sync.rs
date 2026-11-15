//! Phone replica sync (PROTOCOL section 19): the HTTP endpoints, their etags and 304s, and the
//! WebSocket `sync` hint after every kind of change. The ingest and exam parts use the same fake
//! study LLM as `v2_study.rs` (copied below).

mod common;

use beluga_server::config::Config;
use beluga_server::http::start_full;
use beluga_server::study::llm::{FakeLlm, LlmRequest, Part};
use beluga_server::study::{StudyConfig, StudyService};
use beluga_server::util_llm::NoUtil;
use beluga_server::voice::Voice;
use common::*;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

// ----------------------------------------------------------------- fake study LLM (as v2_study.rs)

fn text_of(req: &LlmRequest) -> String {
    req.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
}

fn extract_reply(req: &LlmRequest) -> String {
    let pages = req.parts.iter().filter(|p| matches!(p, Part::Image { .. } | Part::Pdf { .. })).count();
    let first_photo = text_of(req).split('"').nth(1).unwrap_or("x").to_string();
    let mut qs = Vec::new();
    for p in 1..=pages {
        for j in 0..3 {
            let essay = j == 2;
            qs.push(json!({
                "src": "A", "page": p, "section": if essay { "II" } else { "I" }, "n": format!("{}{}", p, j),
                "kind": if essay { "essay" } else { "mcq" }, "level": (["nb", "th", "vd"][j]),
                "points": if essay { 1.5 } else { 0.25 },
                "chapter": "Căn bậc hai, căn bậc ba",
                "topic": if j == 0 { "Căn bậc hai".to_string() } else { "Biến đổi căn thức".to_string() },
                "form": if j == 0 { "Tìm điều kiện xác định".to_string() } else { format!("Rút gọn biểu thức {j}") },
                "text": format!("Câu {first_photo} {p}-{j}: rút gọn $\\sqrt{{{}}}$", p * 10 + j),
                "hasFigure": false, "answer": ""
            }));
        }
    }
    json!({"sources": [{"key": "A", "pages": (1..=pages).collect::<Vec<_>>(), "title": format!("Đề ôn {first_photo}"),
        "kind": "exam", "school": "THCS Nguyễn Du", "date": "", "grade": "9", "durationMin": 90, "totalPoints": 0,
        "fullExam": true, "continuesPrevious": false,
        "sections": [{"label": "I", "title": "Trắc nghiệm", "kind": "mcq", "instructions": "Chọn đáp án đúng."},
                     {"label": "II", "title": "Tự luận", "kind": "essay", "instructions": ""}]}],
        "questions": qs})
    .to_string()
}

fn merge_reply(req: &LlmRequest) -> String {
    let t = text_of(req);
    let raw_part = t.split("RAW (").nth(1).unwrap_or("");
    let mut topics: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in raw_part.lines().skip(1) {
        let cols: Vec<&str> = line.split(" | ").collect();
        if cols.len() < 2 {
            continue;
        }
        let path: Vec<&str> = cols[1].split(" > ").collect();
        let (topic, form) = (path[1].to_string(), path[2].to_string());
        match topics.iter_mut().find(|x| x.0 == topic) {
            Some(x) => x.1.push((form, cols[0].to_string())),
            None => topics.push((topic, vec![(form, cols[0].to_string())])),
        }
    }
    let sources: Vec<String> = t.lines().filter(|l| l.starts_with('S') && l.contains(" | exam | yes")).map(|l| l.split(" | ").next().unwrap().to_string()).collect();
    json!({
        "grade": "9",
        "curriculum": [{"title": "Căn bậc hai, căn bậc ba", "topics": topics.iter().map(|(t, fs)| json!({
            "title": t, "summary": "Tóm tắt", "forms": fs.iter().map(|(f, r)| json!({"title": f, "method": "1. Bước một", "raw": [r]})).collect::<Vec<_>>()
        })).collect::<Vec<_>>()}],
        "structures": [{"title": "Đề giữa kỳ I (12 TN + tự luận)", "sources": sources}],
        "progress": {"covered": ["1.1"], "current": "1.2", "confidence": 0.6, "note": "Suy từ đề mới nhất"}
    })
    .to_string()
}

fn exam_reply(req: &LlmRequest) -> String {
    let t = text_of(req);
    let bp = t.split("BLUEPRINT (").nth(1).unwrap_or("");
    let mut items = Vec::new();
    for line in bp.lines().skip(1) {
        let cols: Vec<&str> = line.split(" | ").collect();
        let Ok(slot) = cols[0].trim().parse::<usize>() else { continue };
        let kind = cols[2];
        items.push(json!({
            "slot": slot, "text": format!("Câu mới số {slot}: tính $\\sqrt{{{}}}$", slot * slot),
            "choices": if kind == "mcq" { json!([format!("${}$", slot + 10), "$1$", "$2$", "$3$"]) } else { json!([]) },
            "statements": [], "answer": if kind == "mcq" { "A" } else { "Kết quả" },
            "solution": format!("Lời giải chi tiết cho câu {slot}: $\\sqrt{{{0}}}={1}$", slot * slot, slot)
        }));
    }
    json!({"title": "Đề luyện giữa kỳ I – Toán 9", "questions": items}).to_string()
}

fn study_cfg() -> StudyConfig {
    let mut c = StudyConfig::default();
    c.ingest.retry_base_ms = 1;
    c.ingest.batch_images = 2;
    c.ingest.concurrency = 2;
    c
}

// ----------------------------------------------------------------- harness

struct Env {
    dir: PathBuf,
    addr: SocketAddr,
}

async fn serve() -> Env {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let mut cfg = Config::for_test(&dir);
    cfg.mock_delay_ms = 2;
    let fake = FakeLlm::new(|req, _n| match req.tag {
        "extract" => Ok(extract_reply(req)),
        "merge" => Ok(merge_reply(req)),
        _ => Ok(exam_reply(req)),
    });
    let study = StudyService::with_llm(dir.join("data"), study_cfg(), Arc::new(fake));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_full(cfg, listener, Voice::none(), Arc::new(NoUtil), Some(study)).await;
    });
    Env { dir, addr }
}

async fn phone(e: &Env) -> (Phone, String, String) {
    let mut p = Phone::connect(e.addr).await;
    let ready = p.hello(json!({"v": 3})).await;
    (p, ready["pairToken"].as_str().unwrap().to_string(), ready["sessionId"].as_str().unwrap().to_string())
}

async fn create_folder(p: &mut Phone, name: &str) -> String {
    p.send(json!({"t":"create_folder","name": name,"subject":"toan"})).await;
    let id = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();
    p.until(|x| x["t"] == "folders").await;
    id
}

/// One mock tutor turn in the phone's current session.
async fn turn(p: &mut Phone, text: &str) {
    p.send(json!({"t":"user_text","text": text})).await;
    p.until(is_turn_end).await;
}

struct Http {
    c: reqwest::Client,
    addr: SocketAddr,
    token: String,
}

impl Http {
    async fn get(&self, path: &str, etag: Option<&str>) -> (u16, Option<String>, Value) {
        let mut rb = self.c.get(format!("http://{}{path}", self.addr)).bearer_auth(&self.token);
        if let Some(t) = etag {
            rb = rb.header("If-None-Match", format!("\"{t}\""));
        }
        let r = rb.send().await.unwrap();
        let status = r.status().as_u16();
        let tag = r.headers().get("etag").map(|v| v.to_str().unwrap().trim_matches('"').to_string());
        let body = r.text().await.unwrap();
        (status, tag, serde_json::from_str(&body).unwrap_or(Value::Null))
    }

    async fn manifest(&self) -> Value {
        let (s, _, v) = self.get("/sync/manifest", None).await;
        assert_eq!(s, 200);
        v
    }
}

fn folder_etag(m: &Value, id: &str) -> String {
    m["folders"].as_array().unwrap().iter().find(|f| f["id"] == id).map(|f| f["etag"].as_str().unwrap().to_string()).unwrap_or_default()
}

fn session_etag(m: &Value, id: &str) -> String {
    m["sessions"].as_array().unwrap().iter().find(|f| f["sessionId"] == id).map(|f| f["etag"].as_str().unwrap().to_string()).unwrap_or_default()
}

/// Wait for a `sync` hint that satisfies `pred`.
async fn hint(p: &mut Phone, pred: impl Fn(&Value) -> bool) -> Value {
    p.until(|x| x["t"] == "sync" && pred(x)).await.pop().unwrap()
}

fn has(v: &Value, key: &str, id: &str) -> bool {
    v[key].as_array().map(|a| a.iter().any(|x| x == id)).unwrap_or(false)
}

// ----------------------------------------------------------------- tests

#[tokio::test]
async fn manifest_shape_auth_and_not_modified() {
    let e = serve().await;
    let (mut p, token, sid) = phone(&e).await;
    let a = create_folder(&mut p, "Tiếng Anh trên lớp").await;
    turn(&mut p, "chào").await;
    let c = reqwest::Client::new();
    for path in ["/sync/manifest", &format!("/sync/folder/{a}"), &format!("/sync/session/{sid}"), "/sync/profile"] {
        let r = c.get(format!("http://{}{path}", e.addr)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path} needs the pair token");
        let r = c.get(format!("http://{}{path}?token=nope", e.addr)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path} rejects a bad token");
        let r = c.get(format!("http://{}{path}?token={token}", e.addr)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path} takes ?token=");
    }
    let h = Http { c, addr: e.addr, token };
    let (s, tag, m) = h.get("/sync/manifest", None).await;
    assert_eq!(s, 200);
    assert_eq!(m["v"], 1);
    assert!(m["serverTime"].is_string());
    assert!(m["profile"]["etag"].is_string() && m["voices"]["etag"].is_string());
    assert_eq!(m["folders"], json!([{"id": a, "etag": folder_etag(&m, &a)}]));
    let sess = m["sessions"].as_array().unwrap();
    assert_eq!(sess.len(), 1);
    assert_eq!(sess[0]["sessionId"], sid.as_str());
    assert!(sess[0]["etag"].is_string() && sess[0].get("folderId").is_none());
    let tag = tag.expect("ETag header");
    let (s, tag2, body) = h.get("/sync/manifest", Some(&tag)).await;
    assert_eq!((s, tag2.as_deref(), body), (304, Some(tag.as_str()), Value::Null));
    for (path, etag) in [(format!("/sync/folder/{a}"), folder_etag(&m, &a)), (format!("/sync/session/{sid}"), session_etag(&m, &sid)), ("/sync/profile".to_string(), m["profile"]["etag"].as_str().unwrap().to_string())] {
        let (s, t, _) = h.get(&path, None).await;
        assert_eq!((s, t.as_deref()), (200, Some(etag.as_str())), "{path}: header etag = manifest etag");
        assert_eq!(h.get(&path, Some(&etag)).await.0, 304, "{path}");
    }
    assert_eq!(h.get("/sync/folder/nope", None).await.0, 404);
    assert_eq!(h.get("/sync/session/nope", None).await.0, 404);
}

#[tokio::test]
async fn a_folder_etag_changes_exactly_when_that_folder_changes() {
    let e = serve().await;
    let (mut p, token, _) = phone(&e).await;
    let a = create_folder(&mut p, "Toán 9").await;
    let b = create_folder(&mut p, "Văn 9").await;
    let h = Http { c: reqwest::Client::new(), addr: e.addr, token };
    let m0 = h.manifest().await;
    let m1 = h.manifest().await;
    assert_eq!((folder_etag(&m0, &a), folder_etag(&m0, &b), &m0["profile"]), (folder_etag(&m1, &a), folder_etag(&m1, &b), &m1["profile"]), "stable without changes");
    let (fa, fb) = (folder_etag(&m0, &a), folder_etag(&m0, &b));

    // rename A: only A
    p.send(json!({"t":"rename_folder","id": a,"name":"Toán 9 · HK1"})).await;
    p.until(|x| x["t"] == "folders").await;
    let m = h.manifest().await;
    let fa2 = folder_etag(&m, &a);
    assert_ne!(fa2, fa);
    assert_eq!(folder_etag(&m, &b), fb);

    // B gets knowledge (as an ingest would write it): only B
    let kdir = e.dir.join("data").join("folders").join(&b);
    std::fs::create_dir_all(&kdir).unwrap();
    std::fs::write(kdir.join("knowledge.json"), r#"{"v":1,"curriculum":[]}"#).unwrap();
    let m = h.manifest().await;
    let fb2 = folder_etag(&m, &b);
    assert_ne!(fb2, fb);
    assert_eq!(folder_etag(&m, &a), fa2);

    // A's memory file: only A; the profile does not move
    let mem = e.dir.join("tutor").join("memory");
    std::fs::create_dir_all(mem.join("folders")).unwrap();
    std::fs::write(mem.join("folders").join(format!("{a}.md")), "# Trí nhớ\n- [yếu] Căn | sai 1\n").unwrap();
    let m = h.manifest().await;
    let fa3 = folder_etag(&m, &a);
    assert_ne!(fa3, fa2);
    assert_eq!((folder_etag(&m, &b), &m["profile"]), (fb2.clone(), &m0["profile"]));

    // a lesson in A: A (its review lists the session) and the new session, not B
    p.send(json!({"t":"new_session","folderId": a})).await;
    let sid = p.until(|x| x["t"] == "ready").await.pop().unwrap()["sessionId"].as_str().unwrap().to_string();
    turn(&mut p, "học căn bậc hai").await;
    let m = h.manifest().await;
    let fa4 = folder_etag(&m, &a);
    assert_ne!(fa4, fa3);
    assert_eq!(folder_etag(&m, &b), fb2);
    let s1 = session_etag(&m, &sid);
    assert!(!s1.is_empty());
    assert_eq!(m["sessions"].as_array().unwrap().iter().find(|s| s["sessionId"] == sid.as_str()).unwrap()["folderId"], a.as_str());

    // the folder bundle follows the same etag; a 304 until the next change
    let (s, tag, bundle) = h.get(&format!("/sync/folder/{a}"), None).await;
    assert_eq!((s, tag.as_deref()), (200, Some(fa4.as_str())));
    assert_eq!(bundle["folder"]["name"], "Toán 9 · HK1");
    assert!(bundle["memoryMd"].as_str().unwrap().contains("Căn"));
    assert_eq!(bundle["review"]["t"], "review");
    assert_eq!(bundle["review"]["sessions"][0]["sessionId"], sid.as_str());
    assert!(bundle["knowledge"].is_null() && bundle["exams"] == json!([]));
    assert_eq!(h.get(&format!("/sync/folder/{a}"), Some(&fa4)).await.0, 304);
    turn(&mut p, "tiếp").await;
    let (s, tag, _) = h.get(&format!("/sync/folder/{a}"), Some(&fa4)).await;
    assert_eq!(s, 200);
    assert_ne!(tag.unwrap(), fa4);
    let m = h.manifest().await;
    assert_ne!(session_etag(&m, &sid), s1, "a turn changes the session etag");
    assert_eq!(folder_etag(&m, &b), fb2);

    // the profile etag moves with profile.md
    std::fs::write(mem.join("profile.md"), "# Hồ sơ\n- [cách] Thích ví dụ số\n").unwrap();
    let m2 = h.manifest().await;
    assert_ne!(m2["profile"], m["profile"]);
    assert_eq!(folder_etag(&m2, &b), fb2);
    let (s, _, prof) = h.get("/sync/profile", None).await;
    assert_eq!(s, 200);
    assert!(prof["md"].as_str().unwrap().contains("Thích ví dụ số"));
    assert_eq!(prof["progress"]["t"], "progress");
}

#[tokio::test]
async fn the_session_bundle_equals_a_full_snapshot() {
    let e = serve().await;
    let (mut p, token, sid) = phone(&e).await;
    turn(&mut p, "giải x^2 = 4").await;
    turn(&mut p, "còn x^2 = 9").await;
    let mut q = Phone::connect(e.addr).await;
    q.hello(json!({"v": 3})).await;
    q.send(json!({"t":"open_session","sessionId": sid})).await;
    let snap = q.until(|x| x["t"] == "snapshot").await.pop().unwrap();
    let h = Http { c: reqwest::Client::new(), addr: e.addr, token };
    let (s, _, b) = h.get(&format!("/sync/session/{sid}"), None).await;
    assert_eq!(s, 200);
    assert!(!snap["events"].as_array().unwrap().is_empty());
    assert_eq!(b["events"], snap["events"]);
    assert_eq!(b["sessionId"], sid.as_str());
    let meta = h.manifest().await;
    assert!(!session_etag(&meta, &sid).is_empty());
    // (opening the session on q took it over from p)
    q.send(json!({"t":"list_sessions"})).await;
    let item = q.until(|x| x["t"] == "sessions").await.pop().unwrap()["items"][0].clone();
    assert_eq!((&b["title"], &b["mode"], &b["updatedAt"]), (&item["title"], &item["mode"], &item["updatedAt"]));
    assert!(b.get("folderId").is_none());
}

#[tokio::test]
async fn a_sync_hint_follows_every_kind_of_change() {
    let e = serve().await;
    let (mut p, token, first_sid) = phone(&e).await;
    let (mut q, _, _) = phone(&e).await; // another phone gets every hint too
    turn(&mut p, "chào").await; // the first session enters the index
    hint(&mut p, |h| has(h, "sessions", &first_sid)).await;

    // folder created / renamed
    let f = create_folder(&mut p, "Toán 9").await;
    hint(&mut p, |h| has(h, "folders", &f)).await;
    hint(&mut q, |h| has(h, "folders", &f)).await;
    p.send(json!({"t":"rename_folder","id": f,"name":"Toán 9 · ôn thi"})).await;
    hint(&mut p, |h| has(h, "folders", &f)).await;

    // a turn ends in a folder session: the session and its folder
    p.send(json!({"t":"new_session","folderId": f})).await;
    let sid = p.until(|x| x["t"] == "ready").await.pop().unwrap()["sessionId"].as_str().unwrap().to_string();
    p.send(json!({"t":"user_text","text":"học căn"})).await;
    let h = hint(&mut p, |h| has(h, "sessions", &sid)).await;
    assert!(has(&h, "folders", &f), "{h}");
    hint(&mut q, |h| has(h, "sessions", &sid)).await;

    // memory_note from the tutor: the folder (its memory file)
    let r: Value = reqwest::Client::new()
        .post(format!("http://{}/mcp/{sid}", e.addr))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_note","arguments":{"subject":"toan","kind":"mistake","topic":"Rút gọn căn","note":"quên điều kiện"}}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["result"].is_object(), "{r}");
    hint(&mut p, |h| has(h, "folders", &f)).await;
    // a style note goes to profile.md: profile
    reqwest::Client::new()
        .post(format!("http://{}/mcp/{sid}", e.addr))
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_note","arguments":{"subject":"toan","kind":"style","topic":"Thích hình vẽ"}}}))
        .send()
        .await
        .unwrap();
    hint(&mut p, |h| h["profile"] == true).await;

    // rename / move a session
    p.send(json!({"t":"rename_session","sessionId": sid,"title":"Căn thức"})).await;
    hint(&mut p, |h| has(h, "sessions", &sid)).await;
    p.send(json!({"t":"move_session","sessionId": sid,"folderId": null})).await;
    let h = hint(&mut p, |h| has(h, "sessions", &sid)).await;
    assert!(has(&h, "folders", &f), "the old folder's review lost the session: {h}");

    // ingest done: the folder
    let up = |n: u8| {
        let (addr, token) = (e.addr, token.clone());
        async move {
            let v: Value = reqwest::Client::new()
                .post(format!("http://{addr}/upload?name=trang{n}.jpg"))
                .bearer_auth(token)
                .header("Content-Type", "image/jpeg")
                .body(vec![0xFF, 0xD8, 0xFF, n])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            v["fileId"].as_str().unwrap().to_string()
        }
    };
    let files = vec![up(1).await, up(2).await];
    p.send(json!({"t":"ingest_start","folderId": f,"files": files})).await;
    p.until(|x| x["t"] == "knowledge").await;
    hint(&mut p, |h| has(h, "folders", &f)).await;
    q.until(|x| x["t"] == "sync" && has(x, "folders", &f)).await;

    // exam done, then deleted: the folder each time
    p.send(json!({"t":"get_knowledge","folderId": f})).await;
    let k = p.until(|x| x["t"] == "knowledge").await.pop().unwrap();
    let structure = k["data"]["structures"][0]["id"].as_str().unwrap().to_string();
    p.send(json!({"t":"gen_exam","folderId": f,"structureId": structure})).await;
    let list = p.until(|x| x["t"] == "exams").await.pop().unwrap();
    let exam_id = list["items"][0]["examId"].as_str().unwrap().to_string();
    hint(&mut p, |h| has(h, "folders", &f)).await;
    // the offline exam page
    let http = Http { c: reqwest::Client::new(), addr: e.addr, token: token.clone() };
    let (_, _, bundle) = http.get(&format!("/sync/folder/{f}"), None).await;
    let ex = &bundle["exams"][0];
    assert_eq!(ex["examId"], exam_id.as_str());
    let html = ex["html"].as_str().unwrap();
    assert!(html.contains("{{BOARD}}/vendor/katex/katex.min.css") && html.contains("{{BOARD}}/vendor/katex/katex.min.js"), "offline asset urls");
    assert!(!html.contains("\"/board/") && html.contains("Câu mới") && html.contains("exam_ready"));
    assert!(bundle["knowledge"]["curriculum"].is_array());
    p.send(json!({"t":"delete_exam","examId": exam_id})).await;
    hint(&mut p, |h| has(h, "folders", &f)).await;

    // deleting a session; deleting the folder
    p.send(json!({"t":"delete_session","sessionId": first_sid})).await;
    let h = hint(&mut p, |h| has(h, "sessions", &first_sid)).await;
    assert!(!has(&h, "folders", &f));
    let (s, _, _) = http.get(&format!("/sync/session/{first_sid}"), None).await;
    assert_eq!(s, 404, "a hinted id that is gone answers 404");
    p.send(json!({"t":"delete_folder","id": f})).await;
    hint(&mut p, |h| has(h, "folders", &f)).await;
    assert_eq!(http.get(&format!("/sync/folder/{f}"), None).await.0, 404);

    // a request that changes nothing brings no hint
    p.drain_quiet(400).await;
    p.send(json!({"t":"list_folders"})).await;
    p.send(json!({"t":"rename_folder","id":"nope","name":"x"})).await;
    let after = p.drain_quiet(700).await;
    assert!(after.iter().all(|x| x["t"] != "sync"), "{after:?}");
    tokio::time::sleep(Duration::from_millis(10)).await;
}
