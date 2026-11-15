//! Off-VPS backup to Cloudflare R2 or any S3-compatible store (PROTOCOL section 20).
//!
//! What: `server/data/` (minus `mcp/`, this module's `backup.json` and temp folders),
//! `tutor/memory/`, `server/voices.json`, `server/models.json`. Archive paths: `data/…`,
//! `memory/…`, `voices.json`, `models.json`.
//!
//! How: a blocking task walks the tree in sorted order and streams it through
//! tar → zstd (level 3) → optional encryption → a temp file under `data/.backup-tmp/`, hashing the
//! plain tar bytes on the way (that SHA-256 is what "unchanged" compares, so encryption's random
//! nonces never defeat the skip). Memory stays bounded: one zstd window, one 1 MiB crypto chunk,
//! and at most one upload part (or one small object) in RAM. Upload is plain S3 SigV4 over
//! `reqwest`: a single PUT up to [`MULTIPART_THRESHOLD`], multipart with fixed-size parts above it
//! (R2 needs equal parts except the last). `latest` is then copied server side to `daily/<date>`,
//! `manifest.json` is written, and old dailies are pruned.
//!
//! Keys (prefix `beluga/`): `latest.tar.zst[.enc]`, `daily/YYYY-MM-DD.tar.zst[.enc]`, `manifest.json`
//! = `{v, at, bytes, sha256, files, encrypted, key}` (`sha256` is over the uncompressed tar,
//! `bytes` is the stored object size).
//!
//! ## Encrypted file format (`.enc`, version 1)
//!
//! ```text
//! offset size  field
//! 0      8     magic "BLGENC01"
//! 8      1     KDF id: 1 = PBKDF2-HMAC-SHA256
//! 9      4     KDF iterations, u32 big endian (600000 by default)
//! 13     16    salt (random)
//! 29     7     nonce prefix (random)
//! 36     4     chunk size C, u32 big endian (plaintext bytes per chunk; 1 MiB)
//! 40     …     chunks
//! ```
//!
//! key = PBKDF2-HMAC-SHA256(passphrase as UTF-8 bytes, salt, iterations, 32 bytes).
//! The plaintext (a `.tar.zst` stream) is cut into chunks of exactly C bytes, the last one holding
//! 0..=C bytes (a stream whose length is a multiple of C ends with an empty chunk). Chunk i is
//! sealed with AES-256-GCM, nonce = prefix(7) ‖ i as u32 big endian ‖ last flag (1 byte: 0, or 1
//! for the final chunk), AAD = the 40 header bytes, and stored as ciphertext ‖ 16-byte tag. This is
//! the STREAM construction: reordering, truncation and header tampering all fail authentication.
//! A reader takes C+16 bytes at a time; a shorter read is the final chunk.

use crate::config::Config;
use crate::http::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PREFIX: &str = "beluga/";
/// Objects up to this size go up in one PUT (held in memory); larger ones use multipart.
pub const MULTIPART_THRESHOLD: u64 = 16 * 1024 * 1024;
/// Multipart part size (R2/S3 minimum is 5 MiB; every part but the last has this size).
pub const PART_SIZE: usize = 8 * 1024 * 1024;
pub const DEFAULT_KDF_ITERATIONS: u32 = 600_000;
const ENC_MAGIC: &[u8; 8] = b"BLGENC01";
const ENC_HEADER_LEN: usize = 40;
const ENC_CHUNK: usize = 1024 * 1024;
const TAG_LEN: usize = 16;
const ZSTD_LEVEL: i32 = 3;
const TMP_DIR: &str = ".backup-tmp";
const STATE_FILE: &str = "backup.json";

// ===================================================================== config

/// What goes into the archive and where a restore puts it back.
#[derive(Clone, Debug)]
pub struct Sources {
    pub data_dir: PathBuf,
    pub memory_dir: PathBuf,
    pub voices_json: PathBuf,
    pub models_json: PathBuf,
}

impl Sources {
    pub fn from_config(cfg: &Config) -> Sources {
        Sources {
            data_dir: cfg.data_dir.clone(),
            memory_dir: cfg.tutor_dir.join("memory"),
            voices_json: cfg.voices_path.clone(),
            models_json: cfg.models_path.clone(),
        }
    }
}

#[derive(Clone)]
pub struct BackupConfig {
    /// `https://<account>.r2.cloudflarestorage.com`, or `R2_ENDPOINT` (any S3-compatible store).
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub prefix: String,
    pub passphrase: Option<String>,
    pub keep_days: usize,
    pub kdf_iterations: u32,
    pub multipart_threshold: u64,
    pub part_size: usize,
    /// Wait this long after the last change before a backup.
    pub debounce: Duration,
    /// At most one automatic backup per this interval.
    pub min_interval: Duration,
    /// Local hour of the nightly backup.
    pub nightly_hour: u32,
    pub sources: Sources,
}

impl BackupConfig {
    /// `None` (backup off) unless the access key, secret, bucket and an account id or endpoint are set.
    pub fn from_config(cfg: &Config) -> Option<BackupConfig> {
        let get = |k: &str| cfg.secret(k);
        let endpoint = match (get("R2_ENDPOINT"), get("R2_ACCOUNT_ID")) {
            (Some(e), _) => e.trim_end_matches('/').to_string(),
            (None, Some(a)) => format!("https://{a}.r2.cloudflarestorage.com"),
            _ => return None,
        };
        Some(BackupConfig {
            endpoint,
            bucket: get("R2_BUCKET")?,
            access_key: get("R2_ACCESS_KEY_ID")?,
            secret_key: get("R2_SECRET_ACCESS_KEY")?,
            region: get("R2_REGION").unwrap_or_else(|| "auto".into()),
            prefix: PREFIX.into(),
            passphrase: get("BACKUP_PASSPHRASE"),
            keep_days: get("BACKUP_KEEP_DAYS").and_then(|v| v.parse().ok()).filter(|n| *n >= 1).unwrap_or(14),
            kdf_iterations: DEFAULT_KDF_ITERATIONS,
            multipart_threshold: MULTIPART_THRESHOLD,
            part_size: PART_SIZE,
            debounce: Duration::from_secs(120),
            min_interval: Duration::from_secs(600),
            nightly_hour: 3,
            sources: Sources::from_config(cfg),
        })
    }

