//! Driver for the real Claude Code CLI: one long-lived `claude -p` stream-json process per phone session.
//!
//! Verified against claude 2.1.325:
//! * user turns go to stdin as `{"type":"user","message":{"role":"user","content":[...]}}`;
//! * `--include-partial-messages` yields `stream_event` lines with `text_delta` fragments;
//! * `{"type":"control_request","request_id":..,"request":{"subtype":"interrupt"}}` aborts the
//!   running turn (the process stays alive, a `result` with `terminal_reason":"aborted_streaming"` follows);
//! * a `result` line marks the end of each turn.

use super::{BrainEvent, BrainMsg, Sink, TurnInput, TurnUsage};
use crate::models::Launch;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

#[derive(Clone)]
pub struct ClaudeCfg {
    pub bin: String,
    pub tutor_dir: PathBuf,
    pub mcp_config: PathBuf,
    /// `--settings` file that switches off the bundled skills the env var leaves on.
    pub settings_file: Option<PathBuf>,
}

/// Settings for the tutor process: with `CLAUDE_CODE_DISABLE_BUNDLED_SKILLS=1` three built-in
/// skills remain (design, doctor, plugin-authoring); these overrides turn them off too, so `init`
/// lists only the eight subject skills (verified with claude 2.1.325).
pub fn tutor_settings() -> serde_json::Value {
    json!({"skillOverrides": {"design": "off", "doctor": "off", "plugin-authoring": "off"}})
}

/// Name of the MCP server and its tool as Claude sees them.
pub const MCP_SERVER: &str = "beluga";
pub const CAPTURE_TOOL: &str = "mcp__beluga__capture_photo";
pub const MEMORY_TOOL: &str = "mcp__beluga__memory_note";

/// `--allowedTools` rules for the subject skills only (`Skill(toan)`, ...): the built-in skills
/// of Claude Code (design, loop, ...) stay denied.
pub fn skill_rules() -> Vec<String> {
    crate::memory::SUBJECTS.iter().map(|(id, _)| format!("Skill({id})")).collect()
}

enum DriverCmd {
    Turn(TurnInput),
    Interrupt,
    Reset(String),
    Shutdown,
}

pub struct ClaudeDriver {
    tx: UnboundedSender<DriverCmd>,
}

/// Command-line flags for the long-lived process. Kept pure so tests can pin them down.
pub fn build_args(cfg: &ClaudeCfg, session_id: &str, resume: bool, launch: &Launch) -> Vec<String> {
    let model = launch.model.as_str();
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--model",
        model,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(if resume { "--resume".into() } else { "--session-id".into() });
    a.push(session_id.to_string());
    // Only the tutor/ project settings (CLAUDE.md, subject skills); never the user's plugins/MCP/skills.
    a.extend(["--setting-sources", "project"].map(String::from));
    // Exactly one MCP server: ours.
    a.extend(["--strict-mcp-config", "--mcp-config"].map(String::from));
    a.push(cfg.mcp_config.to_string_lossy().into_owned());
    // Minimal tools; nothing may ever wait for a permission answer.
    a.extend(["--tools", "Read,Skill"].map(String::from));
    a.extend(["--allowedTools", "Read"].map(String::from));
    a.extend(skill_rules());
    a.extend([CAPTURE_TOOL, MEMORY_TOOL].map(String::from));
    a.extend(["--permission-mode", "dontAsk", "--permission-prompts", "none"].map(String::from));
    if let Some(f) = &cfg.settings_file {
        a.push("--settings".into());
        a.push(f.to_string_lossy().into_owned());
    }
    // Values are the ones `claude --help` lists for `--effort`: low, medium, high, xhigh, max.
    if let Some(e) = &launch.effort {
        a.push("--effort".into());
        a.push(e.clone());
    }
    a
}

impl ClaudeDriver {
    pub fn new(cfg: ClaudeCfg, session_id: String, started: bool, sink: Sink) -> Self {
        let (tx, rx) = unbounded_channel();
        tokio::spawn(run(cfg, session_id, started, sink, rx));
        ClaudeDriver { tx }
    }

    pub fn send_turn(&self, input: TurnInput) {
        let _ = self.tx.send(DriverCmd::Turn(input));
    }

    pub fn interrupt(&self) {
        let _ = self.tx.send(DriverCmd::Interrupt);
    }

