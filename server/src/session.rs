//! One actor per phone session: owns the journal, the lesson parser, the brain (mock or Claude),
//! the attached phone connection, photo capture and TTS fan-out.

use crate::brain::claude::{ClaudeCfg, ClaudeDriver};
use crate::brain::mock::MockBrain;
use crate::brain::{Brain, BrainEvent, BrainMsg, Settings, TurnInput};
use crate::config::Config;
use crate::parser::{Event, LessonParser};
use crate::store::{date_suffix, now_rfc3339, SessionMeta, Store};
use crate::voice::Voice;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

pub enum Cmd {
    Attach {
        conn: u64,
        tx: UnboundedSender<Value>,
        last_seq: Option<u64>,
        settings: Option<Value>,
        pair_token: Option<String>,
    },
    Detach { conn: u64 },
    UserText { text: String },
    /// Server-side STT result: show it as `heard`, then treat as user text.
    Heard { text: String },
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
}

#[derive(Clone)]
pub struct Deps {
    pub cfg: Arc<Config>,
    pub store: Arc<Store>,
    pub voice: Arc<Voice>,
    /// Port the HTTP server is really bound to (used in the MCP URL for claude).
    pub port: u16,
}

struct Running {
    turn: u64,
    parser: LessonParser,
    spoke: bool,
}

