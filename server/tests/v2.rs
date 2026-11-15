//! PROTOCOL v2 end to end with the mock brain, a fake utility model and a fake phone: chat mode,
//! `user` echo, uploads, dictation and STT correction, folders and sessions, folder memory, review
//! quiz kickoff, automatic titles, compaction, usage, review, weekly summary, greeting.

mod common;

use beluga_server::config::Config;
use beluga_server::http::start_with_util;
use beluga_server::util_llm::{FakeUtil, NoUtil, UtilLlm};
use beluga_server::voice::{FakeVoice, Voice};
use common::*;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

struct Env {
    dir: PathBuf,
    addr: SocketAddr,
}

impl Env {
    fn data(&self) -> PathBuf {
        self.dir.join("data")
    }
}

/// Answers like a real utility model would, per job.
fn fake_util(calls: Arc<AtomicUsize>) -> Arc<dyn UtilLlm> {
    FakeUtil::new(move |r| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(match r.purpose {
            "stt_fix" => {
                let raw = r.user.rsplit("Câu STT cần sửa: ").next().unwrap_or("").to_string();
                raw.replace("tam thứt", "tam thức")
            }
            "title" => "\"Toán · Thử tên\"".into(),
            "compact" => "- đang làm bài thử".into(),
            "weekly" => "**Tuần này học gì**\n- thử".into(),
            "greeting" => r#"{"text":"Chào cậu nè","doodle":"whale_swim"}"#.into(),
            _ => "?".into(),
        })
    })
}

async fn serve_with(dir: PathBuf, voice: Arc<Voice>, util: Arc<dyn UtilLlm>, tweak: impl FnOnce(&mut Config)) -> Env {
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let mut cfg = Config::for_test(&dir);
    cfg.mock_delay_ms = 2;
    tweak(&mut cfg);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_with_util(cfg, listener, voice, util).await;
    });
    Env { dir, addr }
}

async fn env_util(util: Arc<dyn UtilLlm>) -> Env {
    serve_with(temp_dir(), Voice::none(), util, |_| {}).await
}

async fn env() -> Env {
    env_util(fake_util(Arc::new(AtomicUsize::new(0)))).await
}

fn kinds(frames: &[Value]) -> Vec<&str> {
    frames.iter().map(|f| f["t"].as_str().unwrap_or("?")).collect()
}

fn chats(frames: &[Value]) -> Vec<String> {
    frames.iter().filter(|f| f["t"] == "chat").map(|f| f["md"].as_str().unwrap().to_string()).collect()
}

fn says(frames: &[Value]) -> Vec<String> {
    frames.iter().filter(|f| f["t"] == "say").map(|f| f["text"].as_str().unwrap().to_string()).collect()
}

async fn turn(p: &mut Phone, msg: Value) -> Vec<Value> {
    p.send(msg).await;
    p.until(is_turn_end).await
}

