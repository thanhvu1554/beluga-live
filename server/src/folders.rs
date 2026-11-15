//! Folders (PROTOCOL sections 8, 17): a free name and an optional one-line description (`about`),
//! stored in `server/data/folders.json`. The teaching kind (`kind`, one of the skill ids) is not
//! picked by the user: the utility model assigns it from name + about (see [`assign_kind`]); until it
//! answers a keyword guess stands in and the folder stays `kindPending` (retried on a later `folders`).
//! Per-folder data lives in `server/data/folders/<id>/` (`knowledge.json` is written by the exam
//! ingest module and only read here; `weekly.json` is the weekly summary).
//!
//! Folders written before v3 have `subject` and no `kind`: they read as `kind = subject`, `about = ""`.

use crate::memory::{fold, normalize_subject, SUBJECTS};
use crate::store::{is_valid_session_id, now_rfc3339, Store};
use crate::usage::Usage;
use crate::util_llm::{self, UtilLlm, UtilReq};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const MAX_NAME: usize = 80;
pub const MAX_ABOUT: usize = 300;
/// Deadline of one kind assignment by the utility model (PROTOCOL section 17).
pub const KIND_MS: u64 = 3000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderMeta {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub about: String,
    /// Teaching kind (a skill id). Empty only in files written before v3 (then `subject` holds it).
    #[serde(default)]
    pub kind: String,
    /// v2 field, kept equal to `kind` so older readers of the file still work.
    #[serde(default)]
    pub subject: String,
    /// `kind` is only a guess: ask the utility model (again) on the next `folders`.
    #[serde(default)]
    pub kind_pending: bool,
    /// Id the phone gave the folder when it was made offline (idempotent `create_folder`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl FolderMeta {
    /// Text the kind is derived from; an answer for an older text is thrown away.
    pub fn kind_source(&self) -> String {
        format!("{}\n{}", self.name, self.about)
    }

    fn migrate(mut self) -> FolderMeta {
        if self.kind.is_empty() {
            self.kind = if self.subject.trim().is_empty() { "chung".into() } else { subject_id(&self.subject) };
        }
        self.subject = self.kind.clone();
        self
    }
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

/// Is `id` one of the teaching kinds (= skill ids)?
pub fn is_kind(id: &str) -> bool {
    SUBJECTS.iter().any(|(k, _)| *k == id)
}

/// Keyword guess of the teaching kind from a folder's name and description: what the folder
/// shows until the utility model answers (and when there is no utility model at all).
pub fn guess_kind(name: &str, about: &str) -> String {
    let text = fold(&format!("{name} {about}"));
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric() && c != '+').filter(|w| !w.is_empty()).collect();
    let joined = format!(" {} ", words.join(" "));
    let has = |ks: &[&str]| ks.iter().any(|k| joined.contains(&format!(" {k} ")));
    let id = if has(&["ssat", "toefl", "ielts", "sat", "toeic"]) {
        "anh-ssat"
    } else if has(&["tieng duc", "deutsch", "german", "duc"]) {
        "duc"
    } else if has(&["tieng anh", "english", "anh van", "anh", "ngoai ngu", "tieng phap", "tieng nhat", "tieng han", "tieng trung", "french", "japanese", "korean", "chinese"]) {
        "ngoai-ngu"
    } else if has(&["tin", "tin hoc", "lap trinh", "c++", "cpp", "python", "code", "coding"]) {
        "tin-cpp"
    } else if has(&["toan", "math", "dai so", "hinh hoc", "giai tich"]) {
        "toan"
    } else if has(&["ly", "li", "vat ly", "vat li", "physics"]) {
        "ly"
    } else if has(&["hoa", "hoa hoc", "chemistry"]) {
        "hoa"
    } else if has(&["van", "ngu van", "literature"]) {
        "van"
    } else {
        "chung"
    };
    id.to_string()
}

