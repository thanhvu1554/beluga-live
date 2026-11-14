//! HTTP + WebSocket front: `/health`, `/board/*`, `/ws`, and the loopback-only `/mcp/<session>` route.

use crate::config::Config;
use crate::session::{spawn_session, Cmd, Deps};
use crate::store::{is_valid_session_id, Store};
use crate::voice::Voice;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
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
}

impl AppState {
    pub fn new(deps: Deps) -> Arc<AppState> {
        Arc::new(AppState { deps, sessions: Mutex::new(HashMap::new()), next_conn: AtomicU64::new(1) })
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
    let voice = match &cfg.eleven {
        Some(e) => Arc::new(Voice::from_eleven(e)),
        None => Voice::none(),
    };
    start_with(cfg, listener, voice).await
}

pub async fn start_with(cfg: Config, listener: TcpListener, voice: Arc<Voice>) -> std::io::Result<()> {
    let port = listener.local_addr()?.port();
    let store = Arc::new(Store::open(&cfg.data_dir, cfg.fixed_pair_code.as_deref())?);
    // Decide TTS availability once, before any phone connects.
    let _ = tokio::time::timeout(Duration::from_secs(8), voice.probe()).await;
    let board_dir = cfg.board_dir.clone();
    let state = AppState::new(Deps { cfg: Arc::new(cfg), store, voice, port });
    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_upgrade))
        .route("/mcp/{sid}", post(mcp_post).get(mcp_get).delete(mcp_delete))
        .nest_service("/board", tower_http::services::ServeDir::new(board_dir))
        .with_state(state);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
}

async fn health(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"ok": true, "v": 1, "boardVersion": st.deps.cfg.board_version()}))
}

// ----- MCP -----

async fn mcp_post(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(sid): Path<String>,
    body: String,
) -> Response {
    if !peer.ip().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(req) = serde_json::from_str::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "bad json").into_response();
    };
    match crate::mcp::handle_body(st.existing_session(&sid), req).await {
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
    let mut sid = match hello["sessionId"].as_str() {
        Some(s) if is_valid_session_id(s) => s.to_string(),
        _ => uuid::Uuid::new_v4().to_string(),
    };
    let mut session = st.session(&sid);
    let _ = session.send(Cmd::Attach {
        conn,
        tx: otx.clone(),
        last_seq: hello["lastSeq"].as_u64(),
        settings: hello.get("settings").cloned(),
        pair_token: new_token,
    });
    drop(otx);

    let mut audio: Option<AudioBuf> = None;
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
                match v["t"].as_str().unwrap_or("") {
                    "settings" => {
                        let _ = session.send(Cmd::Settings(v));
                    }
                    "user_text" => {
                        if let Some(t) = v["text"].as_str() {
                            let _ = session.send(Cmd::UserText { text: t.to_string() });
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
                    "audio_start" => audio = Some(AudioBuf { data: Vec::new() }),
                    "audio_end" => {
                        let Some(buf) = audio.take() else { continue };
                        match st.deps.voice.stt.clone() {
                            Some(stt) => {
                                let session = session.clone();
                                let lang = v["lang"].as_str().unwrap_or("vi").to_string();
                                let reply_tx = session.clone();
                                tokio::spawn(async move {
                                    match stt.transcribe(buf.data, &lang).await {
                                        Ok(text) if !text.is_empty() => {
                                            let _ = reply_tx.send(Cmd::Heard { text });
                                        }
                                        Ok(_) => {}
                                        Err(e) => tracing::warn!("stt failed: {}", e.message),
                                    }
                                });
                            }
                            None => reply(json!({"t": "notice", "code": "stt_unavailable", "text": "Server chưa nhận dạng giọng nói."})),
                        }
                    }
                    "list_sessions" => {
                        let items: Vec<Value> = st
                            .deps
                            .store
                            .list_sessions()
                            .into_iter()
                            .map(|m| json!({"sessionId": m.session_id, "title": m.title, "updatedAt": m.updated_at}))
                            .collect();
                        reply(json!({"t": "sessions", "items": items}));
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
                        // The writer only stays open while some session holds a sender; get a fresh one.
                        let Some(tx) = weak.upgrade() else { break };
                        let _ = session.send(Cmd::Attach {
                            conn,
                            tx,
                            last_seq: Some(0),
                            settings: None,
                            pair_token: None,
                        });
                    }
                    _ => {}
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = session.send(Cmd::Detach { conn });
}
