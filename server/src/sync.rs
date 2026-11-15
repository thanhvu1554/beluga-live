//! Phone replica sync (PROTOCOL section 19): `GET /sync/manifest`, `/sync/folder/<id>`,
//! `/sync/session/<id>`, `/sync/profile`, and the WebSocket `sync` hint.
//!
//! Etags are cheap: each one is a SHA-256 (first 16 hex digits) over a short descriptor built from
//! file sizes and modification times plus small index fields; no journal or knowledge file is read
//! to compute one. They are opaque strings: the JSON carries them bare, the HTTP `ETag` header
//! carries the same value in quotes, and `If-None-Match` accepts either form.
//!
//! Hints: whatever might change replicated data calls [`crate::store::Store::changed`] (the
//! store itself does on session index writes and at every `turn_end`; `http.rs` does after
//! mutating WebSocket requests, job broadcasts and tutor MCP calls). That only wakes the task in
//! [`spawn`], which waits a moment to coalesce, recomputes every etag and pushes
//! `sync{folders?, sessions?, profile?}` with exactly the entries whose etag changed, appeared or
//! disappeared. A periodic pass (every minute) also catches edits made outside the server (memory
//! files touched by hand) and the date change (review due lists depend on today's date).

use crate::http::AppState;
use crate::session::Deps;
use crate::store::{is_valid_session_id, SessionMeta};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// Coalescing delay between a change signal and the etag diff.
const SETTLE: Duration = Duration::from_millis(200);
/// Safety-net rescan interval.
const RESCAN: Duration = Duration::from_secs(60);

/// WebSocket requests that may change replicated data (the hint task diffs etags afterwards, so
/// a request that changed nothing produces no hint).
pub fn is_mutation(t: &str) -> bool {
    matches!(
        t,
        "create_folder"
            | "update_folder"
            | "rename_folder"
            | "delete_folder"
            | "move_session"
            | "rename_session"
            | "delete_session"
            | "delete_exam"
            | "ingest_cancel"
            | "get_weekly"
    )
}

// ----- etags -----

fn hash16(s: &str) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, s.as_bytes());
    d.as_ref()[..8].iter().fold(String::with_capacity(16), |mut o, b| {
        let _ = write!(o, "{b:02x}");
        o
    })
}

/// `len:mtime_ns` of a file, `-` when missing.
fn stat_sig(p: &Path) -> String {
    match std::fs::metadata(p) {
        Ok(m) => {
            let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            format!("{}:{t}", m.len())
        }
        Err(_) => "-".into(),
    }
}

/// Every file under `dir` (sorted, recursive) as `relpath=len:mtime` lines.
fn dir_sig(dir: &Path, out: &mut String) {
    fn walk(base: &Path, dir: &Path, out: &mut String) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        for p in v {
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                let _ = writeln!(out, "{rel}={}", stat_sig(&p));
            }
        }
    }
    walk(dir, dir, out);
}

fn journal_path(deps: &Deps, id: &str) -> std::path::PathBuf {
    deps.store.dir().join("sessions").join(format!("{id}.jsonl"))
}

fn memory_dir(deps: &Deps) -> std::path::PathBuf {
    deps.cfg.tutor_dir.join("memory")
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Session etag: index fields shown in the bundle plus the journal's size and mtime (the journal
/// is append-only, so its size moves with the number of events).
pub fn session_etag(deps: &Deps, m: &SessionMeta) -> String {
    hash16(&format!(
        "S|{}|{}|{}|{}|{}|{}",
        m.session_id,
        m.title,
        m.updated_at,
        m.mode,
        m.folder_id.as_deref().unwrap_or(""),
        stat_sig(&journal_path(deps, &m.session_id))
    ))
}

/// Folder etag: the `folders` item (minus live ingest progress), every file of the folder's data
/// directory (knowledge, exams, weekly summary, ingest records), its memory file, the sessions in
/// it (the review lists them with their step counts) and today's date (review due lists).
pub fn folder_etag(deps: &Deps, item: &Value, sessions: &[SessionMeta]) -> String {
    let id = item["id"].as_str().unwrap_or("");
    let mut item = item.clone();
    if let Some(o) = item.as_object_mut() {
        o.remove("ingest");
    }
    let mut s = format!("F|{}|{item}\n", today());
    dir_sig(&deps.store.dir().join("folders").join(id), &mut s);
    let _ = writeln!(s, "mem={}", stat_sig(&memory_dir(deps).join("folders").join(format!("{id}.md"))));
    for m in sessions.iter().filter(|m| m.folder_id.as_deref() == Some(id)) {
        let _ = writeln!(s, "s={}|{}|{}|{}|{}", m.session_id, m.title, m.updated_at, m.mode, stat_sig(&journal_path(deps, &m.session_id)));
    }
    hash16(&s)
}

/// Profile etag: `profile.md` and the subject memory files (the `progress` frame), plus the date.
pub fn profile_etag(deps: &Deps) -> String {
    let dir = memory_dir(deps);
    let mut s = format!("P|{}\n", today());
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut v: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
        v.sort();
        for p in v {
            let _ = writeln!(s, "{}={}", p.file_name().unwrap_or_default().to_string_lossy(), stat_sig(&p));
        }
    }
    hash16(&s)
}