struct PendingCapture {
    request_id: String,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
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
    settings: Settings,
    running: Option<Running>,
    /// The brain has not yet sent `End` for the last turn it was given.
    brain_busy: bool,
    pending: VecDeque<String>,
    latest_photo: Option<Vec<u8>>,
    capture: Option<PendingCapture>,
    played: u64,
    interrupt_note: Option<String>,
    /// Running TTS tasks; aborted when the turn is interrupted.
    tts_tasks: Vec<tokio::task::AbortHandle>,
    /// `audio` frames with a seq below this belong to an interrupted turn and are dropped.
    audio_floor: u64,
    listener: Option<oneshot::Sender<String>>,
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
    let meta = deps.store.get_session(&id).unwrap_or(SessionMeta {
        session_id: id.clone(),
        title: format!("Phiên mới · {}", date_suffix()),
        updated_at: now_rfc3339(),
        claude_started: false,
    });
    let brain = if deps.cfg.mock {
        Brain::Mock(MockBrain::new(btx, tx.clone(), deps.cfg.mock_delay_ms))
    } else {
        let mcp_dir = deps.store.dir().join("mcp");
        let _ = std::fs::create_dir_all(&mcp_dir);
        let mcp_path = mcp_dir.join(format!("{id}.json"));
        let conf = json!({"mcpServers": {crate::brain::claude::MCP_SERVER: {
            "type": "http", "url": format!("http://127.0.0.1:{}/mcp/{}", deps.port, id)}}});
        let _ = std::fs::write(&mcp_path, conf.to_string());
        Brain::Claude(ClaudeDriver::new(
            ClaudeCfg {
                bin: deps.cfg.claude_bin.clone(),
                tutor_dir: deps.cfg.tutor_dir.clone(),
                mcp_config: mcp_path,
                effort: deps.cfg.claude_effort.clone(),
            },
            id.clone(),
            meta.claude_started,
            btx,
        ))
    };
    let s = Session {
        id,
        deps,
        meta,
        journal,
        next_seq,
        steps,
        turn,
        conn: None,
        settings: Settings::default(),
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

impl Session {
    async fn run(mut self, mut rx: UnboundedReceiver<Cmd>, mut brx: UnboundedReceiver<BrainMsg>) {
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(c) => self.on_cmd(c),
                    None => break,
                },
                msg = brx.recv() => if let Some(m) = msg { self.on_brain(m) },
            }
        }
        self.brain.shutdown();
    }

    fn send(&self, v: Value) {
        if let Some((_, tx)) = &self.conn {
            let _ = tx.send(v);
        }
    }

    fn touch(&mut self) {
        self.meta.updated_at = now_rfc3339();
        self.deps.store.upsert_session(&self.meta);
    }

    fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Attach { conn, tx, last_seq, settings, pair_token } => self.attach(conn, tx, last_seq, settings, pair_token),
            Cmd::Detach { conn } => {
                if self.conn.as_ref().map(|(c, _)| *c) == Some(conn) {
                    self.conn = None;
                    if let Some(p) = self.capture.take() {
                        let _ = p.reply.send(Err("Điện thoại vừa mất kết nối nên chưa chụp được ảnh.".into()));
                    }
                }
            }
            Cmd::UserText { text } => self.user_text(text),
            Cmd::Heard { text } => {
                self.send(json!({"t": "heard", "text": text}));
                self.user_text(text);
            }
            Cmd::Settings(v) => self.settings.apply(&v),
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
                let _ = reply.send(self.present(lines.join("
")));
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
            Cmd::Capture { reply } => self.start_capture(reply),
            Cmd::CaptureTimeout { request_id } => {
                if self.capture.as_ref().map(|p| p.request_id == request_id).unwrap_or(false) {
                    let p = self.capture.take().unwrap();
                    let _ = p.reply.send(Err(
                        "Quá 8 giây điện thoại không gửi ảnh về. Bảo học sinh kiểm tra camera rồi thử chụp lại.".into(),
                    ));
                    self.send(state_frame("thinking", None));
                }
            }
        }
    }

    fn attach(&mut self, conn: u64, tx: UnboundedSender<Value>, last_seq: Option<u64>, settings: Option<Value>, pair_token: Option<String>) {
        if let Some(s) = settings {
            self.settings.apply(&s);
        }
        let voice = &self.deps.voice;
        let mut ready = json!({
            "t": "ready", "v": 1, "sessionId": self.id, "title": self.meta.title,
            "tts": if voice.tts_enabled() { "server" } else { "device" },
            "stt": if voice.stt_enabled() { "server" } else { "device" },
            "boardVersion": self.deps.cfg.board_version(),
        });
        if let Some(ms) = voice.audio_wait_ms() {
            ready["audioWaitMs"] = json!(ms);
        }
        if let Some(t) = pair_token {
            ready["pairToken"] = Value::String(t);
        }
        let _ = tx.send(ready);
        if let Some(last) = last_seq {
            let events: Vec<Value> = self.journal.iter().filter(|e| e["seq"].as_u64().unwrap_or(0) > last).cloned().collect();
            let _ = tx.send(json!({"t": "snapshot", "events": events}));
            let _ = tx.send(json!({"t": "notice", "code": "reconnected", "text": "Đã nối lại."}));
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
    }

    // ----- turns -----

    fn user_text(&mut self, text: String) {
        let mut text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        // A waiting `listen` (general MCP) gets the utterance instead of the tutor.
        if let Some(l) = self.listener.take() {
            match l.send(text) {
                Ok(()) => return,
                Err(back) => text = back, // the listener gave up (timeout): normal turn
            }
        }
        self.pending.push_back(text);
        self.try_start_turn();
    }

    fn try_start_turn(&mut self) {
        if self.brain_busy || self.running.is_some() {
            return;
        }
        let Some(user_text) = self.pending.pop_front() else { return };
        self.turn += 1;
        let prefix = format!("[lang={}] [rigor={}]", self.settings.lang, self.settings.rigor);
        let mut text = format!("{prefix} {user_text}");
        if let Some(note) = self.interrupt_note.take() {
            text.push('\n');
            text.push_str(&note);
        }
        let input = TurnInput {
            turn: self.turn,
            user_text,
            text,
            image: self.latest_photo.take(),
            model: self.settings.model.clone(),
        };
        self.running = Some(Running { turn: self.turn, parser: LessonParser::new(self.turn, self.next_seq, self.steps), spoke: false });
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
            BrainEvent::End => {
                if live {
                    let evs = self.with_parser(|p| p.finish());
                    self.emit_events(evs);
                    self.running = None;
                    let seq = self.alloc_seq();
                    self.emit_frame(json!({"t": "turn_end", "seq": seq, "turn": m.turn}));
                    self.send(state_frame("idle", None));
                }
                if m.turn == self.turn {
                    self.brain_busy = false;
                }
                self.try_start_turn();
            }
        }
    }

    fn alloc_seq(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    fn with_parser(&mut self, f: impl FnOnce(&mut LessonParser) -> Vec<Event>) -> Vec<Event> {
        let Some(r) = self.running.as_mut() else { return vec![] };
        let evs = f(&mut r.parser);
        self.next_seq = r.parser.next_seq();
        self.steps = r.parser.steps();
        evs
    }

    fn emit_events(&mut self, evs: Vec<Event>) {
        for ev in evs {
            match &ev {
                Event::Title { text } => {
                    self.meta.title = format!("{text} · {}", date_suffix());
                    self.touch();
                    self.send(json!({"t": "title", "text": self.meta.title}));
                }
                Event::Say { .. } => {
                    if let Some(r) = self.running.as_mut() {
                        if !r.spoke {
                            r.spoke = true;
                            self.send(state_frame("speaking", None));
                        }
                    }
                    self.emit_frame(ev.to_json());
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
            let result = match tokio::time::timeout(wait, tts.synth(&text, &lang)).await {
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
        self.steps = r.parser.steps();
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
        self.turn += 1;
        let turn = self.turn;
        let mut p = LessonParser::new(turn, self.next_seq, self.steps);
        let mut evs = p.push_str(&lines);
        evs.extend(p.push_str("
"));
        evs.extend(p.finish());
        self.next_seq = p.next_seq();
        self.steps = p.steps();
        let dropped = p.take_dropped();
        let count = evs.iter().filter(|e| e.seq().is_some()).count();
        if evs.iter().any(|e| matches!(e, Event::Say { .. })) {
            self.send(state_frame("speaking", None));
        }
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

fn tts_fallback_notice() -> Value {
    json!({"t": "notice", "code": "tts_fallback", "text": "Giọng đọc của server không dùng được, chuyển sang giọng của máy."})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(seq: u64, turn: u64, text: &str) -> Value {
        json!({"t": "say", "seq": seq, "turn": turn, "text": text, "audio": false})
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