    pub fn reset(&self, new_id: String) {
        let _ = self.tx.send(DriverCmd::Reset(new_id));
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(DriverCmd::Shutdown);
    }
}

enum ProcEvent {
    Line(Value),
    Exit,
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    launch: Launch,
    gen: u64,
    /// `total_cost_usd` of the last `result` (it is cumulative over the process).
    cost_so_far: f64,
}

struct Inflight {
    input: TurnInput,
    got_text: bool,
    retries: u32,
    interrupted: bool,
    rate_limited: bool,
}

struct Driver {
    cfg: ClaudeCfg,
    session_id: String,
    started: bool,
    sink: Sink,
    proc: Option<Proc>,
    gen: u64,
    inflight: Option<Inflight>,
    queue: VecDeque<TurnInput>,
    interrupt_deadline: Option<Instant>,
    last_active: Instant,
    in_text_block: bool,
    init_logged: u64,
    stderr_tail: Arc<Mutex<String>>,
    ptx: UnboundedSender<(u64, ProcEvent)>,
}

const IDLE_KILL: Duration = Duration::from_secs(20 * 60);
const INTERRUPT_GRACE: Duration = Duration::from_secs(6);

async fn run(cfg: ClaudeCfg, session_id: String, started: bool, sink: Sink, mut rx: UnboundedReceiver<DriverCmd>) {
    let (ptx, mut prx) = unbounded_channel();
    let mut d = Driver {
        cfg,
        session_id,
        started,
        sink,
        proc: None,
        gen: 0,
        inflight: None,
        queue: VecDeque::new(),
        interrupt_deadline: None,
        last_active: Instant::now(),
        in_text_block: false,
        init_logged: 0,
        stderr_tail: Arc::new(Mutex::new(String::new())),
        ptx,
    };
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                None | Some(DriverCmd::Shutdown) => break,
                Some(DriverCmd::Turn(t)) => d.queue.push_back(t),
                Some(DriverCmd::Interrupt) => d.interrupt().await,
                Some(DriverCmd::Reset(id)) => {
                    tracing::info!("claude session reset (compaction): {} -> {id}", d.session_id);
                    d.kill_proc();
                    d.session_id = id;
                    d.started = false;
                }
            },
            ev = prx.recv() => {
                if let Some((gen, ev)) = ev {
                    d.on_proc_event(gen, ev).await;
                }
            }
            _ = ticker.tick() => d.on_tick(),
        }
        d.pump().await;
    }
    if let Some(p) = d.proc.as_mut() {
        let _ = p.child.start_kill();
    }
}

fn user_line(inp: &TurnInput) -> String {
    let mut content = Vec::new();
    for a in &inp.attachments {
        let data = base64::engine::general_purpose::STANDARD.encode(&a.data);
        let kind = if a.kind == "pdf" { "document" } else { "image" };
        content.push(json!({"type": kind, "source": {"type": "base64", "media_type": a.mime, "data": data}}));
    }
    content.push(json!({"type":"text","text":inp.text}));
    json!({"type":"user","message":{"role":"user","content":content}}).to_string()
}

impl Driver {
    fn emit(&self, turn: u64, ev: BrainEvent) {
        let _ = self.sink.send(BrainMsg { turn, ev });
    }

    fn end_turn(&mut self, err: Option<(String, String)>) {
        if let Some(inf) = self.inflight.take() {
            let turn = inf.input.turn;
            if let Some((code, text)) = err {
                self.emit(turn, BrainEvent::Error { code, text });
            }
            self.emit(turn, BrainEvent::End);
        }
        self.interrupt_deadline = None;
        self.in_text_block = false;
        self.last_active = Instant::now();
    }

    /// Start the next queued turn when idle.
    async fn pump(&mut self) {
        if self.inflight.is_some() {
            return;
        }
        let Some(input) = self.queue.pop_front() else { return };
        self.inflight = Some(Inflight { input, got_text: false, retries: 0, interrupted: false, rate_limited: false });
        self.write_current().await;
    }

