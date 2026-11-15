//! One actor per phone session: owns the journal, the lesson/chat parser, the brain (mock or
//! Claude), the attached phone connection, photo capture, TTS fan-out, speech-to-text correction,
//! automatic titles, usage records and compaction.

use crate::brain::claude::{ClaudeCfg, ClaudeDriver};
use crate::brain::mock::MockBrain;
use crate::brain::{Attachment, Brain, BrainEvent, BrainMsg, Settings, TurnInput};
use crate::config::Config;
use crate::folders::Folders;
use crate::memory::{self, Kind, Memory};
use crate::models::{Catalog, ModelEntry};
use crate::parser::{ChatParser, Event, LessonParser, TurnParser};
use crate::store::{date_suffix, now_rfc3339, SessionMeta, Store};
use crate::uploads::{FileMeta, Uploads};
use crate::usage::{Usage, UsageRec};
use crate::util_llm::{self, UtilLlm, UtilReq};
use crate::voice::Voice;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

/// STT correction deadline (PROTOCOL section 7): past it the raw text is used.
pub const STT_FIX_MS: u64 = 2500;

pub type MetaEdit = Box<dyn FnOnce(&mut SessionMeta) + Send>;

/// How a session is opened from the phone (`hello` or `new_session`).
#[derive(Debug, Clone, Default)]
pub struct SessionInit {
    /// `hello.v` of the connection (1 = original app, 2 = folders/chat app).
    pub v: u64,
    pub folder_id: Option<String>,
    pub mode: Option<String>,
    /// `review_quiz`: the server starts the first turn itself.
    pub kickoff: Option<String>,
}

pub enum Cmd {
    Attach {
        conn: u64,
        tx: UnboundedSender<Value>,
        last_seq: Option<u64>,
        settings: Option<Value>,
        pair_token: Option<String>,
        init: SessionInit,
    },
    Detach { conn: u64 },
    /// `user_text` from the phone. `via`: `text` (typed) or `voice` (device speech recognition).
    /// `draft`: dictation only (corrected and echoed as `heard`, never sent to Claude).
    UserText { text: String, files: Vec<String>, via: String, draft: bool },
    /// Server-side STT result.
    Heard { text: String, draft: bool },
    /// The utility model's correction of a speech result came back (or timed out: `text == raw`).
    Fixed { raw: String, text: String, draft: bool, via: String, files: Vec<String>, server_stt: bool },
    Settings(Value),
    Interrupt,
    Played(u64),
    Photo { request_id: Option<String>, jpeg: Vec<u8> },
    Audio { seq: u64, data: Vec<u8> },
    /// TTS was just switched off (already recorded in `Voice`): tell the phone once.
    TtsFailed,
    /// Forward a frame to the attached phone (used by background tasks such as STT).
    Notice(Value),
    /// The server gave up on audio for this sentence: the phone should speak it itself.
    AudioSkip { seq: u64 },
    // ---- general MCP (any Claude Code session talking to the phone) ----
    Status { reply: oneshot::Sender<Value> },
    /// Speak plain text (several sentences allowed).
    Speak { text: String, reply: oneshot::Sender<Result<Value, String>> },
    /// Show/say text in the lesson line format (PROTOCOL section 2).
    Present { lines: String, reply: oneshot::Sender<Result<Value, String>> },
    /// Wait for the student's next utterance instead of handing it to the tutor.
    Listen { reply: oneshot::Sender<Result<oneshot::Receiver<String>, String>> },
    McpInterrupt { reply: oneshot::Sender<Value> },
    Capture { reply: oneshot::Sender<Result<Vec<u8>, String>> },
    CaptureTimeout { request_id: String },
    /// `memory_note` tool call from the tutor (per-session MCP route).
    MemoryNote { args: Value, reply: oneshot::Sender<Result<Value, String>> },
    // ---- v2 ----
    /// Change the session's metadata (rename, move to a folder); saved at once.
    EditMeta { edit: MetaEdit, reply: oneshot::Sender<()> },
    /// Stop the actor (the session is being deleted).
    Close { reply: oneshot::Sender<()> },
    /// A title from the utility model.
    AutoTitle(String),
    /// Compaction summary ready (`None`: it failed).
    Compacted(Option<String>),
}

/// A weak item the server invited the student to review at the start of the session.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRef {
    pub subject: String,
    pub topic: String,
    pub note: String,
}

#[derive(Clone)]
pub struct Deps {
    pub cfg: Arc<Config>,
    pub store: Arc<Store>,
    pub voice: Arc<Voice>,
    /// Port the HTTP server is really bound to (used in the MCP URL for claude).
    pub port: u16,
    pub catalog: Arc<Catalog>,
    pub util: Arc<dyn UtilLlm>,
    pub usage: Arc<Usage>,
    pub uploads: Arc<Uploads>,
    pub folders: Arc<Folders>,
}

struct Running {
    turn: u64,
    parser: TurnParser,
    spoke: bool,
}