async fn v2(p: &mut Phone, extra: Value) -> Value {
    let mut h = json!({"v": 2});
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

fn token_of(ready: &Value) -> String {
    ready["pairToken"].as_str().unwrap().to_string()
}

// ------------------------------------------------------------------ chat mode

#[tokio::test]
async fn v2_defaults_to_chat_which_streams_markdown_without_speech() {
    let voice = Arc::new(Voice::build(Some(Arc::new(FakeVoice { fail_status: None, transcript: String::new() })), None, 3000));
    let e = serve_with(temp_dir(), voice, Arc::new(NoUtil), |_| {}).await;
    let mut p = Phone::connect(e.addr).await;
    let ready = v2(&mut p, json!({})).await;
    assert_eq!((ready["v"].as_u64(), ready["mode"].as_str(), ready["model"].as_str(), ready["effort"].as_str()), (Some(2), Some("chat"), Some("sonnet"), Some("medium")));
    let ids: Vec<&str> = ready["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["sonnet", "opus", "haiku"], "no keys in tests: no other providers");
    assert_eq!(ready["models"][0]["tracked"], true);
    assert!(ready["folderId"].is_null());
    let sid = ready["sessionId"].as_str().unwrap().to_string();

    let f = turn(&mut p, json!({"t":"user_text","text":"giải giúp tớ"})).await;
    let k = kinds(&f);
    assert!(!k.contains(&"say") && !k.contains(&"step") && !k.contains(&"audio"), "{k:?}");
    assert!(k.contains(&"chat_delta"), "{k:?}");
    let c = chats(&f);
    assert!(c[0].starts_with("Mock mode chat."), "{c:?}");
    assert!(c[0].contains("Đây là **markdown** với $x^2$.\n\n- ý một"), "{c:?}");
    assert_eq!(c[1], "### Bước 2");
    assert_eq!(c[2], "Xong phần này.");
    let show = f.iter().find(|x| x["t"] == "show").unwrap();
    assert_eq!((show["step"].as_i64(), show["block"]["type"].as_str()), (Some(-1), Some("math")));
    assert!(f.iter().filter(|x| x["t"] == "chat_delta").all(|x| x["seq"].is_null() && x["turn"] == 1));
    // deltas between two sealed events add up to the sealed markdown (before trimming)
    let first_deltas: String = f.iter().take_while(|x| x["t"] != "chat").filter(|x| x["t"] == "chat_delta").map(|x| x["text"].as_str().unwrap()).collect();
    assert_eq!(first_deltas.trim_end(), c[0]);
    // usage is pushed after the turn of a tracked model
    let after = p.drain_quiet(300).await;
    let u = after.iter().find(|x| x["t"] == "usage").expect("usage push");
    assert_eq!(u["claude"]["session"]["turns"], 1);
    assert!(u["claude"]["session"]["costUsd"].as_f64().unwrap() > 0.0);
    assert!(after.iter().all(|x| x["t"] != "audio"), "no TTS in chat");

    // history: the snapshot has user/chat/show but never chat_delta
    let mut q = Phone::connect(e.addr).await;
    q.hello(json!({"v":2, "sessionId": sid, "lastSeq": 0})).await;
    let snap = q.until(|x| x["t"] == "snapshot").await.pop().unwrap();
    let sk: Vec<&str> = snap["events"].as_array().unwrap().iter().map(|x| x["t"].as_str().unwrap()).collect();
    assert_eq!(sk, ["user", "chat", "show", "chat", "chat", "turn_end"]);
}

#[tokio::test]
async fn mode_can_change_inside_a_session_and_is_noted_once_per_change() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    let ready = p.hello(json!({})).await; // v1 app: live
    assert_eq!(ready["mode"], "live");
    let sid = ready["sessionId"].as_str().unwrap().to_string();
    let t1 = turn(&mut p, json!({"t":"user_text","text":"giảng"})).await;
    assert!(!says(&t1).is_empty());
    p.send(json!({"t":"settings","mode":"chat"})).await;
    let t2 = turn(&mut p, json!({"t":"user_text","text":"tiếp"})).await;
    assert!(says(&t2).is_empty());
    assert!(chats(&t2)[0].starts_with("Mock mode chat."), "{t2:#?}");
    let t3 = turn(&mut p, json!({"t":"user_text","text":"nữa"})).await;
    assert!(!chats(&t3)[0].contains("Mock mode"), "noted only when it changes");
    p.send(json!({"t":"list_sessions"})).await;
    let s = p.until(|x| x["t"] == "sessions").await.pop().unwrap();
    let item = s["items"].as_array().unwrap().iter().find(|x| x["sessionId"] == sid).unwrap().clone();
    assert_eq!((item["mode"].as_str(), item["preview"].as_str()), (Some("chat"), Some("nữa")));
}

#[tokio::test]
async fn model_and_effort_settings_show_in_ready() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    let sid = v2(&mut p, json!({"settings":{"model":"opus","effort":"high"}})).await["sessionId"].as_str().unwrap().to_string();
    p.send(json!({"t":"settings","model":"gpt-9","effort":"bogus"})).await; // ignored
    p.send(json!({"t":"open_session","sessionId": sid})).await;
    let r = p.until(|x| x["t"] == "ready").await.pop().unwrap();
    assert_eq!((r["model"].as_str(), r["effort"].as_str()), (Some("opus"), Some("high")));
    p.send(json!({"t":"settings","model":"haiku","effort":null})).await;
    p.send(json!({"t":"open_session","sessionId": sid})).await;
    let r = p.until(|x| x["t"] == "ready").await.pop().unwrap();
    assert_eq!(r["model"], "haiku");
    assert!(r["effort"].is_null());
}

