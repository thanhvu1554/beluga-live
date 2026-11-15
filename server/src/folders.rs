//! Folders (PROTOCOL section 8): user-named, one subject each, stored in `server/data/folders.json`.
//! Per-folder data lives in `server/data/folders/<id>/` (`knowledge.json` is written by the exam
//! ingest module and only read here; `weekly.json` is the weekly summary).

use crate::memory::{fold, normalize_subject, subject_name, SUBJECTS};
use crate::store::{is_valid_session_id, now_rfc3339, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderMeta {
    pub id: String,
    pub name: String,
    pub subject: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct Folders {
    data_dir: PathBuf,
    lock: Mutex<()>,
}

/// Subject id from a free string: an exact id, or a name/alias that clearly names a subject.
pub fn subject_id(s: &str) -> String {
    let t = s.trim();
    if SUBJECTS.iter().any(|(id, _)| *id == t) {
        return t.to_string();
    }
    normalize_subject(t)
}

/// Subject named by the first part of a session title ("Toán · Bất phương trình · 14/11"), if any.
pub fn subject_from_title(title: &str) -> Option<String> {
    let first = title.split('·').next()?.trim();
    if first.is_empty() || fold(first).starts_with("phien moi") {
        return None;
    }
    if let Some((id, _)) = SUBJECTS.iter().find(|(_, n)| fold(n) == fold(first)) {
        return Some(id.to_string());
    }
    let id = normalize_subject(first);
    (id != "chung").then_some(id)
}

impl Folders {
    pub fn open(data_dir: &Path) -> Folders {
        let _ = std::fs::create_dir_all(data_dir);
        Folders { data_dir: data_dir.to_path_buf(), lock: Mutex::new(()) }
    }

    fn path(&self) -> PathBuf {
        self.data_dir.join("folders.json")
    }

    fn read(&self) -> Vec<FolderMeta> {
        std::fs::read_to_string(self.path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn write(&self, v: &[FolderMeta]) {
        let _ = std::fs::write(self.path(), serde_json::to_string_pretty(v).unwrap());
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("folders").join(id)
    }

    pub fn list(&self) -> Vec<FolderMeta> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.read();
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        v
    }

    pub fn get(&self, id: &str) -> Option<FolderMeta> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read().into_iter().find(|f| f.id == id)
    }

    pub fn create(&self, name: &str, subject: &str) -> Result<FolderMeta, String> {
        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
        if name.is_empty() {
            return Err("Tên folder trống.".into());
        }
        let now = now_rfc3339();
        let f = FolderMeta { id: uuid::Uuid::new_v4().to_string(), name, subject: subject_id(subject), created_at: now.clone(), updated_at: now };
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.read();
        v.push(f.clone());
        self.write(&v);
        Ok(f)
    }

    fn modify(&self, id: &str, f: impl FnOnce(&mut FolderMeta)) -> bool {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.read();
        let Some(x) = v.iter_mut().find(|x| x.id == id) else { return false };
        f(x);
        self.write(&v);
        true
    }

    pub fn rename(&self, id: &str, name: &str) -> bool {
        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
        !name.is_empty()
            && self.modify(id, |x| {
                x.name = name;
                x.updated_at = now_rfc3339();
            })
    }

    pub fn touch(&self, id: &str) {
        self.modify(id, |x| x.updated_at = now_rfc3339());
    }

    /// Remove the folder and its knowledge, exams and weekly summary.
    pub fn delete(&self, id: &str) -> bool {
        if !is_valid_session_id(id) {
            return false;
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.read();
        let before = v.len();
        v.retain(|x| x.id != id);
        if v.len() == before {
            return false;
        }
        self.write(&v);
        let _ = std::fs::remove_dir_all(self.dir(id));
        true
    }

    pub fn has_knowledge(&self, id: &str) -> bool {
        self.dir(id).join("knowledge.json").exists()
    }

    /// `knowledge.json` of the folder (PROTOCOL section 11), read-only.
    pub fn knowledge(&self, id: &str) -> Option<Value> {
        if !is_valid_session_id(id) {
            return None;
        }
        serde_json::from_str(&std::fs::read_to_string(self.dir(id).join("knowledge.json")).ok()?).ok()
    }

    pub fn weekly(&self, id: &str) -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(self.dir(id).join("weekly.json")).ok()?).ok()
    }

    pub fn save_weekly(&self, id: &str, v: &Value) {
        let d = self.dir(id);
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("weekly.json"), serde_json::to_string_pretty(v).unwrap());
    }

    /// The `folders` frame.
    pub fn frame(&self, store: &Store, ingest: impl Fn(&str) -> Option<Value>) -> Value {
        let sessions = store.list_sessions();
        let items: Vec<Value> = self
            .list()
            .into_iter()
            .map(|f| {
                let count = sessions.iter().filter(|s| s.folder_id.as_deref() == Some(&f.id)).count();
                let mut v = json!({"id": f.id, "name": f.name, "subject": f.subject, "sessionCount": count,
                       "updatedAt": f.updated_at, "hasKnowledge": self.has_knowledge(&f.id)});
                if let Some(i) = ingest(&f.id) {
                    v["ingest"] = json!({"jobId": i["jobId"], "state": i["state"], "done": i["done"], "total": i["total"]});
                }
                v
            })
            .collect();
        json!({"t": "folders", "items": items})
    }

    /// One-time pass: sessions from before folders go into a default folder per subject when the
    /// subject can be read from the title. Each session is looked at once (`foldered`).
    pub fn sort_old_sessions(&self, store: &Store) {
        let todo: Vec<_> = store.list_sessions().into_iter().filter(|m| !m.foldered).collect();
        if todo.is_empty() {
            return;
        }
        let mut assign: Vec<(String, Option<String>)> = Vec::new();
        for m in &todo {
            let folder = match subject_from_title(&m.title) {
                Some(sub) => {
                    let name = subject_name(&sub);
                    let existing = self.list().into_iter().find(|f| f.subject == sub && f.name == name);
                    match existing {
                        Some(f) => Some(f.id),
                        None => self.create(&name, &sub).ok().map(|f| f.id),
                    }
                }
                None => None,
            };
            assign.push((m.session_id.clone(), folder));
        }
        store.update_sessions(|v| {
            for m in v.iter_mut() {
                if let Some((_, f)) = assign.iter().find(|(id, _)| *id == m.session_id) {
                    if m.folder_id.is_none() {
                        m.folder_id = f.clone();
                    }
                    m.foldered = true;
                }
            }
        });
        tracing::info!("sorted {} older sessions into subject folders", assign.iter().filter(|(_, f)| f.is_some()).count());
    }
}

