//! HTTP + WebSocket front: `/health`, `/board/*`, `/ws`, `/upload`, `/file/<id>`, and the
//! loopback-only `/mcp/<session>` route.

use crate::config::Config;
use crate::folders::Folders;
use crate::greeting::GreetingCache;
use crate::models::Catalog;
use crate::session::{spawn_session, Cmd, Deps, MetaEdit, SessionInit};
use crate::store::{is_valid_session_id, SessionMeta, Store};
use crate::uploads::{Uploads, MAX_UPLOAD_BYTES};
use crate::usage::Usage;
use crate::util_llm::UtilLlm;
use crate::voice::Voice;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

const MAX_AUDIO_BYTES: usize = 30 * 1024 * 1024;

pub struct AppState {
    pub deps: Deps,
    sessions: Mutex<HashMap<String, UnboundedSender<Cmd>>>,
    next_conn: AtomicU64,
    /// The phone connection attached most recently: (connection id, session id).
    active: Mutex<Vec<(u64, String)>>,
    greeting: GreetingCache,
    /// Every open phone connection (for job events that outlive the request: ingest, exams).
    phones: Mutex<Vec<(u64, tokio::sync::mpsc::WeakUnboundedSender<Value>)>>,
    /// Off-VPS backup (PROTOCOL section 20).
    pub backup: crate::backup::Backup,
}

impl AppState {
    pub fn new(deps: Deps) -> Arc<AppState> {
        Arc::new(AppState {
            sessions: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            active: Mutex::new(Vec::new()),
            greeting: GreetingCache::default(),
            phones: Mutex::new(Vec::new()),
            backup: crate::backup::Backup::from_config(&deps.cfg),
            deps,
        })
    }

    /// Send a frame to every connected phone (jobs keep running when a phone goes away).
    pub fn broadcast(&self, v: &Value) {
        self.deps.store.changed();
        let mut p = self.phones.lock().unwrap();
        p.retain(|(_, w)| w.upgrade().map(|t| t.send(v.clone()).is_ok()).unwrap_or(false));
    }

    fn knowledge_frame(&self, folder_id: &str) -> Value {
        let mut k = json!({"t": "knowledge", "folderId": folder_id, "data": self.deps.study.knowledge(folder_id)});
        if let Some(i) = self.deps.study.ingest_status(folder_id) {
            k["ingest"] = i;
        }
        k
    }

    fn exams_frame(&self, folder_id: &str) -> Value {
        json!({"t": "exams", "folderId": folder_id, "items": self.deps.study.list_exams(folder_id)})
    }

    /// `ingest_start`: run the job in the background; its events go to every phone, and when it
    /// is done the new `knowledge` and `folders` follow.
    fn start_ingest(self: &Arc<Self>, v: &Value) -> Result<(), Value> {
        let folder_id = v["folderId"].as_str().unwrap_or("").to_string();
        let err = |note: &str| json!({"t": "ingest", "folderId": folder_id, "jobId": null, "state": "error", "done": 0, "total": 0, "note": note});
        let Some(folder) = self.deps.folders.get(&folder_id) else { return Err(err("Không tìm thấy folder.")) };
        let mut files = Vec::new();
        for id in v["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
            let Some(m) = self.deps.uploads.meta(id) else { return Err(err(&format!("Không tìm thấy tệp {id}."))) };
            let kind = if m.kind == "pdf" { crate::study::FileKind::Pdf } else { crate::study::FileKind::Image };
            files.push(crate::study::IngestFile { file_id: m.file_id.clone(), path: self.deps.uploads.path(&m), kind, name: m.name.clone() });
        }
        let req = crate::study::IngestRequest {
            folder_id: folder_id.clone(),
            subject: folder.subject,
            goal: v["goal"].as_str().map(String::from).filter(|g| !g.trim().is_empty()),
            files,
        };
        let mut h = self.deps.study.start_ingest(req).map_err(|e| err(&e))?;
        let st = self.clone();
        tokio::spawn(async move {
            while let Some(ev) = h.events.recv().await {
                st.broadcast(&ev);
            }
            let r = h.task.await;
            if matches!(r, Ok(Ok(_))) {
                st.deps.folders.touch(&folder_id);
            }
            st.broadcast(&st.knowledge_frame(&folder_id));
            st.broadcast(&st.deps.folders_frame());
        });
        Ok(())
    }