// ------------------------------------------------------------------ uploads

#[tokio::test]
async fn uploads_need_the_pair_token_and_reach_claude_as_attachments() {
    let dir = temp_dir();
    std::fs::write(dir.join("models.json"), r#"[{"id":"sonnet","label":"S","model":"sonnet","vision":true},{"id":"blind","label":"Blind","model":"haiku","vision":false}]"#).unwrap();
    let e = serve_with(dir.clone(), Voice::none(), Arc::new(NoUtil), |c| c.models_path = dir.join("models.json")).await;
    let mut p = Phone::connect(e.addr).await;
    let token = token_of(&v2(&mut p, json!({})).await);
    let c = reqwest::Client::new();
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    let url = format!("http://{}/upload?name=de%201.jpg", e.addr);
    let r = c.post(&url).header("Content-Type", "image/jpeg").body(jpeg.clone()).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = c.post(&url).bearer_auth("nope").header("Content-Type", "image/jpeg").body(jpeg.clone()).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = c.post(&url).bearer_auth(&token).header("Content-Type", "application/pdf").body(jpeg.clone()).send().await.unwrap();
    assert_eq!(r.status(), 400, "content does not match the type");
    let up: Value = c.post(&url).bearer_auth(&token).header("Content-Type", "image/jpeg").body(jpeg.clone()).send().await.unwrap().json().await.unwrap();
    assert_eq!((up["kind"].as_str(), up["bytes"].as_u64(), up["name"].as_str()), (Some("image"), Some(7), Some("de 1.jpg")));
    let id = up["fileId"].as_str().unwrap().to_string();
    assert!(e.data().join("uploads").join(format!("{id}.jpg")).exists());
    let got = c.get(format!("http://{}/file/{id}?token={token}", e.addr)).send().await.unwrap();
    assert_eq!(got.headers()["content-type"], "image/jpeg");
    assert_eq!(got.bytes().await.unwrap().to_vec(), jpeg);
    assert_eq!(c.get(format!("http://{}/file/{id}", e.addr)).send().await.unwrap().status(), 401);
    assert_eq!(c.get(format!("http://{}/file/00000000-0000-0000-0000-000000000000?token={token}", e.addr)).send().await.unwrap().status(), 404);

    let f = turn(&mut p, json!({"t":"user_text","text":"xem đề này","files":[id, "missing-file"]})).await;
    assert!(f.iter().any(|x| x["t"] == "notice" && x["code"] == "bad_file"));
    let echo = f.iter().find(|x| x["t"] == "user").unwrap();
    assert_eq!(echo["files"], json!([{"fileId": id, "kind": "image", "name": "de 1.jpg"}]));
    assert!(chats(&f)[0].contains("Mock thấy tệp image model sonnet."), "{:?}", chats(&f));

    // a model without vision: the file is dropped with a notice
    p.send(json!({"t":"settings","model":"blind"})).await;
    let f = turn(&mut p, json!({"t":"user_text","text":"còn ảnh này","files":[id]})).await;
    assert!(f.iter().any(|x| x["t"] == "notice" && x["code"] == "no_vision"), "{f:#?}");
    assert!(!chats(&f).iter().any(|c| c.contains("Mock thấy tệp")));
}

// ------------------------------------------------------------------ dictation and STT fix

#[tokio::test]
async fn speech_is_corrected_and_dictation_never_reaches_claude() {
    let voice = Arc::new(Voice::build(None, Some(Arc::new(FakeVoice { fail_status: None, transcript: "tam thứt bậc hai".into() })), 3000));
    let e = serve_with(temp_dir(), voice, fake_util(Arc::new(AtomicUsize::new(0))), |_| {}).await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;

    // device speech: corrected, `heard` carries the raw text, the turn uses the corrected one
    let f = turn(&mut p, json!({"t":"user_text","text":"tam thứt bật hai","via":"voice"})).await;
    let h = f.iter().find(|x| x["t"] == "heard").unwrap();
    assert_eq!((h["text"].as_str(), h["raw"].as_str()), (Some("tam thức bật hai"), Some("tam thứt bật hai")));
    let u = f.iter().find(|x| x["t"] == "user").unwrap();
    assert_eq!((u["text"].as_str(), u["via"].as_str()), (Some("tam thức bật hai"), Some("voice")));

    // typed text is never touched
    let f = turn(&mut p, json!({"t":"user_text","text":"tam thứt"})).await;
    assert!(f.iter().all(|x| x["t"] != "heard"));
    assert_eq!(f.iter().find(|x| x["t"] == "user").unwrap()["text"], "tam thứt");

    // dictation (device): heard{draft} only
    p.drain_quiet(300).await;
    p.send(json!({"t":"user_text","text":"tam thứt","draft":true,"via":"voice"})).await;
    let d = p.drain_quiet(400).await;
    assert_eq!(kinds(&d), ["heard"]);
    assert_eq!((d[0]["text"].as_str(), d[0]["draft"].as_bool()), (Some("tam thức"), Some(true)));

    // dictation (server STT)
    p.send(json!({"t":"audio_start","mime":"audio/wav","sampleRate":16000,"draft":true})).await;
    p.send_binary(vec![0u8; 320]).await;
    p.send(json!({"t":"audio_end"})).await;
    let d = p.drain_quiet(400).await;
    assert_eq!(kinds(&d), ["heard"], "{d:#?}");
    assert_eq!((d[0]["text"].as_str(), d[0]["raw"].as_str(), d[0]["draft"].as_bool()), (Some("tam thức bậc hai"), Some("tam thứt bậc hai"), Some(true)));

    // server STT, not a draft: heard then a turn
    p.send(json!({"t":"audio_start","mime":"audio/wav","sampleRate":16000})).await;
    p.send_binary(vec![0u8; 320]).await;
    p.send(json!({"t":"audio_end"})).await;
    let f = p.until(is_turn_end).await;
    assert_eq!(f.iter().find(|x| x["t"] == "user").unwrap()["text"], "tam thức bậc hai");

    // sttFix off: speech goes through as heard
    p.send(json!({"t":"settings","sttFix":false})).await;
    let f = turn(&mut p, json!({"t":"user_text","text":"tam thứt","via":"voice"})).await;
    assert_eq!(f.iter().find(|x| x["t"] == "user").unwrap()["text"], "tam thứt");
}

#[tokio::test]
async fn a_slow_correction_falls_back_to_the_raw_text_within_the_deadline() {
    let e = env_util(FakeUtil::slow(5000, |_| Ok("sửa muộn".into()))).await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    let t0 = Instant::now();
    let f = turn(&mut p, json!({"t":"user_text","text":"tam thứt","via":"voice"})).await;
    assert_eq!(f.iter().find(|x| x["t"] == "user").unwrap()["text"], "tam thứt");
    let ms = t0.elapsed().as_millis();
    assert!((2400..4000).contains(&ms), "{ms} ms");
}

// ------------------------------------------------------------------ folders and sessions

#[tokio::test]
async fn folders_and_session_management() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    let first = v2(&mut p, json!({})).await["sessionId"].as_str().unwrap().to_string();
    turn(&mut p, json!({"t":"user_text","text":"phiên ngoài folder"})).await;

    p.send(json!({"t":"create_folder","name":"Toán giữa kỳ","subject":"Toán"})).await;
    let f = p.until(|x| x["t"] == "folders").await;
    let id = f.iter().find(|x| x["t"] == "folder_created").unwrap()["id"].as_str().unwrap().to_string();
    let item = &f.last().unwrap()["items"][0];
    assert_eq!((item["name"].as_str(), item["subject"].as_str(), item["sessionCount"].as_u64(), item["hasKnowledge"].as_bool()), (Some("Toán giữa kỳ"), Some("toan"), Some(0), Some(false)));

    p.send(json!({"t":"new_session","folderId": id, "mode":"live"})).await;
    let r = p.until(|x| x["t"] == "ready").await.pop().unwrap();
    assert_eq!((r["folderId"].as_str(), r["mode"].as_str()), (Some(id.as_str()), Some("live")));
    let sid = r["sessionId"].as_str().unwrap().to_string();
    assert_ne!(sid, first);
    turn(&mut p, json!({"t":"user_text","text":"bài trong folder"})).await;

    p.send(json!({"t":"list_sessions","folderId": id})).await;
    let s = p.until(|x| x["t"] == "sessions").await.pop().unwrap();
    assert_eq!(s["items"].as_array().unwrap().len(), 1);
    assert_eq!((s["items"][0]["folderId"].as_str(), s["items"][0]["preview"].as_str()), (Some(id.as_str()), Some("bài trong folder")));
    p.send(json!({"t":"list_folders"})).await;
    assert_eq!(p.until(|x| x["t"] == "folders").await.pop().unwrap()["items"][0]["sessionCount"], 1);

    // rename the open session: the running actor keeps the new title (a later turn does not undo it)
    p.send(json!({"t":"rename_session","sessionId": sid, "title":"Đề số 3"})).await;
    let fr = p.until(|x| x["t"] == "sessions").await;
    assert!(fr.iter().any(|x| x["t"] == "title" && x["text"] == "Đề số 3"));
    turn(&mut p, json!({"t":"user_text","text":"tiếp"})).await;
    p.send(json!({"t":"list_sessions"})).await;
    let s = p.until(|x| x["t"] == "sessions").await.pop().unwrap();
    assert!(s["items"].as_array().unwrap().iter().any(|x| x["title"] == "Đề số 3"));

    // move the older session in, then delete the folder: both lose their folder
    p.send(json!({"t":"move_session","sessionId": first, "folderId": id})).await;
    p.until(|x| x["t"] == "sessions").await;
    p.send(json!({"t":"move_session","sessionId": first, "folderId": "no-such-folder"})).await;
    assert_eq!(p.until(|x| x["t"] == "error").await.pop().unwrap()["code"], "no_folder");
    p.send(json!({"t":"rename_folder","id": id, "name":"Đổi tên"})).await;
    assert_eq!(p.until(|x| x["t"] == "folders").await.pop().unwrap()["items"][0]["name"], "Đổi tên");
    std::fs::create_dir_all(e.data().join("folders").join(&id)).unwrap();
    std::fs::write(e.data().join("folders").join(&id).join("knowledge.json"), "{}").unwrap();
    p.send(json!({"t":"delete_folder","id": id})).await;
    assert_eq!(p.until(|x| x["t"] == "folders").await.pop().unwrap()["items"], json!([]));
    assert!(!e.data().join("folders").join(&id).exists(), "knowledge goes with the folder");
    p.send(json!({"t":"list_sessions"})).await;
    let s = p.until(|x| x["t"] == "sessions").await.pop().unwrap();
    assert!(s["items"].as_array().unwrap().iter().all(|x| x["folderId"].is_null()), "{s}");

    // delete: not the open one; another one goes away with its journal
    p.send(json!({"t":"delete_session","sessionId": sid})).await;
    assert_eq!(p.until(|x| x["t"] == "error").await.pop().unwrap()["code"], "busy_session");
    p.until(|x| x["t"] == "sessions").await;
    p.send(json!({"t":"delete_session","sessionId": first})).await;
    let s = p.until(|x| x["t"] == "sessions").await.pop().unwrap();
    assert_eq!(s["items"].as_array().unwrap().len(), 1);
    assert!(!e.data().join("sessions").join(format!("{first}.jsonl")).exists());
}

