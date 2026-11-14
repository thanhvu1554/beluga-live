//! A "brain" produces the raw lesson text for each turn: either the mock script or Claude Code.

pub mod claude;
pub mod mock;

use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct Settings {
    pub lang: String,
    pub model: String,
    pub rigor: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { lang: "vi".into(), model: "sonnet".into(), rigor: "medium".into() }
    }
}

impl Settings {
    /// Apply a `settings` / `hello.settings` object, ignoring invalid values.
    pub fn apply(&mut self, v: &serde_json::Value) {
        let pick = |k: &str, allowed: &[&str]| -> Option<String> {
            v.get(k).and_then(|x| x.as_str()).filter(|s| allowed.contains(s)).map(|s| s.to_string())
        };
        if let Some(x) = pick("lang", &["vi", "en"]) {
            self.lang = x;
        }
        if let Some(x) = pick("model", &["sonnet", "opus", "haiku"]) {
            self.model = x;
        }
        if let Some(x) = pick("rigor", &["light", "medium", "hard"]) {
            self.rigor = x;
        }
    }
}

#[derive(Debug, Clone)]
pub struct TurnInput {
    pub turn: u64,
    /// What the student said (used by the mock to choose a script).
    pub user_text: String,
    /// Full message for Claude: settings prefix, interruption note, then `user_text`.
    pub text: String,
    /// Latest hand-taken photo (JPEG) attached to this turn.
    pub image: Option<Vec<u8>>,
    pub model: String,
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

    pub fn shutdown(&self) {
        if let Brain::Claude(c) = self {
            c.shutdown();
        }
    }
}