    /// Change a session's metadata: through its actor when it is running (so its next save does
    /// not undo the change), else directly in the index. False when the session does not exist.
    pub async fn edit_session(&self, id: &str, edit: impl FnOnce(&mut SessionMeta) + Send + 'static) -> bool {
        let edit: MetaEdit = Box::new(edit);
        let edit = match self.existing_session(id) {
            Some(tx) => {
                let (reply, rx) = tokio::sync::oneshot::channel();
                match tx.send(Cmd::EditMeta { edit, reply }) {
                    Ok(()) => return rx.await.is_ok(),
                    Err(tokio::sync::mpsc::error::SendError(Cmd::EditMeta { edit, .. })) => edit,
                    Err(_) => return false,
                }
            }
            None => edit,
        };
        let mut found = false;
        let mut edit = Some(edit);
        self.deps.store.update_sessions(|v| {
            if let Some(m) = v.iter_mut().find(|m| m.session_id == id) {
                if let Some(f) = edit.take() {
                    f(m);
                }
                found = true;
            }
        });
        found
    }

    /// Stop the actor (if any) and delete the session's files.
    pub async fn delete_session(&self, id: &str) -> bool {
        let tx = self.sessions.lock().unwrap().remove(id);
        if let Some(tx) = tx {
            let (reply, rx) = tokio::sync::oneshot::channel();
            if tx.send(Cmd::Close { reply }).is_ok() {
                let _ = rx.await;
            }
        }
        self.deps.store.delete_session(id)
    }

    pub fn set_active(&self, conn: u64, session_id: &str) {
        let mut a = self.active.lock().unwrap();
        a.retain(|(c, _)| *c != conn);
        a.push((conn, session_id.to_string()));
    }

    pub fn clear_active(&self, conn: u64) {
        self.active.lock().unwrap().retain(|(c, _)| *c != conn);
    }

    /// Session of the newest phone connection still alive (target of the general MCP tools).
    pub fn active_session(&self) -> Option<UnboundedSender<Cmd>> {
        let id = self.active.lock().unwrap().last().map(|(_, s)| s.clone())?;
        self.existing_session(&id)
    }

    pub fn existing_session(&self, id: &str) -> Option<UnboundedSender<Cmd>> {
        self.sessions.lock().unwrap().get(id).filter(|t| !t.is_closed()).cloned()
    }

    pub fn session(&self, id: &str) -> UnboundedSender<Cmd> {
        let mut m = self.sessions.lock().unwrap();
        if let Some(t) = m.get(id).filter(|t| !t.is_closed()) {
            return t.clone();
        }
        let tx = spawn_session(id.to_string(), self.deps.clone());
        m.insert(id.to_string(), tx.clone());
        tx
    }
}

/// Build state from `cfg` (probing ElevenLabs once if configured) and serve forever.
pub async fn start(cfg: Config, listener: TcpListener) -> std::io::Result<()> {
    let voice = Arc::new(Voice::from_config(&cfg));
    start_with(cfg, listener, voice).await
}

/// The utility model from the keys in the config (none in `--mock` unless `BELUGA_UTIL_IN_MOCK`).
pub fn util_from_config(cfg: &Config) -> Arc<dyn UtilLlm> {
    if cfg.mock && !cfg.util_in_mock {
        return Arc::new(crate::util_llm::NoUtil);
    }
    crate::util_llm::from_keys(|k| cfg.secret(k))
}