#[tokio::test]
async fn hello_folder_and_old_sessions_are_sorted_into_subject_folders() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(
        dir.join("data").join("sessions.json"),
        r#"[{"sessionId":"old-1","title":"Toán · Hàm số · 01/11","updatedAt":"2026-11-01T10:00:00+07:00"},
            {"sessionId":"old-2","title":"Phiên mới · 02/11","updatedAt":"2026-11-02T10:00:00+07:00"}]"#,
    )
    .unwrap();
    let e = serve_with(dir, Voice::none(), Arc::new(NoUtil), |_| {}).await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    p.send(json!({"t":"list_folders"})).await;
    let f = p.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert_eq!((f["items"][0]["name"].as_str(), f["items"][0]["subject"].as_str(), f["items"][0]["sessionCount"].as_u64()), (Some("Toán"), Some("toan"), Some(1)));
    let fid = f["items"][0]["id"].as_str().unwrap().to_string();
    p.send(json!({"t":"list_sessions"})).await;
    let s = p.until(|x| x["t"] == "sessions").await.pop().unwrap();
    let get = |id: &str| s["items"].as_array().unwrap().iter().find(|x| x["sessionId"] == id).unwrap().clone();
    assert_eq!((get("old-1")["folderId"].as_str(), get("old-1")["mode"].as_str()), (Some(fid.as_str()), Some("live")));
    assert!(get("old-2")["folderId"].is_null());
    let mut q = Phone::connect(e.addr).await;
    assert_eq!(v2(&mut q, json!({"folderId": fid})).await["folderId"], fid.as_str());
}