    /// (Re)spawn if needed and write the in-flight turn to stdin.
    async fn write_current(&mut self) {
        let Some(launch) = self.inflight.as_ref().map(|i| i.input.launch.clone()) else { return };
        if let Err((code, text)) = self.ensure_proc(&launch).await {
            self.end_turn(Some((code, text)));
            return;
        }
        let line = user_line(&self.inflight.as_ref().unwrap().input);
        let ok = match self.proc.as_mut() {
            Some(p) => p.stdin.write_all(line.as_bytes()).await.is_ok() && p.stdin.write_all(b"\n").await.is_ok() && p.stdin.flush().await.is_ok(),
            None => false,
        };
        if !ok {
            // The process is gone; the Exit event will drive the retry logic.
            tracing::warn!("claude stdin write failed");
            if let Some(p) = self.proc.as_mut() {
                let _ = p.child.start_kill();
            }
        }
    }

    async fn ensure_proc(&mut self, launch: &Launch) -> Result<(), (String, String)> {
        if let Some(p) = &self.proc {
            if p.launch == *launch {
                return Ok(());
            }
            // Model, effort or provider changed: restart with --resume so the conversation continues.
            tracing::info!("claude launch change -> restart ({})", launch.describe());
            self.kill_proc();
        }
        let args = build_args(&self.cfg, &self.session_id, self.started, launch);
        let mut cmd = Command::new(&self.cfg.bin);
        cmd.args(&args)
            .current_dir(&self.cfg.tutor_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for k in &launch.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &launch.env {
            cmd.env(k, v);
        }
        // Only the subject skills: no bundled skills in the tutor's context.
        cmd.env("CLAUDE_CODE_DISABLE_BUNDLED_SKILLS", "1");
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| {
            tracing::error!("cannot start claude: {e}");
            (
                "claude_unavailable".to_string(),
                "Không chạy được Claude Code trên máy tính. Kiểm tra đã cài và đăng nhập chưa.".to_string(),
            )
        })?;
        self.gen += 1;
        let gen = self.gen;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let ptx = self.ptx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                match serde_json::from_str::<Value>(&l) {
                    Ok(v) => {
                        let _ = ptx.send((gen, ProcEvent::Line(v)));
                    }
                    Err(_) => tracing::debug!("claude stdout non-json line"),
                }
            }
            let _ = ptx.send((gen, ProcEvent::Exit));
        });
        let tail = self.stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                t.push_str(&l);
                t.push('\n');
                let len = t.len();
                if len > 2000 {
                    let mut cut = len - 2000;
                    while !t.is_char_boundary(cut) {
                        cut += 1;
                    }
                    *t = t[cut..].to_string();
                }
            }
        });
        tracing::info!("claude started (session {}, resume={}, {})", self.session_id, self.started, launch.describe());
        self.proc = Some(Proc { child, stdin, launch: launch.clone(), gen, cost_so_far: 0.0 });
        Ok(())
    }

    fn kill_proc(&mut self) {
        if let Some(mut p) = self.proc.take() {
            let _ = p.child.start_kill();
        }
    }

    async fn interrupt(&mut self) {
        let Some(inf) = self.inflight.as_mut() else { return };
        if inf.interrupted {
            return;
        }
        inf.interrupted = true;
        let line = json!({"type":"control_request","request_id":uuid::Uuid::new_v4().to_string(),
            "request":{"subtype":"interrupt"}})
        .to_string();
        if let Some(p) = self.proc.as_mut() {
            let _ = p.stdin.write_all(line.as_bytes()).await;
            let _ = p.stdin.write_all(b"\n").await;
            let _ = p.stdin.flush().await;
        }
        // If the CLI does not acknowledge in time, kill it; the next turn resumes the session.
        self.interrupt_deadline = Some(Instant::now() + INTERRUPT_GRACE);
    }

    fn on_tick(&mut self) {
        if let Some(dl) = self.interrupt_deadline {
            if Instant::now() > dl {
                tracing::warn!("claude did not stop after interrupt; killing process");
                self.interrupt_deadline = None;
                self.kill_proc_keep_events();
            }
        }
        if self.inflight.is_none() && self.proc.is_some() && self.last_active.elapsed() > IDLE_KILL {
            tracing::info!("claude idle; stopping process (will --resume on next turn)");
            self.kill_proc();
        }
    }

    /// Kill but keep `self.proc` so the Exit event is still attributed to this process.
    fn kill_proc_keep_events(&mut self) {
        if let Some(p) = self.proc.as_mut() {
            let _ = p.child.start_kill();
        }
    }

    async fn on_proc_event(&mut self, gen: u64, ev: ProcEvent) {
        match ev {
            ProcEvent::Line(v) => {
                if self.proc.as_ref().map(|p| p.gen) == Some(gen) {
                    self.on_line(v);
                }
            }
            ProcEvent::Exit => {
                if self.proc.as_ref().map(|p| p.gen) != Some(gen) {
                    return; // an old process we already replaced
                }
                self.proc = None;
                self.in_text_block = false;
                let tail = self.stderr_tail.lock().unwrap().clone();
                if !tail.is_empty() {
                    tracing::warn!("claude exited; stderr tail: {}", tail.trim());
                }
                let Some(inf) = self.inflight.as_ref() else { return };
                let (turn, interrupted, got_text, retries) = (inf.input.turn, inf.interrupted, inf.got_text, inf.retries);
                // A transcript already exists for this id (the server was killed mid-lesson before
                // `claude_started` was persisted). `--session-id` cannot recreate it: continue with
                // `--resume` and remember it so later restarts go straight to resume.
                if session_in_use(&tail) && !self.started {
                    self.started = true;
                    tracing::warn!("claude session already exists on disk; continuing with --resume");
                    self.emit(turn, BrainEvent::SessionStarted);
                }
                if interrupted {
                    self.end_turn(None);
                } else if !got_text && retries < 1 {
                    if let Some(inf) = self.inflight.as_mut() {
                        inf.retries += 1;
                    }
                    tracing::warn!("claude died before answering; restarting (resume={}) and retrying once", self.started);
                    self.write_current().await;
                } else {
                    let (code, text) = classify_failure(&tail, "Claude Code dừng giữa chừng. Cậu thử nói lại nhé.");
                    self.end_turn(Some((code, text)));
                }
            }
        }
    }

    fn on_line(&mut self, v: Value) {
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "stream_event" => {
                if !v.get("parent_tool_use_id").map(|p| p.is_null()).unwrap_or(true) {
                    return;
                }
                let e = &v["event"];
                match e.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "content_block_start" => {
                        self.in_text_block = e["content_block"]["type"] == "text";
                    }
                    "content_block_delta" if e["delta"]["type"] == "text_delta" => {
                        if let (Some(txt), Some(inf)) = (e["delta"]["text"].as_str(), self.inflight.as_mut()) {
                            if !txt.is_empty() {
                                tracing::debug!("claude text_delta: {} chars", txt.chars().count());
                                inf.got_text = true;
                                let turn = inf.input.turn;
                                self.emit(turn, BrainEvent::Text(txt.to_string()));
                            }
                        }
                    }
                    "content_block_stop" => {
                        if self.in_text_block {
                            self.in_text_block = false;
                            if let Some(inf) = &self.inflight {
                                // Blocks are separated by a line break so lines never merge across tool calls.
                                self.emit(inf.input.turn, BrainEvent::Text("\n".into()));
                            }
                        }
                    }
                    _ => {}
                }
            }
            "system" if v["subtype"] == "init" => {
                let names = |k: &str| -> String {
                    v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default()
                };
                // The CLI repeats `init` for every turn; log it once per process.
                if self.init_logged != self.gen {
                    self.init_logged = self.gen;
                    tracing::info!("claude init: model={} tools=[{}] skills=[{}]", v["model"].as_str().unwrap_or("?"), names("tools"), names("skills"));
                }
                if let Some(arr) = v["mcp_servers"].as_array() {
                    for s in arr {
                        if s["status"] != "connected" {
                            tracing::warn!("claude MCP server {} status {}", s["name"], s["status"]);
                        }
                    }
                }
            }
            "assistant" => {
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                        let input: String = b["input"].to_string().chars().take(200).collect();
                        tracing::info!("claude tool_use: {} {}", b["name"].as_str().unwrap_or("?"), input);
                    }
                }
            }
            "user" => {
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for b in blocks.iter().filter(|b| b["type"] == "tool_result" && b["is_error"] == true) {
                        let t: String = b["content"].to_string().chars().take(200).collect();
                        tracing::warn!("claude tool_result error: {t}");
                    }
                }
            }
            "rate_limit_event" => {
                if let Some(inf) = &self.inflight {
                    self.emit(inf.input.turn, BrainEvent::RateLimit(v["rate_limit_info"].clone()));
                }
                let st = v["rate_limit_info"]["status"].as_str().unwrap_or("allowed");
                if st != "allowed" && st != "allowed_warning" {
                    if let Some(inf) = self.inflight.as_mut() {
                        inf.rate_limited = true;
                    }
                }
            }
            "result" => {
                let Some(inf) = self.inflight.as_ref() else { return };
                let turn = inf.input.turn;
                let aborted = v["terminal_reason"] == "aborted_streaming" || inf.interrupted;
                let is_error = v["is_error"].as_bool().unwrap_or(false);
                let rate_limited = inf.rate_limited;
                if !self.started && !is_error {
                    self.started = true;
                    self.emit(turn, BrainEvent::SessionStarted);
                }
                if let Some(u) = self.turn_usage(&v) {
                    self.emit(turn, BrainEvent::Usage(u));
                }
                if is_error && !aborted {
                    let detail = v["result"].as_str().unwrap_or("");
                    let (code, text) = if rate_limited {
                        ("rate_limited".to_string(), RATE_LIMIT_TEXT.to_string())
                    } else {
                        classify_failure(detail, "Claude Code báo lỗi. Cậu thử lại sau nhé.")
                    };
                    self.end_turn(Some((code, text)));
                } else {
                    self.end_turn(None);
                }
            }
            _ => {}
        }
    }
}