pub async fn start_with(cfg: Config, listener: TcpListener, voice: Arc<Voice>) -> std::io::Result<()> {
    let util = util_from_config(&cfg);
    start_with_util(cfg, listener, voice, util).await
}

pub async fn start_with_util(cfg: Config, listener: TcpListener, voice: Arc<Voice>, util: Arc<dyn UtilLlm>) -> std::io::Result<()> {
    start_full(cfg, listener, voice, util, None).await
}

/// The study engine from the config: OpenRouter when its key exists (never in `--mock` unless
/// `BELUGA_UTIL_IN_MOCK`).
pub fn study_from_config(cfg: &Config) -> crate::study::StudyService {
    let map = if cfg.mock && !cfg.util_in_mock { Default::default() } else { cfg.secrets.clone() };
    crate::study::StudyService::new(cfg.data_dir.clone(), crate::study::StudyConfig::from_map(&map))
}

/// Everything injectable (tests pass a fake utility model and a study engine with a fake LLM).
pub async fn start_full(cfg: Config, listener: TcpListener, voice: Arc<Voice>, util: Arc<dyn UtilLlm>, study: Option<crate::study::StudyService>) -> std::io::Result<()> {
    let study = study.unwrap_or_else(|| study_from_config(&cfg));
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open(&cfg.data_dir, cfg.fixed_pair_code.as_deref())?);
    // Decide TTS availability once, before any phone connects.
    let _ = tokio::time::timeout(Duration::from_secs(8), voice.probe()).await;
    let board_dir = cfg.board_dir.clone();
    let catalog = Arc::new(Catalog::load(&cfg.models_path, |k| cfg.secret(k), cfg.claude_model_map.clone(), cfg.claude_effort.clone()));
    tracing::info!(
        "models: {} · utility model: {}",
        catalog.entries().iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "),
        if util.available() { "on" } else { "off" }
    );
    let folders = Arc::new(Folders::open(&cfg.data_dir));
    folders.sort_old_sessions(&store);
    let voices = Arc::new(crate::voices::VoiceCatalog::load(&cfg.voices_path, &cfg.voice_clips_dir, &cfg.data_dir, &voice.default_voice_id));
    tracing::info!("voices: {} in the catalogue, current {}", voices.entries().len(), voices.current());
    let deps = Deps {
        voices,
        usage: Arc::new(Usage::open(&cfg.data_dir)),
        uploads: Arc::new(Uploads::open(&cfg.data_dir)),
        cfg: Arc::new(cfg),
        store,
        voice,
        port,
        catalog,
        util,
        folders,
        study,
    };
    let state = AppState::new(deps);
    crate::sync::spawn(state.clone());
    crate::backup::spawn(state.clone());
    let app = Router::new()
        .route("/health", get(health))
        .route("/upload", post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 1024)))
        .route("/file/{id}", get(file_get))
        .route("/exam/{id}", get(exam_get))
        .route("/voice/{file}", get(voice_preview_get))
        .route("/ws", get(ws_upgrade))
        .route("/mcp", post(mcp_general_post).get(mcp_get).delete(mcp_delete))
        .route("/mcp/{sid}", post(mcp_post).get(mcp_get).delete(mcp_delete))
        .merge(crate::sync::routes())
        .nest_service("/board", tower_http::services::ServeDir::new(board_dir))
        .with_state(state);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
}

async fn health(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"ok": true, "v": 2, "boardVersion": st.deps.cfg.board_version()}))
}

// ----- uploads -----

#[derive(serde::Deserialize)]
struct UploadQuery {
    name: Option<String>,
    token: Option<String>,
}

fn bearer(h: &HeaderMap) -> String {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim()
        .to_string()
}

