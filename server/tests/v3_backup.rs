//! Backup to R2 (PROTOCOL section 20) against a fake S3 server that checks every SigV4
//! signature with its own implementation and keeps objects in memory.

mod common;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use beluga_server::backup::{self, BackupConfig, Outcome, RestoreOpts, Sources};
use beluga_server::config::Config;
use beluga_server::http::start_full;
use beluga_server::util_llm::NoUtil;
use beluga_server::voice::Voice;
use common::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

const AK: &str = "AKTEST";
const SK: &str = "secret-test-key";
const BUCKET: &str = "beluga-bk";

// ----------------------------------------------------------------- fake S3

#[derive(Default)]
struct Fake {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    uploads: Mutex<HashMap<String, BTreeMap<u32, Vec<u8>>>>,
    /// (method, key, query) of every accepted request.
    log: Mutex<Vec<(String, String, String)>>,
    rejected: Mutex<u32>,
    completed_multipart: Mutex<u32>,
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha(b: &[u8]) -> String {
    hexs(ring::digest::digest(&ring::digest::SHA256, b).as_ref())
}

fn mac(k: &[u8], m: &str) -> Vec<u8> {
    ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, k), m.as_bytes()).as_ref().to_vec()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn strict_encode(s: &str) -> String {
    s.bytes()
        .map(|c| if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") })
        .collect()
}

/// Independent SigV4 check of a received request.
fn verify(method: &Method, uri: &Uri, h: &HeaderMap, body: &[u8]) -> Result<(), String> {
    let auth = h.get("authorization").and_then(|v| v.to_str().ok()).ok_or("no auth")?;
    let rest = auth.strip_prefix("AWS4-HMAC-SHA256 ").ok_or("bad scheme")?;
    let mut parts = HashMap::new();
    for p in rest.split(", ") {
        let (k, v) = p.split_once('=').ok_or("bad auth part")?;
        parts.insert(k, v);
    }
    let cred: Vec<&str> = parts["Credential"].split('/').collect();
    if cred[0] != AK || cred[2] != "auto" || cred[3] != "s3" || cred[4] != "aws4_request" {
        return Err(format!("bad credential {cred:?}"));
    }
    let payload = h.get("x-amz-content-sha256").and_then(|v| v.to_str().ok()).ok_or("no payload hash")?;
    if payload != "UNSIGNED-PAYLOAD" && payload != sha(body) {
        return Err("payload hash mismatch".into());
    }
    let mut q: Vec<(String, String)> = uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (strict_encode(&pct_decode(k)), strict_encode(&pct_decode(v)))
        })
        .collect();
    q.sort();
    let cq = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let signed = parts["SignedHeaders"];
    if !signed.split(';').any(|x| x == "host") {
        return Err("host not signed".into());
    }
    let ch: String = signed.split(';').map(|n| format!("{n}:{}\n", h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").trim())).collect();
    let canonical = format!("{method}\n{}\n{cq}\n{ch}\n{signed}\n{payload}", uri.path());
    let amz = h.get("x-amz-date").and_then(|v| v.to_str().ok()).ok_or("no date")?;
    let scope = format!("{}/auto/s3/aws4_request", cred[1]);
    let sts = format!("AWS4-HMAC-SHA256\n{amz}\n{scope}\n{}", sha(canonical.as_bytes()));
    let mut k = mac(format!("AWS4{SK}").as_bytes(), cred[1]);
    for x in ["auto", "s3", "aws4_request"] {
        k = mac(&k, x);
    }
    if hexs(&mac(&k, &sts)) != parts["Signature"] {
        return Err("SignatureDoesNotMatch".into());
    }
    Ok(())
}

fn xml_err(code: StatusCode, c: &str) -> Response {
    (code, format!("<Error><Code>{c}</Code></Error>")).into_response()
}

async fn s3_handler(State(f): State<Arc<Fake>>, method: Method, uri: Uri, h: HeaderMap, body: Bytes) -> Response {
    if let Err(e) = verify(&method, &uri, &h, &body) {
        *f.rejected.lock().unwrap() += 1;
        return xml_err(StatusCode::FORBIDDEN, &format!("SignatureDoesNotMatch {e}"));
    }
    let path = pct_decode(uri.path());
    let query: HashMap<String, String> = uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect();
    let rest = path.trim_start_matches('/');
    let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
    if bucket != BUCKET {
        return xml_err(StatusCode::NOT_FOUND, "NoSuchBucket");
    }
    f.log.lock().unwrap().push((method.to_string(), key.to_string(), uri.query().unwrap_or("").to_string()));
    match (method.as_str(), key.is_empty()) {
        ("GET", true) => {
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            let objs = f.objects.lock().unwrap();
            let items: String = objs
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, v)| format!("<Contents><Key>{k}</Key><Size>{}</Size></Contents>", v.len()))
                .collect();
            format!("<?xml version=\"1.0\"?><ListBucketResult><IsTruncated>false</IsTruncated>{items}</ListBucketResult>").into_response()
        }
        ("PUT", false) if query.contains_key("uploadId") => {
            let n: u32 = query["partNumber"].parse().unwrap();
            let mut ups = f.uploads.lock().unwrap();
            let Some(u) = ups.get_mut(&query["uploadId"]) else { return xml_err(StatusCode::NOT_FOUND, "NoSuchUpload") };
            u.insert(n, body.to_vec());
            ([("etag", format!("\"{}\"", &sha(&body)[..16]))], "").into_response()
        }
        ("PUT", false) => {
            if let Some(src) = h.get("x-amz-copy-source").and_then(|v| v.to_str().ok()) {
                let src = pct_decode(src);
                let src_key = src.trim_start_matches('/').split_once('/').map(|x| x.1.to_string()).unwrap_or_default();
                let mut objs = f.objects.lock().unwrap();
                let Some(data) = objs.get(&src_key).cloned() else { return xml_err(StatusCode::NOT_FOUND, "NoSuchKey") };
                objs.insert(key.to_string(), data);
                return "<CopyObjectResult><ETag>\"x\"</ETag></CopyObjectResult>".into_response();
            }
            f.objects.lock().unwrap().insert(key.to_string(), body.to_vec());
            ([("etag", "\"x\"")], "").into_response()
        }
        ("POST", false) if query.contains_key("uploads") => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            f.uploads.lock().unwrap().insert(id.clone(), BTreeMap::new());
            format!("<InitiateMultipartUploadResult><UploadId>{id}</UploadId></InitiateMultipartUploadResult>").into_response()
        }
        ("POST", false) if query.contains_key("uploadId") => {
            let Some(parts) = f.uploads.lock().unwrap().remove(&query["uploadId"]) else { return xml_err(StatusCode::NOT_FOUND, "NoSuchUpload") };
            let text = String::from_utf8_lossy(&body);
            let listed = text.matches("<PartNumber>").count();
            if listed != parts.len() || parts.keys().copied().ne(1..=parts.len() as u32) {
                return xml_err(StatusCode::BAD_REQUEST, "InvalidPart");
            }
            let sizes: Vec<usize> = parts.values().map(|p| p.len()).collect();
            if sizes[..sizes.len() - 1].iter().any(|s| *s != sizes[0]) {
                return xml_err(StatusCode::BAD_REQUEST, "PartsNotEqual");
            }
            let data: Vec<u8> = parts.into_values().flatten().collect();
            f.objects.lock().unwrap().insert(key.to_string(), data);
            *f.completed_multipart.lock().unwrap() += 1;
            "<CompleteMultipartUploadResult><ETag>\"x\"</ETag></CompleteMultipartUploadResult>".into_response()
        }
        ("GET", false) => match f.objects.lock().unwrap().get(key) {
            Some(d) => d.clone().into_response(),
            None => xml_err(StatusCode::NOT_FOUND, "NoSuchKey"),
        },
        ("DELETE", false) => {
            if let Some(id) = query.get("uploadId") {
                f.uploads.lock().unwrap().remove(id);
            } else {
                f.objects.lock().unwrap().remove(key);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        _ => xml_err(StatusCode::BAD_REQUEST, "NotImplemented"),
    }
}