impl Driver {
    /// Usage of the turn that just ended. `usage` in `result` is per turn; `total_cost_usd` is
    /// cumulative over the process (measured with claude 2.1.325), so the turn's cost is the delta.
    fn turn_usage(&mut self, v: &Value) -> Option<TurnUsage> {
        let u = v.get("usage")?;
        let n = |x: &Value, k: &str| x[k].as_u64().unwrap_or(0);
        let total = v["total_cost_usd"].as_f64().unwrap_or(0.0);
        let cli_cost_usd = match self.proc.as_mut() {
            Some(p) => {
                let d = (total - p.cost_so_far).max(0.0);
                p.cost_so_far = total;
                d
            }
            None => total,
        };
        let last = u["iterations"].as_array().and_then(|a| a.last()).unwrap_or(u);
        Some(TurnUsage {
            input_tokens: n(u, "input_tokens"),
            output_tokens: n(u, "output_tokens"),
            cache_read_tokens: n(u, "cache_read_input_tokens"),
            cache_write_tokens: n(u, "cache_creation_input_tokens"),
            cli_cost_usd,
            context_tokens: n(last, "input_tokens") + n(last, "cache_read_input_tokens") + n(last, "cache_creation_input_tokens"),
        })
    }
}

const RATE_LIMIT_TEXT: &str = "Claude hết hạn mức tạm thời rồi. Đợi một lúc rồi thử lại nhé.";