// ------------------------------------------------------------------ folder memory, review quiz

fn knowledge() -> &'static str {
    r#"{"v":1,"curriculum":[{"id":"c1","title":"Căn bậc hai","topics":[
        {"id":"c1.t1","title":"Rút gọn biểu thức chứa căn"},{"id":"c1.t2","title":"Trục căn thức"},{"id":"c1.t3","title":"Phương trình chứa căn"}]}],
        "schoolProgress":{"coveredTopicIds":["c1.t1"],"currentTopicId":"c1.t2","confidence":0.7}}"#
}

async fn make_folder(p: &mut Phone, e: &Env) -> String {
    p.send(json!({"t":"create_folder","name":"Toán 9","subject":"toan"})).await;
    let id = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();
    p.until(|x| x["t"] == "folders").await;
    let d = e.data().join("folders").join(&id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("knowledge.json"), knowledge()).unwrap();
    id
}

#[tokio::test]
async fn folder_sessions_use_the_folder_memory_and_school_progress() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    let fid = make_folder(&mut p, &e).await;
    p.send(json!({"t":"new_session","folderId": fid})).await;
    let sid = p.until(|x| x["t"] == "ready").await.pop().unwrap()["sessionId"].as_str().unwrap().to_string();

    let r = rpc(e.addr, &sid, "memory_note", json!({"kind":"understood","topic":"Rút gọn biểu thức chứa căn","note":"ví dụ số 4 và 9","topicId":"c1.t1"})).await;
    assert!(r["isError"].is_null(), "{r}");
    rpc(e.addr, &sid, "memory_note", json!({"subject":"toan","kind":"mistake","topic":"Trục căn thức","note":"quên nhân liên hợp","topicId":"c1.t2"})).await;
    let file = std::fs::read_to_string(e.dir.join("tutor").join("memory").join("folders").join(format!("{fid}.md"))).unwrap();
    assert!(file.contains("- [hiểu] Rút gọn biểu thức chứa căn | ngày ") && file.contains("| id c1.t1 | ví dụ số 4 và 9"), "{file}");
    assert!(file.contains("- [yếu] Trục căn thức | sai 1"), "{file}");
    assert!(!e.dir.join("tutor").join("memory").join("toan.md").exists());

    let f = turn(&mut p, json!({"t":"user_text","text":"học tiếp"})).await;
    let all = chats(&f).join("\n");
    assert!(all.contains("Mock nhớ: Trên trường đang học: Trục căn thức [c1.t2] (độ tin 0.7)"), "{all}");
    assert!(all.contains("CHƯA học (muốn dùng thì phải dạy từ đầu): Phương trình chứa căn [c1.t3]"), "{all}");
    assert!(all.contains("Đã hiểu sau khi giảng lại: Rút gọn biểu thức chứa căn (ví dụ số 4 và 9)"), "{all}");

    // review frame: curriculum topics with the memory laid over them
    p.send(json!({"t":"get_review","folderId": fid})).await;
    let rv = p.until(|x| x["t"] == "review").await.pop().unwrap();
    let topics = rv["topics"].as_array().unwrap();
    let st = |id: &str| topics.iter().find(|t| t["topicId"] == id).unwrap()["state"].as_str().unwrap().to_string();
    assert_eq!((st("c1.t1"), st("c1.t2"), st("c1.t3")), ("solid".into(), "weak".into(), "new".into()));
    assert_eq!(topics[0]["chapter"], "Căn bậc hai");
    assert_eq!(rv["weak"][0]["topic"], "Trục căn thức");
    assert!(rv["weekly"].is_null());
    assert_eq!(rv["sessions"][0]["sessionId"], sid.as_str());

    // weekly summary from the utility model, saved and returned in a new review
    p.send(json!({"t":"get_weekly","folderId": fid})).await;
    let rv = p.until(|x| x["t"] == "review").await.pop().unwrap();
    assert_eq!(rv["weekly"]["md"], "**Tuần này học gì**\n- thử");
    assert!(e.data().join("folders").join(&fid).join("weekly.json").exists());
    p.send(json!({"t":"get_review","folderId":"nope"})).await;
    assert_eq!(p.until(|x| x["t"] == "error").await.pop().unwrap()["code"], "no_folder");
}