struct PendingCapture {
    request_id: String,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// A student message waiting for its turn.
struct UserMsg {
    text: String,
    files: Vec<FileMeta>,
    via: String,
    /// Server-made first turn (`review_quiz`): no `user` echo.
    kickoff: bool,
}

struct Session {
    id: String,
    deps: Deps,
    meta: SessionMeta,
    journal: Vec<Value>,
    next_seq: u64,
    steps: usize,
    turn: u64,
    conn: Option<(u64, UnboundedSender<Value>)>,
    /// `hello.v` of the attached connection.
    conn_v: u64,
    settings: Settings,
    running: Option<Running>,
    /// The brain has not yet sent `End` for the last turn it was given.
    brain_busy: bool,
    pending: VecDeque<UserMsg>,
    latest_photo: Option<Vec<u8>>,
    capture: Option<PendingCapture>,
    played: u64,
    interrupt_note: Option<String>,
    /// Running TTS tasks; aborted when the turn is interrupted.
    tts_tasks: Vec<tokio::task::AbortHandle>,
    /// `audio` frames with a seq below this belong to an interrupted turn and are dropped.
    audio_floor: u64,
    listener: Option<oneshot::Sender<String>>,
    /// Offered for review and not yet answered: the next user turn carries a note about it.
    pending_review: Option<ReviewRef>,
    /// Do not list the session in the index (used while the server talks first).
    quiet: bool,
    /// (turn, model entry) of the turn given to the brain last (for usage records).
    turn_model: Option<(u64, ModelEntry)>,
    /// Input size of the last finished turn (compaction trigger).
    last_context: u64,
    title_asked: bool,
    compacting: bool,
    /// A finished compaction waiting for the brain to become idle.
    compact_ready: Option<String>,
    /// Summary to put in front of the next turn (after a compaction).
    compact_note: Option<String>,
    brain: Brain,
    self_tx: UnboundedSender<Cmd>,
}

/// Start the actor for `id` and return its command channel.
pub fn spawn_session(id: String, deps: Deps) -> UnboundedSender<Cmd> {
    let (tx, rx) = unbounded_channel();
    let (btx, brx) = unbounded_channel();
    let journal = deps.store.load_events(&id);
    let next_seq = journal.iter().filter_map(|e| e["seq"].as_u64()).max().unwrap_or(0) + 1;
    let steps = journal.iter().filter(|e| e["t"] == "step").count();
    let turn = journal.iter().filter_map(|e| e["turn"].as_u64()).max().unwrap_or(0);
    let meta = deps.store.get_session(&id).unwrap_or_else(|| SessionMeta::new(&id));
    let pending_review = pending_review_from_journal(&journal, turn);
    let brain = if deps.cfg.mock {
        Brain::Mock(MockBrain::new(btx, tx.clone(), deps.cfg.mock_delay_ms))
    } else {
        let mcp_dir = deps.store.dir().join("mcp");
        let _ = std::fs::create_dir_all(&mcp_dir);
        let mcp_path = mcp_dir.join(format!("{id}.json"));
        let conf = json!({"mcpServers": {crate::brain::claude::MCP_SERVER: {
            "type": "http", "url": format!("http://127.0.0.1:{}/mcp/{}", deps.port, id)}}});
        let _ = std::fs::write(&mcp_path, conf.to_string());
        let settings_path = mcp_dir.join("tutor-settings.json");
        let settings_file = std::fs::write(&settings_path, crate::brain::claude::tutor_settings().to_string()).ok().map(|_| settings_path);
        Brain::Claude(ClaudeDriver::new(
            ClaudeCfg { bin: deps.cfg.claude_bin.clone(), tutor_dir: deps.cfg.tutor_dir.clone(), mcp_config: mcp_path, settings_file },
            meta.claude_session(),
            meta.claude_started,
            btx,
        ))
    };
    let settings = Settings { model: deps.catalog.default_id(), ..Settings::default() };
    let s = Session {
        id,
        deps,
        meta,
        journal,
        next_seq,
        steps,
        turn,
        conn: None,
        conn_v: 1,
        settings,
        running: None,
        brain_busy: false,
        pending: VecDeque::new(),
        latest_photo: None,
        capture: None,
        played: 0,
        interrupt_note: None,
        tts_tasks: Vec::new(),
        audio_floor: 0,
        listener: None,
        pending_review,
        quiet: false,
        turn_model: None,
        last_context: 0,
        title_asked: false,
        compacting: false,
        compact_ready: None,
        compact_note: None,
        brain,
        self_tx: tx.clone(),
    };
    tokio::spawn(s.run(rx, brx));
    tx
}

fn state_frame(state: &str, tool: Option<&str>) -> Value {
    match tool {
        Some(t) => json!({"t": "state", "state": state, "tool": t}),
        None => json!({"t": "state", "state": state}),
    }
}

fn notice(code: &str, text: &str) -> Value {
    json!({"t": "notice", "code": code, "text": text})
}

/// Tell Claude where the student cut in, from the `say` events of the interrupted turn.
pub fn build_interrupt_note(journal: &[Value], turn: u64, played: u64) -> String {
    let says: Vec<(u64, &str)> = journal
        .iter()
        .filter(|e| e["t"] == "say" && e["turn"].as_u64() == Some(turn))
        .filter_map(|e| Some((e["seq"].as_u64()?, e["text"].as_str()?)))
        .collect();
    let heard: Vec<&str> = says.iter().filter(|(s, _)| *s <= played).map(|(_, t)| *t).collect();
    let unheard: Vec<&str> = says.iter().filter(|(s, _)| *s > played).map(|(_, t)| *t).collect();
    let cut = |s: String| -> String { s.chars().take(300).collect() };
    let heard_s = match heard.last() {
        Some(last) => format!("đã nghe tới câu: \"{}\"", cut(last.to_string())),
        None => "chưa nghe được câu nào của lượt đó".to_string(),
    };
    let unheard_s = if unheard.is_empty() {
        String::new()
    } else {
        format!(" Phần chưa nghe: \"{}\".", cut(unheard.join(" ")))
    };
    format!("(Ghi chú hệ thống: học sinh vừa ngắt lời cậu; bạn ấy {heard_s}.{unheard_s} Trả lời điều bạn ấy nói trước, rồi quay lại chỗ dở nếu cần.)")
}

/// Assistant text of the journal as plain lines (`say` text, `chat` markdown), oldest first.
fn assistant_lines(journal: &[Value]) -> Vec<String> {
    journal
        .iter()
        .filter_map(|e| match e["t"].as_str()? {
            "say" => e["text"].as_str().map(String::from),
            "chat" => e["md"].as_str().map(String::from),
            _ => None,
        })
        .collect()
}

impl Session {
    async fn run(mut self, mut rx: UnboundedReceiver<Cmd>, mut brx: UnboundedReceiver<BrainMsg>) {
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Close { reply }) => {
                        self.closing();
                        let _ = reply.send(());
                        break;
                    }
                    Some(c) => self.on_cmd(c),
                    None => break,
                },
                msg = brx.recv() => if let Some(m) = msg { self.on_brain(m) },
            }
        }
        self.brain.shutdown();
    }

    fn closing(&mut self) {
        for h in self.tts_tasks.drain(..) {
            h.abort();
        }
        if let Some(p) = self.capture.take() {
            let _ = p.reply.send(Err("Phiên đã bị xóa.".into()));
        }
        self.quiet = true;
    }

    fn send(&self, v: Value) {
        if let Some((_, tx)) = &self.conn {
            let _ = tx.send(v);
        }
    }

    fn touch(&mut self) {
        if self.quiet {
            return;
        }
        self.meta.updated_at = now_rfc3339();
        self.deps.store.upsert_session(&self.meta);
        if let Some(f) = &self.meta.folder_id {
            self.deps.folders.touch(f);
        }
    }

    fn is_chat(&self) -> bool {
        self.meta.mode == "chat"
    }

    fn folder_subject(&self) -> Option<(String, String)> {
        let f = self.meta.folder_id.as_ref()?;
        self.deps.folders.get(f).map(|m| (m.id, m.subject))
    }

    fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Attach { conn, tx, last_seq, settings, pair_token, init } => self.attach(conn, tx, last_seq, settings, pair_token, init),
            Cmd::Detach { conn } => {
                if self.conn.as_ref().map(|(c, _)| *c) == Some(conn) {
                    self.conn = None;
                    if let Some(p) = self.capture.take() {
                        let _ = p.reply.send(Err("Điện thoại vừa mất kết nối nên chưa chụp được ảnh.".into()));
                    }
                }
            }
            Cmd::UserText { text, files, via, draft } => self.incoming(text, files, via, draft, false),
            Cmd::Heard { text, draft } => self.incoming(text, vec![], "voice".into(), draft, true),
            Cmd::Fixed { raw, text, draft, via, files, server_stt } => {
                let changed = text != raw;
                if server_stt || draft || changed {
                    let mut h = json!({"t": "heard", "text": text});
                    if changed {
                        h["raw"] = json!(raw);
                    }
                    if draft {
                        h["draft"] = json!(true);
                    }
                    self.send(h);
                }
                if !draft {
                    self.user_text(text, files, via);
                }
            }
            Cmd::Settings(v) => self.apply_settings(&v),
            Cmd::Interrupt => self.interrupt(),
            Cmd::Played(seq) => self.played = self.played.max(seq),
            Cmd::Photo { request_id, jpeg } => self.photo(request_id, jpeg),
            Cmd::Audio { seq, data } => {
                if seq >= self.audio_floor {
                    self.send(json!({
                        "t": "audio", "seq": seq, "mime": "audio/mpeg",
                        "data": base64::engine::general_purpose::STANDARD.encode(data)}));
                }
            }
            Cmd::TtsFailed => self.send(tts_fallback_notice()),
            Cmd::Notice(v) => self.send(v),
            Cmd::AudioSkip { seq } => {
                if seq >= self.audio_floor {
                    self.send(json!({"t": "audio_skip", "seq": seq}));
                }
            }
            Cmd::Status { reply } => {
                let _ = reply.send(self.status());
            }
            Cmd::Speak { text, reply } => {
                // Plain speech: a leading '@' must not turn a sentence into a directive.
                let lines: Vec<String> = text.lines().map(|l| l.trim().trim_start_matches('@').to_string()).collect();
                let _ = reply.send(self.present(lines.join("\n")));
            }
            Cmd::Present { lines, reply } => {
                let _ = reply.send(self.present(lines));
            }
            Cmd::Listen { reply } => {
                let r = if self.conn.is_none() {
                    Err("Điện thoại đang không kết nối.".to_string())
                } else {
                    let (tx, rx) = oneshot::channel();
                    self.listener = Some(tx); // replaces (and cancels) an older listen
                    Ok(rx)
                };
                let _ = reply.send(r);
            }
            Cmd::McpInterrupt { reply } => {
                let _ = reply.send(self.mcp_interrupt());
            }
            Cmd::Capture { reply } => {
                if self.is_chat() {
                    let _ = reply.send(Err("Đang ở chế độ chat nên không có camera. Nhờ học sinh đính kèm ảnh vào tin nhắn.".into()));
                } else {
                    self.start_capture(reply);
                }
            }
            Cmd::MemoryNote { args, reply } => {
                let _ = reply.send(self.memory_note(&args));
            }
            Cmd::CaptureTimeout { request_id } => {
                if self.capture.as_ref().map(|p| p.request_id == request_id).unwrap_or(false) {
                    let p = self.capture.take().unwrap();
                    let _ = p.reply.send(Err(
                        "Quá 8 giây điện thoại không gửi ảnh về. Bảo học sinh kiểm tra camera rồi thử chụp lại.".into(),
                    ));
                    self.send(state_frame("thinking", None));
                }
            }
            Cmd::EditMeta { edit, reply } => {
                let old_title = self.meta.title.clone();
                edit(&mut self.meta);
                self.deps.store.upsert_session(&self.meta);
                if self.meta.title != old_title {
                    self.send(json!({"t": "title", "text": self.meta.title}));
                }
                let _ = reply.send(());
            }
            Cmd::Close { reply } => {
                let _ = reply.send(()); // handled in `run`
            }
            Cmd::AutoTitle(t) => {
                if !self.meta.has_title() {
                    self.meta.title = format!("{t} · {}", date_suffix());
                    self.meta.titled = true;
                    self.touch();
                    self.send(json!({"t": "title", "text": self.meta.title}));
                }
            }
            Cmd::Compacted(summary) => {
                self.compacting = false;
                if let Some(s) = summary {
                    self.compact_ready = Some(s);
                    self.apply_compaction();
                }
            }
        }
    }

    fn apply_settings(&mut self, v: &Value) {
        let cat = self.deps.catalog.clone();
        self.settings.apply(v, |m| cat.get(m).is_some());
        if let Some(m) = v.get("mode").and_then(|m| m.as_str()).filter(|m| *m == "chat" || *m == "live") {
            if self.meta.mode != m {
                self.meta.mode = m.to_string();
                if self.deps.store.get_session(&self.id).is_some() {
                    self.deps.store.upsert_session(&self.meta);
                }
            }
        }
    }

    fn ready_frame(&self, pair_token: Option<String>) -> Value {
        let voice = &self.deps.voice;
        let entry = self.deps.catalog.resolve(&self.settings.model);
        let mut ready = json!({
            "t": "ready", "v": if self.conn_v >= 2 { 2 } else { 1 }, "sessionId": self.id, "title": self.meta.title,
            "tts": if voice.tts_enabled() { "server" } else { "device" },
            "stt": if voice.stt_enabled() { "server" } else { "device" },
            "boardVersion": self.deps.cfg.board_version(),
            "mode": self.meta.mode,
            "folderId": self.meta.folder_id,
            "models": self.deps.catalog.public(),
            "model": entry.id,
            "effort": self.deps.catalog.effort_for(&entry, self.settings.effort.as_deref()),
        });
        if let Some(ms) = voice.audio_wait_ms() {
            ready["audioWaitMs"] = json!(ms);
        }
        if let Some(t) = pair_token {
            ready["pairToken"] = Value::String(t);
        }
        ready
    }

    fn attach(&mut self, conn: u64, tx: UnboundedSender<Value>, last_seq: Option<u64>, settings: Option<Value>, pair_token: Option<String>, init: SessionInit) {
        self.conn_v = init.v.max(1);
        let fresh = self.journal.is_empty() && self.deps.store.get_session(&self.id).is_none() && self.turn == 0;
        if fresh {
            if let Some(f) = init.folder_id.as_ref().filter(|f| self.deps.folders.get(f).is_some()) {
                self.meta.folder_id = Some(f.clone());
            }
            let wanted = init.mode.clone().or_else(|| settings.as_ref().and_then(|s| s["mode"].as_str().map(String::from)));
            self.meta.mode = match wanted.as_deref() {
                Some("chat") => "chat".into(),
                Some("live") => "live".into(),
                _ if self.conn_v >= 2 => "chat".into(),
                _ => "live".into(),
            };
        }
        if let Some(s) = settings {
            let mut s = s;
            if fresh && init.mode.is_some() {
                s["mode"] = json!(self.meta.mode); // an explicit new_session mode wins
            }
            self.apply_settings(&s);
        }
        let _ = tx.send(self.ready_frame(pair_token));
        if let Some(last) = last_seq {
            let events: Vec<Value> = self.journal.iter().filter(|e| e["seq"].as_u64().unwrap_or(0) > last).cloned().collect();
            let _ = tx.send(json!({"t": "snapshot", "events": events}));
            let _ = tx.send(notice("reconnected", "Đã nối lại."));
        }
        if self.deps.voice.take_unannounced() {
            let _ = tx.send(tts_fallback_notice());
        }
        if self.running.is_some() {
            let _ = tx.send(state_frame("thinking", None));
        }
        // Replacing the sender drops the previous connection's channel, which closes that socket.
        self.conn = Some((conn, tx));
        if let Some(p) = self.capture.take() {
            let _ = p.reply.send(Err("Điện thoại vừa nối lại giữa chừng, chưa chụp được ảnh. Thử gọi lại.".into()));
        }
        if fresh && init.kickoff.as_deref() == Some("review_quiz") {
            let text = crate::review::quiz_kickoff(&self.deps, self.meta.folder_id.as_deref());
            self.pending.push_back(UserMsg { text, files: vec![], via: "text".into(), kickoff: true });
            self.try_start_turn();
            return;
        }
        self.maybe_offer_review();
    }

    /// A brand-new session with something due for review: the server speaks first (no Claude
    /// call): one `say`, the `review_offer` block, `turn_end`. At most once per session.
    fn maybe_offer_review(&mut self) {
        if self.turn != 0 || !self.journal.is_empty() || self.running.is_some() || self.brain_busy || self.conn.is_none() {
            return;
        }
        let mem = Memory::new(&self.deps.cfg.tutor_dir);
        let item = match self.folder_subject() {
            Some((f, sub)) => mem.folder_review_candidate(&f, &sub, memory::today()),
            None => mem.review_candidate(memory::today()),
        };
        let Some(item) = item else { return };
        let lines = offer_lines(&self.settings.lang, &item);
        self.quiet = true;
        let chat = self.is_chat();
        let r = self.emit_lines(lines, chat);
        self.quiet = false;
        if r.is_ok() {
            self.pending_review = Some(ReviewRef { subject: item.subject, topic: item.topic, note: item.note });
        }
    }

    fn memory_note(&mut self, args: &Value) -> Result<Value, String> {
        let field = |k: &str| args[k].as_str().unwrap_or("").trim().to_string();
        let kind = Kind::parse(&field("kind")).ok_or("kind phải là mistake, mastered, style, progress hoặc understood.")?;
        let topic = field("topic");
        if topic.is_empty() {
            return Err("Thiếu topic.".into());
        }
        let topic_id = field("topicId");
        let mut subject = field("subject");
        let folder = self.folder_subject();
        if let (Some((_, sub)), true) = (&folder, subject.is_empty()) {
            subject = sub.clone();
        }
        let r = Memory::new(&self.deps.cfg.tutor_dir).note_at(
            folder.as_ref().map(|(f, _)| f.as_str()),
            &subject,
            kind,
            &topic,
            &field("note"),
            Some(topic_id.as_str()).filter(|s| !s.is_empty()),
            memory::today(),
        )?;
        tracing::info!("memory_note: {} {:?} {:?} -> {}", r.subject, kind, r.topic, r.action);
        Ok(r.to_json())
    }

    // ----- incoming words -----

    /// A message or speech result from the phone. Speech (and dictation) goes through the
    /// utility model's correction first when it is configured and `settings.sttFix` is on.
    fn incoming(&mut self, text: String, files: Vec<String>, via: String, draft: bool, server_stt: bool) {
        let text = text.trim().to_string();
        if text.is_empty() {
            if draft {
                self.send(json!({"t": "heard", "text": "", "draft": true}));
            }
            return;
        }
        let speech = server_stt || draft || via == "voice";
        if speech && self.settings.stt_fix && self.deps.util.available() {
            self.spawn_fix(text, files, via, draft, server_stt);
            return;
        }
        let _ = self.self_tx.send(Cmd::Fixed { raw: text.clone(), text, draft, via, files, server_stt });
    }

    fn spawn_fix(&self, raw: String, files: Vec<String>, via: String, draft: bool, server_stt: bool) {
        let (subject, terms) = match self.folder_subject() {
            Some((f, sub)) => {
                let terms = self.deps.folders.knowledge(&f).map(|k| crate::folders::curriculum_terms(&k, 30)).unwrap_or_default();
                (memory::subject_name(&sub), terms)
            }
            None => (String::new(), vec![]),
        };
        let mut recent: Vec<String> = assistant_lines(&self.journal).into_iter().rev().take(2).collect();
        recent.reverse();
        let recent: Vec<String> = recent.into_iter().map(|s| s.chars().take(300).collect()).collect();
        let req = UtilReq::new("stt_fix", STT_FIX_SYSTEM, stt_fix_prompt(&subject, &terms, &recent, &raw), 200);
        let util = self.deps.util.clone();
        let usage = self.deps.usage.clone();
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            let fixed = util_llm::ask(&util, &usage, req, Duration::from_millis(STT_FIX_MS)).await;
            let text = match fixed {
                Ok(t) => accept_fix(&raw, &t),
                Err(e) => {
                    tracing::info!("stt fix skipped: {e}");
                    raw.clone()
                }
            };
            let _ = tx.send(Cmd::Fixed { raw, text, draft, via, files, server_stt });
        });
    }

    fn user_text(&mut self, text: String, files: Vec<String>, via: String) {
        let mut text = text.trim().to_string();
        if text.is_empty() && files.is_empty() {
            return;
        }
        // A waiting `listen` (general MCP) gets the utterance instead of the tutor.
        if let Some(l) = self.listener.take() {
            match l.send(text) {
                Ok(()) => return,
                Err(back) => text = back, // the listener gave up (timeout): normal turn
            }
        }
        let mut metas = Vec::new();
        for id in files.iter().take(30) {
            match self.deps.uploads.meta(id) {
                Some(m) => metas.push(m),
                None => self.send(notice("bad_file", "Không tìm thấy tệp đính kèm (cần tải lên qua /upload trước).")),
            }
        }
        self.pending.push_back(UserMsg { text, files: metas, via, kickoff: false });
        self.try_start_turn();
    }

    // ----- turns -----

    fn try_start_turn(&mut self) {
        if self.brain_busy || self.running.is_some() {
            return;
        }
        let Some(msg) = self.pending.pop_front() else { return };
        self.turn += 1;
        let entry = self.deps.catalog.resolve(&self.settings.model);

        let mut attachments = Vec::new();
        if let Some(jpeg) = self.latest_photo.take() {
            attachments.push(Attachment { kind: "image".into(), mime: "image/jpeg".into(), data: jpeg });
        }
        for f in &msg.files {
            if let Some((m, data)) = self.deps.uploads.read(&f.file_id) {
                attachments.push(Attachment { kind: m.kind, mime: m.mime, data });
            }
        }
        if !entry.vision && !attachments.is_empty() {
            attachments.clear();
            self.send(notice("no_vision", &format!("{} không đọc được ảnh/PDF, tớ bỏ tệp đính kèm. Đổi sang model khác để gửi ảnh.", entry.label)));
        }

        // The memory summary goes in once per Claude session (also not again after a `--resume`).
        let summary = if self.meta.memory_sent {
            None
        } else {
            self.meta.memory_sent = true;
            let mem = Memory::new(&self.deps.cfg.tutor_dir);
            match self.folder_subject() {
                Some((f, sub)) => mem.folder_summary(&f, &sub, self.deps.folders.knowledge(&f).as_ref(), memory::today()),
                None => mem.summary(memory::today()),
            }
        };
        let mode_note = if self.meta.mode_noted.as_deref() != Some(self.meta.mode.as_str()) {
            self.meta.mode_noted = Some(self.meta.mode.clone());
            Some(self.meta.mode.clone())
        } else {
            None
        };
        let review = self.pending_review.take();
        let interrupt = self.interrupt_note.take();
        let compact = self.compact_note.take();
        let ctx = TurnCtx {
            mode: mode_note.as_deref(),
            memory: summary.as_deref(),
            review: review.as_ref(),
            interrupt: interrupt.as_deref(),
            compact: compact.as_deref(),
        };
        let text = compose_turn_text(&self.settings, &msg.text, &ctx);

        if !msg.kickoff {
            let files: Vec<Value> = msg.files.iter().map(|f| json!({"fileId": f.file_id, "kind": f.kind, "name": f.name})).collect();
            let seq = self.alloc_seq();
            let mut echo = json!({"t": "user", "seq": seq, "turn": self.turn, "text": msg.text, "via": msg.via});
            if !files.is_empty() {
                echo["files"] = Value::Array(files);
            }
            if !msg.text.is_empty() {
                self.meta.preview = Some(msg.text.chars().take(80).collect());
            }
            self.emit_frame(echo);
        }
        let parser = if self.is_chat() {
            TurnParser::Chat(ChatParser::new(self.turn, self.next_seq))
        } else {
            TurnParser::Live(LessonParser::new(self.turn, self.next_seq, self.steps))
        };
        let input = TurnInput {
            turn: self.turn,
            user_text: msg.text,
            text,
            attachments,
            model: entry.id.clone(),
            mode: self.meta.mode.clone(),
            launch: self.deps.catalog.launch(&entry, self.settings.effort.as_deref()),
        };
        self.turn_model = Some((self.turn, entry));
        self.running = Some(Running { turn: self.turn, parser, spoke: false });
        self.brain_busy = true;
        self.touch();
        self.send(state_frame("thinking", None));
        self.brain.send_turn(input);
    }

    fn on_brain(&mut self, m: BrainMsg) {
        let live = self.running.as_ref().map(|r| r.turn) == Some(m.turn);
        match m.ev {
            BrainEvent::Text(s) => {
                if live {
                    let evs = self.with_parser(|p| p.push_str(&s));
                    self.emit_events(evs);
                }
            }
            BrainEvent::SessionStarted => {
                self.meta.claude_started = true;
                self.deps.store.upsert_session(&self.meta);
            }
            BrainEvent::Error { code, text } => {
                tracing::warn!("brain error for turn {}: {code}", m.turn);
                self.send(json!({"t": "error", "code": code, "text": text, "fatal": false}));
            }
            BrainEvent::Usage(u) => self.record_usage(m.turn, &u),
            BrainEvent::RateLimit(info) => self.deps.usage.set_limit(&info),
            BrainEvent::End => {
                if live {
                    let evs = self.with_parser(|p| p.finish());
                    self.emit_events(evs);
                    self.running = None;
                    let seq = self.alloc_seq();
                    self.emit_frame(json!({"t": "turn_end", "seq": seq, "turn": m.turn}));
                    self.send(state_frame("idle", None));
                    let tracked = self.turn_model.as_ref().map(|(t, e)| *t == m.turn && e.tracked()).unwrap_or(false);
                    if tracked && self.conn_v >= 2 {
                        self.send(self.deps.usage.report(Some(&self.id)));
                    }
                    self.maybe_auto_title(m.turn);
                }
                if m.turn == self.turn {
                    self.brain_busy = false;
                }
                self.maybe_compact();
                self.apply_compaction();
                self.try_start_turn();
            }
        }
    }

    fn record_usage(&mut self, turn: u64, u: &crate::brain::TurnUsage) {
        let entry = match &self.turn_model {
            Some((t, e)) if *t == turn => e.clone(),
            _ => self.deps.catalog.resolve(&self.settings.model),
        };
        let cost = Catalog::turn_cost(&entry, u.input_tokens, u.output_tokens, u.cache_read_tokens, u.cache_write_tokens, u.cli_cost_usd);
        self.deps.usage.record(&UsageRec {
            kind: "turn".into(),
            session_id: Some(self.id.clone()),
            folder_id: self.meta.folder_id.clone(),
            model_id: entry.id.clone(),
            provider: entry.provider.clone(),
            tracked: entry.tracked(),
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_tokens: u.cache_read_tokens,
            cache_write_tokens: u.cache_write_tokens,
            cost_usd: cost,
            ..Default::default()
        });
        self.last_context = u.context_tokens;
    }

    /// After the first real turn without `@title`: ask the utility model for a short title.
    fn maybe_auto_title(&mut self, turn: u64) {
        if self.meta.has_title() || self.title_asked || !self.deps.util.available() {
            return;
        }
        let user: Vec<String> = self.journal.iter().filter(|e| e["t"] == "user").filter_map(|e| e["text"].as_str().map(String::from)).collect();
        let answer: Vec<String> = self
            .journal
            .iter()
            .filter(|e| e["turn"].as_u64() == Some(turn))
            .filter_map(|e| e["text"].as_str().or(e["md"].as_str()).filter(|_| e["t"] == "say" || e["t"] == "chat").map(String::from))
            .collect();
        if user.is_empty() && answer.is_empty() {
            return;
        }
        self.title_asked = true;
        let subject = self.folder_subject().map(|(_, s)| memory::subject_name(&s)).unwrap_or_default();
        let prompt = format!(
            "Môn của folder (có thể trống): {subject}\nHọc sinh: {}\nGia sư: {}",
            cut(&user.join(" / "), 400),
            cut(&answer.join(" "), 600)
        );
        let req = UtilReq::new("title", TITLE_SYSTEM, prompt, 40);
        let (util, usage, tx) = (self.deps.util.clone(), self.deps.usage.clone(), self.self_tx.clone());
        tokio::spawn(async move {
            match util_llm::ask(&util, &usage, req, Duration::from_secs(10)).await {
                Ok(t) => {
                    let t: String = util_llm::one_line(&t).chars().take(40).collect();
                    if !t.trim().is_empty() {
                        let _ = tx.send(Cmd::AutoTitle(t.trim().to_string()));
                    }
                }
                Err(e) => tracing::info!("auto title skipped: {e}"),
            }
        });
    }

    /// The last turn's input was over `BELUGA_COMPACT_TOKENS`: summarize the journal with the
    /// utility model; the switch to a fresh Claude session happens when the brain is idle.
    fn maybe_compact(&mut self) {
        if self.compacting || self.compact_ready.is_some() || self.last_context <= self.deps.cfg.compact_tokens || !self.deps.util.available() {
            return;
        }
        self.compacting = true;
        let transcript = transcript(&self.journal, 40_000);
        let req = UtilReq::new("compact", COMPACT_SYSTEM, transcript, 1500);
        let (util, usage, tx) = (self.deps.util.clone(), self.deps.usage.clone(), self.self_tx.clone());
        tokio::spawn(async move {
            let r = util_llm::ask(&util, &usage, req, Duration::from_secs(90)).await;
            if let Err(e) = &r {
                tracing::warn!("compaction failed: {e}");
            }
            let _ = tx.send(Cmd::Compacted(r.ok().filter(|s| !s.trim().is_empty())));
        });
    }

    fn apply_compaction(&mut self) {
        if self.brain_busy || self.running.is_some() {
            return;
        }
        let Some(summary) = self.compact_ready.take() else { return };
        let new_id = uuid::Uuid::new_v4().to_string();
        tracing::info!("session {} compacted ({} tokens) -> claude session {new_id}", self.id, self.last_context);
        self.brain.reset(new_id.clone());
        self.meta.claude_id = Some(new_id);
        self.meta.claude_started = false;
        self.meta.memory_sent = false;
        self.meta.mode_noted = None;
        self.last_context = 0;
        self.compact_note = Some(summary);
        self.deps.store.upsert_session(&self.meta);
        self.send(notice("compacted", "Phiên dài quá nên tớ đã tóm tắt lại phần trước để nhớ gọn hơn."));
    }

    fn alloc_seq(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    fn with_parser(&mut self, f: impl FnOnce(&mut TurnParser) -> Vec<Event>) -> Vec<Event> {
        let Some(r) = self.running.as_mut() else { return vec![] };
        let evs = f(&mut r.parser);
        self.next_seq = r.parser.next_seq();
        if let Some(st) = r.parser.steps() {
            self.steps = st;
        }
        evs
    }

    fn mark_speaking(&mut self) {
        if let Some(r) = self.running.as_mut() {
            if !r.spoke {
                r.spoke = true;
                self.send(state_frame("speaking", None));
            }
        }
    }

    fn emit_events(&mut self, evs: Vec<Event>) {
        for ev in evs {
            match &ev {
                Event::Title { .. } if self.meta.user_titled => {}
                Event::Title { text } => {
                    self.meta.title = format!("{text} · {}", date_suffix());
                    self.meta.titled = true;
                    self.touch();
                    self.send(json!({"t": "title", "text": self.meta.title}));
                }
                Event::Say { .. } | Event::Chat { .. } => {
                    self.mark_speaking();
                    self.emit_frame(ev.to_json());
                }
                Event::ChatDelta { .. } => {
                    self.mark_speaking();
                    self.send(ev.to_json()); // streamed only: never journaled
                }
                Event::Done { .. } => {
                    self.emit_frame(ev.to_json());
                    self.send(state_frame("done", None));
                }
                _ => self.emit_frame(ev.to_json()),
            }
        }
    }

    /// Journal a sequenced frame and send it to the phone (`say` gets `audio:true` when TTS is on).
    fn emit_frame(&mut self, frame: Value) {
        self.journal.push(frame.clone());
        self.deps.store.append_event(&self.id, &frame);
        self.touch();
        let mut out = frame;
        if out["t"] == "say" && self.deps.voice.tts_enabled() {
            out["audio"] = Value::Bool(true);
            self.spawn_tts(out["seq"].as_u64().unwrap_or(0), out["text"].as_str().unwrap_or("").to_string());
        }
        self.send(out);
    }

    /// Start synthesizing one sentence right away (parallel, bounded by `Voice::gate`).
    /// Success -> `Cmd::Audio`; failures only mean "no audio for this sentence" (the phone speaks
    /// it itself) unless they switch TTS off, which is announced once.
    fn spawn_tts(&mut self, seq: u64, text: String) {
        let voice = self.deps.voice.clone();
        let tx = self.self_tx.clone();
        let lang = self.settings.lang.clone();
        let wait = voice.tts_deadline(&text);
        let handle = tokio::spawn(async move {
            let Some(tts) = voice.tts.clone() else {
                let _ = tx.send(Cmd::AudioSkip { seq });
                return;
            };
            let _permit = voice.gate.acquire().await;
            if !voice.tts_enabled() {
                let _ = tx.send(Cmd::AudioSkip { seq });
                return;
            }
            // A dropped connection is common when several sentences start at once: retry it
            // once inside the same deadline before giving the sentence up.
            let attempt = async {
                match tts.synth(&text, &lang).await {
                    Err(e) if e.status == 0 => {
                        tracing::info!("tts retry for seq {seq}: {}", e.message);
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                        tts.synth(&text, &lang).await
                    }
                    other => other,
                }
            };
            let result = match tokio::time::timeout(wait, attempt).await {
                Ok(r) => r,
                Err(_) => Err(crate::voice::VoiceError::timeout()),
            };
            match result {
                Ok(data) => {
                    voice.record_ok();
                    let _ = tx.send(Cmd::Audio { seq, data });
                }
                Err(e) => {
                    tracing::warn!("tts failed for seq {seq}: {}", e.message);
                    let _ = tx.send(Cmd::AudioSkip { seq });
                    if voice.record_failure(&e) {
                        let _ = tx.send(Cmd::TtsFailed);
                    }
                }
            }
        });
        self.tts_tasks.retain(|h| !h.is_finished());
        self.tts_tasks.push(handle.abort_handle());
    }

    fn interrupt(&mut self) {
        let Some(mut r) = self.running.take() else { return };
        r.parser.discard();
        self.next_seq = r.parser.next_seq();
        if let Some(st) = r.parser.steps() {
            self.steps = st;
        }
        self.interrupt_note = Some(build_interrupt_note(&self.journal, r.turn, self.played));
        let seq = self.alloc_seq();
        self.emit_frame(json!({"t": "turn_end", "seq": seq, "turn": r.turn, "interrupted": true}));
        self.send(state_frame("idle", None));
        for h in self.tts_tasks.drain(..) {
            h.abort();
        }
        self.audio_floor = self.next_seq;
        if let Some(p) = self.capture.take() {
            let _ = p.reply.send(Err("Học sinh đã ngắt lời, bỏ việc chụp ảnh.".into()));
        }
        self.brain.interrupt();
    }

    // ----- general MCP helpers -----

    fn status(&self) -> Value {
        let busy = self.running.is_some() || self.brain_busy || !self.pending.is_empty();
        let state = if busy {
            "busy"
        } else if self.listener.as_ref().map(|l| !l.is_closed()).unwrap_or(false) {
            "listening"
        } else {
            "idle"
        };
        let v = &self.deps.voice;
        json!({
            "connected": self.conn.is_some(),
            "sessionId": self.id,
            "title": self.meta.title,
            "state": state,
            "mode": self.meta.mode,
            "tts": if v.tts_enabled() { v.tts_provider } else { "device" },
            "stt": if v.stt_enabled() { v.stt_provider } else { "device" },
            "lang": self.settings.lang,
            "lastSeq": self.next_seq.saturating_sub(1),
        })
    }

    /// Run `lines` through the real parser as one self-contained "turn" and send the events.
    fn present(&mut self, lines: String) -> Result<Value, String> {
        if self.conn.is_none() {
            return Err("Điện thoại đang không kết nối.".into());
        }
        if self.running.is_some() || self.brain_busy || !self.pending.is_empty() {
            return Err("Gia sư đang bận (đang giảng hoặc đang xử lý lời học sinh). Gọi interrupt trước nếu muốn chen ngang.".into());
        }
        if lines.trim().is_empty() {
            return Err("Không có nội dung.".into());
        }
        self.emit_lines(lines, false)
    }

    /// Run `lines` through the parser (chat or live) as one self-contained turn and send its events.
    fn emit_lines(&mut self, lines: String, chat: bool) -> Result<Value, String> {
        self.turn += 1;
        let turn = self.turn;
        let mut p = if chat { TurnParser::Chat(ChatParser::new(turn, self.next_seq)) } else { TurnParser::Live(LessonParser::new(turn, self.next_seq, self.steps)) };
        let mut evs = p.push_str(&lines);
        evs.extend(p.push_str("\n"));
        evs.extend(p.finish());
        self.next_seq = p.next_seq();
        if let Some(st) = p.steps() {
            self.steps = st;
        }
        let dropped = p.take_dropped();
        let count = evs.iter().filter(|e| e.seq().is_some()).count();
        if evs.iter().any(|e| matches!(e, Event::Say { .. } | Event::Chat { .. })) {
            self.send(state_frame("speaking", None));
        }
        let evs: Vec<Event> = evs.into_iter().filter(|e| !matches!(e, Event::ChatDelta { .. })).collect();
        self.emit_events(evs);
        let seq = self.alloc_seq();
        self.emit_frame(json!({"t": "turn_end", "seq": seq, "turn": turn}));
        self.send(state_frame("idle", None));
        Ok(json!({"turn": turn, "events": count, "dropped": dropped}))
    }

    /// Stop the tutor turn if one is running; otherwise cut off what an MCP caller queued.
    fn mcp_interrupt(&mut self) -> Value {
        if self.running.is_some() {
            self.interrupt();
            return json!({"interrupted": true, "was": "tutor_turn"});
        }
        for h in self.tts_tasks.drain(..) {
            h.abort();
        }
        self.audio_floor = self.next_seq;
        if self.turn > 0 && self.conn.is_some() {
            let seq = self.alloc_seq();
            let turn = self.turn;
            self.emit_frame(json!({"t": "turn_end", "seq": seq, "turn": turn, "interrupted": true}));
        }
        json!({"interrupted": true, "was": "queued_playback"})
    }

    // ----- photos -----

    fn start_capture(&mut self, reply: oneshot::Sender<Result<Vec<u8>, String>>) {
        if self.conn.is_none() {
            let _ = reply.send(Err("Điện thoại đang không kết nối nên chưa chụp được ảnh.".into()));
            return;
        }
        if self.capture.is_some() {
            let _ = reply.send(Err("Đang chờ một ảnh khác, thử lại sau vài giây.".into()));
            return;
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        self.send(state_frame("tool", Some("capture")));
        self.send(json!({"t": "capture_request", "requestId": request_id}));
        self.capture = Some(PendingCapture { request_id: request_id.clone(), reply });
        let tx = self.self_tx.clone();
        let wait = Duration::from_millis(self.deps.cfg.capture_timeout_ms);
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let _ = tx.send(Cmd::CaptureTimeout { request_id });
        });
    }

    fn save_photo(&self, jpeg: &[u8]) {
        let dir = self.deps.cfg.tutor_dir.join("photos").join(&self.id);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = std::fs::write(dir.join(format!("{ms}.jpg")), jpeg);
    }

    fn photo(&mut self, request_id: Option<String>, jpeg: Vec<u8>) {
        self.save_photo(&jpeg);
        let matches = match (&request_id, &self.capture) {
            (Some(r), Some(p)) => *r == p.request_id,
            _ => false,
        };
        if matches {
            let p = self.capture.take().unwrap();
            let _ = p.reply.send(Ok(jpeg));
            self.send(state_frame("thinking", None));
        } else {
            // Hand-taken photo (or a late answer): attach to the next user turn.
            self.latest_photo = Some(jpeg);
        }
    }
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