    fn ext(&self) -> &'static str {
        if self.passphrase.is_some() { ".tar.zst.enc" } else { ".tar.zst" }
    }

    pub fn latest_key(&self) -> String {
        format!("{}latest{}", self.prefix, self.ext())
    }

    pub fn manifest_key(&self) -> String {
        format!("{}manifest.json", self.prefix)
    }

    fn daily_key(&self, date: &str) -> String {
        format!("{}daily/{date}{}", self.prefix, self.ext())
    }

    /// Host part of the endpoint (for logs; never print keys).
    pub fn host(&self) -> &str {
        let e = self.endpoint.split("://").nth(1).unwrap_or(&self.endpoint);
        e.split('/').next().unwrap_or(e)
    }
}

// ===================================================================== S3 (SigV4)

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut o, x| {
        let _ = write!(o, "{x:02x}");
        o
    })
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, b).as_ref())
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::sign(&k, msg).as_ref().to_vec()
}

/// RFC 3986 encoding as SigV4 wants it (unreserved kept; `/` kept only in paths).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b'/' if keep_slash => o.push('/'),
            _ => {
                let _ = write!(o, "%{b:02X}");
            }
        }
    }
    o
}

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub struct S3 {
    http: reqwest::Client,
    cfg: BackupConfig,
}

/// One object of a listing.
#[derive(Debug, Clone)]
pub struct Obj {
    pub key: String,
    pub size: u64,
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Text of every `<tag>…</tag>` in `xml` (flat, good enough for S3 responses).
fn xml_all(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(xml_unescape(&after[..j]));
        rest = &after[j + close.len()..];
    }
    out
}

impl S3 {
    pub fn new(cfg: &BackupConfig) -> S3 {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .build()
            .unwrap_or_default();
        S3 { http, cfg: cfg.clone() }
    }

    /// Send one signed request (retrying network errors and 5xx a few times).
    async fn send(&self, method: &str, key: Option<&str>, query: &[(&str, &str)], headers: &[(&str, String)], body: Vec<u8>) -> Result<reqwest::Response, String> {
        let path = match key {
            Some(k) => format!("/{}/{}", uri_encode(&self.cfg.bucket, false), uri_encode(k, true)),
            None => format!("/{}", uri_encode(&self.cfg.bucket, false)),
        };
        let mut q: Vec<(String, String)> = query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
        q.sort();
        let qs = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
        let url = if qs.is_empty() { format!("{}{path}", self.cfg.endpoint) } else { format!("{}{path}?{qs}", self.cfg.endpoint) };
        let payload_hash = if body.is_empty() { EMPTY_SHA256.to_string() } else { sha256_hex(&body) };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let now = chrono::Utc::now();
            let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
            let date = now.format("%Y%m%d").to_string();
            let mut hs: Vec<(String, String)> = vec![
                ("host".into(), self.cfg.host().to_string()),
                ("x-amz-content-sha256".into(), payload_hash.clone()),
                ("x-amz-date".into(), amz_date.clone()),
            ];
            for (k, v) in headers {
                hs.push((k.to_ascii_lowercase(), v.trim().to_string()));
            }
            hs.sort();
            let signed = hs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
            let canon_headers: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
            let canonical = format!("{method}\n{path}\n{qs}\n{canon_headers}\n{signed}\n{payload_hash}");
            let scope = format!("{date}/{}/s3/aws4_request", self.cfg.region);
            let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical.as_bytes()));
            let mut k = hmac(format!("AWS4{}", self.cfg.secret_key).as_bytes(), date.as_bytes());
            k = hmac(&k, self.cfg.region.as_bytes());
            k = hmac(&k, b"s3");
            k = hmac(&k, b"aws4_request");
            let sig = hex(&hmac(&k, to_sign.as_bytes()));
            let auth = format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={sig}", self.cfg.access_key);
            let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
            let mut rb = self.http.request(m, &url).header("authorization", auth);
            for (k, v) in &hs {
                if k != "host" {
                    rb = rb.header(k.as_str(), v.as_str());
                }
            }
            let r = rb.body(body.clone()).send().await;
            match r {
                Ok(resp) if resp.status().is_server_error() && attempt < 4 => {
                    tracing::warn!("backup: {method} {} -> {}, retrying", key.unwrap_or(""), resp.status());
                }
                Ok(resp) => return Ok(resp),
                Err(e) if attempt < 4 => tracing::warn!("backup: {method} {} failed ({e}), retrying", key.unwrap_or("")),
                Err(e) => return Err(format!("{method} {}: {e}", key.unwrap_or(""))),
            }
            tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
        }
    }

    async fn ok(&self, method: &str, key: Option<&str>, query: &[(&str, &str)], headers: &[(&str, String)], body: Vec<u8>) -> Result<reqwest::Response, String> {
        let r = self.send(method, key, query, headers, body).await?;
        if r.status().is_success() {
            return Ok(r);
        }
        let st = r.status();
        let text = r.text().await.unwrap_or_default();
        let code = xml_all(&text, "Code").into_iter().next().unwrap_or_default();
        Err(format!("{method} {}: HTTP {} {code}", key.unwrap_or(""), st.as_u16()))
    }

    pub async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<(), String> {
        self.ok("PUT", Some(key), &[], &[("content-type", content_type.to_string())], body).await.map(|_| ())
    }

    /// Upload a file: one PUT up to the multipart threshold, else multipart with fixed-size parts.
    pub async fn put_file(&self, key: &str, path: &Path) -> Result<u64, String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size <= self.cfg.multipart_threshold {
            let data = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
            self.put(key, data, "application/octet-stream").await?;
            return Ok(size);
        }
        let r = self.ok("POST", Some(key), &[("uploads", "")], &[("content-type", "application/octet-stream".into())], Vec::new()).await?;
        let text = r.text().await.map_err(|e| e.to_string())?;
        let Some(upload_id) = xml_all(&text, "UploadId").into_iter().next() else { return Err("multipart: no UploadId".into()) };
        match self.upload_parts(key, path, &upload_id).await {
            Ok(xml) => {
                let r = self.ok("POST", Some(key), &[("uploadId", &upload_id)], &[("content-type", "application/xml".into())], xml.into_bytes()).await;
                let r = match r {
                    Ok(r) => r.text().await.map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                match r {
                    Ok(t) if !t.contains("<Error>") => Ok(size),
                    Ok(t) => {
                        let _ = self.send("DELETE", Some(key), &[("uploadId", &upload_id)], &[], Vec::new()).await;
                        Err(format!("multipart complete failed: {}", xml_all(&t, "Code").join(",")))
                    }
                    Err(e) => {
                        let _ = self.send("DELETE", Some(key), &[("uploadId", &upload_id)], &[], Vec::new()).await;
                        Err(e)
                    }
                }
            }
            Err(e) => {
                let _ = self.send("DELETE", Some(key), &[("uploadId", &upload_id)], &[], Vec::new()).await;
                Err(e)
            }
        }
    }

    async fn upload_parts(&self, key: &str, path: &Path, upload_id: &str) -> Result<String, String> {
        use tokio::io::AsyncReadExt;
        let mut f = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
        let mut xml = String::from("<CompleteMultipartUpload>");
        let mut n = 0u32;
        loop {
            let mut buf = vec![0u8; self.cfg.part_size];
            let mut filled = 0;
            while filled < buf.len() {
                let k = f.read(&mut buf[filled..]).await.map_err(|e| e.to_string())?;
                if k == 0 {
                    break;
                }
                filled += k;
            }
            if filled == 0 {
                break;
            }
            buf.truncate(filled);
            n += 1;
            let num = n.to_string();
            let r = self.ok("PUT", Some(key), &[("partNumber", &num), ("uploadId", upload_id)], &[], buf).await?;
            let etag = r.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let _ = write!(xml, "<Part><PartNumber>{n}</PartNumber><ETag>{}</ETag></Part>", etag.replace('&', "&amp;").replace('"', "&quot;"));
            if filled < self.cfg.part_size {
                break;
            }
        }
        xml.push_str("</CompleteMultipartUpload>");
        Ok(xml)
    }

    /// Server-side copy (no re-upload).
    pub async fn copy(&self, from: &str, to: &str) -> Result<(), String> {
        let src = format!("/{}/{}", self.cfg.bucket, uri_encode(from, true));
        let r = self.ok("PUT", Some(to), &[], &[("x-amz-copy-source", src)], Vec::new()).await?;
        let t = r.text().await.unwrap_or_default();
        if t.contains("<Error>") {
            return Err(format!("copy failed: {}", xml_all(&t, "Code").join(",")));
        }
        Ok(())
    }

    /// The object's bytes, `None` when it does not exist (small objects only).
    pub async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let r = self.send("GET", Some(key), &[], &[], Vec::new()).await?;
        if r.status().as_u16() == 404 {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(format!("GET {key}: HTTP {}", r.status().as_u16()));
        }
        r.bytes().await.map(|b| Some(b.to_vec())).map_err(|e| e.to_string())
    }

    /// Stream the object into `path`; `None` when it does not exist.
    pub async fn get_to_file(&self, key: &str, path: &Path) -> Result<Option<u64>, String> {
        use tokio::io::AsyncWriteExt;
        let mut r = self.send("GET", Some(key), &[], &[], Vec::new()).await?;
        if r.status().as_u16() == 404 {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(format!("GET {key}: HTTP {}", r.status().as_u16()));
        }
        let mut f = tokio::fs::File::create(path).await.map_err(|e| e.to_string())?;
        let mut n = 0u64;
        while let Some(chunk) = r.chunk().await.map_err(|e| e.to_string())? {
            n += chunk.len() as u64;
            f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        f.flush().await.map_err(|e| e.to_string())?;
        Ok(Some(n))
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<Obj>, String> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut q: Vec<(&str, &str)> = vec![("list-type", "2"), ("prefix", prefix)];
            if let Some(t) = token.as_deref() {
                q.push(("continuation-token", t));
            }
            let r = self.ok("GET", None, &q, &[], Vec::new()).await?;
            let t = r.text().await.map_err(|e| e.to_string())?;
            for c in xml_all(&t, "Contents") {
                let key = xml_all(&c, "Key").into_iter().next().unwrap_or_default();
                let size = xml_all(&c, "Size").into_iter().next().and_then(|s| s.parse().ok()).unwrap_or(0);
                out.push(Obj { key, size });
            }
            let truncated = xml_all(&t, "IsTruncated").first().map(|s| s == "true").unwrap_or(false);
            token = xml_all(&t, "NextContinuationToken").into_iter().next();
            if !truncated || token.is_none() {
                return Ok(out);
            }
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), String> {
        let r = self.send("DELETE", Some(key), &[], &[], Vec::new()).await?;
        if r.status().is_success() || r.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(format!("DELETE {key}: HTTP {}", r.status().as_u16()))
        }
    }
}