#[tokio::test]
async fn review_quiz_kickoff_starts_the_first_turn_itself() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    let fid = make_folder(&mut p, &e).await;
    let mem = e.dir.join("tutor").join("memory").join("folders");
    std::fs::create_dir_all(&mem).unwrap();
    std::fs::write(mem.join(format!("{fid}.md")), "# x\n\n## Đang yếu\n- [yếu] Trục căn thức | sai 2 | đúng 0 | bậc 0 | lần cuối 2026-01-01 | ôn 2026-01-02 | quên liên hợp\n").unwrap();
    p.send(json!({"t":"new_session","folderId": fid,"mode":"chat","kickoff":"review_quiz"})).await;
    let f = p.until(is_turn_end).await;
    assert!(f.iter().any(|x| x["t"] == "ready"));
    assert!(f.iter().all(|x| x["t"] != "user"), "the kickoff is not a student message");
    let all = chats(&f).join("\n");
    assert!(all.contains("Mock ôn nhanh: Toán · Trục căn thức (sai 2, tới hạn): quên liên hợp"), "{all}");
    assert!(f.iter().all(|x| x["block"]["type"] != "review_offer"), "no separate offer");
}

#[tokio::test]
async fn a_folder_session_offers_review_from_the_folder_file() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    let fid = make_folder(&mut p, &e).await;
    let mem = e.dir.join("tutor").join("memory").join("folders");
    std::fs::create_dir_all(&mem).unwrap();
    std::fs::write(mem.join(format!("{fid}.md")), "## Đang yếu\n- [yếu] Trục căn thức | sai 2 | đúng 0 | bậc 0 | lần cuối 2026-01-01 | ôn 2026-01-02\n").unwrap();
    p.send(json!({"t":"new_session","folderId": fid,"mode":"chat"})).await;
    let f = p.until(is_turn_end).await;
    assert!(chats(&f)[0].contains("trục căn thức"), "{f:#?}");
    let show = f.iter().find(|x| x["t"] == "show").unwrap();
    assert_eq!((show["step"].as_i64(), show["block"]["type"].as_str()), (Some(-1), Some("review_offer")));
}

