//! PROTOCOL v3 sections 17 and 18 (apart from the isolation invariant, see `v3_isolation.rs`):
//! free-form folders with a description, the teaching kind assigned by the utility model without
//! holding up the reply, idempotent `clientId`, `update_folder`, the `[folder: …]` line, `get_home`
//! and its automatic re-push, `hello.v = 3`.

mod common;

use beluga_server::config::Config;
use beluga_server::http::start_with_util;
use beluga_server::util_llm::{FakeUtil, NoUtil, UtilLlm};
use beluga_server::voice::Voice;
use common::*;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;

struct Env {
    dir: PathBuf,
    addr: SocketAddr,
}

async fn serve(util: Arc<dyn UtilLlm>) -> Env {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let mut cfg = Config::for_test(&dir);
    cfg.mock_delay_ms = 1;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_with_util(cfg, listener, Voice::none(), util).await;
    });
    Env { dir, addr }
}

/// A utility model that names the kind after a delay and counts the kind requests.
fn kind_util(delay_ms: u64, calls: Arc<AtomicUsize>, answer: &'static str) -> Arc<dyn UtilLlm> {
    FakeUtil::slow(delay_ms, move |r| {
        if r.purpose == "folder_kind" {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(r.system.contains("ngoai-ngu") && r.user.contains("Tên folder:"), "{r:?}");
            return Ok(answer.to_string());
        }
        Ok("ok".into())
    })
}

async fn v3(p: &mut Phone) -> Value {
    p.hello(json!({"v": 3})).await
}

fn item<'a>(folders: &'a Value, id: &str) -> &'a Value {
    folders["items"].as_array().unwrap().iter().find(|x| x["id"] == id).unwrap()
}

#[tokio::test]
async fn kind_is_assigned_in_the_background_and_a_fresh_folders_follows() {
    let calls = Arc::new(AtomicUsize::new(0));
    let e = serve(kind_util(300, calls.clone(), "ngoai-ngu")).await;
    let mut p = Phone::connect(e.addr).await;
    let ready = v3(&mut p).await;
    assert_eq!(ready["v"], 3);

    let t0 = Instant::now();
    p.send(json!({"t":"create_folder","name":"Tiếng Anh trên lớp","about":"Anh 9 Global Success, cô hay cho viết lại câu","clientId":"c-1"})).await;
    let f = p.until(|x| x["t"] == "folders").await;
    assert!(t0.elapsed().as_millis() < 250, "the reply does not wait for the model");
    let created = f.iter().find(|x| x["t"] == "folder_created").unwrap();
    assert_eq!(created["clientId"], "c-1");
    let id = created["id"].as_str().unwrap().to_string();
    let first = item(f.last().unwrap(), &id).clone();
    assert_eq!((first["name"].as_str(), first["about"].as_str()), (Some("Tiếng Anh trên lớp"), Some("Anh 9 Global Success, cô hay cho viết lại câu")));
    assert_eq!(first["kind"], "ngoai-ngu", "keyword guess meanwhile");
    assert_eq!(first["subject"], first["kind"], "subject mirrors kind for v2 apps");

    // the model answers: a fresh `folders` arrives by itself
    let later = p.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert_eq!(item(&later, &id)["kind"], "ngoai-ngu");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // same clientId again (offline queue replay): the same folder, no second one
    p.send(json!({"t":"create_folder","name":"Tiếng Anh trên lớp","clientId":"c-1"})).await;
    let f = p.until(|x| x["t"] == "folders").await;
    assert_eq!(f.iter().find(|x| x["t"] == "folder_created").unwrap()["id"], id.as_str());
    assert_eq!(f.last().unwrap()["items"].as_array().unwrap().len(), 1);

    // a new description asks again; a v2 rename does too
    p.send(json!({"t":"update_folder","id": id,"about":"Luyện SSAT cuối tuần"})).await;
    let up = p.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert_eq!((item(&up, &id)["about"].as_str(), item(&up, &id)["kind"].as_str()), (Some("Luyện SSAT cuối tuần"), Some("anh-ssat")));
    let again = p.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert_eq!(item(&again, &id)["kind"], "ngoai-ngu", "the model's answer wins over the guess");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    p.send(json!({"t":"update_folder","id":"nope","name":"x"})).await;
    assert_eq!(p.until(|x| x["t"] == "error").await.pop().unwrap()["code"], "no_folder");

    let saved: Value = serde_json::from_str(&std::fs::read_to_string(e.dir.join("data").join("folders.json")).unwrap()).unwrap();
    assert_eq!((saved[0]["kind"].as_str(), saved[0]["kindPending"].as_bool(), saved[0]["clientId"].as_str()), (Some("ngoai-ngu"), Some(false), Some("c-1")));
}