// ===================================================================== encryption

fn derive_key(pass: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut key = [0u8; 32];
    let it = std::num::NonZeroU32::new(iterations.max(1)).unwrap();
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, it, salt, pass.as_bytes(), &mut key);
    key
}

fn aead_key(key: &[u8; 32]) -> ring::aead::LessSafeKey {
    ring::aead::LessSafeKey::new(ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, key).expect("32-byte key"))
}

fn chunk_nonce(prefix: &[u8], i: u32, last: bool) -> ring::aead::Nonce {
    let mut n = [0u8; 12];
    n[..7].copy_from_slice(prefix);
    n[7..11].copy_from_slice(&i.to_be_bytes());
    n[11] = last as u8;
    ring::aead::Nonce::assume_unique_for_key(n)
}

/// Writer that encrypts into the `.enc` format; [`EncWriter::finish`] seals the final chunk.
pub struct EncWriter<W: Write> {
    inner: W,
    key: ring::aead::LessSafeKey,
    header: [u8; ENC_HEADER_LEN],
    buf: Vec<u8>,
    index: u32,
}

impl<W: Write> EncWriter<W> {
    pub fn new(mut inner: W, pass: &str, iterations: u32) -> io::Result<Self> {
        use ring::rand::SecureRandom;
        let rng = ring::rand::SystemRandom::new();
        let mut header = [0u8; ENC_HEADER_LEN];
        header[..8].copy_from_slice(ENC_MAGIC);
        header[8] = 1;
        header[9..13].copy_from_slice(&iterations.to_be_bytes());
        rng.fill(&mut header[13..36]).map_err(|_| io::Error::other("no randomness"))?;
        header[36..40].copy_from_slice(&(ENC_CHUNK as u32).to_be_bytes());
        let key = aead_key(&derive_key(pass, &header[13..29], iterations));
        inner.write_all(&header)?;
        Ok(EncWriter { inner, key, header, buf: Vec::with_capacity(ENC_CHUNK + TAG_LEN), index: 0 })
    }

