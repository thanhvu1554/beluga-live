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
        s.upsert_session(&SessionMeta {
            session_id: id.clone(),
            title: "Toán".into(),
            updated_at: "2026-11-14T10:00:00+07:00".into(),
            claude_started: false,
            memory_sent: false,
        });
        s.append_event(&id, &serde_json::json!({"t":"say","seq":1}));
        s.append_event(&id, &serde_json::json!({"t":"say","seq":2}));
        assert_eq!(s.load_events(&id).len(), 2);
        assert_eq!(s.list_sessions().len(), 1);
        assert_eq!(s.get_session(&id).unwrap().title, "Toán");
        assert!(s.get_session("../x").is_none());
    }
}