const STT_FIX_SYSTEM: &str = "Bạn sửa lỗi nhận dạng giọng nói (STT) trong lời một học sinh Việt Nam nói với gia sư. \
Chỉ sửa từ bị nghe nhầm (thuật ngữ môn học, tên riêng, số, ký hiệu bị đọc thành chữ, dấu thanh), giữ nguyên ý, cách nói \
và ngôn ngữ của học sinh (có thể lẫn tiếng Anh). Không trả lời câu hỏi, không giải thích, không thêm ngoặc kép. \
Không chắc thì giữ nguyên. Chỉ in ra đúng câu đã sửa.";

fn stt_fix_prompt(subject: &str, terms: &[String], recent: &[String], raw: &str) -> String {
    let mut p = String::new();
    if !subject.is_empty() {
        p.push_str(&format!("Môn: {subject}\n"));
    }
    if !terms.is_empty() {
        p.push_str(&format!("Thuật ngữ trong giáo trình: {}\n", terms.join("; ")));
    }
    if !recent.is_empty() {
        p.push_str(&format!("Gia sư vừa nói: {}\n", recent.join(" / ")));
    }
    p.push_str(&format!("Câu STT cần sửa: {raw}"));
    p
}

/// Keep the model's correction only when it looks like a correction (not an answer or an essay).
pub fn accept_fix(raw: &str, fixed: &str) -> String {
    let f = util_llm::one_line(fixed);
    let (rl, fl) = (raw.chars().count(), f.chars().count());
    if f.is_empty() || fl > rl * 2 + 20 || fl * 2 + 10 < rl {
        return raw.to_string();
    }
    f
}