const KIND_SYSTEM: &str = "Bạn xếp một folder học tập của một học sinh Việt Nam vào đúng MỘT kiểu dạy, dựa vào tên và mô tả \
folder. Các kiểu dạy (id: phạm vi):\n\
toan: Toán\nly: Vật lý\nhoa: Hóa học\nvan: Ngữ văn\n\
anh-ssat: tiếng Anh luyện thi chuẩn hóa quốc tế (SSAT, TOEFL, IELTS, SAT); KHÔNG dùng cho tiếng Anh trên lớp\n\
duc: tiếng Đức\ntin-cpp: Tin học, lập trình\n\
ngoai-ngu: ngoại ngữ theo chương trình trên lớp (tiếng Anh trên lớp, tiếng Pháp, Nhật, Hàn, Trung...)\n\
chung: môn khác (Sinh, Sử, Địa, Giáo dục công dân...) hoặc không rõ\n\
Chỉ in ra đúng id, không giải thích.";

/// Read a kind id out of a model answer (`"ngoai-ngu"`, `ngoai-ngu.`, ...); `None` if it is not one.
pub fn parse_kind(answer: &str) -> Option<String> {
    let a = util_llm::one_line(answer).to_lowercase();
    let a = a.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
    is_kind(a).then(|| a.to_string())
}

/// Ask the utility model for the kind of a folder (3 s deadline).
pub async fn assign_kind(util: &Arc<dyn UtilLlm>, usage: &Usage, name: &str, about: &str) -> Result<String, String> {
    let prompt = format!("Tên folder: {name}\nMô tả: {}", if about.trim().is_empty() { "(không có)" } else { about });
    let answer = util_llm::ask(util, usage, UtilReq::new("folder_kind", KIND_SYSTEM, prompt, 12), Duration::from_millis(KIND_MS)).await?;
    parse_kind(&answer).ok_or_else(|| format!("not a kind: {answer:?}"))
}

