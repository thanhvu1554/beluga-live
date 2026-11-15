//! The study engine wired into the WebSocket and HTTP API (PROTOCOL sections 11-13) with a fake
//! LLM: ingest jobs and their events, knowledge, exams, the exam page, cancel, folder delete, and
//! the knowledge feeding the tutor (memory digest, review overlay).

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

fn text_of(req: &LlmRequest) -> String {
    req.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
}

/// Page reply: one full exam per photo (the fake image bytes carry its number), 2 MCQ + 1 essay.
fn extract_reply(req: &LlmRequest) -> String {
    let p = req.parts.iter().find_map(|x| if let Part::Image { data, .. } = x { data.last().copied() } else { None }).unwrap_or(0);
    let qs: Vec<Value> = (0..3)
        .map(|j| {
            let essay = j == 2;
            json!({"sec": if essay { "II" } else { "I" }, "n": (if essay { 1 } else { j + 1 }).to_string(),
                "kind": if essay { "essay" } else { "mcq" }, "level": (["nb", "th", "vd"][j]), "pts": if essay { 1.5 } else { 0.25 },
                "chapter": "Căn bậc hai, căn bậc ba",
                "topic": if j == 0 { "Căn bậc hai" } else { "Biến đổi căn thức" },
                "form": if j == 0 { "Tìm điều kiện xác định".to_string() } else { format!("Rút gọn biểu thức {j}") },
                "text": format!("Câu {p}-{j}: rút gọn $\\sqrt{{{}}}$", 10 + j), "fig": false, "ans": ""})
        })
        .collect();
    json!({"doc": {"title": format!("Đề ôn {p}"), "kind": "exam", "school": "THCS Nguyễn Du", "date": "", "grade": "9", "minutes": 90,
        "points": 0, "fullExam": true, "continues": false}, "empty": false,
        "sections": [{"label": "I", "title": "Trắc nghiệm", "kind": "mcq", "note": "Chọn đáp án đúng."},
                     {"label": "II", "title": "Tự luận", "kind": "essay", "note": ""}],
        "questions": qs})
    .to_string()
}

/// Merge reply: one chapter, a topic per distinct raw topic, one form per raw.
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
    json!({"chapters": [{"id": "", "title": "Căn bậc hai, căn bậc ba", "topics": topics.iter().map(|(t, fs)| json!({
        "id": "", "title": t, "summary": "Tóm tắt", "forms": fs.iter().map(|(f, r)| json!({"id": "", "title": f, "method": "1. Bước một", "raw": [r]})).collect::<Vec<_>>()
    })).collect::<Vec<_>>()}]})
    .to_string()
}

/// Exam reply from the BLUEPRINT lines; `skip` drops one slot (to trigger a repair).
fn exam_reply(req: &LlmRequest, skip: Option<usize>) -> String {
    let t = text_of(req);
    let bp = t.split("BLUEPRINT (").nth(1).unwrap_or("");
    let mut items = Vec::new();
    for line in bp.lines().skip(1) {
        let cols: Vec<&str> = line.split(" | ").collect();
        let Ok(slot) = cols[0].trim().parse::<usize>() else { continue };
        if Some(slot) == skip {
            continue;
        }
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

fn cfg() -> StudyConfig {
    let mut c = StudyConfig::default();
    c.ingest.retry_base_ms = 1;
    c.ingest.concurrency = 2;
    c
}

fn fake(delay_ms: u64) -> FakeLlm {
    let mut f = FakeLlm::new(move |req, _n| match req.tag {
        "extract" => Ok(extract_reply(req)),
        "merge" => Ok(merge_reply(req)),
        _ => Ok(exam_reply(req, None)),
    });
    f.delay = Duration::from_millis(delay_ms);
    f
}

struct Env {
    dir: PathBuf,
    addr: SocketAddr,
}

async fn serve(delay_ms: u64) -> Env {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let mut cfg = Config::for_test(&dir);
    cfg.mock_delay_ms = 2;
    let study = StudyService::with_llm(dir.join("data"), self::cfg(), Arc::new(fake(delay_ms)));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_full(cfg, listener, Voice::none(), Arc::new(NoUtil), Some(study)).await;
    });
    Env { dir, addr }
}