const TITLE_SYSTEM: &str = "Đặt tên ngắn cho một phiên học (tối đa 40 ký tự) theo khuôn `Môn · Chủ đề`, ví dụ \
`Toán · Bất phương trình bậc hai`. Môn viết tắt quen thuộc: Toán, Lý, Hóa, Văn, Anh, Đức, Tin, Sinh, Sử, Địa. \
Chỉ in ra tên, không ngoặc, không giải thích.";

const COMPACT_SYSTEM: &str = "Bạn tóm tắt một phiên học giữa gia sư và học sinh để gia sư học tiếp trong một ngữ cảnh mới. \
Viết tiếng Việt, gạch đầu dòng, tối đa 25 dòng: bài/đề đang làm (chép nguyên công thức quan trọng), đã giảng tới bước nào, \
học sinh đã hiểu gì, còn sai hoặc lúng túng chỗ nào, cách giảng nào hiệu quả, câu hỏi đang chờ học sinh trả lời (nếu có), \
việc tiếp theo nên làm. Không bịa thêm.";

/// The session journal as a readable transcript for the utility model (newest part kept).
fn transcript(journal: &[Value], max_chars: usize) -> String {
    let mut lines = Vec::new();
    for e in journal {
        let l = match e["t"].as_str().unwrap_or("") {
            "user" => format!("Học sinh: {}", e["text"].as_str().unwrap_or("")),
            "say" => format!("Gia sư: {}", e["text"].as_str().unwrap_or("")),
            "chat" => format!("Gia sư: {}", e["md"].as_str().unwrap_or("")),
            "step" => format!("[Bước: {}]", e["title"].as_str().unwrap_or("")),
            "show" => {
                let b = &e["block"];
                let body = ["tex", "md", "code", "word"].iter().find_map(|k| b[*k].as_str()).unwrap_or("");
                format!("[Bảng {}: {}]", b["type"].as_str().unwrap_or(""), cut(body, 300))
            }
            "ask" => format!("Gia sư hỏi: {}", e["prompt"].as_str().unwrap_or("")),
            _ => continue,
        };
        lines.push(l);
    }
    let all = lines.join("\n");
    let n = all.chars().count();
    if n <= max_chars {
        all
    } else {
        all.chars().skip(n - max_chars).collect()
    }
}