/// Voices etag: the catalogue, the owner's choice and the stored clips.
pub fn voices_etag(deps: &Deps) -> String {
    let mut s = format!("V|{}|{}|{}\n", deps.voices.current(), stat_sig(&deps.cfg.voices_path), stat_sig(&deps.store.dir().join("voice.json")));
    dir_sig(&deps.cfg.voice_clips_dir, &mut s);
    hash16(&s)
}

/// Every etag of the replica, keyed for diffing.
#[derive(Default, PartialEq, Clone, Debug)]
pub struct Etags {
    pub profile: String,
    pub voices: String,
    pub folders: BTreeMap<String, String>,
    /// sessionId -> (etag, folderId)
    pub sessions: BTreeMap<String, (String, Option<String>)>,
}

pub fn etags(deps: &Deps) -> Etags {
    let sessions = deps.store.list_sessions();
    let folders_frame = deps.folders_frame();
    let mut e = Etags { profile: profile_etag(deps), voices: voices_etag(deps), ..Default::default() };
    for item in folders_frame["items"].as_array().into_iter().flatten() {
        if let Some(id) = item["id"].as_str() {
            e.folders.insert(id.to_string(), folder_etag(deps, item, &sessions));
        }
    }
    for m in &sessions {
        e.sessions.insert(m.session_id.clone(), (session_etag(deps, m), m.folder_id.clone()));
    }
    e
}

/// The `sync` hint between two etag sets, or `None` when nothing the phone mirrors changed.
pub fn hint(old: &Etags, new: &Etags) -> Option<Value> {
    fn changed<V: PartialEq>(a: &BTreeMap<String, V>, b: &BTreeMap<String, V>) -> Vec<String> {
        let mut out: Vec<String> = b.iter().filter(|(k, v)| a.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        out.extend(a.keys().filter(|k| !b.contains_key(*k)).cloned());
        out
    }
    let folders = changed(&old.folders, &new.folders);
    let sessions = changed(&old.sessions, &new.sessions);
    let profile = old.profile != new.profile;
    if folders.is_empty() && sessions.is_empty() && !profile {
        return None;
    }
    let mut v = json!({"t": "sync"});
    if !folders.is_empty() {
        v["folders"] = json!(folders);
    }
    if !sessions.is_empty() {
        v["sessions"] = json!(sessions);
    }
    if profile {
        v["profile"] = json!(true);
    }
    Some(v)
}

/// Install the store's change hook and run the hint task: push `sync` to every phone (and mark
/// the backup dirty) whenever an etag changes.
pub fn spawn(st: Arc<AppState>) {
    let wake = Arc::new(tokio::sync::Notify::new());
    let w = wake.clone();
    st.deps.store.set_on_change(move || w.notify_one());
    tokio::spawn(async move {
        let d = st.clone();
        let mut last = tokio::task::spawn_blocking(move || etags(&d.deps)).await.unwrap_or_default();
        loop {
            tokio::select! {
                _ = wake.notified() => {}
                _ = tokio::time::sleep(RESCAN) => {}
            }
            tokio::time::sleep(SETTLE).await;
            let d = st.clone();
            let Ok(now) = tokio::task::spawn_blocking(move || etags(&d.deps)).await else { continue };
            if let Some(h) = hint(&last, &now) {
                tracing::debug!("sync hint: {h}");
                st.broadcast(&h);
                st.backup.mark_dirty();
            }
            last = now;
        }
    });
}

// ----- HTTP -----

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sync/manifest", get(manifest_get))
        .route("/sync/folder/{id}", get(folder_get))
        .route("/sync/session/{id}", get(session_get))
        .route("/sync/profile", get(profile_get))
}

