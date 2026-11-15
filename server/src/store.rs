//! On-disk state under `server/data/`: pairing, session index and per-session event journals.

use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub session_id: String,
    pub title: String,
    /// RFC 3339 timestamp.
    pub updated_at: String,
    /// True once the Claude session file exists (so the next start must use `--resume`).
    #[serde(default)]
    pub claude_started: bool,
    /// The `[memory]` summary already went into this session's Claude context (never sent twice,
    /// also not after a `--resume`).
    #[serde(default)]
    pub memory_sent: bool,
    /// Where the `[memory]` (and `[folder: …]`) Claude last got came from: a folder id, or `""` for
    /// "no folder". A session moved to another folder gets the new folder's context on its next turn.
    /// `None` with `memory_sent` = written before v3 (treated as the session's folder at that time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<String>,
    /// Folder (PROTOCOL section 8); `None` = no folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// `chat` or `live`. Sessions from before v2 were live.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// The `[mode: ...]` note Claude has last seen in this session's context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_noted: Option<String>,
    /// The title came from `@title`, the utility model or the user (not the placeholder).
    #[serde(default)]
    pub titled: bool,
    /// The user renamed the session: `@title` no longer changes it.
    #[serde(default)]
    pub user_titled: bool,
    /// Last thing the student wrote (for the session list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Claude Code session id when it differs from `session_id` (after a compaction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_id: Option<String>,
    /// Already looked at by the one-time "put old sessions into subject folders" pass.
    #[serde(default)]
    pub foldered: bool,
}

fn default_mode() -> String {
    "live".into()
}

impl SessionMeta {
    pub fn new(id: &str) -> SessionMeta {
        SessionMeta {
            session_id: id.to_string(),
            title: format!("Phiên mới · {}", date_suffix()),
            updated_at: now_rfc3339(),
            claude_started: false,
            memory_sent: false,
            memory_scope: None,
            folder_id: None,
            mode: "live".into(),
            mode_noted: None,
            titled: false,
            user_titled: false,
            preview: None,
            claude_id: None,
            foldered: true,
        }
    }

    /// The id the Claude Code transcript lives under.
    pub fn claude_session(&self) -> String {
        self.claude_id.clone().unwrap_or_else(|| self.session_id.clone())
    }

    /// Move the session to `folder` (or out of any folder). Pins down where the context Claude
    /// already has came from, so the next turn knows it must attach the new folder's context.
    pub fn set_folder(&mut self, folder: Option<String>) {
        if self.memory_sent && self.memory_scope.is_none() {
            self.memory_scope = Some(self.folder_id.clone().unwrap_or_default());
        }
        self.folder_id = folder;
    }

    /// The folder (or `""`) whose context Claude already has, if any.
    pub fn context_scope(&self) -> Option<String> {
        match (&self.memory_scope, self.memory_sent) {
            (Some(s), true) => Some(s.clone()),
            (None, true) => Some(self.folder_id.clone().unwrap_or_default()),
            _ => None,
        }
    }

    pub fn has_title(&self) -> bool {
        self.titled || !self.title.starts_with("Phiên mới")
    }
}

#[derive(Serialize, Deserialize, Default)]
struct PairFile {
    code: String,
    tokens: Vec<String>,
}