fn tts_fallback_notice() -> Value {
    notice("tts_fallback", "Giọng đọc của server không dùng được, chuyển sang giọng của máy.")
}

/// Extra context for one turn, each part optional.
#[derive(Default)]
pub struct TurnCtx<'a> {
    /// `chat` / `live`: sent on the first turn and when the mode changes.
    pub mode: Option<&'a str>,
    pub memory: Option<&'a str>,
    pub review: Option<&'a ReviewRef>,
    pub interrupt: Option<&'a str>,
    /// Summary of the conversation before a compaction.
    pub compact: Option<&'a str>,
}

/// The text of one user turn: settings prefix (and mode note) and the student's words, then (when
/// present) the compaction summary, the memory summary, the review note and the interruption note.
pub fn compose_turn_text(settings: &Settings, user_text: &str, ctx: &TurnCtx) -> String {
    let mut text = format!("[lang={}] [rigor={}]", settings.lang, settings.rigor);
    if let Some(m) = ctx.mode {
        text.push_str(&format!(" [mode: {m}]"));
    }
    text.push(' ');
    text.push_str(user_text);
    if let Some(c) = ctx.compact {
        text.push_str(&format!("\n[tóm tắt phiên trước, ngữ cảnh cũ đã được thu gọn]\n{}\n[/tóm tắt]", c.trim()));
    }
    if let Some(m) = ctx.memory {
        text.push('\n');
        text.push_str(m);
    }
    if let Some(r) = ctx.review {
        text.push_str(&format!("\n[ôn lại: {} · {}", r.subject, r.topic));
        if !r.note.is_empty() {
            text.push_str(&format!(" · {}", r.note));
        }
        text.push(']');
    }
    if let Some(note) = ctx.interrupt {
        text.push('\n');
        text.push_str(note);
    }
    text
}