#[derive(serde::Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

fn authorized(st: &AppState, q: &TokenQuery, h: &HeaderMap) -> bool {
    let bearer = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim()
        .to_string();
    let token = q.token.clone().unwrap_or(bearer);
    st.deps.store.check_token(&token)
}

/// True when `If-None-Match` names `etag` (quoted or not, weak or not, or `*`).
fn not_modified(h: &HeaderMap, etag: &str) -> bool {
    let Some(v) = h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) else { return false };
    v.split(',').map(|t| t.trim().trim_start_matches("W/").trim_matches('"')).any(|t| t == etag || t == "*")
}

fn with_etag(h: &HeaderMap, etag: &str, body: impl FnOnce() -> Option<Value>) -> Response {
    let tag = format!("\"{etag}\"");
    if not_modified(h, etag) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, tag), (header::CACHE_CONTROL, "private, no-cache".into())]).into_response();
    }
    match body() {
        Some(v) => (
            [(header::ETAG, tag), (header::CACHE_CONTROL, "private, no-cache".to_string()), (header::CONTENT_TYPE, "application/json".to_string())],
            v.to_string(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The manifest body (without `serverTime`, which the handler adds).
pub fn manifest(deps: &Deps) -> Value {
    let e = etags(deps);
    json!({
        "v": 1,
        "profile": {"etag": e.profile},
        "folders": e.folders.iter().map(|(id, etag)| json!({"id": id, "etag": etag})).collect::<Vec<_>>(),
        "sessions": e.sessions.iter().map(|(id, (etag, f))| {
            let mut v = json!({"sessionId": id, "etag": etag});
            if let Some(f) = f {
                v["folderId"] = json!(f);
            }
            v
        }).collect::<Vec<_>>(),
        "voices": {"etag": e.voices},
    })
}

async fn manifest_get(State(st): State<Arc<AppState>>, Query(q): Query<TokenQuery>, h: HeaderMap) -> Response {
    if !authorized(&st, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let d = st.clone();
    let Ok(mut m) = tokio::task::spawn_blocking(move || manifest(&d.deps)).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let etag = hash16(&m.to_string());
    m["serverTime"] = json!(crate::store::now_rfc3339());
    with_etag(&h, &etag, || Some(m))
}

/// Exam page for offline use: same page as `/exam/<id>`, with board assets under `{{BOARD}}`.
pub fn offline_exam_html(html: &str) -> String {
    html.replace("\"/board/", "\"{{BOARD}}/").replace("'/board/", "'{{BOARD}}/").replace("(/board/", "({{BOARD}}/")
}

/// `/sync/folder/<id>` body.
pub fn folder_bundle(deps: &Deps, id: &str) -> Option<Value> {
    if !is_valid_session_id(id) {
        return None;
    }
    let item = deps.folders_frame()["items"].as_array()?.iter().find(|f| f["id"] == id)?.clone();
    let exams: Vec<Value> = deps
        .study
        .list_exams(id)
        .into_iter()
        .map(|mut e| {
            let html = e["examId"].as_str().and_then(|x| deps.study.exam_html(x)).map(|h| offline_exam_html(&h));
            e["html"] = json!(html);
            e
        })
        .collect();
    let memory_md = std::fs::read_to_string(memory_dir(deps).join("folders").join(format!("{id}.md"))).unwrap_or_default();
    Some(json!({
        "folder": item,
        "knowledge": deps.study.knowledge(id),
        "review": crate::review::review_frame(deps, id),
        "exams": exams,
        "memoryMd": memory_md,
    }))
}

async fn folder_get(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, Query(q): Query<TokenQuery>, h: HeaderMap) -> Response {
    if !authorized(&st, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let d = st.clone();
    let r = tokio::task::spawn_blocking(move || {
        let sessions = d.deps.store.list_sessions();
        let item = d.deps.folders_frame()["items"].as_array().and_then(|a| a.iter().find(|f| f["id"] == id.as_str()).cloned())?;
        let etag = folder_etag(&d.deps, &item, &sessions);
        Some((etag, id))
    })
    .await;
    let Ok(Some((etag, id))) = r else { return StatusCode::NOT_FOUND.into_response() };
    if not_modified(&h, &etag) {
        return with_etag(&h, &etag, || None);
    }
    let d = st.clone();
    let body = tokio::task::spawn_blocking(move || folder_bundle(&d.deps, &id)).await.ok().flatten();
    with_etag(&h, &etag, || body)
}

/// `/sync/session/<id>` body: the index fields and every journaled event, exactly what a full
/// `snapshot` (`open_session`, `lastSeq` 0) carries.
pub fn session_bundle(deps: &Deps, m: &SessionMeta) -> Value {
    let events: Vec<Value> = deps.store.load_events(&m.session_id).into_iter().filter(|e| e["seq"].as_u64().unwrap_or(0) > 0).collect();
    let mut v = json!({"sessionId": m.session_id, "title": m.title, "mode": m.mode, "updatedAt": m.updated_at, "events": events});
    if let Some(f) = &m.folder_id {
        v["folderId"] = json!(f);
    }
    v
}

async fn session_get(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, Query(q): Query<TokenQuery>, h: HeaderMap) -> Response {
    if !authorized(&st, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !is_valid_session_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(m) = st.deps.store.get_session(&id) else { return StatusCode::NOT_FOUND.into_response() };
    let etag = session_etag(&st.deps, &m);
    if not_modified(&h, &etag) {
        return with_etag(&h, &etag, || None);
    }
    let d = st.clone();
    let body = tokio::task::spawn_blocking(move || session_bundle(&d.deps, &m)).await.ok();
    with_etag(&h, &etag, || body)
}

/// `/sync/profile` body.
pub fn profile_bundle(deps: &Deps) -> Value {
    let md = std::fs::read_to_string(memory_dir(deps).join("profile.md")).unwrap_or_default();
    json!({"md": md, "progress": crate::memory::Memory::new(&deps.cfg.tutor_dir).progress()})
}

async fn profile_get(State(st): State<Arc<AppState>>, Query(q): Query<TokenQuery>, h: HeaderMap) -> Response {
    if !authorized(&st, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let etag = profile_etag(&st.deps);
    if not_modified(&h, &etag) {
        return with_etag(&h, &etag, || None);
    }
    let d = st.clone();
    let body = tokio::task::spawn_blocking(move || profile_bundle(&d.deps)).await.ok();
    with_etag(&h, &etag, || body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_none_match_forms() {
        let mut h = HeaderMap::new();
        assert!(!not_modified(&h, "abc"));
        h.insert(header::IF_NONE_MATCH, "\"abc\"".parse().unwrap());
        assert!(not_modified(&h, "abc"));
        h.insert(header::IF_NONE_MATCH, "W/\"x\", abc".parse().unwrap());
        assert!(not_modified(&h, "abc"));
        h.insert(header::IF_NONE_MATCH, "\"abd\"".parse().unwrap());
        assert!(!not_modified(&h, "abc"));
    }

    #[test]
    fn hint_lists_changed_added_and_removed() {
        let mut a = Etags::default();
        a.folders.insert("f1".into(), "1".into());
        a.folders.insert("f2".into(), "2".into());
        a.sessions.insert("s1".into(), ("1".into(), None));
        let mut b = a.clone();
        assert!(hint(&a, &b).is_none());
        b.folders.insert("f1".into(), "9".into());
        b.folders.remove("f2");
        b.folders.insert("f3".into(), "3".into());
        b.profile = "p".into();
        let h = hint(&a, &b).unwrap();
        assert_eq!(h["folders"], json!(["f1", "f3", "f2"]));
        assert!(h.get("sessions").is_none());
        assert_eq!(h["profile"], true);
    }

    #[test]
    fn offline_exam_assets_point_at_the_board_placeholder() {
        let h = r#"<link rel="stylesheet" href="/board/vendor/katex/katex.min.css"><script src="/board/vendor/katex/katex.min.js"></script>"#;
        let o = offline_exam_html(h);
        assert!(o.contains(r#"href="{{BOARD}}/vendor/katex/katex.min.css""#) && o.contains(r#"src="{{BOARD}}/vendor/katex/katex.min.js""#));
        assert!(!o.contains("\"/board/"));
    }
}