    fn seal(&mut self, last: bool) -> io::Result<()> {
        let nonce = chunk_nonce(&self.header[29..36], self.index, last);
        self.key
            .seal_in_place_append_tag(nonce, ring::aead::Aad::from(&self.header[..]), &mut self.buf)
            .map_err(|_| io::Error::other("encryption failed"))?;
        self.inner.write_all(&self.buf)?;
        self.buf.clear();
        self.index = self.index.checked_add(1).ok_or_else(|| io::Error::other("archive too large"))?;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for EncWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = ENC_CHUNK - self.buf.len();
        let n = room.min(data.len());
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == ENC_CHUNK {
            self.seal(false)?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Reader for the `.enc` format. A wrong passphrase fails on the first chunk.
pub struct DecReader<R: Read> {
    inner: R,
    key: ring::aead::LessSafeKey,
    header: [u8; ENC_HEADER_LEN],
    chunk: usize,
    index: u32,
    plain: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> DecReader<R> {
    /// `inner` starts right after the 8 magic bytes are known to match (they are re-read here).
    pub fn new(mut inner: R, pass: &str) -> io::Result<Self> {
        let mut header = [0u8; ENC_HEADER_LEN];
        inner.read_exact(&mut header)?;
        if &header[..8] != ENC_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not an encrypted backup"));
        }
        if header[8] != 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown key derivation"));
        }
        let iterations = u32::from_be_bytes(header[9..13].try_into().unwrap());
        let chunk = u32::from_be_bytes(header[36..40].try_into().unwrap()) as usize;
        if chunk == 0 || chunk > 64 * 1024 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"));
        }
        let key = aead_key(&derive_key(pass, &header[13..29], iterations));
        Ok(DecReader { inner, key, header, chunk, index: 0, plain: Vec::new(), pos: 0, done: false })
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut buf = vec![0u8; self.chunk + TAG_LEN];
        let mut filled = 0;
        while filled < buf.len() {
            let k = self.inner.read(&mut buf[filled..])?;
            if k == 0 {
                break;
            }
            filled += k;
        }
        if filled < TAG_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "encrypted backup is truncated"));
        }
        let last = filled < buf.len();
        buf.truncate(filled);
        let nonce = chunk_nonce(&self.header[29..36], self.index, last);
        let plain_len = self
            .key
            .open_in_place(nonce, ring::aead::Aad::from(&self.header[..]), &mut buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "wrong passphrase or damaged backup"))?
            .len();
        buf.truncate(plain_len);
        self.plain = buf;
        self.pos = 0;
        self.index += 1;
        self.done = last;
        Ok(())
    }
}