/// Lower the first letter of a topic so it reads inside a sentence ("Xét dấu" -> "xét dấu"),
/// but leave acronyms and names such as "C++" or "SSAT" alone.
fn inline_topic(topic: &str) -> String {
    let mut c = topic.chars();
    match (c.next(), c.next()) {
        (Some(a), b) if a.is_uppercase() && b.map(|b| !b.is_uppercase() && b != '+').unwrap_or(true) => {
            a.to_lowercase().chain(topic.chars().skip(1)).collect()
        }
        _ => topic.to_string(),
    }
}

/// Lesson lines of the review invitation: spoken sentence, step, `review_offer` block.
pub fn offer_lines(lang: &str, item: &memory::DueItem) -> String {
    let (say, step) = if lang == "en" {
        (format!("Last time you kept mixing up {}. Want a quick review question?", inline_topic(&item.topic)), "Review")
    } else {
        (format!("Lần trước cậu hay nhầm {}, ôn một câu không?", inline_topic(&item.topic)), "Ôn lại")
    };
    let name = memory::subject_name(&item.subject);
    let mut fallback = format!("**{step}** · {name} · {}", item.topic);
    if !item.note.is_empty() {
        fallback.push_str(&format!("\n\n{}", item.note));
    }
    let mut block = json!({
        "type": "review_offer", "topic": item.topic, "subject": item.subject, "subjectName": name,
        "lastSeen": item.last.format("%Y-%m-%d").to_string(), "fallback": fallback,
    });
    if !item.note.is_empty() {
        block["note"] = json!(item.note);
    }
    format!("{say}\n@step {}\n@show {block}", json!({"title": step}))
}

