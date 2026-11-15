//! The tutor's voice: a small catalogue (`server/voices.json`) with a stored preview clip per voice
//! (`server/voices/<id>.mp3`), and the owner's choice persisted in `server/data/voice.json`.
//! Wait and pick-up lines (`fillers`, PROTOCOL section 21) are stored once per voice as
//! `server/voices/<voiceId>/<fillerId>.mp3`.
//!
//! The choice is global (single-owner app): every later `say` of every session uses it. The voice
//! id from `.env` (`GENVOICE_VOICE_ID` / `ELEVENLABS_VOICE_ID`) is the default until a choice is made.

use crate::voice::Tts;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// GenVoice "Chi": the default voice when `.env` names none.
pub const DEFAULT_VOICE_ID: &str = "5yEPTrA0uAhZsBEhHjfi";
/// What every preview clip says (overridable by `sample` in `voices.json`).
pub const DEFAULT_SAMPLE: &str = "Chào cậu! Hôm nay tớ với cậu cùng ôn phương trình bậc hai nhé. Đầu tiên, mình tính delta.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceEntry {
    pub id: String,
    pub name: String,
    /// `female` or `male`.
    pub gender: String,
    /// `north`, `central`, `south` or empty.
    #[serde(default)]
    pub accent: String,
    /// One short line in Vietnamese.
    #[serde(default)]
    pub desc: String,
}

/// One wait/pick-up line, read with every voice of the catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Filler {
    pub id: String,
    pub text: String,
    /// `short`, `long`, `vision` or `hello`.
    pub when: String,
}

pub const FILLER_WHEN: &[&str] = &["short", "long", "vision", "hello"];

#[derive(Debug, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    sample: Option<String>,
    voices: Vec<VoiceEntry>,
    #[serde(default)]
    fillers: Vec<Filler>,
}

pub struct VoiceCatalog {
    entries: Vec<VoiceEntry>,
    fillers: Vec<Filler>,
    sample: String,
    clips_dir: PathBuf,
    choice_path: PathBuf,
    default_id: String,
    chosen: Mutex<Option<String>>,
}

fn builtin() -> Vec<VoiceEntry> {
    vec![VoiceEntry {
        id: DEFAULT_VOICE_ID.into(),
        name: "Chi".into(),
        gender: "female".into(),
        accent: "north".into(),
        desc: "Giọng nữ trẻ miền Bắc, ấm và tự nhiên.".into(),
    }]
}