#[tokio::test]
async fn a_slow_or_bad_model_keeps_the_guess_and_is_asked_again_later() {
    // past the 3 s deadline nothing changes and no fresh `folders` comes
    let e = serve(kind_util(3500, Arc::new(AtomicUsize::new(0)), "toan")).await;
    let mut p = Phone::connect(e.addr).await;
    v3(&mut p).await;
    p.send(json!({"t":"create_folder","name":"Linh tinh"})).await;
    let f = p.until(|x| x["t"] == "folders").await.pop().unwrap();
    assert_eq!(f["items"][0]["kind"], "chung");
    assert!(p.drain_quiet(3600).await.iter().all(|x| x["t"] != "folders"));
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(e.dir.join("data").join("folders.json")).unwrap()).unwrap();
    assert_eq!((saved[0]["kind"].as_str(), saved[0]["kindPending"].as_bool()), (Some("chung"), Some(true)));

    // a failing model: the next `folders` tries again
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let down = FakeUtil::new(move |r| {
        if r.purpose == "folder_kind" {
            c2.fetch_add(1, Ordering::SeqCst);
        }
        Err("down".into())
    });
    let e1 = serve(down).await;
    let mut p = Phone::connect(e1.addr).await;
    v3(&mut p).await;
    p.send(json!({"t":"create_folder","name":"Linh tinh"})).await;
    p.until(|x| x["t"] == "folders").await;
    p.drain_quiet(200).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    p.send(json!({"t":"list_folders"})).await;
    p.until(|x| x["t"] == "folders").await;
    p.drain_quiet(200).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // an answer that is not a kind is ignored too
    let e2 = serve(kind_util(0, Arc::new(AtomicUsize::new(0)), "sinh học")).await;
    let mut q = Phone::connect(e2.addr).await;
    v3(&mut q).await;
    q.send(json!({"t":"create_folder","name":"Toán hình"})).await;
    assert_eq!(q.until(|x| x["t"] == "folders").await.pop().unwrap()["items"][0]["kind"], "toan");
    assert!(q.drain_quiet(300).await.iter().all(|x| x["t"] != "folders"));

    // no utility model at all: the guess stays, nothing is attempted
    let e3 = serve(Arc::new(NoUtil)).await;
    let mut r = Phone::connect(e3.addr).await;
    v3(&mut r).await;
    r.send(json!({"t":"create_folder","name":"Hóa 10","about":""})).await;
    assert_eq!(r.until(|x| x["t"] == "folders").await.pop().unwrap()["items"][0]["kind"], "hoa");
}

#[tokio::test]
async fn first_turn_in_a_folder_names_the_folder_and_its_kind() {
    let e = serve(Arc::new(NoUtil)).await;
    let mut p = Phone::connect(e.addr).await;
    v3(&mut p).await;
    p.send(json!({"t":"create_folder","name":"Tiếng Anh trên lớp","about":"Anh 9 | sách Global Success"})).await;
    let id = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();
    p.send(json!({"t":"create_folder","name":"Toán 9","subject":"toan"})).await;
    let id2 = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();

    p.send(json!({"t":"new_session","folderId": id,"mode":"chat"})).await;
    p.until(|x| x["t"] == "ready").await;
    p.send(json!({"t":"user_text","text":"học bài mới"})).await;
    let f = p.until(is_turn_end).await;
    let chat: String = f.iter().filter(|x| x["t"] == "chat").map(|x| x["md"].as_str().unwrap()).collect::<Vec<_>>().join("\n");
    assert!(chat.contains("Mock folder: Tiếng Anh trên lớp | Anh 9 / sách Global Success | kiểu dạy: ngoai-ngu"), "{chat}");
    p.send(json!({"t":"user_text","text":"tiếp"})).await;
    let f = p.until(is_turn_end).await;
    assert!(f.iter().all(|x| !x["md"].as_str().unwrap_or("").contains("Mock folder")), "once per session");

    // no description: the middle part is left out
    p.send(json!({"t":"new_session","folderId": id2,"mode":"chat"})).await;
    p.until(|x| x["t"] == "ready").await;
    p.send(json!({"t":"user_text","text":"học"})).await;
    let f = p.until(is_turn_end).await;
    let chat: String = f.iter().filter(|x| x["t"] == "chat").map(|x| x["md"].as_str().unwrap()).collect::<Vec<_>>().join("\n");
    assert!(chat.lines().any(|l| l == "Mock folder: Toán 9 | kiểu dạy: toan"), "{chat}");
}