async fn fake_s3() -> (Arc<Fake>, String) {
    let f = Arc::new(Fake::default());
    let app = axum::Router::new().fallback(s3_handler).with_state(f.clone());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (f, format!("http://{addr}"))
}

// ----------------------------------------------------------------- helpers

/// A repo-like tree: `server/data`, `tutor/memory`, `server/voices.json`, `server/models.json`.
fn make_tree(root: &Path) -> Sources {
    let data = root.join("server").join("data");
    let mem = root.join("tutor").join("memory");
    let w = |p: PathBuf, b: &[u8]| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    };
    w(data.join("pair.json"), br#"{"code":"424242","tokens":["t1"]}"#);
    w(data.join("sessions.json"), "[{\"sessionId\":\"s1\",\"title\":\"Toán · Căn bậc hai\"}]".as_bytes());
    w(data.join("sessions").join("s1.jsonl"), b"{\"t\":\"user\",\"seq\":1}\n{\"t\":\"turn_end\",\"seq\":2}\n");
    w(data.join("folders").join("f1").join("knowledge.json"), "{\"v\":1,\"title\":\"Đề ôn giữa kỳ\"}".as_bytes());
    w(data.join("folders").join("f1").join("exams").join("ex-1.json"), b"{\"examId\":\"ex-1\"}");
    w(data.join("mcp").join("s1.json"), b"{\"secret\":\"not backed up\"}");
    let noise: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    w(data.join("uploads").join("u1.jpg"), &noise);
    w(data.join("uploads").join("empty.bin"), b"");
    std::fs::create_dir_all(data.join("study")).unwrap();
    w(mem.join("profile.md"), "# Hồ sơ học sinh\n- [cách] Thích hình vẽ\n".as_bytes());
    w(mem.join("folders").join("f1.md"), "# Trí nhớ · folder f1\n- [yếu] Rút gọn căn | sai 2\n".as_bytes());
    w(root.join("server").join("voices.json"), br#"{"voices":[]}"#);
    w(root.join("server").join("models.json"), br#"[{"id":"sonnet"}]"#);
    Sources {
        data_dir: data,
        memory_dir: mem,
        voices_json: root.join("server").join("voices.json"),
        models_json: root.join("server").join("models.json"),
    }
}

fn bcfg(endpoint: &str, src: Sources, pass: Option<&str>) -> BackupConfig {
    BackupConfig {
        endpoint: endpoint.into(),
        bucket: BUCKET.into(),
        access_key: AK.into(),
        secret_key: SK.into(),
        region: "auto".into(),
        prefix: "beluga/".into(),
        passphrase: pass.map(String::from),
        keep_days: 14,
        kdf_iterations: 1000,
        multipart_threshold: backup::MULTIPART_THRESHOLD,
        part_size: backup::PART_SIZE,
        debounce: Duration::from_secs(120),
        min_interval: Duration::from_secs(600),
        nightly_hour: 3,
        sources: src,
    }
}

/// relative path -> bytes for every file under the backed-up locations (minus excluded ones).
fn snapshot(s: &Sources) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, tag: &str, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if rel.starts_with("mcp") || rel == "backup.json" || rel.starts_with(".backup-tmp") {
                continue;
            }
            if p.is_dir() {
                out.insert(format!("{tag}/{rel}/"), vec![]);
                walk(base, tag, &p, out);
            } else {
                out.insert(format!("{tag}/{rel}"), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&s.data_dir, "data", &s.data_dir, &mut out);
    walk(&s.memory_dir, "memory", &s.memory_dir, &mut out);
    for p in [&s.voices_json, &s.models_json] {
        if let Ok(b) = std::fs::read(p) {
            out.insert(p.file_name().unwrap().to_string_lossy().to_string(), b);
        }
    }
    out
}

fn wipe(s: &Sources) {
    let _ = std::fs::remove_dir_all(&s.data_dir);
    let _ = std::fs::remove_dir_all(&s.memory_dir);
    let _ = std::fs::remove_file(&s.voices_json);
    let _ = std::fs::remove_file(&s.models_json);
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn no_leftovers(s: &Sources) {
    let parent = s.data_dir.parent().unwrap();
    let left: Vec<String> = std::fs::read_dir(parent).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with(".beluga-restore-")).collect();
    assert!(left.is_empty(), "staging left behind: {left:?}");
    assert!(!s.data_dir.join(".backup-tmp").exists(), "temp archive left behind");
}

// ----------------------------------------------------------------- tests

#[tokio::test]
async fn full_round_trip_restores_a_byte_identical_tree() {
    let (fake, ep) = fake_s3().await;
    let root = temp_dir();
    let src = make_tree(&root);
    let before = snapshot(&src);
    let cfg = bcfg(&ep, src.clone(), None);

    let out = backup::run_backup(&cfg).await.unwrap();
    let Outcome::Uploaded { bytes, files, key, .. } = out else { panic!("{out:?}") };
    assert_eq!(key, "beluga/latest.tar.zst");
    assert_eq!(files, 11, "every file except mcp/");
    {
        let objs = fake.objects.lock().unwrap();
        assert_eq!(objs["beluga/latest.tar.zst"].len() as u64, bytes);
        assert_eq!(objs[&format!("beluga/daily/{}.tar.zst", today())], objs["beluga/latest.tar.zst"], "daily is a copy of latest");
        let m: serde_json::Value = serde_json::from_slice(&objs["beluga/manifest.json"]).unwrap();
        assert_eq!((m["bytes"].as_u64(), m["files"].as_u64(), m["encrypted"].as_bool()), (Some(bytes), Some(11), Some(false)));
        assert_eq!(m["sha256"].as_str().unwrap().len(), 64);
        assert!(m["at"].is_string());
        // the daily copy was server side: exactly one upload of the archive bytes
        let puts = fake.log.lock().unwrap().iter().filter(|(m, k, _)| m == "PUT" && k.ends_with(".tar.zst")).count();
        assert_eq!(puts, 2, "one PUT of latest + one copy request");
    }
    assert_eq!(*fake.rejected.lock().unwrap(), 0);
    no_leftovers(&src);

    wipe(&src);
    let msg = backup::restore(&cfg, &RestoreOpts::default()).await.unwrap();
    assert!(msg.contains("11 files"), "{msg}");
    assert_eq!(snapshot(&src), before, "restored tree is byte-identical");
    assert!(!src.data_dir.join("mcp").exists(), "mcp/ is not part of the backup");
    no_leftovers(&src);
}

#[tokio::test]
async fn encrypted_round_trip_and_a_wrong_passphrase_touches_nothing() {
    let (fake, ep) = fake_s3().await;
    let root = temp_dir();
    let src = make_tree(&root);
    let cfg = bcfg(&ep, src.clone(), Some("đúng rồi"));
    let Outcome::Uploaded { key, .. } = backup::run_backup(&cfg).await.unwrap() else { panic!() };
    assert_eq!(key, "beluga/latest.tar.zst.enc");
    {
        let objs = fake.objects.lock().unwrap();
        let obj = &objs["beluga/latest.tar.zst.enc"];
        assert_eq!(&obj[..8], b"BLGENC01");
        assert!(objs.contains_key(&format!("beluga/daily/{}.tar.zst.enc", today())));
        let m: serde_json::Value = serde_json::from_slice(&objs["beluga/manifest.json"]).unwrap();
        assert_eq!(m["encrypted"], true);
    }

    // newer local data that a failed restore must not touch
    std::fs::write(src.memory_dir.join("profile.md"), "mới hơn bản sao lưu").unwrap();
    let current = snapshot(&src);
    let wrong = bcfg(&ep, src.clone(), Some("sai"));
    let e = backup::restore(&wrong, &RestoreOpts::default()).await.unwrap_err();
    assert!(e.contains("wrong passphrase"), "{e}");
    let none = bcfg(&ep, src.clone(), None);
    let e = backup::restore(&none, &RestoreOpts { key: Some("latest.tar.zst.enc".into()), ..Default::default() }).await.unwrap_err();
    assert!(e.contains("encrypted"), "{e}");
    assert_eq!(snapshot(&src), current, "failed restores left the data alone");
    no_leftovers(&src);

    // the right passphrase restores, keeping what it replaced next to it
    let msg = backup::restore(&cfg, &RestoreOpts::default()).await.unwrap();
    assert!(msg.contains("before-restore"), "{msg}");
    let restored = std::fs::read_to_string(src.memory_dir.join("profile.md")).unwrap();
    assert!(restored.contains("Thích hình vẽ"));
    let kept: Vec<_> = std::fs::read_dir(src.memory_dir.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("memory.before-restore-")).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read_to_string(kept[0].path().join("profile.md")).unwrap(), "mới hơn bản sao lưu");
}

#[tokio::test]
async fn large_archives_go_up_in_equal_multipart_parts() {
    let (fake, ep) = fake_s3().await;
    let root = temp_dir();
    let src = make_tree(&root);
    // ~1.2 MB of incompressible bytes
    let mut x: u64 = 0x9E3779B97F4A7C15;
    let big: Vec<u8> = (0..1_200_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(src.data_dir.join("uploads").join("big.pdf"), &big).unwrap();
    let before = snapshot(&src);
    let mut cfg = bcfg(&ep, src.clone(), Some("pw"));
    cfg.multipart_threshold = 256 * 1024;
    cfg.part_size = 200 * 1024;
    let Outcome::Uploaded { bytes, .. } = backup::run_backup(&cfg).await.unwrap() else { panic!() };
    assert!(bytes > 1_200_000);
    assert_eq!(*fake.completed_multipart.lock().unwrap(), 1);
    let parts = fake.log.lock().unwrap().iter().filter(|(m, _, q)| m == "PUT" && q.contains("partNumber")).count();
    assert_eq!(parts as u64, bytes.div_ceil(200 * 1024));
    wipe(&src);
    backup::restore(&cfg, &RestoreOpts::default()).await.unwrap();
    assert_eq!(snapshot(&src), before);
}

#[tokio::test]
async fn unchanged_data_is_skipped_and_a_change_uploads_again() {
    let (fake, ep) = fake_s3().await;
    let root = temp_dir();
    let src = make_tree(&root);
    for pass in [None, Some("pw")] {
        let cfg = bcfg(&ep, src.clone(), pass);
        let first = backup::run_backup(&cfg).await.unwrap();
        assert!(matches!(first, Outcome::Uploaded { .. }), "{first:?}");
        let puts = || fake.log.lock().unwrap().iter().filter(|(m, _, _)| m == "PUT").count();
        let n = puts();
        // Past a whole second: the temp dir each run makes inside `data` moves that directory's mtime.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let again = backup::run_backup(&cfg).await.unwrap();
        assert!(matches!(again, Outcome::Unchanged { .. }), "same data, even encrypted: {again:?}");
        assert_eq!(puts(), n, "nothing uploaded");
        std::fs::write(src.memory_dir.join("folders").join("f1.md"), format!("đổi {pass:?}")).unwrap();
        assert!(matches!(backup::run_backup(&cfg).await.unwrap(), Outcome::Uploaded { .. }));
        assert!(puts() > n);
    }
    // switching encryption on removed the stale plain `latest`
    let objs = fake.objects.lock().unwrap();
    assert!(objs.contains_key("beluga/latest.tar.zst.enc") && !objs.contains_key("beluga/latest.tar.zst"));
}

#[tokio::test]
async fn retention_keeps_the_newest_dailies() {
    let (fake, ep) = fake_s3().await;
    let root = temp_dir();
    let src = make_tree(&root);
    {
        let mut objs = fake.objects.lock().unwrap();
        for d in ["2026-01-01", "2026-01-02", "2026-01-03", "2026-01-04"] {
            objs.insert(format!("beluga/daily/{d}.tar.zst"), b"old".to_vec());
        }
        objs.insert("beluga/daily/notes.txt".into(), b"not a backup".to_vec());
        objs.insert("other/daily/2020-01-01.tar.zst".into(), b"other app".to_vec());
    }
    let mut cfg = bcfg(&ep, src.clone(), None);
    cfg.keep_days = 3;
    backup::run_backup(&cfg).await.unwrap();
    let objs = fake.objects.lock().unwrap();
    let dailies: Vec<&String> = objs.keys().filter(|k| k.starts_with("beluga/daily/") && k.ends_with(".tar.zst")).collect();
    let today_key = format!("beluga/daily/{}.tar.zst", today());
    assert_eq!(dailies, vec![&"beluga/daily/2026-01-03.tar.zst".to_string(), &"beluga/daily/2026-01-04.tar.zst".to_string(), &today_key]);
    assert!(objs.contains_key("beluga/daily/notes.txt") && objs.contains_key("other/daily/2020-01-01.tar.zst"), "only our archives are pruned");
}

#[tokio::test]
async fn a_wrong_secret_is_rejected_by_the_signature_check() {
    let (fake, ep) = fake_s3().await;
    let root = temp_dir();
    let mut cfg = bcfg(&ep, make_tree(&root), None);
    cfg.secret_key = "wrong".into();
    let e = backup::run_backup(&cfg).await.unwrap_err();
    assert!(e.contains("403"), "{e}");
    assert!(*fake.rejected.lock().unwrap() > 0);
    assert!(fake.objects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn restore_refuses_a_running_server_with_data_unless_forced_and_can_unpack_elsewhere() {
    let (_fake, ep) = fake_s3().await;
    let root = temp_dir();
    let src = make_tree(&root);
    let cfg = bcfg(&ep, src.clone(), None);
    backup::run_backup(&cfg).await.unwrap();
    let before = snapshot(&src);
    let running = RestoreOpts { server_running: true, ..Default::default() };
    let e = backup::restore(&cfg, &running).await.unwrap_err();
    assert!(e.contains("--force"), "{e}");
    assert_eq!(snapshot(&src), before);
    backup::restore(&cfg, &RestoreOpts { force: true, ..running }).await.unwrap();
    assert_eq!(snapshot(&src), before);

    let to = root.join("elsewhere");
    backup::restore(&cfg, &RestoreOpts { to: Some(to.clone()), ..Default::default() }).await.unwrap();
    assert!(to.join("data").join("sessions").join("s1.jsonl").exists() && to.join("memory").join("profile.md").exists() && to.join("models.json").exists());
    let e = backup::restore(&cfg, &RestoreOpts { to: Some(to.clone()), ..Default::default() }).await.unwrap_err();
    assert!(e.contains("not empty"), "{e}");
    // a missing key fails before anything moves
    let e = backup::restore(&cfg, &RestoreOpts { key: Some("daily/1999-01-01.tar.zst".into()), force: true, ..Default::default() }).await.unwrap_err();
    assert!(e.contains("no backup"), "{e}");
    assert_eq!(snapshot(&src), before);
    no_leftovers(&src);
}

async fn serve(cfg: Config) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = start_full(cfg, listener, Voice::none(), Arc::new(NoUtil), None).await;
    });
    addr
}

#[tokio::test]
async fn missing_env_means_disabled_without_errors() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let cfg = Config::for_test(&dir);
    assert!(BackupConfig::from_config(&cfg).is_none());
    let mut partial = cfg.clone();
    partial.secrets.insert("R2_BUCKET".into(), "b".into());
    partial.secrets.insert("R2_ACCOUNT_ID".into(), "acc".into());
    assert!(BackupConfig::from_config(&partial).is_none(), "keys missing: still off");
    let addr = serve(cfg).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({"v": 3})).await;
    p.send(json!({"t": "get_backup"})).await;
    let f = p.until(|x| x["t"] == "backup").await.pop().unwrap();
    assert_eq!((f["enabled"].as_bool(), f["provider"].as_str()), (Some(false), Some("r2")));
    assert!(f.get("lastError").is_none() && f.get("nextAt").is_none());
    p.send(json!({"t": "backup_now"})).await;
    let f = p.until(|x| x["t"] == "backup").await.pop().unwrap();
    assert_eq!(f["enabled"], false);
    assert!(f.get("lastError").is_none());
    assert!(!dir.join("data").join(".backup-tmp").exists());
}