/// After a restart: an offer that no later turn has answered yet is still pending.
fn pending_review_from_journal(journal: &[Value], turn: u64) -> Option<ReviewRef> {
    let e = journal.iter().rev().find(|e| e["t"] == "show" && e["block"]["type"] == "review_offer" && e["turn"].as_u64() == Some(turn))?;
    let b = &e["block"];
    Some(ReviewRef {
        subject: b["subject"].as_str()?.to_string(),
        topic: b["topic"].as_str()?.to_string(),
        note: b["note"].as_str().unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(seq: u64, turn: u64, text: &str) -> Value {
        json!({"t": "say", "seq": seq, "turn": turn, "text": text, "audio": false})
    }

    fn rref(note: &str) -> ReviewRef {
        ReviewRef { subject: "toan".into(), topic: "Xét dấu tam thức khi a âm".into(), note: note.into() }
    }

    #[test]
    fn turn_text_layers_in_a_fixed_order() {
        let st = Settings::default();
        assert_eq!(compose_turn_text(&st, "chào", &TurnCtx::default()), "[lang=vi] [rigor=medium] chào");
        let ctx = TurnCtx {
            mode: Some("chat"),
            memory: Some("[memory]\nx\n[/memory]"),
            review: Some(&rref("hay quên")),
            interrupt: Some("(Ghi chú hệ thống: ngắt)"),
            compact: Some("- đang làm bài 2"),
        };
        assert_eq!(
            compose_turn_text(&st, "ừ", &ctx),
            "[lang=vi] [rigor=medium] [mode: chat] ừ\n[tóm tắt phiên trước, ngữ cảnh cũ đã được thu gọn]\n- đang làm bài 2\n[/tóm tắt]\n[memory]\nx\n[/memory]\n[ôn lại: toan · Xét dấu tam thức khi a âm · hay quên]\n(Ghi chú hệ thống: ngắt)"
        );
        // empty note: no trailing field
        let t = compose_turn_text(&st, "ừ", &TurnCtx { review: Some(&rref("")), ..Default::default() });
        assert!(t.ends_with("\n[ôn lại: toan · Xét dấu tam thức khi a âm]"), "{t}");
    }

    #[test]
    fn stt_fix_is_kept_only_when_plausible() {
        assert_eq!(accept_fix("tam thứt bật hai", "\"tam thức bậc hai\""), "tam thức bậc hai");
        assert_eq!(accept_fix("x bình", ""), "x bình");
        let essay = "Đây là câu trả lời rất dài cho câu hỏi của học sinh về tam thức bậc hai và cách giải chi tiết";
        assert_eq!(accept_fix("tam thức", essay), "tam thức");
        let p = stt_fix_prompt("Toán", &["Căn bậc hai".into()], &["Nhìn màn hình nhé.".into()], "căn bật hai");
        assert!(p.contains("Môn: Toán") && p.contains("Căn bậc hai") && p.contains("Gia sư vừa nói") && p.ends_with("căn bật hai"));
    }

    #[test]
    fn transcript_keeps_the_newest_part() {
        let j = vec![
            json!({"t":"user","text":"giảng câu a"}),
            say(1, 1, "Nhìn màn hình nhé."),
            json!({"t":"show","block":{"type":"math","tex":"x^2-1"}}),
            json!({"t":"chat","md":"**Bước 1**"}),
            json!({"t":"audio"}),
        ];
        let t = transcript(&j, 10_000);
        assert_eq!(t, "Học sinh: giảng câu a\nGia sư: Nhìn màn hình nhé.\n[Bảng math: x^2-1]\nGia sư: **Bước 1**");
        assert_eq!(transcript(&j, 12), "Gia sư: **Bước 1**".chars().skip(6).collect::<String>());
    }

    #[test]
    fn offer_lines_are_valid_lesson_lines() {
        let item = memory::DueItem {
            subject: "toan".into(),
            topic: "Xét dấu tam thức khi a âm".into(),
            note: "hay quên đổi chiều".into(),
            last: chrono::NaiveDate::from_ymd_opt(2026, 11, 14).unwrap(),
            due: chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap(),
            count: 3,
        };
        for lang in ["vi", "en"] {
            let mut p = crate::parser::LessonParser::new(1, 1, 0);
            let mut ev = p.push_str(&offer_lines(lang, &item));
            ev.extend(p.push_str("\n"));
            ev.extend(p.finish());
            assert!(p.take_dropped().is_empty(), "{lang}");
            let Event::Say { text, .. } = &ev[0] else { panic!("{ev:?}") };
            assert!(!text.contains('!'), "{text}");
            assert!(text.contains("xét dấu tam thức khi a âm"), "{text}");
            assert!(matches!(&ev[1], Event::Step { .. }));
            let Event::Show { block, .. } = &ev[2] else { panic!("{ev:?}") };
            assert_eq!(block["type"], "review_offer");
            assert_eq!(block["note"], "hay quên đổi chiều");
            assert!(block["fallback"].as_str().unwrap().contains("Xét dấu"));
            assert_eq!(ev.len(), 3);
        }
        assert_eq!(inline_topic("Xét dấu"), "xét dấu");
        assert_eq!(inline_topic("C++ pointer"), "C++ pointer");
        assert_eq!(inline_topic("SSAT reading"), "SSAT reading");
        assert_eq!(inline_topic("X"), "x");
    }

    #[test]
    fn pending_review_is_recovered_only_until_a_later_turn() {
        let offer = json!({"t":"show","seq":2,"turn":1,"step":0,"block":{"type":"review_offer","subject":"toan","topic":"T","note":"n","id":"b2"}});
        let j = vec![say(1, 1, "Lần trước..."), offer, json!({"t":"turn_end","seq":3,"turn":1})];
        assert_eq!(pending_review_from_journal(&j, 1), Some(ReviewRef { subject: "toan".into(), topic: "T".into(), note: "n".into() }));
        assert_eq!(pending_review_from_journal(&j, 2), None, "a later turn already happened");
        assert_eq!(pending_review_from_journal(&[], 0), None);
    }

    #[test]
    fn interrupt_note_uses_played_seq() {
        let j = vec![say(1, 1, "Một."), say(2, 1, "Hai."), say(3, 1, "Ba."), say(4, 2, "Khác lượt.")];
        let n = build_interrupt_note(&j, 1, 2);
        assert!(n.contains("\"Hai.\""), "{n}");
        assert!(n.contains("\"Ba.\""), "{n}");
        assert!(!n.contains("Khác lượt"));
        let none = build_interrupt_note(&j, 1, 0);
        assert!(none.contains("chưa nghe được câu nào"), "{none}");
        assert!(!build_interrupt_note(&j, 1, 3).contains("Phần chưa nghe"));
    }
}
