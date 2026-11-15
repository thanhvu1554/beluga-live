//! A "brain" produces the raw lesson text for each turn: either the mock script or Claude Code.

pub mod claude;
pub mod mock;

use crate::models::Launch;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct Settings {
    pub lang: String,
    /// Catalogue id (`sonnet`, `deepseek`, ...).
    pub model: String,
    pub rigor: String,
    /// Wanted effort; `None` = the model's default.
    pub effort: Option<String>,
    /// Correct speech-to-text results with the utility model (`settings.sttFix`, default on).
    pub stt_fix: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { lang: "vi".into(), model: "sonnet".into(), rigor: "medium".into(), effort: None, stt_fix: true }
    }
}

impl Settings {
    /// Apply a `settings` / `hello.settings` object, ignoring invalid values. `known_model` says
    /// whether a model id is in the catalogue. (`mode` is handled by the session.)
    pub fn apply(&mut self, v: &serde_json::Value, known_model: impl Fn(&str) -> bool) {
        let pick = |k: &str, allowed: &[&str]| -> Option<String> {
            v.get(k).and_then(|x| x.as_str()).filter(|s| allowed.contains(s)).map(|s| s.to_string())
        };
        if let Some(x) = pick("lang", &["vi", "en"]) {
            self.lang = x;
        }
        if let Some(m) = v.get("model").and_then(|x| x.as_str()).filter(|m| known_model(m)) {
            self.model = m.to_string();
        }
        if let Some(x) = pick("rigor", &["light", "medium", "hard"]) {
            self.rigor = x;
        }
        match v.get("effort") {
            Some(serde_json::Value::Null) => self.effort = None,
            Some(e) => {
                if let Some(e) = e.as_str().filter(|e| crate::models::EFFORTS.contains(e)) {
                    self.effort = Some(e.to_string());
                }
            }
            None => {}
        }
        if let Some(b) = v.get("sttFix").and_then(|x| x.as_bool()) {
            self.stt_fix = b;
        }
    }
}

/// A file sent along with a user turn (image or PDF).
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// `image` or `pdf`.
    pub kind: String,
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TurnInput {
    pub turn: u64,
    /// What the student said (used by the mock to choose a script).
    pub user_text: String,
    /// Full message for Claude: settings prefix, interruption note, then `user_text`.
    pub text: String,
    /// Photos and files for this turn (hand-taken photo, uploads).
    pub attachments: Vec<Attachment>,
    /// Catalogue id (for the mock and logs).
    pub model: String,
    /// `chat` or `live`.
    pub mode: String,
    /// How the `claude` process must run for this turn.
    pub launch: Launch,
}

/// Token counts of one finished turn (from the stream-json `result`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// This turn's share of the CLI's cost estimate (`total_cost_usd` is cumulative per process).
    pub cli_cost_usd: f64,
    /// Input size of the last model call of the turn (input + cache read + cache write).
    pub context_tokens: u64,
}

#[derive(Debug, Clone)]
pub enum BrainEvent {
    /// A fragment of lesson text (any size).
    Text(String),
    /// The turn is over (normally or aborted).
    End,
    /// The Claude session file now exists; later starts must use `--resume`.
    SessionStarted,
    Error { code: String, text: String },
    /// Sent just before `End` when the CLI reported usage.
    Usage(TurnUsage),
    /// `rate_limit_event.rate_limit_info` of the Claude plan.
    RateLimit(serde_json::Value),
}

#[derive(Debug, Clone)]
pub struct BrainMsg {
    pub turn: u64,
    pub ev: BrainEvent,
}

pub type Sink = UnboundedSender<BrainMsg>;

pub enum Brain {
    Mock(mock::MockBrain),
    Claude(claude::ClaudeDriver),
}

impl Brain {
    pub fn send_turn(&self, input: TurnInput) {
        match self {
            Brain::Mock(m) => m.send_turn(input),
            Brain::Claude(c) => c.send_turn(input),
        }
    }

    /// Stop the running turn; the brain still sends `End` for it.
    pub fn interrupt(&self) {
        match self {
            Brain::Mock(m) => m.interrupt(),
            Brain::Claude(c) => c.interrupt(),
        }
    }

    /// Start over in a fresh Claude session (`new_id`) after a compaction.
    pub fn reset(&self, new_id: String) {
        if let Brain::Claude(c) = self {
            c.reset(new_id);
        }
    }

    pub fn shutdown(&self) {
        if let Brain::Claude(c) = self {
            c.shutdown();
        }
    }
}