/// A voice id is used in a file name and a URL path: keep it to plain characters.
pub fn is_valid_voice_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl VoiceCatalog {
    /// `catalog_path`: `server/voices.json` (built-in fallback with just "Chi" when missing or
    /// broken). `clips_dir`: `server/voices/`. `data_dir`: where the choice is kept.
    pub fn load(catalog_path: &Path, clips_dir: &Path, data_dir: &Path, default_id: &str) -> VoiceCatalog {
        let (mut entries, sample, mut fillers) = match std::fs::read_to_string(catalog_path) {
            Ok(t) => match serde_json::from_str::<CatalogFile>(&t) {
                Ok(f) => (f.voices, f.sample, f.fillers),
                Err(e) => {
                    tracing::warn!("{} is not valid ({e}); using the built-in voice list", catalog_path.display());
                    (builtin(), None, Vec::new())
                }
            },
            Err(_) => (builtin(), None, Vec::new()),
        };
        fillers.retain(|f| {
            let ok = is_valid_voice_id(&f.id) && FILLER_WHEN.contains(&f.when.as_str()) && !f.text.trim().is_empty();
            if !ok {
                tracing::warn!("voices.json: skipping filler {:?}", f.id);
            }
            ok
        });
        entries.retain(|e| {
            let ok = is_valid_voice_id(&e.id);
            if !ok {
                tracing::warn!("voices.json: skipping voice with a bad id {:?}", e.id);
            }
            ok
        });
        if entries.is_empty() {
            entries = builtin();
        }
        let choice_path = data_dir.join("voice.json");
        let chosen = std::fs::read_to_string(&choice_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["voice"].as_str().map(String::from))
            .filter(|id| {
                let known = entries.iter().any(|e| &e.id == id);
                if !known {
                    tracing::warn!("saved voice {id} is not in the catalogue any more; using the default");
                }
                known
            });
        let default_id = if is_valid_voice_id(default_id) { default_id.to_string() } else { DEFAULT_VOICE_ID.to_string() };
        VoiceCatalog {
            entries,
            fillers,
            sample: sample.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_SAMPLE.into()),
            clips_dir: clips_dir.to_path_buf(),
            choice_path,
            default_id,
            chosen: Mutex::new(chosen),
        }
    }

    pub fn entries(&self) -> &[VoiceEntry] {
        &self.entries
    }

    pub fn sample(&self) -> &str {
        &self.sample
    }

    pub fn get(&self, id: &str) -> Option<&VoiceEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The voice every `say` uses now: the owner's choice, else the `.env` default.
    pub fn current(&self) -> String {
        self.chosen.lock().unwrap().clone().unwrap_or_else(|| self.default_id.clone())
    }

    /// Choose a voice from the catalogue and persist it. False (nothing changes) for an unknown id.
    pub fn select(&self, id: &str) -> bool {
        if self.get(id).is_none() {
            tracing::warn!("settings.voice {id:?} is not in the catalogue; ignored");
            return false;
        }
        *self.chosen.lock().unwrap() = Some(id.to_string());
        let body = serde_json::to_vec_pretty(&json!({"voice": id})).unwrap_or_default();
        if let Err(e) = crate::study::ingest::write_atomic(&self.choice_path, &body) {
            tracing::warn!("could not save the voice choice: {e}");
        }
        tracing::info!("voice set to {id}");
        true
    }

    /// Path of the stored preview clip, when the id is in the catalogue.
    pub fn clip_path(&self, id: &str) -> Option<PathBuf> {
        self.get(id).map(|e| self.clips_dir.join(format!("{}.mp3", e.id)))
    }

    pub fn has_clip(&self, id: &str) -> bool {
        self.clip_path(id).map(|p| p.is_file()).unwrap_or(false)
    }

    pub fn fillers(&self) -> &[Filler] {
        &self.fillers
    }

    /// Path of a wait/pick-up clip, when both the voice and the filler are in the catalogue.
    pub fn filler_path(&self, voice: &str, filler: &str) -> Option<PathBuf> {
        let v = self.get(voice)?;
        let f = self.fillers.iter().find(|f| f.id == filler)?;
        Some(self.clips_dir.join(&v.id).join(format!("{}.mp3", f.id)))
    }

    /// Filler ids that have a clip for this voice.
    pub fn filler_clips(&self, voice: &str) -> Vec<String> {
        self.fillers.iter().filter(|f| self.filler_path(voice, &f.id).map(|p| p.is_file()).unwrap_or(false)).map(|f| f.id.clone()).collect()
    }

    /// The `voices` frame. `provider` is the active TTS provider; `device` lists no voices.
    pub fn frame(&self, provider: &str) -> Value {
        let items: Vec<Value> = if provider == "device" {
            Vec::new()
        } else {
            self.entries
                .iter()
                .map(|e| json!({"id": e.id, "name": e.name, "gender": e.gender, "accent": e.accent, "desc": e.desc,
                    "preview": self.has_clip(&e.id), "fillers": self.filler_clips(&e.id)}))
                .collect()
        };
        // `type` duplicates the protocol's `t` tag (the voice picker spec was written with `type`).
        json!({"t": "voices", "type": "voices", "items": items, "current": self.current(), "provider": provider, "fillers": self.fillers})
    }

    /// (voice id, filler) pairs that have no clip yet.
    pub fn missing_fillers(&self) -> Vec<(String, Filler)> {
        let mut out = Vec::new();
        for e in &self.entries {
            for f in &self.fillers {
                if !self.filler_path(&e.id, &f.id).map(|p| p.is_file()).unwrap_or(false) {
                    out.push((e.id.clone(), f.clone()));
                }
            }
        }
        out
    }

    /// Create the wait/pick-up clips that do not exist yet (one synthesis per missing clip, one at a
    /// time). Returns (created, failed) as `voiceId/fillerId`. Used by `beluga-server gen-voice-fillers`.
    pub async fn generate_missing_fillers(&self, tts: &dyn Tts, lang: &str) -> (Vec<String>, Vec<(String, String)>) {
        let (mut made, mut failed) = (Vec::new(), Vec::new());
        for (voice, f) in self.missing_fillers() {
            let key = format!("{voice}/{}", f.id);
            let Some(path) = self.filler_path(&voice, &f.id) else { continue };
            if let Some(dir) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    failed.push((key, format!("mkdir: {e}")));
                    continue;
                }
            }
            match tts.synth_voice(&f.text, lang, Some(&voice)).await {
                Ok(bytes) if !bytes.is_empty() => match crate::study::ingest::write_atomic(&path, &bytes) {
                    Ok(()) => made.push(key),
                    Err(err) => failed.push((key, format!("write: {err}"))),
                },
                Ok(_) => failed.push((key, "empty audio".into())),
                Err(err) => failed.push((key, err.message)),
            }
        }
        (made, failed)
    }

    /// Create the preview clips that do not exist yet (one synthesis per missing voice).
    /// Returns (created, failed) ids. Used by `beluga-server gen-voice-previews`.
    pub async fn generate_missing(&self, tts: &dyn Tts, lang: &str) -> (Vec<String>, Vec<(String, String)>) {
        let (mut made, mut failed) = (Vec::new(), Vec::new());
        for e in &self.entries {
            if self.has_clip(&e.id) {
                continue;
            }
            let path = self.clips_dir.join(format!("{}.mp3", e.id));
            match tts.synth_voice(&self.sample, lang, Some(&e.id)).await {
                Ok(bytes) if !bytes.is_empty() => match crate::study::ingest::write_atomic(&path, &bytes) {
                    Ok(()) => made.push(e.id.clone()),
                    Err(err) => failed.push((e.id.clone(), format!("write: {err}"))),
                },
                Ok(_) => failed.push((e.id.clone(), "empty audio".into())),
                Err(err) => failed.push((e.id.clone(), err.message)),
            }
        }
        (made, failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("beluga-voices-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn committed_catalogue_is_valid_and_has_every_clip() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(dir.join("voices.json")).unwrap();
        let f: CatalogFile = serde_json::from_str(&text).unwrap();
        assert!(f.voices.iter().any(|v| v.id == DEFAULT_VOICE_ID), "Chi stays in the list");
        for v in &f.voices {
            assert!(is_valid_voice_id(&v.id), "{}", v.id);
            assert!(v.gender == "female" || v.gender == "male", "{}", v.id);
            assert!(["north", "central", "south", ""].contains(&v.accent.as_str()), "{}", v.id);
            assert!(!v.desc.is_empty(), "{}", v.id);
            let clip = dir.join("voices").join(format!("{}.mp3", v.id));
            let bytes = std::fs::read(&clip).unwrap_or_else(|_| panic!("missing clip {}", clip.display()));
            assert!(bytes.len() > 4000, "{} looks too small", v.id);
            assert!(bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0), "{} is not an mp3", v.id);
            for fl in &f.fillers {
                let clip = dir.join("voices").join(&v.id).join(format!("{}.mp3", fl.id));
                let bytes = std::fs::read(&clip).unwrap_or_else(|_| panic!("missing filler clip {}", clip.display()));
                assert!(bytes.len() > 2000, "{}/{} looks too small", v.id, fl.id);
                assert!(bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0), "{}/{} is not an mp3", v.id, fl.id);
            }
        }
        // the wait and pick-up lines of PROTOCOL section 21, word for word
        let texts = |w: &str| f.fillers.iter().filter(|x| x.when == w).map(|x| x.text.as_str()).collect::<Vec<_>>();
        assert_eq!(texts("short"), ["Đợi tớ một chút nha, tớ đang nghĩ cho cậu nè.", "Ừm, để tớ nghĩ một xíu nha.", "Câu này hay đó, chờ tớ tí nha."]);
        assert_eq!(texts("long"), ["Tớ vẫn đang nghĩ nè, sắp xong rồi, cậu chờ thêm chút nha.", "Câu này hơi khó, tớ nghĩ kỹ thêm một chút cho chắc nha."]);
        assert_eq!(texts("vision"), ["Để tớ nhìn kỹ đề của cậu một chút nha."]);
        assert_eq!(texts("hello"), ["A lô, tớ nghe nè.", "Tớ đây, cậu cần gì nè?"]);
        assert!(f.fillers.iter().all(|x| is_valid_voice_id(&x.id)));
    }

    #[test]
    fn filler_paths_frame_and_missing_list() {
        let d = tmp();
        std::fs::write(
            d.join("v.json"),
            r#"{"voices":[{"id":"aaa","name":"A","gender":"female"},{"id":"bbb","name":"B","gender":"male"}],
                "fillers":[{"id":"short-1","text":"Đợi tớ nha.","when":"short"},{"id":"hello-1","text":"A lô.","when":"hello"},
                           {"id":"bad","text":"x","when":"never"},{"id":"../x","text":"x","when":"short"}]}"#,
        )
        .unwrap();
        let clips = d.join("clips");
        let c = VoiceCatalog::load(&d.join("v.json"), &clips, &d, "aaa");
        assert_eq!(c.fillers().len(), 2, "bad entries skipped");
        assert_eq!(c.missing_fillers().len(), 4);
        assert_eq!(c.filler_path("aaa", "short-1").unwrap(), clips.join("aaa").join("short-1.mp3"));
        assert!(c.filler_path("zzz", "short-1").is_none() && c.filler_path("aaa", "bad").is_none() && c.filler_path("aaa", "../x").is_none());
        std::fs::create_dir_all(clips.join("aaa")).unwrap();
        std::fs::write(clips.join("aaa").join("hello-1.mp3"), b"ID3").unwrap();
        let f = c.frame("genvoice");
        assert_eq!(f["fillers"].as_array().unwrap().len(), 2);
        assert_eq!(f["fillers"][1], json!({"id":"hello-1","text":"A lô.","when":"hello"}));
        assert_eq!(f["items"][0]["fillers"], json!(["hello-1"]));
        assert_eq!(f["items"][1]["fillers"], json!([]));
        assert_eq!(c.missing_fillers().len(), 3);
    }

    #[test]
    fn missing_or_broken_file_falls_back_to_chi() {
        let d = tmp();
        let c = VoiceCatalog::load(&d.join("none.json"), &d.join("clips"), &d, DEFAULT_VOICE_ID);
        assert_eq!(c.entries().len(), 1);
        assert_eq!(c.current(), DEFAULT_VOICE_ID);
        std::fs::write(d.join("bad.json"), "{not json").unwrap();
        let c = VoiceCatalog::load(&d.join("bad.json"), &d.join("clips"), &d, DEFAULT_VOICE_ID);
        assert_eq!(c.entries()[0].name, "Chi");
    }

    #[test]
    fn choice_persists_and_unknown_ids_are_ignored() {
        let d = tmp();
        std::fs::write(
            d.join("v.json"),
            r#"{"voices":[{"id":"aaa","name":"A","gender":"female","accent":"north","desc":"x"},
                          {"id":"bbb","name":"B","gender":"male","accent":"south","desc":"y"},
                          {"id":"../x","name":"bad","gender":"male"}]}"#,
        )
        .unwrap();
        let c = VoiceCatalog::load(&d.join("v.json"), &d.join("clips"), &d, "aaa");
        assert_eq!(c.entries().len(), 2, "bad id skipped");
        assert_eq!(c.current(), "aaa");
        assert!(!c.select("zzz"));
        assert_eq!(c.current(), "aaa");
        assert!(c.select("bbb"));
        let c2 = VoiceCatalog::load(&d.join("v.json"), &d.join("clips"), &d, "aaa");
        assert_eq!(c2.current(), "bbb");
        assert!(c2.clip_path("../x").is_none());
        let f = c2.frame("genvoice");
        assert_eq!(f["items"].as_array().unwrap().len(), 2);
        assert_eq!(f["items"][0]["preview"], false);
        assert_eq!(c2.frame("device")["items"].as_array().unwrap().len(), 0);
    }
}