/// Topic titles of the folder's curriculum (for STT correction hints).
pub fn curriculum_terms(k: &Value, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for ch in k["curriculum"].as_array().into_iter().flatten() {
        for t in ch["topics"].as_array().into_iter().flatten() {
            if let Some(s) = t["title"].as_str() {
                out.push(s.to_string());
            }
        }
        if let Some(s) = ch["title"].as_str() {
            out.push(s.to_string());
        }
    }
    out.dedup();
    out.truncate(max);
    out
}

/// (topicId, title, chapter title) of every curriculum topic.
pub fn curriculum_topics(k: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for ch in k["curriculum"].as_array().into_iter().flatten() {
        let chapter = ch["title"].as_str().unwrap_or("").to_string();
        for t in ch["topics"].as_array().into_iter().flatten() {
            if let (Some(id), Some(title)) = (t["id"].as_str(), t["title"].as_str()) {
                out.push((id.to_string(), title.to_string(), chapter.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionMeta;

    #[test]
    fn subjects_from_titles() {
        assert_eq!(subject_from_title("Toán · Bất phương trình bậc hai · 14/11").as_deref(), Some("toan"));
        assert_eq!(subject_from_title("Vật lý · Rơi tự do · 01/11").as_deref(), Some("ly"));
        assert_eq!(subject_from_title("Tin · C++ · 01/11").as_deref(), Some("tin-cpp"));
        assert_eq!(subject_from_title("Anh · SSAT · Reading").as_deref(), Some("anh-ssat"));
        assert_eq!(subject_from_title("Phiên mới · 14/11"), None);
        assert_eq!(subject_from_title("Sinh · ADN"), None);
        assert_eq!(subject_id("Toán"), "toan");
        assert_eq!(subject_id("anh-ssat"), "anh-ssat");
    }

    #[test]
    fn crud_and_old_session_sorting() {
        let d = std::env::temp_dir().join(format!("beluga-fold-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&d, Some("111111")).unwrap();
        let f = Folders::open(&d);
        for (id, title) in [("s1", "Toán · A · 01/11"), ("s2", "Toán · B · 02/11"), ("s3", "Phiên mới · 03/11"), ("s4", "Hóa · C · 03/11")] {
            store.upsert_session(&SessionMeta { title: title.into(), foldered: false, ..SessionMeta::new(id) });
        }
        f.sort_old_sessions(&store);
        let toan = f.list().into_iter().find(|x| x.subject == "toan").unwrap();
        assert_eq!(toan.name, "Toán");
        assert_eq!(f.list().len(), 2);
        let get = |id: &str| store.get_session(id).unwrap();
        assert_eq!(get("s1").folder_id.as_deref(), Some(toan.id.as_str()));
        assert_eq!(get("s2").folder_id, get("s1").folder_id);
        assert_eq!(get("s3").folder_id, None);
        assert!(get("s3").foldered);
        // a second pass does nothing (a session the user moved out stays out)
        store.update_sessions(|v| v.iter_mut().for_each(|m| if m.session_id == "s2" { m.folder_id = None }));
        f.sort_old_sessions(&store);
        assert_eq!(get("s2").folder_id, None);
        assert_eq!(f.frame(&store, |_| None)["items"].as_array().unwrap().iter().find(|x| x["subject"] == "toan").unwrap()["sessionCount"], 1);

        let n = f.create("  Ôn giữa kỳ  ", "Toán").unwrap();
        assert_eq!((n.name.as_str(), n.subject.as_str()), ("Ôn giữa kỳ", "toan"));
        assert!(f.create(" ", "toan").is_err());
        assert!(f.rename(&n.id, "Đổi tên"));
        std::fs::create_dir_all(f.dir(&n.id)).unwrap();
        std::fs::write(f.dir(&n.id).join("knowledge.json"), r#"{"curriculum":[{"id":"c1","title":"Căn","topics":[{"id":"c1.t1","title":"Rút gọn"}]}]}"#).unwrap();
        assert!(f.has_knowledge(&n.id));
        let k = f.knowledge(&n.id).unwrap();
        assert_eq!(curriculum_topics(&k), vec![("c1.t1".to_string(), "Rút gọn".to_string(), "Căn".to_string())]);
        assert_eq!(curriculum_terms(&k, 5), vec!["Rút gọn", "Căn"]);
        assert!(f.delete(&n.id));
        assert!(!f.dir(&n.id).exists());
        assert!(!f.delete(&n.id));
    }
}