// ------------------------------------------------------------------ titles, compaction

#[tokio::test]
async fn auto_title_and_compaction_with_the_utility_model() {
    let e = env().await;
    let mut p = Phone::connect(e.addr).await;
    let sid = v2(&mut p, json!({})).await["sessionId"].as_str().unwrap().to_string();
    turn(&mut p, json!({"t":"user_text","text":"#ctx giảng bài dài"})).await;
    let after = p.drain_quiet(500).await;
    let title = after.iter().find(|x| x["t"] == "title").expect("auto title");
    assert!(title["text"].as_str().unwrap().starts_with("Toán · Thử tên · "), "{title}");
    assert!(after.iter().any(|x| x["t"] == "notice" && x["code"] == "compacted"), "{after:#?}");
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(e.data().join("sessions.json")).unwrap()).unwrap();
    let m = meta.as_array().unwrap().iter().find(|m| m["sessionId"] == sid.as_str()).unwrap().clone();
    assert!(m["claudeId"].as_str().map(|c| c != sid).unwrap_or(false), "{m}");
    assert_eq!(m["claudeStarted"], false);
    assert_eq!(m["titled"], true);

    let f = turn(&mut p, json!({"t":"user_text","text":"tiếp"})).await;
    let all = chats(&f).join("\n");
    assert!(all.contains("Mock thấy tóm tắt."), "{all}");
    assert!(all.contains("Mock mode chat."), "the fresh context hears the mode again");
    // no second title request, no second compaction
    let after = p.drain_quiet(300).await;
    assert!(after.iter().all(|x| x["t"] != "title" && x["code"] != "compacted"));
}