fn clean(s: &str, max: usize) -> String {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect::<String>().trim().to_string()
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
        let v: Vec<FolderMeta> = std::fs::read_to_string(self.path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        v.into_iter().map(FolderMeta::migrate).collect()
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

    /// v2 create: the user picked the subject.
    pub fn create(&self, name: &str, subject: &str) -> Result<FolderMeta, String> {
        self.create_v3(name, "", Some(subject), None).map(|(f, _)| f)
    }

    /// v3 create (PROTOCOL section 17). A `subject` other than `chung` is taken as the kind;
    /// otherwise the kind is a keyword guess and stays pending for the utility model. Returns the
    /// folder and whether it is new (false: the same `clientId` was already used).
    pub fn create_v3(&self, name: &str, about: &str, subject: Option<&str>, client_id: Option<&str>) -> Result<(FolderMeta, bool), String> {
        let name = clean(name, MAX_NAME);
        if name.is_empty() {
            return Err("Tên folder trống.".into());
        }
        let about = clean(about, MAX_ABOUT);
        let client_id = client_id.map(|c| clean(c, 80)).filter(|c| !c.is_empty());
        let given = subject.map(subject_id).filter(|s| s != "chung");
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.read();
        if let Some(c) = &client_id {
            if let Some(f) = v.iter().find(|f| f.client_id.as_ref() == Some(c)) {
                return Ok((f.clone(), false));
            }
        }
        let now = now_rfc3339();
        let (kind, pending) = match given {
            Some(k) => (k, false),
            None => (guess_kind(&name, &about), true),
        };
        let f = FolderMeta {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            about,
            subject: kind.clone(),
            kind,
            kind_pending: pending,
            client_id,
            created_at: now.clone(),
            updated_at: now,
        };
        v.push(f.clone());
        self.write(&v);
        Ok((f, true))
    }

    fn modify(&self, id: &str, f: impl FnOnce(&mut FolderMeta)) -> bool {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.read();
        let Some(x) = v.iter_mut().find(|x| x.id == id) else { return false };
        f(x);
        x.subject = x.kind.clone();
        self.write(&v);
        true
    }

    pub fn rename(&self, id: &str, name: &str) -> bool {
        self.update(id, Some(name), None)
    }

    /// `update_folder` (and `rename_folder`): a changed name or description asks for the kind again
    /// (the keyword guess stands in when it names a kind; otherwise the old kind stays meanwhile).
    /// False when the folder does not exist or the new name is empty.
    pub fn update(&self, id: &str, name: Option<&str>, about: Option<&str>) -> bool {
        let name = name.map(|n| clean(n, MAX_NAME));
        if name.as_deref() == Some("") {
            return false;
        }
        let about = about.map(|a| clean(a, MAX_ABOUT));
        self.modify(id, |x| {
            let before = x.kind_source();
            if let Some(n) = name {
                x.name = n;
            }
            if let Some(a) = about {
                x.about = a;
            }
            if x.kind_source() != before {
                let g = guess_kind(&x.name, &x.about);
                if g != "chung" {
                    x.kind = g;
                }
                x.kind_pending = true;
            }
            x.updated_at = now_rfc3339();
        })
    }

    /// The utility model's answer for the folder as it was named then (`source`); dropped when the
    /// name or description changed meanwhile. True when the stored kind changed.
    pub fn set_kind(&self, id: &str, source: &str, kind: &str) -> bool {
        if !is_kind(kind) {
            return false;
        }
        let mut changed = false;
        self.modify(id, |x| {
            if x.kind_source() == source {
                changed = x.kind != kind || x.kind_pending;
                x.kind = kind.to_string();
                x.kind_pending = false;
            }
        });
        changed
    }

    /// Folders whose kind is still a guess.
    pub fn pending_kinds(&self) -> Vec<FolderMeta> {
        self.list().into_iter().filter(|f| f.kind_pending).collect()
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
                let mut v = json!({"id": f.id, "name": f.name, "about": f.about, "kind": f.kind, "subject": f.kind,
                       "sessionCount": count, "updatedAt": f.updated_at, "hasKnowledge": self.has_knowledge(&f.id)});
                if let Some(i) = ingest(&f.id) {
                    v["ingest"] = json!({"jobId": i["jobId"], "state": i["state"], "done": i["done"], "total": i["total"]});
                }
                v
            })
            .collect();
        json!({"t": "folders", "items": items})
    }

    /// Sessions from before folders are no longer filed into subject folders (PROTOCOL section 18:
    /// folders are free-form knowledge streams, the server never puts a session into one by itself).
    /// This only marks them as looked at, so nothing ever files them later either.
    pub fn sort_old_sessions(&self, store: &Store) {
        if store.list_sessions().iter().all(|m| m.foldered) {
            return;
        }
        store.update_sessions(|v| v.iter_mut().for_each(|m| m.foldered = true));
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

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("beluga-fold-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn kinds_from_names_and_answers() {
        assert_eq!(subject_id("Toán"), "toan");
        assert_eq!(subject_id("anh-ssat"), "anh-ssat");
        for (name, about, kind) in [
            ("Tiếng Anh trên lớp", "", "ngoai-ngu"),
            ("Anh 9", "theo sách Global Success, cô hay cho ngữ pháp và viết lại câu", "ngoai-ngu"),
            ("Luyện SSAT", "", "anh-ssat"),
            ("Tiếng Đức A1", "", "duc"),
            ("Toán giữa kỳ", "", "toan"),
            ("Ôn thi vào 10", "đại số và hình học", "toan"),
            ("Vật lý 10", "", "ly"),
            ("Hóa hữu cơ", "", "hoa"),
            ("Ngữ văn", "", "van"),
            ("Lập trình C++", "", "tin-cpp"),
            ("Sinh học", "", "chung"),
            ("Linh tinh", "", "chung"),
        ] {
            assert_eq!(guess_kind(name, about), kind, "{name} | {about}");
        }
        assert_eq!(parse_kind("ngoai-ngu").as_deref(), Some("ngoai-ngu"));
        assert_eq!(parse_kind(" \"Toan\".\n").as_deref(), Some("toan"));
        assert_eq!(parse_kind("`anh-ssat`").as_deref(), Some("anh-ssat"));
        assert_eq!(parse_kind("sinh"), None);
        assert_eq!(parse_kind("toan hoặc ly"), None);
    }

    #[test]
    fn v2_files_migrate_and_old_sessions_are_never_filed() {
        let d = tmp();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("folders.json"), r#"[{"id":"f1","name":"Toán","subject":"toan","createdAt":"a","updatedAt":"a"}]"#).unwrap();
        let store = Store::open(&d, Some("111111")).unwrap();
        let f = Folders::open(&d);
        let m = f.get("f1").unwrap();
        assert_eq!((m.kind.as_str(), m.about.as_str(), m.kind_pending), ("toan", "", false));
        let fr = f.frame(&store, |_| None);
        assert_eq!((fr["items"][0]["kind"].as_str(), fr["items"][0]["subject"].as_str(), fr["items"][0]["about"].as_str()), (Some("toan"), Some("toan"), Some("")));

        store.upsert_session(&SessionMeta { title: "Toán · A · 01/11".into(), foldered: false, ..SessionMeta::new("s1") });
        f.sort_old_sessions(&store);
        let s = store.get_session("s1").unwrap();
        assert!(s.folder_id.is_none() && s.foldered, "no session is filed into a folder by the server");
        assert_eq!(f.list().len(), 1, "no folder is made");
    }

    #[test]
    fn crud_client_ids_and_kind_answers() {
        let d = tmp();
        let f = Folders::open(&d);
        let (a, new) = f.create_v3("  Tiếng Anh trên lớp ", "Anh 9 Global Success", None, Some("c-1")).unwrap();
        assert!(new);
        assert_eq!((a.name.as_str(), a.kind.as_str(), a.kind_pending), ("Tiếng Anh trên lớp", "ngoai-ngu", true));
        let (again, new) = f.create_v3("khác", "", None, Some("c-1")).unwrap();
        assert!(!new);
        assert_eq!(again.id, a.id, "same clientId: same folder");
        assert_eq!(f.list().len(), 1);
        assert!(f.create_v3(" ", "", None, None).is_err());
        let long = "x".repeat(500);
        let (b, _) = f.create_v3("B", &long, Some("Toán"), None).unwrap();
        assert_eq!((b.kind.as_str(), b.kind_pending, b.about.chars().count()), ("toan", false, MAX_ABOUT));
        let (c, _) = f.create_v3("C", "", Some("chung"), None).unwrap();
        assert!(c.kind_pending, "a default `chung` subject still asks the model");

        // an answer for the current text applies; one for an older text is dropped
        let src = a.kind_source();
        assert!(f.update(&a.id, None, Some("Ôn SSAT")));
        assert_eq!(f.get(&a.id).unwrap().kind, "anh-ssat", "guess follows the new text");
        assert!(!f.set_kind(&a.id, &src, "ngoai-ngu"), "stale answer");
        let now = f.get(&a.id).unwrap();
        assert!(now.kind_pending);
        assert!(f.set_kind(&a.id, &now.kind_source(), "anh-ssat"));
        assert!(!f.get(&a.id).unwrap().kind_pending);
        assert!(!f.set_kind(&a.id, &now.kind_source(), "sinh"), "unknown kinds are refused");
        assert_eq!(f.pending_kinds().len(), 1, "only C");
        assert!(!f.update(&a.id, Some("  "), None));
        assert!(f.rename(&b.id, "Toán 9"));
        assert!(f.get(&b.id).unwrap().kind_pending, "a rename asks again");

        std::fs::create_dir_all(f.dir(&b.id)).unwrap();
        std::fs::write(f.dir(&b.id).join("knowledge.json"), r#"{"curriculum":[{"id":"c1","title":"Căn","topics":[{"id":"c1.t1","title":"Rút gọn"}]}]}"#).unwrap();
        assert!(f.has_knowledge(&b.id));
        let k = f.knowledge(&b.id).unwrap();
        assert_eq!(curriculum_topics(&k), vec![("c1.t1".to_string(), "Rút gọn".to_string(), "Căn".to_string())]);
        assert_eq!(curriculum_terms(&k, 5), vec!["Rút gọn", "Căn"]);
        assert!(f.delete(&b.id));
        assert!(!f.dir(&b.id).exists());
        assert!(!f.delete(&b.id));
    }
}
