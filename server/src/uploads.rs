//! Uploaded files (PROTOCOL section 9): `server/data/uploads/<fileId>.<ext>` plus a small
//! `<fileId>.json` with the metadata.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const MAX_UPLOAD_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub file_id: String,
    /// `image` or `pdf`.
    pub kind: String,
    pub name: String,
    pub bytes: usize,
    pub mime: String,
    pub created_at: String,
}

impl FileMeta {
    pub fn public(&self) -> Value {
        json!({"fileId": self.file_id, "kind": self.kind, "bytes": self.bytes, "name": self.name})
    }
}

pub struct Uploads {
    dir: PathBuf,
}

/// Content type -> (kind, extension), checked against the file's magic bytes.
fn classify(mime: &str, data: &[u8]) -> Result<(&'static str, &'static str, &'static str), String> {
    let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let (kind, ext, canon, magic): (&str, &str, &str, &[u8]) = match mime.as_str() {
        "image/jpeg" | "image/jpg" => ("image", "jpg", "image/jpeg", &[0xFF, 0xD8, 0xFF]),
        "image/png" => ("image", "png", "image/png", &[0x89, b'P', b'N', b'G']),
        "application/pdf" => ("pdf", "pdf", "application/pdf", b"%PDF"),
        _ => return Err("Chỉ nhận image/jpeg, image/png hoặc application/pdf.".into()),
    };
    if !data.starts_with(magic) {
        return Err("Nội dung tệp không khớp với Content-Type.".into());
    }
    Ok((kind, ext, canon))
}

fn clean_name(name: &str, ext: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .take(120)
        .collect();
    let n = n.trim().to_string();
    if n.is_empty() {
        format!("tep.{ext}")
    } else {
        n
    }
}

impl Uploads {
    pub fn open(data_dir: &Path) -> Uploads {
        let dir = data_dir.join("uploads");
        let _ = std::fs::create_dir_all(&dir);
        Uploads { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, name: &str, mime: &str, data: &[u8]) -> Result<FileMeta, String> {
        if data.is_empty() {
            return Err("Tệp rỗng.".into());
        }
        if data.len() > MAX_UPLOAD_BYTES {
            return Err("Tệp lớn hơn 30 MB.".into());
        }
        let (kind, ext, canon) = classify(mime, data)?;
        let id = uuid::Uuid::new_v4().to_string();
        let meta = FileMeta {
            file_id: id.clone(),
            kind: kind.into(),
            name: clean_name(name, ext),
            bytes: data.len(),
            mime: canon.into(),
            created_at: crate::store::now_rfc3339(),
        };
        std::fs::write(self.dir.join(format!("{id}.{ext}")), data).map_err(|e| format!("Không lưu được tệp: {e}"))?;
        std::fs::write(self.dir.join(format!("{id}.json")), serde_json::to_string(&meta).unwrap()).map_err(|e| format!("Không lưu được tệp: {e}"))?;
        Ok(meta)
    }

    pub fn meta(&self, id: &str) -> Option<FileMeta> {
        if !crate::store::is_valid_session_id(id) {
            return None;
        }
        serde_json::from_str(&std::fs::read_to_string(self.dir.join(format!("{id}.json"))).ok()?).ok()
    }

    pub fn path(&self, m: &FileMeta) -> PathBuf {
        let ext = match m.mime.as_str() {
            "image/png" => "png",
            "application/pdf" => "pdf",
            _ => "jpg",
        };
        self.dir.join(format!("{}.{ext}", m.file_id))
    }

    pub fn read(&self, id: &str) -> Option<(FileMeta, Vec<u8>)> {
        let m = self.meta(id)?;
        let data = std::fs::read(self.path(&m)).ok()?;
        Some((m, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_checks_type_and_reads_back() {
        let d = std::env::temp_dir().join(format!("beluga-up-{}", uuid::Uuid::new_v4()));
        let u = Uploads::open(&d);
        let m = u.save("../đề 1.jpg", "image/jpeg", &[0xFF, 0xD8, 0xFF, 1]).unwrap();
        assert_eq!((m.kind.as_str(), m.name.as_str(), m.bytes), ("image", "..đề 1.jpg", 4));
        assert_eq!(u.read(&m.file_id).unwrap().1, vec![0xFF, 0xD8, 0xFF, 1]);
        assert_eq!(u.save("a.pdf", "application/pdf", b"%PDF-1.4").unwrap().kind, "pdf");
        assert!(u.save("a.png", "image/png", b"%PDF").is_err(), "magic mismatch");
        assert!(u.save("a.gif", "image/gif", b"GIF89a").is_err());
        assert!(u.save("a.jpg", "image/jpeg", &[]).is_err());
        assert!(u.meta("../x").is_none());
        assert!(u.read("00000000-0000-0000-0000-000000000000").is_none());
    }
}