#[tokio::test]
async fn without_a_utility_model_titles_stay_and_nothing_is_compacted() {
    let e = env_util(Arc::new(NoUtil)).await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    turn(&mut p, json!({"t":"user_text","text":"#ctx giảng"})).await;
    let after = p.drain_quiet(300).await;
    assert!(after.iter().all(|x| x["t"] != "title" && x["code"] != "compacted"), "{after:#?}");
}

// ------------------------------------------------------------------ usage, greeting, reserved

#[tokio::test]
async fn usage_greeting_and_reserved_messages() {
    let calls = Arc::new(AtomicUsize::new(0));
    let e = env_util(fake_util(calls.clone())).await;
    let mut p = Phone::connect(e.addr).await;
    v2(&mut p, json!({})).await;
    p.send(json!({"t":"get_usage"})).await;
    let u = p.until(|x| x["t"] == "usage").await.pop().unwrap();
    assert_eq!(u["claude"]["session"]["turns"], 0);
    for k in ["today", "week"] {
        assert!(u["claude"][k]["inputTokens"].is_u64(), "{u}");
    }
    assert!(u["other"]["today"]["costUsd"].is_number() && u["ingest"]["total"]["costUsd"].is_number());

    p.send(json!({"t":"get_greeting","localTime":"2026-11-15T21:00","name":"Minh"})).await;
    let g = p.until(|x| x["t"] == "greeting").await.pop().unwrap();
    assert_eq!((g["text"].as_str(), g["doodle"].as_str()), (Some("Chào cậu nè"), Some("whale_swim")));
    let n = calls.load(Ordering::SeqCst);
    p.send(json!({"t":"get_greeting","localTime":"2026-11-15T21:30","name":"Minh"})).await;
    assert_eq!(p.until(|x| x["t"] == "greeting").await.pop().unwrap()["text"], "Chào cậu nè");
    assert_eq!(calls.load(Ordering::SeqCst), n, "cached");
    // utility calls are journaled as "other" spend
    p.send(json!({"t":"get_usage"})).await;
    let u = p.until(|x| x["t"] == "usage").await.pop().unwrap();
    assert!(u["other"]["today"]["costUsd"].as_f64().unwrap() > 0.0, "{u}");

    for t in ["ingest_start", "ingest_cancel", "get_knowledge", "gen_exam", "list_exams", "delete_exam"] {
        p.send(json!({"t": t, "folderId": "x"})).await;
        let n = p.until(|x| x["t"] == "notice").await.pop().unwrap();
        assert_eq!(n["code"], "not_ready", "{t}");
    }

    let e2 = env_util(Arc::new(NoUtil)).await;
    let mut q = Phone::connect(e2.addr).await;
    q.hello(json!({})).await;
    q.send(json!({"t":"get_greeting","localTime":"2026-11-15T01:45"})).await;
    let g = q.until(|x| x["t"] == "greeting").await.pop().unwrap();
    assert_eq!(g["doodle"], "cat_sleep", "built-in fallback");
    let _ = Duration::from_millis(0);
}