pub struct Store {
    dir: PathBuf,
    lock: Mutex<()>,
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// `dd/mm` in local time, appended to session titles.
pub fn date_suffix() -> String {
    chrono::Local::now().format("%d/%m").to_string()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn is_valid_session_id(id: &str) -> bool {
    valid_id(id)
}

impl Store {
    pub fn open(dir: &Path, fixed_code: Option<&str>) -> std::io::Result<Store> {
        std::fs::create_dir_all(dir.join("sessions"))?;
        let s = Store { dir: dir.to_path_buf(), lock: Mutex::new(()) };
        let mut pf = s.read_pair();
        if let Some(c) = fixed_code {
            pf.code = c.to_string();
            s.write_pair(&pf);
        } else if pf.code.len() != 6 {
            pf.code = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
            s.write_pair(&pf);
        }
        Ok(s)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn read_pair(&self) -> PairFile {
        std::fs::read_to_string(self.dir.join("pair.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write_pair(&self, p: &PairFile) {
        let _ = std::fs::write(self.dir.join("pair.json"), serde_json::to_string_pretty(p).unwrap());
    }

    pub fn pair_code(&self) -> String {
        let _g = self.lock.lock().unwrap();
        self.read_pair().code
    }

    pub fn check_token(&self, token: &str) -> bool {
        let _g = self.lock.lock().unwrap();
        !token.is_empty() && self.read_pair().tokens.iter().any(|t| t == token)
    }

    /// Issue and persist a fresh random pairing token.
    pub fn add_token(&self) -> String {
        let _g = self.lock.lock().unwrap();
        let token = format!("{:032x}", rand::rng().random::<u128>());
        let mut pf = self.read_pair();
        pf.tokens.push(token.clone());
        self.write_pair(&pf);
        token
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    fn read_index(&self) -> Vec<SessionMeta> {
        std::fs::read_to_string(self.index_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Newest first.
    pub fn list_sessions(&self) -> Vec<SessionMeta> {
        let _g = self.lock.lock().unwrap();
        let mut v = self.read_index();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        v
    }

    pub fn get_session(&self, id: &str) -> Option<SessionMeta> {
        let _g = self.lock.lock().unwrap();
        self.read_index().into_iter().find(|m| m.session_id == id)
    }

    pub fn upsert_session(&self, meta: &SessionMeta) {
        if !valid_id(&meta.session_id) {
            return;
        }
        let _g = self.lock.lock().unwrap();
        let mut v = self.read_index();
        match v.iter_mut().find(|m| m.session_id == meta.session_id) {
            Some(m) => *m = meta.clone(),
            None => v.push(meta.clone()),
        }
        let _ = std::fs::write(self.index_path(), serde_json::to_string_pretty(&v).unwrap());
    }

    /// Read-modify-write the index under the lock.
    pub fn update_sessions(&self, f: impl FnOnce(&mut Vec<SessionMeta>)) {
        let _g = self.lock.lock().unwrap();
        let mut v = self.read_index();
        f(&mut v);
        let _ = std::fs::write(self.index_path(), serde_json::to_string_pretty(&v).unwrap());
    }

    /// Remove a session from the index and delete its journal and MCP config.
    pub fn delete_session(&self, id: &str) -> bool {
        if !valid_id(id) {
            return false;
        }
        let mut found = false;
        self.update_sessions(|v| {
            let before = v.len();
            v.retain(|m| m.session_id != id);
            found = v.len() != before;
        });
        let _ = std::fs::remove_file(self.journal_path(id));
        let _ = std::fs::remove_file(self.dir.join("mcp").join(format!("{id}.json")));
        found
    }

    fn journal_path(&self, id: &str) -> PathBuf {
        self.dir.join("sessions").join(format!("{id}.jsonl"))
    }

    pub fn append_event(&self, id: &str, ev: &Value) {
        if !valid_id(id) {
            return;
        }
        let _g = self.lock.lock().unwrap();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.journal_path(id)) {
            let _ = writeln!(f, "{}", ev);
        }
    }

    pub fn load_events(&self, id: &str) -> Vec<Value> {
        if !valid_id(id) {
            return vec![];
        }
        let _g = self.lock.lock().unwrap();
        std::fs::read_to_string(self.journal_path(id))
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("beluga-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pairing_code_is_stable_and_tokens_persist() {
        let d = tmp();
        let s = Store::open(&d, None).unwrap();
        let code = s.pair_code();
        assert_eq!(code.len(), 6);
        let t = s.add_token();
        assert!(s.check_token(&t));
        assert!(!s.check_token("nope"));
        let s2 = Store::open(&d, None).unwrap();
        assert_eq!(s2.pair_code(), code);
        assert!(s2.check_token(&t));
    }

    #[test]
    fn sessions_and_journal_roundtrip() {
        let d = tmp();
        let s = Store::open(&d, Some("123456")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        s.upsert_session(&SessionMeta { title: "Toán".into(), updated_at: "2026-11-14T10:00:00+07:00".into(), ..SessionMeta::new(&id) });
        s.append_event(&id, &serde_json::json!({"t":"say","seq":1}));
        s.append_event(&id, &serde_json::json!({"t":"say","seq":2}));
        assert_eq!(s.load_events(&id).len(), 2);
        assert_eq!(s.list_sessions().len(), 1);
        assert_eq!(s.get_session(&id).unwrap().title, "Toán");
        assert!(s.get_session("../x").is_none());
        // an index written before v2 still loads: live mode, no folder, not yet foldered
        std::fs::write(d.join("sessions.json"), format!(r#"[{{"sessionId":"{id}","title":"Toán · A · 01/11","updatedAt":"x"}}]"#)).unwrap();
        let m = s.get_session(&id).unwrap();
        assert_eq!((m.mode.as_str(), m.folder_id.is_none(), m.foldered, m.has_title()), ("live", true, false, true));
        assert!(s.delete_session(&id));
        assert!(s.load_events(&id).is_empty() && s.list_sessions().is_empty());
    }
}