/// The CLI rejects `--session-id` when that id already has a transcript
/// (`Error: Session ID … is already in use.`); the way forward is `--resume`.
pub fn session_in_use(stderr: &str) -> bool {
    stderr.to_lowercase().contains("already in use")
}

/// Map CLI error text to a protocol error code and a student-friendly message.
pub fn classify_failure(detail: &str, default_text: &str) -> (String, String) {
    let d = detail.to_lowercase();
    if ["rate limit", "usage limit", "limit reached", "hit your limit", "quota", "429", "overloaded", "too many requests"]
        .iter()
        .any(|k| d.contains(k))
    {
        return ("rate_limited".into(), RATE_LIMIT_TEXT.into());
    }
    if ["not logged in", "/login", "log in", "login", "authenticat", "unauthorized", "invalid api key", "401"]
        .iter()
        .any(|k| d.contains(k))
    {
        return (
            "claude_unavailable".into(),
            "Claude Code trên máy tính chưa đăng nhập. Mở terminal chạy claude để đăng nhập.".into(),
        );
    }
    ("claude_error".into(), default_text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ClaudeCfg {
        ClaudeCfg { bin: "claude".into(), tutor_dir: PathBuf::from("tutor"), mcp_config: PathBuf::from("mcp.json"), settings_file: Some(PathBuf::from("s.json")) }
    }

    fn l(model: &str, effort: Option<&str>) -> Launch {
        Launch { model: model.into(), effort: effort.map(String::from), ..Default::default() }
    }

    #[test]
    fn args_carry_model_effort_and_settings() {
        let a = build_args(&cfg(), "sid", false, &l("ds/deepseek-flash", None));
        assert_eq!(a[a.iter().position(|x| x == "--model").unwrap() + 1], "ds/deepseek-flash");
        assert!(!a.contains(&"--effort".to_string()));
        let a = build_args(&cfg(), "sid", false, &l("sonnet", Some("high")));
        assert_eq!(a[a.iter().position(|x| x == "--effort").unwrap() + 1], "high");
        assert_eq!(a[a.iter().position(|x| x == "--settings").unwrap() + 1], "s.json");
        assert_eq!(tutor_settings()["skillOverrides"]["design"], "off");
    }

    #[test]
    fn args_new_vs_resume() {
        let a = build_args(&cfg(), "sid", false, &l("sonnet", None));
        let pos = a.iter().position(|x| x == "--session-id").unwrap();
        assert_eq!(a[pos + 1], "sid");
        assert!(!a.contains(&"--resume".to_string()));
        let r = build_args(&cfg(), "sid", true, &l("haiku", None));
        let pos = r.iter().position(|x| x == "--resume").unwrap();
        assert_eq!(r[pos + 1], "sid");
        assert!(!r.contains(&"--session-id".to_string()));
        assert_eq!(r[r.iter().position(|x| x == "--model").unwrap() + 1], "haiku");
    }

    #[test]
    fn args_are_locked_down() {
        let a = build_args(&cfg(), "sid", false, &l("sonnet", Some("medium")));
        for flag in ["--strict-mcp-config", "--include-partial-messages", "--verbose"] {
            assert!(a.contains(&flag.to_string()), "{flag}");
        }
        let get = |k: &str| a[a.iter().position(|x| x == k).unwrap() + 1].clone();
        assert_eq!(get("--setting-sources"), "project");
        assert_eq!(get("--tools"), "Read,Skill");
        assert_eq!(get("--permission-mode"), "dontAsk");
        assert_eq!(get("--permission-prompts"), "none");
        assert!(a.contains(&CAPTURE_TOOL.to_string()));
        assert!(a.contains(&MEMORY_TOOL.to_string()));
        // only the subject skills are allowed, never a bare `Skill`
        let from = a.iter().position(|x| x == "--allowedTools").unwrap() + 1;
        let to = a.iter().skip(from).position(|x| x.starts_with("--")).map(|p| from + p).unwrap_or(a.len());
        let allowed = &a[from..to];
        assert!(!allowed.iter().any(|x| x == "Skill"), "{allowed:?}");
        for id in ["toan", "ly", "hoa", "van", "anh-ssat", "duc", "tin-cpp", "chung"] {
            assert!(allowed.contains(&format!("Skill({id})")), "{id}: {allowed:?}");
        }
        assert_eq!(allowed.len(), 1 + 8 + 2);
        for banned in ["Bash", "Write", "Edit", "WebFetch", "--dangerously-skip-permissions"] {
            assert!(!a.iter().any(|x| x == banned), "{banned}");
        }
    }

    #[test]
    fn session_in_use_is_detected() {
        assert!(session_in_use("Error: Session ID 7ff8 is already in use.\n"));
        assert!(!session_in_use("Not logged in · Please run /login"));
        assert!(!session_in_use(""));
    }

    #[test]
    fn failure_classification() {
        assert_eq!(classify_failure("You've hit your limit", "x").0, "rate_limited");
        assert_eq!(classify_failure("Not logged in · Please run /login", "x").0, "claude_unavailable");
        assert_eq!(classify_failure("something odd", "x").0, "claude_error");
    }

    #[test]
    fn user_line_has_image_first() {
        let att = |kind: &str, mime: &str| super::super::Attachment { kind: kind.into(), mime: mime.into(), data: vec![1, 2, 3] };
        let inp = TurnInput {
            turn: 1,
            user_text: "a".into(),
            text: "[lang=vi] a".into(),
            attachments: vec![att("image", "image/png"), att("pdf", "application/pdf")],
            model: "sonnet".into(),
            mode: "chat".into(),
            launch: l("sonnet", None),
        };
        let v: Value = serde_json::from_str(&user_line(&inp)).unwrap();
        let c = &v["message"]["content"];
        assert_eq!((c[0]["type"].as_str(), c[0]["source"]["media_type"].as_str()), (Some("image"), Some("image/png")));
        assert_eq!((c[1]["type"].as_str(), c[1]["source"]["media_type"].as_str()), (Some("document"), Some("application/pdf")));
        assert_eq!(c[2]["text"], "[lang=vi] a");
    }
}