async fn upload(State(st): State<Arc<AppState>>, Query(q): Query<UploadQuery>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !st.deps.store.check_token(&bearer(&headers)) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "bad_pair"}))).into_response();
    }
    let mime = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    match st.deps.uploads.save(q.name.as_deref().unwrap_or(""), mime, &body) {
        Ok(m) => Json(m.public()).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

async fn exam_get(State(st): State<Arc<AppState>>, Path(id): Path<String>, Query(q): Query<UploadQuery>, headers: HeaderMap) -> Response {
    let token = q.token.unwrap_or_else(|| bearer(&headers));
    if !st.deps.store.check_token(&token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match st.deps.study.exam_html(&id) {
        Some(html) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn file_get(State(st): State<Arc<AppState>>, Path(id): Path<String>, Query(q): Query<UploadQuery>, headers: HeaderMap) -> Response {
    let token = q.token.unwrap_or_else(|| bearer(&headers));
    if !st.deps.store.check_token(&token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match st.deps.uploads.read(&id) {
        Some((m, data)) => ([(header::CONTENT_TYPE, m.mime), (header::CACHE_CONTROL, "private, max-age=86400".to_string())], data).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /voice/<id>.mp3`: the stored preview clip of a catalogue voice (no credits spent).
async fn voice_preview_get(State(st): State<Arc<AppState>>, Path(file): Path<String>, Query(q): Query<UploadQuery>, headers: HeaderMap) -> Response {
    let token = q.token.unwrap_or_else(|| bearer(&headers));
    if !st.deps.store.check_token(&token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(path) = file.strip_suffix(".mp3").and_then(|id| st.deps.voices.clip_path(id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(data) => ([(header::CONTENT_TYPE, "audio/mpeg"), (header::CACHE_CONTROL, "private, max-age=86400")], data).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

impl AppState {
    fn voices_frame(&self) -> Value {
        self.deps.voices.frame(self.deps.voice.active_tts_provider())
    }
}

// ----- MCP -----

async fn mcp_post(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(sid): Path<String>,
    body: String,
) -> Response {
    if !allowed_peer(peer.ip()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(req) = serde_json::from_str::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "bad json").into_response();
    };
    let r = crate::mcp::handle_body(st.existing_session(&sid), req).await;
    st.deps.store.changed(); // memory_note may have written memory files
    match r {
        Some(resp) => Json(resp).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// General MCP for any Claude Code session on this PC: talks to the newest live phone.
async fn mcp_general_post(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: String,
) -> Response {
    if !allowed_peer(peer.ip()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(req) = serde_json::from_str::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "bad json").into_response();
    };
    match crate::mcp::handle_general_body(st.active_session(), req).await {
        Some(resp) => Json(resp).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn mcp_get() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED // no server-initiated SSE stream
}

async fn mcp_delete() -> StatusCode {
    StatusCode::OK
}

// ----- WebSocket -----

async fn ws_upgrade(State(st): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(40 * 1024 * 1024).on_upgrade(move |s| handle_socket(st, s))
}

fn err_frame(code: &str, text: &str, fatal: bool) -> Value {
    json!({"t": "error", "code": code, "text": text, "fatal": fatal})
}

struct AudioBuf {
    data: Vec<u8>,
    sample_rate: u32,
    draft: bool,
}


fn sessions_frame(st: &AppState, folder: Option<&str>) -> Value {
    let items: Vec<Value> = st
        .deps
        .store
        .list_sessions()
        .into_iter()
        .filter(|m| folder.map(|f| m.folder_id.as_deref() == Some(f)).unwrap_or(true))
        .map(|m| {
            let mut v = json!({"sessionId": m.session_id, "title": m.title, "updatedAt": m.updated_at, "mode": m.mode});
            if let Some(f) = m.folder_id {
                v["folderId"] = json!(f);
            }
            if let Some(p) = m.preview {
                v["preview"] = json!(p);
            }
            v
        })
        .collect();
    json!({"t": "sessions", "items": items})
}

async fn handle_socket(st: Arc<AppState>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (otx, mut orx) = unbounded_channel::<Value>();
    // Writer ends (and closes the socket) when every sender is gone: that is how a replaced
    // or detached connection gets closed.
    tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        loop {
            tokio::select! {
                v = orx.recv() => match v {
                    Some(v) => { if sink.send(Message::Text(v.to_string().into())).await.is_err() { return; } }
                    None => { let _ = sink.close().await; return; }
                },
                _ = ping.tick() => { if sink.send(Message::Ping(Vec::new().into())).await.is_err() { return; } }
            }
        }
    });
    let weak = otx.downgrade();
    let weak_reply = weak.clone();
    let reply = move |v: Value| {
        if let Some(t) = weak_reply.upgrade() {
            let _ = t.send(v);
        }
    };

    // --- hello ---
    let first = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(m)) = stream.next().await {
            if let Message::Text(t) = m {
                return serde_json::from_str::<Value>(t.as_str()).ok();
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let Some(hello) = first.filter(|h| h["t"] == "hello") else {
        let _ = otx.send(err_frame("bad_hello", "Cần gửi hello trước.", true));
        return;
    };
    let token = hello["pairToken"].as_str().unwrap_or("");
    let code = hello["pairCode"].as_str().unwrap_or("");
    let mut new_token = None;
    if !st.deps.store.check_token(token) {
        if !code.is_empty() && code == st.deps.store.pair_code() {
            new_token = Some(st.deps.store.add_token());
        } else {
            let _ = otx.send(err_frame("bad_pair", "Mã ghép đôi không đúng.", true));
            tokio::time::sleep(Duration::from_millis(800)).await; // slow down guessing
            return;
        }
    }

    let conn = st.next_conn.fetch_add(1, Ordering::Relaxed);
    st.phones.lock().unwrap().push((conn, otx.downgrade()));
    let mut sid = match hello["sessionId"].as_str() {
        Some(s) if is_valid_session_id(s) => s.to_string(),
        _ => uuid::Uuid::new_v4().to_string(),
    };
    let conn_v = hello["v"].as_u64().unwrap_or(1);
    let mut session = st.session(&sid);
    let _ = session.send(Cmd::Attach {
        conn,
        tx: otx.clone(),
        last_seq: hello["lastSeq"].as_u64(),
        settings: hello.get("settings").cloned(),
        pair_token: new_token,
        init: SessionInit { v: conn_v, folder_id: hello["folderId"].as_str().map(String::from), mode: None, kickoff: None },
    });
    drop(otx);
    st.set_active(conn, &sid);

    let mut audio: Option<AudioBuf> = None;
    // Language for server-side STT; follows the phone's settings.
    let mut lang = hello["settings"]["lang"].as_str().filter(|l| *l == "en").unwrap_or("vi").to_string();
    if let Some(id) = hello["settings"]["voice"].as_str() {
        if id != st.deps.voices.current() {
            st.deps.voices.select(id);
        }
    }
    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Binary(b) => {
                if let Some(a) = audio.as_mut() {
                    if a.data.len() + b.len() <= MAX_AUDIO_BYTES {
                        a.data.extend_from_slice(&b);
                    }
                }
            }
            Message::Text(t) => {
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                let mutation = crate::sync::is_mutation(v["t"].as_str().unwrap_or(""));
                // `t` is the protocol's tag; `type` is accepted too for the voice messages.
                match v["t"].as_str().or(v["type"].as_str()).unwrap_or("") {
                    "settings" => {
                        if let Some(l) = v["lang"].as_str().filter(|l| *l == "vi" || *l == "en") {
                            lang = l.to_string();
                        }
                        // The voice is global (all sessions), so it is handled here, not per session.
                        let picked = v["voice"].as_str().map(String::from);
                        let _ = session.send(Cmd::Settings(v));
                        if let Some(id) = picked {
                            st.deps.voices.select(&id);
                            reply(st.voices_frame());
                        }
                    }
                    "get_voices" => reply(st.voices_frame()),
                    "user_text" => {
                        let files: Vec<String> = v["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(String::from)).collect();
                        if let Some(t) = v["text"].as_str() {
                            let via = if v["via"] == "voice" { "voice" } else { "text" };
                            let draft = v["draft"].as_bool().unwrap_or(false);
                            let _ = session.send(Cmd::UserText { text: t.to_string(), files, via: via.into(), draft });
                        }
                    }
                    "interrupt" => {
                        let _ = session.send(Cmd::Interrupt);
                    }
                    "played" => {
                        if let Some(s) = v["seq"].as_u64() {
                            let _ = session.send(Cmd::Played(s));
                        }
                    }
                    "photo" => {
                        let jpeg = v["jpeg"]
                            .as_str()
                            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok());
                        match jpeg {
                            Some(jpeg) => {
                                let _ = session.send(Cmd::Photo {
                                    request_id: v["requestId"].as_str().map(|s| s.to_string()),
                                    jpeg,
                                });
                            }
                            None => reply(json!({"t": "notice", "code": "bad_photo", "text": "Ảnh không đọc được."})),
                        }
                    }
                    "audio_start" => {
                        let rate = v["sampleRate"].as_u64().filter(|r| (8000..=48000).contains(r)).unwrap_or(16000);
                        audio = Some(AudioBuf { data: Vec::new(), sample_rate: rate as u32, draft: v["draft"].as_bool().unwrap_or(false) });
                    }
                    "audio_end" => {
                        let Some(buf) = audio.take() else { continue };
                        let Some(stt) = st.deps.voice.stt.clone() else {
                            reply(json!({"t": "notice", "code": "stt_unavailable", "text": "Server chưa nhận dạng giọng nói."}));
                            continue;
                        };
                        let tx = session.clone();
                        let lang = lang.clone();
                        let pcm_len = if buf.data.starts_with(b"RIFF") { buf.data.len().saturating_sub(44) } else { buf.data.len() };
                        let secs = pcm_len as f64 / (buf.sample_rate as f64 * 2.0);
                        let draft = buf.draft;
                        let limit = st.deps.voice.stt_deadline(secs);
                        tokio::spawn(async move {
                            let notice = |code: &str, text: &str| Cmd::Notice(json!({"t": "notice", "code": code, "text": text}));
                            if buf.data.is_empty() {
                                let _ = tx.send(notice("stt_empty", "Không nghe thấy gì."));
                                return;
                            }
                            let wav = crate::voice::ensure_wav(buf.data, buf.sample_rate);
                            match tokio::time::timeout(limit, stt.transcribe(wav, &lang)).await {
                                Ok(Ok(text)) if !text.trim().is_empty() => {
                                    let _ = tx.send(Cmd::Heard { text, draft });
                                }
                                Ok(Ok(_)) => {
                                    let _ = tx.send(notice("stt_empty", "Không nghe rõ, cậu nói lại nhé."));
                                }
                                Ok(Err(e)) => {
                                    tracing::warn!("stt failed: {}", e.message);
                                    let _ = tx.send(notice("stt_failed", "Nhận dạng giọng nói lỗi, thử lại nhé."));
                                }
                                Err(_) => {
                                    tracing::warn!("stt timed out");
                                    let _ = tx.send(notice("stt_failed", "Nhận dạng giọng nói quá lâu, thử lại nhé."));
                                }
                            }
                        });
                    }
                    "get_progress" => {
                        reply(crate::memory::Memory::new(&st.deps.cfg.tutor_dir).progress());
                    }
                    "list_sessions" => {
                        reply(sessions_frame(&st, v["folderId"].as_str()));
                    }
                    "list_folders" => reply(st.deps.folders_frame()),
                    "create_folder" => match st.deps.folders.create(v["name"].as_str().unwrap_or(""), v["subject"].as_str().unwrap_or("chung")) {
                        Ok(f) => {
                            reply(json!({"t": "folder_created", "id": f.id}));
                            reply(st.deps.folders_frame());
                        }
                        Err(e) => reply(err_frame("bad_folder", &e, false)),
                    },
                    "rename_folder" => {
                        let id = v["id"].as_str().unwrap_or("");
                        if !st.deps.folders.rename(id, v["name"].as_str().unwrap_or("")) {
                            reply(err_frame("no_folder", "Không đổi được tên folder.", false));
                        }
                        reply(st.deps.folders_frame());
                    }
                    "delete_folder" => {
                        let id = v["id"].as_str().unwrap_or("").to_string();
                        let _ = st.deps.study.delete_folder_data(&id);
                        if st.deps.folders.delete(&id) {
                            let inside: Vec<String> = st.deps.store.list_sessions().into_iter().filter(|m| m.folder_id.as_deref() == Some(id.as_str())).map(|m| m.session_id).collect();
                            for sid2 in inside {
                                st.edit_session(&sid2, |m| m.folder_id = None).await;
                            }
                        } else {
                            reply(err_frame("no_folder", "Không tìm thấy folder.", false));
                        }
                        reply(st.deps.folders_frame());
                    }
                    "move_session" => {
                        let target = v["folderId"].as_str().map(String::from);
                        if target.as_ref().map(|f| st.deps.folders.get(f).is_none()).unwrap_or(false) {
                            reply(err_frame("no_folder", "Không tìm thấy folder.", false));
                            continue;
                        }
                        let id = v["sessionId"].as_str().unwrap_or("").to_string();
                        if !st.edit_session(&id, move |m| m.folder_id = target).await {
                            reply(err_frame("no_session", "Không tìm thấy phiên đó.", false));
                        }
                        reply(sessions_frame(&st, None));
                    }
                    "rename_session" => {
                        let id = v["sessionId"].as_str().unwrap_or("").to_string();
                        let title: String = v["title"].as_str().unwrap_or("").trim().chars().take(80).collect();
                        if title.is_empty() || !st.edit_session(&id, move |m| {
                            m.title = title;
                            m.titled = true;
                            m.user_titled = true;
                        }).await {
                            reply(err_frame("no_session", "Không đổi được tên phiên.", false));
                        }
                        reply(sessions_frame(&st, None));
                    }
                    "delete_session" => {
                        let id = v["sessionId"].as_str().unwrap_or("").to_string();
                        if id == sid {
                            reply(err_frame("busy_session", "Không xóa được phiên đang mở.", false));
                        } else if !st.delete_session(&id).await {
                            reply(err_frame("no_session", "Không tìm thấy phiên đó.", false));
                        }
                        reply(sessions_frame(&st, None));
                    }
                    "new_session" => {
                        let _ = session.send(Cmd::Detach { conn });
                        sid = uuid::Uuid::new_v4().to_string();
                        session = st.session(&sid);
                        st.set_active(conn, &sid);
                        let Some(tx) = weak.upgrade() else { break };
                        let mode = v["mode"].as_str().filter(|m| *m == "chat" || *m == "live").map(String::from);
                        let init = SessionInit {
                            v: conn_v,
                            folder_id: v["folderId"].as_str().map(String::from),
                            mode,
                            kickoff: v["kickoff"].as_str().filter(|k| *k == "review_quiz").map(String::from),
                        };
                        let _ = session.send(Cmd::Attach { conn, tx, last_seq: None, settings: None, pair_token: None, init });
                    }
                    "get_usage" => reply(st.deps.usage_frame(Some(&sid))),
                    "get_review" => reply(crate::review::review_frame(&st.deps, v["folderId"].as_str().unwrap_or(""))),
                    "get_weekly" => {
                        let (st2, folder, r) = (st.clone(), v["folderId"].as_str().unwrap_or("").to_string(), reply.clone());
                        tokio::spawn(async move { r(crate::review::weekly(&st2.deps, &folder).await) });
                    }
                    "get_greeting" => {
                        let (st2, r) = (st.clone(), reply.clone());
                        let local = v["localTime"].as_str().unwrap_or("").to_string();
                        let name = v["name"].as_str().map(String::from).filter(|n| !n.trim().is_empty());
                        tokio::spawn(async move { r(crate::greeting::greeting(&st2.deps, &st2.greeting, &local, name.as_deref()).await) });
                    }
                    "ingest_start" => {
                        if let Err(e) = st.start_ingest(&v) {
                            reply(e);
                        }
                    }
                    "ingest_cancel" => {
                        let f = v["folderId"].as_str().unwrap_or("");
                        if !st.deps.study.cancel(f) {
                            reply(st.knowledge_frame(f));
                        }
                    }
                    "get_knowledge" => reply(st.knowledge_frame(v["folderId"].as_str().unwrap_or(""))),
                    "gen_exam" => {
                        let folder = v["folderId"].as_str().unwrap_or("").to_string();
                        let structure = v["structureId"].as_str().unwrap_or("").to_string();
                        let note = v["note"].as_str().map(String::from).filter(|n| !n.trim().is_empty());
                        let st2 = st.clone();
                        tokio::spawn(async move {
                            let b = st2.clone();
                            let _ = st2.deps.study.gen_exam(&folder, &structure, note.as_deref(), move |e| b.broadcast(&e)).await;
                            st2.broadcast(&st2.exams_frame(&folder));
                        });
                    }
                    "list_exams" => reply(st.exams_frame(v["folderId"].as_str().unwrap_or(""))),
                    "delete_exam" => {
                        let id = v["examId"].as_str().unwrap_or("");
                        let folder = st.deps.study.exam(id).and_then(|e| e["folderId"].as_str().map(String::from));
                        if !st.deps.study.delete_exam(id) {
                            reply(err_frame("no_exam", "Không tìm thấy đề đó.", false));
                        }
                        if let Some(f) = folder {
                            reply(st.exams_frame(&f));
                        }
                    }
                    "open_session" => {
                        let Some(id) = v["sessionId"].as_str().filter(|s| is_valid_session_id(s)) else { continue };
                        if st.deps.store.get_session(id).is_none() && st.existing_session(id).is_none() {
                            reply(err_frame("no_session", "Không tìm thấy phiên đó.", false));
                            continue;
                        }
                        let _ = session.send(Cmd::Detach { conn });
                        sid = id.to_string();
                        session = st.session(&sid);
                        st.set_active(conn, &sid);
                        // The writer only stays open while some session holds a sender; get a fresh one.
                        let Some(tx) = weak.upgrade() else { break };
                        let _ = session.send(Cmd::Attach {
                            conn,
                            tx,
                            last_seq: Some(0),
                            settings: None,
                            pair_token: None,
                            init: SessionInit { v: conn_v, ..Default::default() },
                        });
                    }
                    "get_backup" => reply(st.backup.frame()),
                    "backup_now" => {
                        st.backup.kick();
                        reply(st.backup.frame());
                    }
                    _ => {}
                }
                if mutation {
                    st.deps.store.changed();
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = session.send(Cmd::Detach { conn });
    st.clear_active(conn);
    st.phones.lock().unwrap().retain(|(c, _)| *c != conn);
}

/// MCP routes are for local Claude Code only: never reachable over Tailscale or the LAN.
pub fn allowed_peer(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_may_use_mcp() {
        assert!(allowed_peer("127.0.0.1".parse().unwrap()));
        assert!(allowed_peer("::1".parse().unwrap()));
        assert!(!allowed_peer("100.64.1.2".parse().unwrap()), "tailscale range");
        assert!(!allowed_peer("192.168.1.10".parse().unwrap()));
        assert!(!allowed_peer("8.8.8.8".parse().unwrap()));
    }
}