#[tokio::test]
async fn home_is_pushed_again_after_turns_and_list_changes_only_once_asked() {
    let e = serve(Arc::new(NoUtil)).await;
    let mut p = Phone::connect(e.addr).await;
    v3(&mut p).await;
    p.send(json!({"t":"create_folder","name":"Toán 9","subject":"toan"})).await;
    let fid = p.until(|x| x["t"] == "folder_created").await.pop().unwrap()["id"].as_str().unwrap().to_string();
    p.until(|x| x["t"] == "folders").await;

    // not asked yet: no `home` anywhere (v2 apps never see one)
    p.send(json!({"t":"user_text","text":"chào"})).await;
    let f = p.until(is_turn_end).await;
    assert!(f.iter().all(|x| x["t"] != "home"));
    assert!(p.drain_quiet(150).await.iter().all(|x| x["t"] != "home"));

    p.send(json!({"t":"get_home","folderId": fid})).await;
    let h = p.until(|x| x["t"] == "home").await.pop().unwrap();
    assert_eq!(h["folderId"], fid.as_str());
    assert!(h["continue"].is_null() && h["due"] == json!([]) && h["quick"] == json!([]), "nothing in the folder yet: {h}");

    // a turn in the folder: `turn_end` is followed by a fresh `home` of that folder
    p.send(json!({"t":"new_session","folderId": fid,"mode":"chat"})).await;
    let sid = p.until(|x| x["t"] == "ready").await.pop().unwrap()["sessionId"].as_str().unwrap().to_string();
    p.send(json!({"t":"user_text","text":"bài trong folder"})).await;
    p.until(is_turn_end).await;
    let h = p.until(|x| x["t"] == "home").await.pop().unwrap();
    assert_eq!((h["folderId"].as_str(), h["continue"]["sessionId"].as_str(), h["continue"]["folderName"].as_str()), (Some(fid.as_str()), Some(sid.as_str()), Some("Toán 9")));

    // `folders` and `sessions` replies are followed by one too
    p.send(json!({"t":"list_folders"})).await;
    let f = p.until(|x| x["t"] == "home").await;
    assert_eq!(f[f.len() - 2]["t"], "folders");
    p.send(json!({"t":"rename_session","sessionId": sid,"title":"Đề 1"})).await;
    let h = p.until(|x| x["t"] == "home").await.pop().unwrap();
    assert_eq!(h["continue"]["title"], "Đề 1");

    // the scope follows the last ask: home of everything now
    p.send(json!({"t":"get_home"})).await;
    let h = p.until(|x| x["t"] == "home").await.pop().unwrap();
    assert!(h["folderId"].is_null());
    p.send(json!({"t":"list_sessions"})).await;
    assert!(p.until(|x| x["t"] == "home").await.pop().unwrap()["folderId"].is_null());
}

/// Real utility model (DeepSeek, then OpenRouter) naming the kind of a few folders. Spends a tiny
/// amount: `DEEPSEEK_API_KEY=... cargo test --test v3 real_kind -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn real_kind_assignment() {
    let util = beluga_server::util_llm::from_keys(|k| std::env::var(k).ok());
    assert!(util.available(), "needs DEEPSEEK_API_KEY or OPENROUTER_API_KEY");
    let usage = beluga_server::usage::Usage::open(&temp_dir());
    for (name, about, want) in [
        ("Tiếng Anh trên lớp", "Anh 9 Global Success, cô hay cho ngữ pháp và viết lại câu", "ngoai-ngu"),
        ("Luyện SSAT", "Reading và verbal cho kỳ thi SSAT Upper", "anh-ssat"),
        ("Ôn thi vào 10", "Đại số và hình học lớp 9", "toan"),
        ("Sinh học", "Di truyền lớp 9", "chung"),
        ("Hóa 10", "", "hoa"),
    ] {
        let t = Instant::now();
        let got = beluga_server::folders::assign_kind(&util, &usage, name, about).await;
        println!("{name} | {about} -> {got:?} ({} ms)", t.elapsed().as_millis());
        assert_eq!(got.as_deref(), Ok(want), "{name}");
    }
    println!("cost: {}", usage.report(None)["other"]["today"]);
}