async fn upload(addr: SocketAddr, token: &str, n: u8) -> String {
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

async fn setup(e: &Env) -> (Phone, String, String, Vec<String>) {
    let mut p = Phone::connect(e.addr).await;
    let token = p.hello(json!({"v": 2})).await["pairToken"].as_str().unwrap().to_string();
    p.send(json!({"t":"create_folder","name":"Toán 9","subject":"toan"})).await;
    let fid = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();
    p.until(|x| x["t"] == "folders").await;
    let mut files = Vec::new();
    for n in 1..=3 {
        files.push(upload(e.addr, &token, n).await);
    }
    (p, token, fid, files)
}

#[tokio::test]
async fn ingest_then_knowledge_exams_and_the_exam_page() {
    let e = serve(0).await;
    let (mut p, token, fid, files) = setup(&e).await;
    let mut q = Phone::connect(e.addr).await; // a second phone sees the job too
    q.hello(json!({"v": 2})).await;

    p.send(json!({"t":"ingest_start","folderId": fid, "files": files, "goal":"ôn giữa kỳ 1"})).await;
    let evs = p.until(|x| x["t"] == "knowledge").await;
    let ingest: Vec<&Value> = evs.iter().filter(|x| x["t"] == "ingest").collect();
    assert_eq!(ingest.first().unwrap()["state"], "queued");
    assert_eq!(ingest.last().unwrap()["state"], "done");
    assert!(ingest.iter().all(|x| x["folderId"] == fid.as_str() && x["jobId"].is_string()));
    let k = evs.last().unwrap();
    assert_eq!(k["folderId"], fid.as_str());
    assert!(!k["data"]["curriculum"].as_array().unwrap().is_empty());
    assert_eq!(k["ingest"]["state"], "done");
    let folders = p.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert_eq!((folders["items"][0]["hasKnowledge"].as_bool(), folders["items"][0]["ingest"]["state"].as_str()), (Some(true), Some("done")));
    assert!(q.until(|x| x["t"] == "knowledge").await.iter().any(|x| x["t"] == "ingest"));

    p.send(json!({"t":"get_knowledge","folderId": fid})).await;
    let k = p.until(|x| x["t"] == "knowledge").await.pop().unwrap();
    let sid = k["data"]["structures"][0]["id"].as_str().unwrap().to_string();
    p.send(json!({"t":"get_knowledge","folderId":"nope"})).await;
    assert!(p.until(|x| x["t"] == "knowledge").await.pop().unwrap()["data"].is_null());

    // exams
    p.send(json!({"t":"gen_exam","folderId": fid, "structureId": sid, "note":"khó hơn một chút"})).await;
    let evs = p.until(|x| x["t"] == "exams").await;
    let states: Vec<&str> = evs.iter().filter(|x| x["t"] == "exam").map(|x| x["state"].as_str().unwrap()).collect();
    assert_eq!(states.first(), Some(&"writing"));
    assert_eq!(states.last(), Some(&"done"));
    let list = evs.last().unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let exam_id = list["items"][0]["examId"].as_str().unwrap().to_string();
    let c = reqwest::Client::new();
    let page = c.get(format!("http://{}/exam/{exam_id}?token={token}", e.addr)).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    let html = page.text().await.unwrap();
    assert!(html.contains("Câu mới") && html.contains("exam_ready"), "page renders the exam");
    assert_eq!(c.get(format!("http://{}/exam/{exam_id}", e.addr)).send().await.unwrap().status(), 401);
    assert_eq!(c.get(format!("http://{}/exam/ex-nope?token={token}", e.addr)).send().await.unwrap().status(), 404);
    p.send(json!({"t":"list_exams","folderId": fid})).await;
    assert_eq!(p.until(|x| x["t"] == "exams").await.pop().unwrap()["items"][0]["examId"], exam_id.as_str());
    p.send(json!({"t":"delete_exam","examId": exam_id})).await;
    assert_eq!(p.until(|x| x["t"] == "exams").await.pop().unwrap()["items"], json!([]));

    // the knowledge feeds the tutor: [memory] has where the class is, review overlays the curriculum
    p.send(json!({"t":"new_session","folderId": fid})).await;
    p.until(|x| x["t"] == "ready").await;
    p.send(json!({"t":"user_text","text":"học tiếp"})).await;
    let turn = p.until(is_turn_end).await;
    let text: String = turn.iter().filter(|x| x["t"] == "chat").map(|x| x["md"].as_str().unwrap()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Mock nhớ: Trên trường"), "{text}");
    p.send(json!({"t":"get_review","folderId": fid})).await;
    let rv = p.until(|x| x["t"] == "review").await.pop().unwrap();
    assert!(!rv["topics"].as_array().unwrap().is_empty());
    assert!(rv["topics"].as_array().unwrap().iter().all(|t| t["state"] == "new" && t["chapter"].is_string()));

    // usage includes the study spend
    p.send(json!({"t":"get_usage"})).await;
    let u = p.until(|x| x["t"] == "usage").await.pop().unwrap();
    assert!(u["ingest"]["total"]["costUsd"].as_f64().unwrap() > 0.0, "{u}");
    assert!(u["exam"]["total"]["costUsd"].as_f64().unwrap() > 0.0, "{u}");

    // deleting the folder removes its knowledge
    let fdir = e.dir.join("data").join("folders").join(&fid);
    assert!(fdir.join("knowledge.json").exists());
    p.send(json!({"t":"delete_folder","id": fid})).await;
    p.until(|x| x["t"] == "folders").await;
    assert!(!fdir.exists());
}

#[tokio::test]
async fn a_job_survives_the_phone_going_away_and_can_be_cancelled() {
    let e = serve(150).await;
    let (mut p, token, fid, files) = setup(&e).await;
    p.send(json!({"t":"ingest_start","folderId": fid, "files": files})).await;
    assert_eq!(p.until(|x| x["t"] == "ingest").await.pop().unwrap()["state"], "queued");
    drop(p);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut q = Phone::connect(e.addr).await;
    q.hello(json!({"v": 2})).await;
    q.send(json!({"t":"list_folders"})).await;
    let f = q.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert!(f["items"][0]["ingest"]["jobId"].is_string(), "{f}");
    q.send(json!({"t":"get_knowledge","folderId": fid})).await;
    let k = q.until(|x| x["t"] == "knowledge").await.pop().unwrap();
    assert!(k["ingest"]["state"].is_string());
    // the job finishes and the reconnected phone gets the result
    let done = q.until(|x| x["t"] == "knowledge" && !x["data"].is_null()).await;
    assert_eq!(done.last().unwrap()["ingest"]["state"], "done");

    // a second job, cancelled
    let more: Vec<String> = vec![upload(e.addr, &token, 7).await, upload(e.addr, &token, 8).await, upload(e.addr, &token, 9).await];
    q.send(json!({"t":"ingest_start","folderId": fid, "files": more})).await;
    let _ = q.until(|x| x["t"] == "ingest").await;
    q.send(json!({"t":"ingest_cancel","folderId": fid})).await;
    let evs = q.until(|x| x["t"] == "knowledge").await;
    assert_eq!(evs.iter().filter(|x| x["t"] == "ingest").last().unwrap()["state"], "cancelled");
    assert!(!evs.last().unwrap()["data"].is_null(), "the earlier knowledge stays");
    // nothing to cancel: the phone gets the current state
    q.send(json!({"t":"ingest_cancel","folderId": fid})).await;
    assert_eq!(q.until(|x| x["t"] == "knowledge").await.pop().unwrap()["ingest"]["state"], "cancelled");
}

#[tokio::test]
async fn bad_ingest_requests_answer_with_an_error_event() {
    let e = serve(0).await;
    let (mut p, _token, fid, _files) = setup(&e).await;
    p.send(json!({"t":"ingest_start","folderId":"nope","files":[]})).await;
    let ev = p.until(|x| x["t"] == "ingest").await.pop().unwrap();
    assert_eq!((ev["state"].as_str(), ev["folderId"].as_str()), (Some("error"), Some("nope")));
    p.send(json!({"t":"ingest_start","folderId": fid,"files":["missing"]})).await;
    assert!(p.until(|x| x["t"] == "ingest").await.pop().unwrap()["note"].as_str().unwrap().contains("missing"));
    p.send(json!({"t":"ingest_start","folderId": fid,"files":[]})).await;
    assert_eq!(p.until(|x| x["t"] == "ingest").await.pop().unwrap()["state"], "error");
    p.send(json!({"t":"gen_exam","folderId": fid,"structureId":"s1"})).await;
    let evs = p.until(|x| x["t"] == "exams").await;
    assert!(evs.iter().any(|x| x["t"] == "exam" && x["state"] == "error"), "no knowledge yet");
    p.send(json!({"t":"delete_exam","examId":"ex-nope"})).await;
    assert_eq!(p.until(|x| x["t"] == "error").await.pop().unwrap()["code"], "no_exam");
}