impl<R: Read> Read for DecReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.plain.len() {
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let n = out.len().min(self.plain.len() - self.pos);
        out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

// ===================================================================== archive

/// Counts and hashes the plain tar bytes on their way to the compressor.
struct HashWriter<W: Write> {
    inner: W,
    ctx: ring::digest::Context,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(b)?;
        self.ctx.update(&b[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The archive file written by [`build_archive`].
#[derive(Debug, Clone)]
pub struct Built {
    pub path: PathBuf,
    pub bytes: u64,
    /// SHA-256 of the uncompressed tar.
    pub sha256: String,
    pub files: u64,
    pub encrypted: bool,
}

/// (archive path, source path, is_dir) of everything to back up, sorted.
fn collect(src: &Sources) -> Vec<(String, PathBuf, bool)> {
    fn walk(dir: &Path, arch: &str, skip_top: &[&str], out: &mut Vec<(String, PathBuf, bool)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut v: Vec<_> = rd.flatten().collect();
        v.sort_by_key(|e| e.file_name());
        for e in v {
            let name = e.file_name().to_string_lossy().to_string();
            if skip_top.contains(&name.as_str()) || name.starts_with(".beluga-restore-") {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let a = format!("{arch}/{name}");
            if ft.is_dir() {
                out.push((a.clone(), e.path(), true));
                walk(&e.path(), &a, &[], out);
            } else if ft.is_file() {
                out.push((a, e.path(), false));
            }
        }
    }
    let mut out = Vec::new();
    if src.data_dir.is_dir() {
        out.push(("data".into(), src.data_dir.clone(), true));
        walk(&src.data_dir, "data", &["mcp", TMP_DIR, STATE_FILE], &mut out);
    }
    if src.memory_dir.is_dir() {
        out.push(("memory".into(), src.memory_dir.clone(), true));
        walk(&src.memory_dir, "memory", &[], &mut out);
    }
    for (name, p) in [("voices.json", &src.voices_json), ("models.json", &src.models_json)] {
        if p.is_file() {
            out.push((name.into(), p.clone(), false));
        }
    }
    out
}

fn mtime_secs(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

/// Write the archive of `src` to `out` (blocking; run it on a blocking thread).
pub fn build_archive(src: &Sources, out: &Path, pass: Option<(&str, u32)>) -> io::Result<Built> {
    enum Sink {
        Plain(io::BufWriter<std::fs::File>),
        Enc(Box<EncWriter<io::BufWriter<std::fs::File>>>),
    }
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            match self {
                Sink::Plain(w) => w.write(b),
                Sink::Enc(w) => w.write(b),
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            match self {
                Sink::Plain(w) => w.flush(),
                Sink::Enc(w) => w.flush(),
            }
        }
    }
    let file = io::BufWriter::with_capacity(256 * 1024, std::fs::File::create(out)?);
    let sink = match pass {
        Some((p, it)) => Sink::Enc(Box::new(EncWriter::new(file, p, it)?)),
        None => Sink::Plain(file),
    };
    let z = zstd::stream::write::Encoder::new(sink, ZSTD_LEVEL)?;
    let hw = HashWriter { inner: z, ctx: ring::digest::Context::new(&ring::digest::SHA256) };
    let mut tar = tar::Builder::new(hw);
    let mut files = 0u64;
    for (arch, path, is_dir) in collect(src) {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let mut h = tar::Header::new_gnu();
        h.set_mtime(mtime_secs(&meta));
        h.set_uid(0);
        h.set_gid(0);
        if is_dir {
            // A directory's own mtime moves whenever an entry comes or goes, and each run creates
            // and removes the temp dir inside `data`: left in, no two archives would ever hash the same.
            h.set_mtime(0);
            h.set_entry_type(tar::EntryType::Directory);
            h.set_mode(0o755);
            h.set_size(0);
            tar.append_data(&mut h, format!("{arch}/"), io::empty())?;
        } else {
            let Ok(mut f) = std::fs::File::open(&path) else { continue };
            // The size is fixed now; a file that grows or shrinks while being read (a journal
            // getting a line) is cut or zero-padded to it so the archive stays valid.
            let size = f.metadata().map(|m| m.len()).unwrap_or(meta.len());
            h.set_entry_type(tar::EntryType::Regular);
            h.set_mode(0o644);
            h.set_size(size);
            let reader = (&mut f).take(size).chain(io::repeat(0)).take(size);
            tar.append_data(&mut h, &arch, reader)?;
            files += 1;
        }
    }
    let hw = tar.into_inner()?;
    let sha = hex(hw.ctx.finish().as_ref());
    let sink = hw.inner.finish()?;
    match sink {
        Sink::Plain(mut w) => w.flush()?,
        Sink::Enc(w) => {
            w.finish()?;
        }
    }
    let bytes = std::fs::metadata(out)?.len();
    Ok(Built { path: out.to_path_buf(), bytes, sha256: sha, files, encrypted: pass.is_some() })
}

/// True when the file starts with the encrypted-backup magic.
pub fn is_encrypted(path: &Path) -> io::Result<bool> {
    let mut m = [0u8; 8];
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut m)?;
    Ok(n == 8 && &m == ENC_MAGIC)
}

/// Unpack an archive into `dest` (blocking). Only `data/`, `memory/`, `voices.json` and
/// `models.json` entries are accepted, and none may escape `dest`. Returns the number of files.
pub fn extract_archive(file: &Path, pass: Option<&str>, dest: &Path) -> Result<u64, String> {
    let enc = is_encrypted(file).map_err(|e| format!("cannot read the archive: {e}"))?;
    let f = io::BufReader::with_capacity(256 * 1024, std::fs::File::open(file).map_err(|e| e.to_string())?);
    let reader: Box<dyn Read> = if enc {
        let Some(p) = pass else { return Err("the backup is encrypted: set BACKUP_PASSPHRASE".into()) };
        let mut d = DecReader::new(f, p).map_err(|e| e.to_string())?;
        // Authenticate the first chunk before anything touches the disk.
        let mut first = Vec::new();
        (&mut d).take(1).read_to_end(&mut first).map_err(|e| e.to_string())?;
        Box::new(io::Cursor::new(first).chain(d))
    } else {
        Box::new(f)
    };
    let z = zstd::stream::read::Decoder::new(reader).map_err(|e| e.to_string())?;
    let mut ar = tar::Archive::new(z);
    ar.set_preserve_mtime(true);
    ar.set_overwrite(true);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut files = 0u64;
    for e in ar.entries().map_err(|e| format!("bad archive: {e}"))? {
        let mut e = e.map_err(|e| format!("bad archive: {e}"))?;
        let p = e.path().map_err(|e| e.to_string())?.to_path_buf();
        let mut comps = p.components();
        let ok_top = matches!(comps.next(), Some(std::path::Component::Normal(t)) if ["data", "memory", "voices.json", "models.json"].contains(&t.to_string_lossy().as_ref()));
        let ok_rest = p.components().all(|c| matches!(c, std::path::Component::Normal(_)));
        if !ok_top || !ok_rest {
            return Err(format!("unexpected path in archive: {}", p.display()));
        }
        let kind = e.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        if !e.unpack_in(dest).map_err(|err| format!("extract {}: {err}", p.display()))? {
            return Err(format!("unsafe path in archive: {}", p.display()));
        }
        if kind.is_file() {
            files += 1;
        }
    }
    Ok(files)
}

// ===================================================================== backup run

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Uploaded { bytes: u64, sha256: String, files: u64, key: String },
    /// Same tar SHA-256 as the stored manifest: nothing uploaded.
    Unchanged { sha256: String },
}

/// One backup: archive, compare with the remote manifest, upload, daily copy, manifest, prune.
pub async fn run_backup(cfg: &BackupConfig) -> Result<Outcome, String> {
    let tmp = cfg.sources.data_dir.join(TMP_DIR);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("temp dir: {e}"))?;
    let out = tmp.join(format!("{}{}", uuid::Uuid::new_v4().simple(), cfg.ext()));
    let r = run_backup_inner(cfg, &out).await;
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

async fn run_backup_inner(cfg: &BackupConfig, out: &Path) -> Result<Outcome, String> {
    let (src, o, pass, it) = (cfg.sources.clone(), out.to_path_buf(), cfg.passphrase.clone(), cfg.kdf_iterations);
    let t0 = Instant::now();
    let built = tokio::task::spawn_blocking(move || build_archive(&src, &o, pass.as_deref().map(|p| (p, it))))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("archive: {e}"))?;
    tracing::info!("backup: archive {} files, {} bytes in {} ms", built.files, built.bytes, t0.elapsed().as_millis());
    let s3 = S3::new(cfg);
    let remote: Option<Value> = s3.get_bytes(&cfg.manifest_key()).await?.and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(m) = &remote {
        if m["sha256"] == built.sha256.as_str() && m["encrypted"] == built.encrypted && m["key"] == cfg.latest_key().as_str() {
            tracing::info!("backup: unchanged, skipped");
            return Ok(Outcome::Unchanged { sha256: built.sha256 });
        }
    }
    let latest = cfg.latest_key();
    s3.put_file(&latest, &built.path).await?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let daily = cfg.daily_key(&today);
    s3.copy(&latest, &daily).await?;
    let manifest = json!({"v": 1, "at": crate::store::now_rfc3339(), "bytes": built.bytes, "sha256": built.sha256,
                          "files": built.files, "encrypted": built.encrypted, "key": latest, "daily": daily});
    s3.put(&cfg.manifest_key(), serde_json::to_vec_pretty(&manifest).unwrap(), "application/json").await?;
    // The other variant of `latest` (encryption switched on or off) would be stale.
    let other = if built.encrypted { format!("{}latest.tar.zst", cfg.prefix) } else { format!("{}latest.tar.zst.enc", cfg.prefix) };
    let _ = s3.delete(&other).await;
    if let Err(e) = prune(&s3, cfg).await {
        tracing::warn!("backup: pruning old dailies failed: {e}");
    }
    tracing::info!("backup: uploaded {} bytes to {} in {} ms", built.bytes, cfg.host(), t0.elapsed().as_millis());
    Ok(Outcome::Uploaded { bytes: built.bytes, sha256: built.sha256, files: built.files, key: latest })
}

/// Keep the newest `keep_days` daily archives.
pub async fn prune(s3: &S3, cfg: &BackupConfig) -> Result<Vec<String>, String> {
    let dprefix = format!("{}daily/", cfg.prefix);
    let mut keys: Vec<String> = s3.list(&dprefix).await?.into_iter().map(|o| o.key).filter(|k| k.ends_with(".tar.zst") || k.ends_with(".tar.zst.enc")).collect();
    keys.sort();
    keys.reverse();
    let old: Vec<String> = keys.into_iter().skip(cfg.keep_days).collect();
    for k in &old {
        s3.delete(k).await?;
    }
    Ok(old)
}

// ===================================================================== restore

#[derive(Debug, Clone, Default)]
pub struct RestoreOpts {
    /// Object key (with or without the `beluga/` prefix); default: the latest backup.
    pub key: Option<String>,
    /// Unpack the archive tree here instead of restoring in place.
    pub to: Option<PathBuf>,
    pub force: bool,
    /// The server is running (in-place restore over a non-empty data dir then needs `force`).
    pub server_running: bool,
}

fn non_empty(p: &Path) -> bool {
    std::fs::read_dir(p).map(|mut r| r.next().is_some()).unwrap_or(false)
}

fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Different filesystem: copy, then remove the source.
    fn copy_all(a: &Path, b: &Path) -> io::Result<()> {
        if a.is_dir() {
            std::fs::create_dir_all(b)?;
            for e in std::fs::read_dir(a)? {
                let e = e?;
                copy_all(&e.path(), &b.join(e.file_name()))?;
            }
        } else {
            std::fs::copy(a, b)?;
        }
        Ok(())
    }
    copy_all(from, to)?;
    if from.is_dir() { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }
}

/// Download, verify and unpack a backup, then swap it in. A failure before the swap leaves the
/// current data untouched; the replaced data is kept next to it as `<name>.before-restore-<time>`.
/// Returns a human-readable report.
pub async fn restore(cfg: &BackupConfig, opts: &RestoreOpts) -> Result<String, String> {
    let src = &cfg.sources;
    if opts.to.is_none() && opts.server_running && non_empty(&src.data_dir) && !opts.force {
        return Err("the server is running and its data folder is not empty: stop the server first, or pass --force".into());
    }
    if let Some(to) = &opts.to {
        if non_empty(to) && !opts.force {
            return Err(format!("{} is not empty (pass --force to replace it)", to.display()));
        }
    }
    let s3 = S3::new(cfg);
    let key = match &opts.key {
        Some(k) if k.starts_with(&cfg.prefix) => k.clone(),
        Some(k) => format!("{}{k}", cfg.prefix),
        None => {
            let m: Option<Value> = s3.get_bytes(&cfg.manifest_key()).await?.and_then(|b| serde_json::from_slice(&b).ok());
            match m.as_ref().and_then(|m| m["key"].as_str()) {
                Some(k) => k.to_string(),
                None => cfg.latest_key(),
            }
        }
    };
    let anchor = opts.to.clone().unwrap_or_else(|| src.data_dir.clone());
    let parent = anchor.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(".beluga-restore-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let r = restore_staged(&s3, cfg, &key, &staging, opts).await;
    let _ = std::fs::remove_dir_all(&staging);
    r
}

async fn restore_staged(s3: &S3, cfg: &BackupConfig, key: &str, staging: &Path, opts: &RestoreOpts) -> Result<String, String> {
    let archive = staging.join("archive");
    let Some(bytes) = s3.get_to_file(key, &archive).await? else { return Err(format!("no backup at {key}")) };
    let tree = staging.join("tree");
    let (a, t, pass) = (archive.clone(), tree.clone(), cfg.passphrase.clone());
    let files = tokio::task::spawn_blocking(move || extract_archive(&a, pass.as_deref(), &t)).await.map_err(|e| e.to_string())??;
    let _ = std::fs::remove_file(&archive);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let src = &cfg.sources;
    let targets: Vec<(PathBuf, PathBuf)> = match &opts.to {
        Some(to) => vec![(tree.clone(), to.clone())],
        None => [("data", &src.data_dir), ("memory", &src.memory_dir), ("voices.json", &src.voices_json), ("models.json", &src.models_json)]
            .into_iter()
            .map(|(n, p)| (tree.join(n), p.clone()))
            .filter(|(s, _)| s.exists())
            .collect(),
    };
    // Swap: move each current target aside, move the restored one in; undo everything on failure.
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut kept = Vec::new();
    for (staged, target) in &targets {
        let aside = if target.exists() {
            let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut old = target.with_file_name(format!("{name}.before-restore-{stamp}"));
            let mut n = 1;
            while old.exists() {
                n += 1;
                old = target.with_file_name(format!("{name}.before-restore-{stamp}-{n}"));
            }
            let empty_dir = target.is_dir() && !non_empty(target);
            let moved = if empty_dir { std::fs::remove_dir(target) } else { std::fs::rename(target, &old) };
            if let Err(e) = moved {
                rollback(&done);
                return Err(format!("cannot move {} aside: {e}", target.display()));
            }
            (!empty_dir).then_some(old)
        } else {
            None
        };
        if let Some(p) = target.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        if let Err(e) = move_path(staged, target) {
            if let Some(old) = &aside {
                let _ = std::fs::rename(old, target);
            }
            rollback(&done);
            return Err(format!("cannot move the restored {} into place: {e}", target.display()));
        }
        if let Some(old) = &aside {
            kept.push(old.display().to_string());
        }
        done.push((target.clone(), aside));
    }
    let mut msg = format!("Restored {key} ({bytes} bytes, {files} files).");
    if !kept.is_empty() {
        msg.push_str(&format!(" Previous data kept at: {}", kept.join(", ")));
    }
    Ok(msg)
}

fn rollback(done: &[(PathBuf, Option<PathBuf>)]) {
    for (target, aside) in done.iter().rev() {
        let _ = if target.is_dir() { std::fs::remove_dir_all(target) } else { std::fs::remove_file(target) };
        if let Some(old) = aside {
            let _ = std::fs::rename(old, target);
        }
    }
}

// ===================================================================== scheduler + status

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Saved {
    last_at: Option<String>,
    last_bytes: Option<u64>,
    last_error: Option<String>,
    last_sha256: Option<String>,
}

#[derive(Default)]
struct Live {
    saved: Saved,
    dirty_since: Option<Instant>,
    last_change: Option<Instant>,
    last_run: Option<Instant>,
    failures: u32,
    running: bool,
    next_at: Option<chrono::DateTime<chrono::Local>>,
}

/// Backup service held by the server: status for `get_backup`, the `backup_now` trigger, and
/// the debounce/nightly schedule.
pub struct Backup {
    cfg: Option<BackupConfig>,
    state_path: PathBuf,
    live: Mutex<Live>,
    kick: tokio::sync::Notify,
    poke: tokio::sync::Notify,
}

impl Backup {
    pub fn new(cfg: Option<BackupConfig>, data_dir: &Path) -> Backup {
        let state_path = data_dir.join(STATE_FILE);
        let saved: Saved = std::fs::read_to_string(&state_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Backup { cfg, state_path, live: Mutex::new(Live { saved, ..Default::default() }), kick: Default::default(), poke: Default::default() }
    }

    pub fn from_config(cfg: &Config) -> Backup {
        Backup::new(BackupConfig::from_config(cfg), &cfg.data_dir)
    }

    pub fn enabled(&self) -> bool {
        self.cfg.is_some()
    }

    pub fn config(&self) -> Option<&BackupConfig> {
        self.cfg.as_ref()
    }

    /// Replicated data changed: schedule a debounced backup.
    pub fn mark_dirty(&self) {
        if self.cfg.is_none() {
            return;
        }
        let mut l = self.live.lock().unwrap();
        let now = Instant::now();
        l.dirty_since.get_or_insert(now);
        l.last_change = Some(now);
        drop(l);
        self.poke.notify_one();
    }

    /// `backup_now`: run as soon as possible (ignores the debounce and the minimum interval).
    pub fn kick(&self) {
        if self.cfg.is_some() {
            self.kick.notify_one();
        }
    }

    /// The `backup` frame.
    pub fn frame(&self) -> Value {
        let l = self.live.lock().unwrap();
        let mut v = json!({"t": "backup", "enabled": self.cfg.is_some(), "provider": "r2", "running": l.running});
        if let Some(a) = &l.saved.last_at {
            v["lastAt"] = json!(a);
        }
        if let Some(b) = l.saved.last_bytes {
            v["lastBytes"] = json!(b);
        }
        if let Some(e) = &l.saved.last_error {
            v["lastError"] = json!(e);
        }
        if let (Some(n), true) = (l.next_at, self.cfg.is_some()) {
            v["nextAt"] = json!(n.to_rfc3339_opts(chrono::SecondsFormat::Secs, false));
        }
        v
    }

    fn save(&self, s: &Saved) {
        if let Ok(t) = serde_json::to_string_pretty(s) {
            let tmp = self.state_path.with_extension("json.tmp");
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(&tmp, &self.state_path);
            }
        }
    }

    /// When the next automatic backup is due (and whether it is the nightly one).
    fn next_due(&self, cfg: &BackupConfig) -> (Instant, chrono::DateTime<chrono::Local>) {
        let now_i = Instant::now();
        let now = chrono::Local::now();
        let nightly = next_local_hour(now, cfg.nightly_hour);
        let to_nightly = (nightly - now).to_std().unwrap_or_default();
        let mut due = now_i + to_nightly;
        let l = self.live.lock().unwrap();
        if let (Some(since), Some(last)) = (l.dirty_since, l.last_change) {
            let mut d = (last + cfg.debounce).min(since + cfg.min_interval);
            if let Some(run) = l.last_run {
                let backoff = cfg.min_interval * 2u32.saturating_pow(l.failures.min(5));
                d = d.max(run + backoff.min(Duration::from_secs(6 * 3600)));
            }
            due = due.min(d);
        }
        let at = now + chrono::Duration::from_std(due.saturating_duration_since(now_i)).unwrap_or_default();
        (due, at)
    }

    /// One run with status bookkeeping; returns the new `backup` frame.
    pub async fn run_now(&self) -> Value {
        let Some(cfg) = self.cfg.clone() else { return self.frame() };
        let started = Instant::now();
        {
            let mut l = self.live.lock().unwrap();
            if l.running {
                drop(l);
                return self.frame();
            }
            l.running = true;
        }
        let r = run_backup(&cfg).await;
        let saved = {
            let mut l = self.live.lock().unwrap();
            l.running = false;
            l.last_run = Some(Instant::now());
            match &r {
                Ok(o) => {
                    l.failures = 0;
                    l.saved.last_at = Some(crate::store::now_rfc3339());
                    l.saved.last_error = None;
                    match o {
                        Outcome::Uploaded { bytes, sha256, .. } => {
                            l.saved.last_bytes = Some(*bytes);
                            l.saved.last_sha256 = Some(sha256.clone());
                        }
                        Outcome::Unchanged { sha256 } => l.saved.last_sha256 = Some(sha256.clone()),
                    }
                    // Changes that came in while this run was going stay pending.
                    if l.last_change.map(|c| c <= started).unwrap_or(true) {
                        l.dirty_since = None;
                        l.last_change = None;
                    }
                }
                Err(e) => {
                    tracing::warn!("backup failed: {e}");
                    l.failures += 1;
                    l.saved.last_error = Some(e.chars().take(300).collect());
                    l.dirty_since.get_or_insert(started);
                    l.last_change.get_or_insert(started);
                }
            }
            l.saved.clone()
        };
        self.save(&saved);
        let (_, at) = self.next_due(&cfg);
        self.live.lock().unwrap().next_at = Some(at);
        self.frame()
    }
}

/// The next `hour:00` local time strictly after `now`.
pub fn next_local_hour(now: chrono::DateTime<chrono::Local>, hour: u32) -> chrono::DateTime<chrono::Local> {
    use chrono::{Duration as D, TimeZone};
    let mut day = now.date_naive();
    for _ in 0..3 {
        if let Some(t) = day.and_hms_opt(hour.min(23), 0, 0).and_then(|t| chrono::Local.from_local_datetime(&t).earliest()) {
            if t > now {
                return t;
            }
        }
        day += D::days(1);
    }
    now + D::days(1)
}

/// Run the schedule (only when backup is configured): nightly, debounced after changes, and
/// on `backup_now`. Every run ends with a `backup` frame to every phone.
pub fn spawn(st: Arc<AppState>) {
    let Some(cfg) = st.backup.config().cloned() else { return };
    tracing::info!(
        "backup: on → {} bucket {} ({}), keep {} dailies, nightly at {:02}:00",
        cfg.host(),
        cfg.bucket,
        if cfg.passphrase.is_some() { "encrypted" } else { "not encrypted" },
        cfg.keep_days,
        cfg.nightly_hour
    );
    tokio::spawn(async move {
        loop {
            let (due, at) = st.backup.next_due(&cfg);
            st.backup.live.lock().unwrap().next_at = Some(at);
            tokio::select! {
                _ = tokio::time::sleep_until(due.into()) => {}
                _ = st.backup.kick.notified() => {}
                _ = st.backup.poke.notified() => continue,
            }
            let f = st.backup.run_now().await;
            st.broadcast(&f);
        }
    });
}

// ===================================================================== CLI

async fn server_running(port: u16) -> bool {
    let c = reqwest::Client::builder().timeout(Duration::from_millis(1500)).build().unwrap_or_default();
    matches!(c.get(format!("http://127.0.0.1:{port}/health")).send().await, Ok(r) if r.status().is_success())
}

fn usage_off() -> i32 {
    eprintln!("Sao lưu đang tắt: cần R2_ACCOUNT_ID (hoặc R2_ENDPOINT), R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_BUCKET trong server/.env.");
    2
}

/// `backup-now`, `backup-status`, `restore [--key K] [--to DIR] [--force]`; `None` for other commands.
pub async fn cli(cfg: &Config, args: &[String]) -> Option<i32> {
    let cmd = args.first()?.as_str();
    if !matches!(cmd, "backup-now" | "backup-status" | "restore") {
        return None;
    }
    let svc = Backup::from_config(cfg);
    let Some(bc) = svc.config().cloned() else { return Some(usage_off()) };
    Some(match cmd {
        "backup-now" => {
            println!("Sao lưu lên {} / {} ({})…", bc.host(), bc.bucket, if bc.passphrase.is_some() { "mã hóa" } else { "không mã hóa" });
            let f = svc.run_now().await;
            match f["lastError"].as_str() {
                Some(e) => {
                    eprintln!("Lỗi: {e}");
                    1
                }
                None => {
                    println!("Xong: {} ({} byte).", f["lastAt"].as_str().unwrap_or(""), f["lastBytes"]);
                    0
                }
            }
        }
        "backup-status" => {
            println!("Bật: {} / bucket {} · mã hóa: {} · giữ {} bản daily", bc.host(), bc.bucket, if bc.passphrase.is_some() { "có" } else { "không" }, bc.keep_days);
            println!("Trạng thái máy này: {}", svc.frame());
            let s3 = S3::new(&bc);
            match s3.get_bytes(&bc.manifest_key()).await {
                Ok(Some(b)) => println!("manifest.json: {}", String::from_utf8_lossy(&b)),
                Ok(None) => println!("Chưa có bản sao lưu nào trên bucket."),
                Err(e) => {
                    eprintln!("Không đọc được bucket: {e}");
                    return Some(1);
                }
            }
            match s3.list(&bc.prefix).await {
                Ok(objs) => {
                    for o in objs {
                        println!("  {}  {} byte", o.key, o.size);
                    }
                    0
                }
                Err(e) => {
                    eprintln!("Không liệt kê được: {e}");
                    1
                }
            }
        }
        _ => {
            let mut opts = RestoreOpts::default();
            let mut it = args.iter().skip(1);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--key" => opts.key = it.next().cloned(),
                    "--to" => opts.to = it.next().map(PathBuf::from),
                    "--force" => opts.force = true,
                    _ => {}
                }
            }
            opts.server_running = opts.to.is_none() && server_running(cfg.port).await;
            println!("Khôi phục từ {} / {}…", bc.host(), bc.bucket);
            match restore(&bc, &opts).await {
                Ok(m) => {
                    println!("{m}");
                    0
                }
                Err(e) => {
                    eprintln!("Không khôi phục: {e}. Dữ liệu hiện tại giữ nguyên.");
                    1
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_round_trips_any_length_and_rejects_tampering() {
        for len in [0usize, 1, ENC_CHUNK - 1, ENC_CHUNK, ENC_CHUNK + 5, 2 * ENC_CHUNK] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let mut w = EncWriter::new(Vec::new(), "pw", 10).unwrap();
            for c in data.chunks(70_000) {
                w.write_all(c).unwrap();
            }
            let enc = w.finish().unwrap();
            assert_eq!(&enc[..8], ENC_MAGIC);
            let mut out = Vec::new();
            DecReader::new(&enc[..], "pw").unwrap().read_to_end(&mut out).unwrap();
            assert_eq!(out, data, "len {len}");
            let mut bad = Vec::new();
            assert!(DecReader::new(&enc[..], "nope").unwrap().read_to_end(&mut bad).is_err());
            if len >= ENC_CHUNK {
                // dropping the final chunk is detected
                let cut = &enc[..ENC_HEADER_LEN + ENC_CHUNK + TAG_LEN];
                assert!(DecReader::new(cut, "pw").unwrap().read_to_end(&mut Vec::new()).is_err(), "truncation at {len}");
            }
            let mut flipped = enc.clone();
            flipped[20] ^= 1; // salt changed: different key and AAD
            assert!(DecReader::new(&flipped[..], "pw").unwrap().read_to_end(&mut Vec::new()).is_err());
        }
    }

    #[test]
    fn uri_encoding_and_xml() {
        assert_eq!(uri_encode("beluga/daily/2026-11-15.tar.zst", true), "beluga/daily/2026-11-15.tar.zst");
        assert_eq!(uri_encode("a b/c", false), "a%20b%2Fc");
        assert_eq!(xml_all("<a><K>x&amp;y</K><K>z</K></a>", "K"), vec!["x&y", "z"]);
    }

    #[test]
    fn nightly_is_the_next_three_am() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 15, 2, 30, 0).unwrap();
        assert_eq!(next_local_hour(now, 3), chrono::Local.with_ymd_and_hms(2026, 11, 15, 3, 0, 0).unwrap());
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 15, 3, 0, 0).unwrap();
        assert_eq!(next_local_hour(now, 3), chrono::Local.with_ymd_and_hms(2026, 11, 16, 3, 0, 0).unwrap());
    }
}