#[tokio::test]
async fn backup_now_over_the_websocket_reports_to_every_phone() {
    let (fake, ep) = fake_s3().await;
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let mut cfg = Config::for_test(&dir);
    for (k, v) in [("R2_ENDPOINT", ep.as_str()), ("R2_ACCOUNT_ID", "ignored-when-endpoint-set"), ("R2_ACCESS_KEY_ID", AK), ("R2_SECRET_ACCESS_KEY", SK), ("R2_BUCKET", BUCKET), ("BACKUP_KEEP_DAYS", "5")] {
        cfg.secrets.insert(k.into(), v.into());
    }
    let b = BackupConfig::from_config(&cfg).unwrap();
    assert_eq!((b.endpoint.as_str(), b.keep_days, b.region.as_str(), b.passphrase.is_none()), (ep.as_str(), 5, "auto", true));
    let mut acc = cfg.clone();
    acc.secrets.remove("R2_ENDPOINT");
    assert_eq!(BackupConfig::from_config(&acc).unwrap().endpoint, "https://ignored-when-endpoint-set.r2.cloudflarestorage.com");

    let addr = serve(cfg).await;
    let mut p = Phone::connect(addr).await;
    p.hello(json!({"v": 3})).await;
    let mut q = Phone::connect(addr).await;
    q.hello(json!({"v": 3})).await;
    p.send(json!({"t": "get_backup"})).await;
    let f = p.until(|x| x["t"] == "backup").await.pop().unwrap();
    assert_eq!(f["enabled"], true);
    assert!(f["nextAt"].is_string(), "{f}");
    p.send(json!({"t": "backup_now"})).await;
    let done = |x: &serde_json::Value| x["t"] == "backup" && x["lastAt"].is_string();
    let f = p.until(done).await.pop().unwrap();
    assert!(f["lastBytes"].as_u64().unwrap() > 0 && f.get("lastError").is_none(), "{f}");
    q.until(done).await;
    assert!(fake.objects.lock().unwrap().contains_key("beluga/latest.tar.zst"));
    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("data").join("backup.json")).unwrap()).unwrap();
    assert_eq!(state["lastBytes"], f["lastBytes"]);
}
