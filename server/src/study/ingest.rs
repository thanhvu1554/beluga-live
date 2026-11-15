//! Ingest job: photos/PDFs of exams → question records → one `knowledge.json` per folder.
//!
//! 1. **Pages** (`pages.rs`): one unit per page, read concurrently (bounded), each checked locally,
//!    cached by content, retried / escalated to the stronger model when broken or suspicious; then a
//!    numbering check across neighbouring pages.
//! 2. **Stitch**: pages without a header continue the document before them; overlapping photos are
//!    deduped at page borders; records of re-uploaded pages (same file or same bytes) are replaced.
//!    All records go to `ingest/records.json`, so a later ingest only adds.
//! 3. **Merge** (`merge.rs`): new question groups are placed into the existing tree (deterministically
//!    when already known, otherwise by one compact model call); everything countable is Rust.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::harness::{call_json, CallError, RetryPolicy};
use super::llm::{Llm, LlmRequest, Part, Usage};
use super::merge;
use super::pages::{self, model_label, PageResult, PageUnit, ReadOptions, Reader};

pub const LEVELS: [&str; 4] = ["nb", "th", "vd", "vdc"];
pub const KINDS: [&str; 4] = ["mcq", "truefalse", "short", "essay"];

// ---------------------------------------------------------------- inputs

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Image,
    Pdf,
}

#[derive(Clone, Debug)]
pub struct IngestFile {
    pub file_id: String,
    pub path: PathBuf,
    pub kind: FileKind,
    /// Original name shown in notes (`IMG_0123.jpg`, `de-on-3.pdf`).
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct IngestInput {
    pub folder_id: String,
    pub subject: String,
    pub goal: Option<String>,
    pub files: Vec<IngestFile>,
    /// Current `knowledge.json` of the folder, if any.
    pub existing: Option<Value>,
    /// Records of earlier ingests (`ingest/records.json`), if any.
    pub previous: Option<Records>,
}

#[derive(Clone, Debug)]
pub struct IngestOptions {
    /// Reads every page (cheap, vision).
    pub model: String,
    pub effort: Option<String>,
    /// Re-reads pages the cheap model got wrong or that look suspicious; `None` = no escalation.
    pub escalate_model: Option<String>,
    pub escalate_effort: Option<String>,
    /// Reads PDF pages when `model` cannot take PDFs (DeepSeek). Default: the escalation model or Haiku.
    pub pdf_model: Option<String>,
    pub merge_model: String,
    pub merge_effort: Option<String>,
    /// Pages read at the same time.
    pub concurrency: usize,
    /// Extra attempts of one call on a broken reply or a transport error (each attempt already fails
    /// over between providers).
    pub retries: usize,
    pub retry_base_ms: u64,
    pub max_tokens_extract: u32,
    pub max_tokens_merge: u32,
    pub page_timeout_secs: u64,
    pub merge_timeout_secs: u64,
}

pub const HAIKU: &str = "anthropic/claude-haiku-5.5";
pub const FLASH: &str = "deepseek-flash";

impl Default for IngestOptions {
    fn default() -> Self {
        IngestOptions {
            model: FLASH.into(),
            effort: None,
            escalate_model: Some(HAIKU.into()),
            escalate_effort: None,
            pdf_model: None,
            // Haiku with the strict tree schema mostly returns an empty skeleton (bench, NOTES.md).
            merge_model: FLASH.into(),
            merge_effort: None,
            concurrency: 8,
            retries: 1,
            retry_base_ms: 800,
            max_tokens_extract: 8_000,
            max_tokens_merge: 12_000,
            page_timeout_secs: 90,
            merge_timeout_secs: 150,
        }
    }
}

// ---------------------------------------------------------------- records (compact, persisted)

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SecRec {
    pub label: String,
    pub title: String,
    pub kind: String,
    pub instructions: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SrcRec {
    /// `<first fileId>#<k>`.
    pub id: String,
    pub file_ids: Vec<String>,
    /// sha256 of the pages read for this document (a page uploaded again under a new fileId replaces it).
    pub shas: Vec<String>,
    pub name: String,
    /// `exam|review|worksheet`.
    pub kind: String,
    pub title: String,
    pub school: String,
    pub date: String,
    pub grade: String,
    pub duration_min: u32,
    pub total_points: f64,
    /// A complete test paper (header, timing/point scheme), usable as an exam structure.
    pub full_exam: bool,
    pub pages: u32,
    pub sections: Vec<SecRec>,
    /// Ingest run counter (higher = newer upload), for progress inference.
    pub run: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QRec {
    pub id: String,
    pub src: String,
    /// fileId of the page the question is on.
    pub file: String,
    /// Printed number, e.g. `3a`.
    pub n: String,
    /// Section label as printed (`I`, `Phần A`), empty if none.
    pub section: String,
    pub kind: String,
    pub level: String,
    pub points: f64,
    pub chapter: String,
    pub topic: String,
    pub form: String,
    pub text: String,
    pub has_figure: bool,
    pub answer: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Records {
    pub run: u32,
    pub sources: Vec<SrcRec>,
    pub questions: Vec<QRec>,
}

// ---------------------------------------------------------------- errors / progress

#[derive(Debug, Clone, PartialEq)]
pub enum IngestError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::Cancelled => write!(f, "cancelled"),
            IngestError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl From<CallError> for IngestError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Cancelled => IngestError::Cancelled,
            CallError::Failed(m) => IngestError::Failed(m),
        }
    }
}

pub type Emit = Arc<dyn Fn(Value) + Send + Sync>;

/// Build an `ingest` event exactly as PROTOCOL §11.
#[allow(clippy::too_many_arguments)]
pub fn ingest_event(
    folder_id: &str,
    job_id: &str,
    state: &str,
    done: usize,
    total: usize,
    note: &str,
    found: Option<(usize, usize)>,
    cost: Option<f64>,
) -> Value {
    let mut v = json!({"t": "ingest", "folderId": folder_id, "jobId": job_id, "state": state,
                       "done": done, "total": total, "note": note});
    if let Some((q, t)) = found {
        v["found"] = json!({"questions": q, "topics": t});
    }
    if let Some(c) = cost {
        v["costUsd"] = json!(round(c, 4));
    }
    v
}

pub(crate) struct Progress {
    pub done: usize,
    pub questions: usize,
    pub topics: HashSet<String>,
}

pub(crate) struct Ctx {
    pub folder_id: String,
    pub job_id: String,
    pub total: usize,
    pub emit: Emit,
    pub spent: Mutex<Usage>,
    pub progress: Mutex<Progress>,
}

impl Ctx {
    pub fn send(&self, state: &str, note: &str) {
        let p = self.progress.lock().unwrap();
        (self.emit)(ingest_event(
            &self.folder_id,
            &self.job_id,
            state,
            p.done.min(self.total),
            self.total,
            note,
            Some((p.questions, p.topics.len())),
            Some(self.cost()),
        ));
    }
    pub fn cost(&self) -> f64 {
        self.spent.lock().unwrap().cost_usd
    }
}

// ---------------------------------------------------------------- files

#[derive(Clone, Debug)]
pub struct PageRef {
    pub file_id: String,
    pub file_name: String,
    /// 1-based page inside its file.
    pub page_in_file: usize,
}

pub fn image_mime(path: &Path, data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        return "image/png";
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return "image/webp";
    }
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    }
}

/// Page count of a PDF; falls back to counting `/Type /Page` objects when lopdf cannot parse it.
pub fn pdf_page_count(data: &[u8]) -> usize {
    if let Ok(doc) = lopdf::Document::load_mem(data) {
        let n = doc.get_pages().len();
        if n > 0 {
            return n;
        }
    }
    let s = String::from_utf8_lossy(data);
    let mut n = 0;
    let mut rest: &str = &s;
    while let Some(i) = rest.find("/Type") {
        let after = rest[i + 5..].trim_start();
        if after.starts_with("/Page") && !after.starts_with("/Pages") {
            n += 1;
        }
        rest = &rest[i + 5..];
    }
    n.max(1)
}

/// Split a PDF into chunks of `per` pages. Returns `(first page, last page, bytes)`; `None` if the PDF
/// cannot be processed (the caller then sends it whole).
pub fn split_pdf(data: &[u8], per: usize) -> Option<Vec<(usize, usize, Vec<u8>)>> {
    let doc = lopdf::Document::load_mem(data).ok()?;
    let n = doc.get_pages().len();
    if n == 0 {
        return None;
    }
    let mut out = Vec::new();
    let mut start = 1;
    while start <= n {
        let end = (start + per - 1).min(n);
        let mut d = doc.clone();
        let drop: Vec<u32> = (1..=n as u32).filter(|p| (*p as usize) < start || (*p as usize) > end).collect();
        d.delete_pages(&drop);
        d.prune_objects();
        let mut buf = Vec::new();
        d.save_to(&mut buf).ok()?;
        out.push((start, end, buf));
        start = end + 1;
    }
    Some(out)
}

// ---------------------------------------------------------------- small helpers

pub fn s_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub fn num_of(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().replace(',', ".").parse().unwrap_or(0.0),
        _ => 0.0,
    }
    .max(0.0)
}

pub fn norm_level(s: &str) -> String {
    let l = s.trim().to_lowercase();
    match l.as_str() {
        "nb" | "nhận biết" | "nhan biet" | "biết" => "nb",
        "th" | "thông hiểu" | "thong hieu" | "hiểu" => "th",
        "vdc" | "vận dụng cao" | "van dung cao" => "vdc",
        "vd" | "vận dụng" | "van dung" => "vd",
        _ => "th",
    }
    .to_string()
}

pub fn norm_kind(s: &str) -> String {
    let l = s.trim().to_lowercase();
    match l.as_str() {
        "mcq" | "trắc nghiệm" | "multiple choice" => "mcq",
        "truefalse" | "true_false" | "đúng sai" | "đúng/sai" => "truefalse",
        "short" | "trả lời ngắn" => "short",
        _ => "essay",
    }
    .to_string()
}

/// Lowercase, collapse whitespace, trim punctuation: for matching titles.
pub fn norm(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c.is_ascii_punctuation() || c == '…')
        .to_string()
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

pub fn round(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

pub fn level_counts(qs: &[&QRec]) -> BTreeMap<&'static str, usize> {
    let mut m: BTreeMap<&'static str, usize> = LEVELS.iter().map(|l| (*l, 0)).collect();
    for q in qs {
        let l = LEVELS.iter().find(|l| **l == q.level).copied().unwrap_or("th");
        *m.get_mut(l).unwrap() += 1;
    }
    m
}

fn fill(dst: &mut String, src: &str) {
    if dst.is_empty() && !src.is_empty() {
        *dst = src.to_string();
    }
}

/// Fill missing points so every question has some: printed total minus known points shared equally,
/// otherwise 0.25 for objective items and the rest of 10 for essay items.
pub fn fill_points(qs: &[&QRec], total: f64) -> Vec<f64> {
    let known: f64 = qs.iter().map(|q| q.points).sum();
    let missing: Vec<usize> = (0..qs.len()).filter(|i| qs[*i].points <= 0.0).collect();
    let mut pts: Vec<f64> = qs.iter().map(|q| q.points).collect();
    if missing.is_empty() {
        return pts;
    }
    let total = if total > 0.0 { total } else { 10.0 };
    let remain = total - known;
    if remain > 0.0 && known > 0.0 {
        let each = round(remain / missing.len() as f64, 2);
        for i in missing {
            pts[i] = each;
        }
        return pts;
    }
    let objective: Vec<usize> = missing.iter().copied().filter(|i| qs[*i].kind != "essay").collect();
    let essays: Vec<usize> = missing.iter().copied().filter(|i| qs[*i].kind == "essay").collect();
    for i in &objective {
        pts[*i] = 0.25;
    }
    let left = (total - known - 0.25 * objective.len() as f64).max(0.5 * essays.len() as f64);
    if !essays.is_empty() {
        let each = (left / essays.len() as f64 * 4.0).round() / 4.0;
        for i in essays {
            pts[i] = each.max(0.25);
        }
    }
    pts
}

// ---------------------------------------------------------------- stitching

fn qkey(q: &QRec) -> (String, String) {
    (norm(&q.section), q.n.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect())
}

/// Page readings (unit order; `None` = page failed) → records. A page without a header continues the
/// document of the page before it; the same question at the border of two pages (overlapping photos)
/// is kept once, with the longer text.
pub fn stitch(units: &[PageUnit], outs: &[Option<pages::PageOut>], run: u32) -> Records {
    let mut rec = Records { run, ..Default::default() };
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut prev: Option<usize> = None;
    for (u, out) in units.iter().zip(outs) {
        let Some(p) = out else {
            prev = None;
            continue;
        };
        let file = u.file_id().to_string();
        let joined = match prev {
            Some(i) if pages::continues(p) => {
                let s = &mut rec.sources[i];
                if !s.file_ids.contains(&file) {
                    s.file_ids.push(file.clone());
                }
                s.shas.push(u.sha.clone());
                s.pages += u.pages.len() as u32;
                for sec in &p.doc.sections {
                    if !s.sections.iter().any(|x| norm(&x.label) == norm(&sec.label)) {
                        s.sections.push(sec.clone());
                    }
                }
                fill(&mut s.title, &p.doc.title);
                fill(&mut s.school, &p.doc.school);
                fill(&mut s.date, &p.doc.date);
                fill(&mut s.grade, &p.doc.grade);
                if s.duration_min == 0 {
                    s.duration_min = p.doc.duration_min;
                }
                if s.total_points == 0.0 {
                    s.total_points = p.doc.total_points;
                }
                s.full_exam |= p.doc.full_exam;
                Some(i)
            }
            _ => None,
        };
        let si = joined.unwrap_or_else(|| {
            let k = per_file.entry(file.clone()).or_insert(0);
            *k += 1;
            let mut s = p.doc.clone();
            s.id = format!("{file}#{k}");
            s.file_ids = vec![file.clone()];
            s.shas = vec![u.sha.clone()];
            s.name = u.pages[0].file_name.clone();
            s.pages = u.pages.len() as u32;
            s.run = run;
            rec.sources.push(s);
            rec.sources.len() - 1
        });
        let sid = rec.sources[si].id.clone();
        let before = rec.questions.len();
        for (j, q) in p.questions.iter().enumerate() {
            // Border dedupe: the first items of a continuing page against the last items before it.
            if joined.is_some() && j < 2 {
                let tail = rec.questions[..before].iter().rposition(|x| x.src == sid && qkey(x) == qkey(q));
                if let Some(t) = tail.filter(|t| before - t <= 3) {
                    if q.text.chars().count() > rec.questions[t].text.chars().count() {
                        rec.questions[t].text = q.text.clone();
                        rec.questions[t].file = file.clone();
                    }
                    continue;
                }
            }
            let mut q = q.clone();
            q.src = sid.clone();
            q.file = file.clone();
            q.id = format!("{sid}/q{}", rec.questions.iter().filter(|x| x.src == sid).count() + 1);
            rec.questions.push(q);
        }
        prev = Some(si);
    }
    rec
}

/// Old records + new: documents of files (or page bytes) read again are replaced by the new reading.
pub fn combine_records(prev: Option<Records>, new: Records) -> Records {
    let Some(mut prev) = prev else { return new };
    let new_files: HashSet<&String> = new.sources.iter().flat_map(|s| s.file_ids.iter()).collect();
    let new_shas: HashSet<&String> = new.sources.iter().flat_map(|s| s.shas.iter()).collect();
    let dropped: HashSet<String> = prev
        .sources
        .iter()
        .filter(|s| s.file_ids.iter().any(|f| new_files.contains(f)) || s.shas.iter().any(|h| new_shas.contains(h)))
        .map(|s| s.id.clone())
        .collect();
    prev.sources.retain(|s| !dropped.contains(&s.id));
    prev.questions.retain(|q| !dropped.contains(&q.src));
    let taken: HashSet<String> = prev.sources.iter().map(|s| s.id.clone()).collect();
    let mut rename: HashMap<String, String> = HashMap::new();
    for s in &new.sources {
        if taken.contains(&s.id) {
            rename.insert(s.id.clone(), format!("{}r{}", s.id, new.run));
        }
    }
    for mut s in new.sources {
        if let Some(r) = rename.get(&s.id) {
            s.id = r.clone();
        }
        prev.sources.push(s);
    }
    for mut q in new.questions {
        if let Some(r) = rename.get(&q.src) {
            q.id = q.id.replacen(&q.src, r, 1);
            q.src = r.clone();
        }
        prev.questions.push(q);
    }
    prev.run = new.run;
    prev
}

// ---------------------------------------------------------------- validation + io

/// Schema check of a knowledge value; returns the problems found (empty = valid).
pub fn validate_knowledge(k: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    for key in ["v", "folderId", "subject", "updatedAt", "sources", "curriculum", "structures", "schoolProgress", "stats"] {
        if k.get(key).is_none() {
            errs.push(format!("missing {key}"));
        }
    }
    let mut topic_ids = HashSet::new();
    let mut form_ids = HashSet::new();
    let mut ids = HashSet::new();
    let mut topic_total = 0u64;
    for c in k["curriculum"].as_array().into_iter().flatten() {
        let cid = s_of(&c["id"]);
        if cid.is_empty() || !ids.insert(cid.clone()) {
            errs.push(format!("bad chapter id {cid}"));
        }
        for t in c["topics"].as_array().into_iter().flatten() {
            let tid = s_of(&t["id"]);
            if !tid.starts_with(&format!("{cid}.t")) || !topic_ids.insert(tid.clone()) {
                errs.push(format!("bad topic id {tid}"));
            }
            let mut fsum = 0;
            for f in t["forms"].as_array().into_iter().flatten() {
                let fid = s_of(&f["id"]);
                if !fid.starts_with(&format!("{tid}.f")) || !form_ids.insert(fid.clone()) {
                    errs.push(format!("bad form id {fid}"));
                }
                let n = f["count"].as_u64().unwrap_or(0);
                let lv: u64 = LEVELS.iter().map(|l| f["levels"][l].as_u64().unwrap_or(0)).sum();
                if lv != n {
                    errs.push(format!("{fid}: levels {lv} != count {n}"));
                }
                fsum += n;
            }
            if t["count"].as_u64() != Some(fsum) {
                errs.push(format!("{tid}: count != sum of forms"));
            }
            topic_total += fsum;
        }
    }
    let nq = k["stats"]["questions"].as_u64().unwrap_or(0);
    if !k["curriculum"].as_array().map(|a| a.is_empty()).unwrap_or(true) && topic_total < nq {
        errs.push(format!("curriculum covers {topic_total} of {nq} questions"));
    }
    for s in k["structures"].as_array().into_iter().flatten() {
        let sid = s_of(&s["id"]);
        let secs: Vec<String> = s["sections"].as_array().into_iter().flatten().map(|x| s_of(&x["id"])).collect();
        let mut total = 0.0;
        for sec in s["sections"].as_array().into_iter().flatten() {
            if !KINDS.contains(&sec["kind"].as_str().unwrap_or("")) {
                errs.push(format!("{sid}: bad section kind"));
            }
            let n: u64 = s["matrix"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["section"] == sec["id"])
                .map(|m| m["count"].as_u64().unwrap_or(0))
                .sum();
            if Some(n) != sec["count"].as_u64() {
                errs.push(format!("{sid}/{}: matrix count {n} != section count", s_of(&sec["id"])));
            }
        }
        for m in s["matrix"].as_array().into_iter().flatten() {
            total += m["points"].as_f64().unwrap_or(0.0);
            if !secs.contains(&s_of(&m["section"])) {
                errs.push(format!("{sid}: matrix row with unknown section"));
            }
            let tid = s_of(&m["topicId"]);
            if !tid.is_empty() && !topic_ids.contains(&tid) {
                errs.push(format!("{sid}: matrix row with unknown topic {tid}"));
            }
            if !LEVELS.contains(&m["level"].as_str().unwrap_or("")) {
                errs.push(format!("{sid}: bad level"));
            }
        }
        if (total - s["totalPoints"].as_f64().unwrap_or(-1.0)).abs() > 0.02 {
            errs.push(format!("{sid}: totalPoints != matrix points"));
        }
    }
    for t in k["schoolProgress"]["coveredTopicIds"].as_array().into_iter().flatten() {
        if !topic_ids.contains(&s_of(t)) {
            errs.push(format!("schoolProgress: unknown topic {t}"));
        }
    }
    errs
}

/// Write via a temp file + rename so readers never see half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---------------------------------------------------------------- the job

/// Run one ingest. Emits `ingest` events (reading → merging → done | error | cancelled), writes
/// `knowledge.json`, `ingest/records.json`, `ingest/assign.json` and page cache entries, returns the
/// knowledge.
pub async fn run_ingest(
    llm: Arc<dyn Llm>,
    opts: IngestOptions,
    input: IngestInput,
    folder_dir: PathBuf,
    job_id: String,
    emit: Emit,
    cancel: watch::Receiver<bool>,
) -> Result<Value, IngestError> {
    let units = match pages::plan_pages(&input.files) {
        Ok(b) => b,
        Err(e) => {
            emit(ingest_event(&input.folder_id, &job_id, "error", 0, 0, &e, None, Some(0.0)));
            return Err(IngestError::Failed(e));
        }
    };
    let total: usize = units.iter().map(|b| b.pages.len()).sum();
    let ctx = Arc::new(Ctx {
        folder_id: input.folder_id.clone(),
        job_id: job_id.clone(),
        total,
        emit: emit.clone(),
        spent: Mutex::new(Usage::default()),
        progress: Mutex::new(Progress { done: 0, questions: 0, topics: HashSet::new() }),
    });
    let r = run_inner(llm, &opts, &input, &folder_dir, &units, ctx.clone(), cancel).await;
    match &r {
        Ok(k) => {
            let n = k["stats"]["questions"].as_u64().unwrap_or(0) as usize;
            let topics: usize = k["curriculum"].as_array().into_iter().flatten().map(|c| c["topics"].as_array().map(|t| t.len()).unwrap_or(0)).sum();
            let ns = k["structures"].as_array().map(|a| a.len()).unwrap_or(0);
            ctx.progress.lock().unwrap().questions = n;
            let note = format!("Xong: {n} câu, {topics} chủ đề, {ns} cấu trúc đề");
            emit(ingest_event(&ctx.folder_id, &ctx.job_id, "done", total, total, &note, Some((n, topics)), Some(ctx.cost())));
        }
        Err(IngestError::Cancelled) => ctx.send("cancelled", "Đã dừng nạp đề (các trang đã đọc được giữ lại cho lần sau)"),
        Err(IngestError::Failed(m)) => ctx.send("error", &format!("Lỗi: {}", clip(m, 160))),
    }
    r
}

fn page_note(u: &PageUnit, total: usize, r: &PageResult) -> String {
    let label = u.label(total);
    match &r.out {
        None => format!("Không đọc được {label}, bỏ qua"),
        Some(o) => {
            let title = if o.doc.title.is_empty() { String::new() } else { format!(": {}", clip(&o.doc.title, 60)) };
            let how = if r.cached {
                " (đã đọc ở lần trước)".to_string()
            } else if r.escalated {
                format!(" ({} đọc lại)", model_label(&r.model))
            } else {
                String::new()
            };
            let warn = if r.issues.is_empty() { String::new() } else { format!(" · cần xem lại: {}", r.issues.join("; ")) };
            format!("Đã đọc {label}{title} · {} câu{how}{warn}", o.questions.len())
        }
    }
}

async fn run_inner(
    llm: Arc<dyn Llm>,
    opts: &IngestOptions,
    input: &IngestInput,
    folder_dir: &Path,
    units: &[PageUnit],
    ctx: Arc<Ctx>,
    cancel: watch::Receiver<bool>,
) -> Result<Value, IngestError> {
    let total = ctx.total;
    let ingest_dir = folder_dir.join("ingest");
    let pdf_model = opts.pdf_model.clone().or_else(|| opts.escalate_model.clone()).unwrap_or_else(|| HAIKU.into());
    let reader = Reader {
        llm: llm.clone(),
        opts: ReadOptions {
            model: opts.model.clone(),
            effort: opts.effort.clone(),
            escalate: opts.escalate_model.clone(),
            escalate_effort: opts.escalate_effort.clone(),
            pdf_model,
            concurrency: opts.concurrency,
            retries: opts.retries,
            retry_base_ms: opts.retry_base_ms,
            max_tokens: opts.max_tokens_extract,
            timeout_secs: opts.page_timeout_secs,
            subject: input.subject.clone(),
        },
        cache_dir: ingest_dir.join("pages"),
        ctx: ctx.clone(),
        cancel: cancel.clone(),
    };
    let check = match &opts.escalate_model {
        Some(m) if *m != opts.model => format!("{}, trang nghi ngờ đọc lại bằng {}", model_label(&opts.model), model_label(m)),
        _ => model_label(&opts.model),
    };
    ctx.send("reading", &format!("Chuẩn bị đọc {total} trang ({check})"));

    // --- pass 1: every page, bounded concurrency
    let reader = Arc::new(reader);
    let futs = (0..units.len()).map(|i| {
        let reader = reader.clone();
        let ctx = ctx.clone();
        async move {
            let u = &units[i];
            let r = reader.resolve(u).await;
            if let Ok(r) = &r {
                let note = {
                    let mut p = ctx.progress.lock().unwrap();
                    p.done += u.pages.len();
                    if let Some(o) = &r.out {
                        p.questions += o.questions.len();
                        for q in &o.questions {
                            p.topics.insert(norm(&q.topic));
                        }
                    }
                    page_note(u, total, r)
                };
                ctx.send("reading", &note);
            }
            (i, r)
        }
    });
    let mut stream = futures_util::stream::iter(futs).buffer_unordered(opts.concurrency.max(1));
    let mut results: Vec<PageResult> = vec![PageResult::default(); units.len()];
    while let Some((i, r)) = stream.next().await {
        match r {
            Ok(r) => results[i] = r,
            Err(CallError::Cancelled) => return Err(IngestError::Cancelled),
            Err(CallError::Failed(m)) => results[i] = PageResult { error: Some(m), ..Default::default() },
        }
    }
    drop(stream);
    if *cancel.borrow() {
        return Err(IngestError::Cancelled);
    }

    // --- pass 2: numbering gaps between neighbouring pages → stronger model
    let outs: Vec<Option<pages::PageOut>> = results.iter().map(|r| r.out.clone()).collect();
    let mut flagged: BTreeMap<usize, String> = BTreeMap::new();
    for (i, why) in pages::neighbour_gaps(&outs) {
        flagged.entry(i).or_insert(why);
    }
    flagged.retain(|i, _| opts.escalate_model.as_ref().is_some_and(|m| results[*i].model != *m));
    if !flagged.is_empty() {
        let names: Vec<String> = flagged.keys().map(|i| units[*i].first_page.to_string()).collect();
        let why = flagged.values().next().cloned().unwrap_or_default();
        ctx.send("reading", &format!("Trang {} chưa khớp nhau ({why}), đọc lại bằng {}", names.join(", "),
            model_label(opts.escalate_model.as_deref().unwrap_or(""))));
        let todo: Vec<(usize, PageResult)> = flagged.keys().map(|i| (*i, results[*i].clone())).collect();
        let futs = todo.into_iter().map(|(i, cur)| {
            let reader = reader.clone();
            async move { (i, reader.escalate(&units[i], cur).await) }
        });
        let mut stream = futures_util::stream::iter(futs).buffer_unordered(opts.concurrency.max(1));
        while let Some((i, r)) = stream.next().await {
            match r {
                Ok(r) => results[i] = r,
                Err(CallError::Cancelled) => return Err(IngestError::Cancelled),
                Err(CallError::Failed(_)) => {}
            }
        }
    }
    if *cancel.borrow() {
        return Err(IngestError::Cancelled);
    }
    let failed = results.iter().filter(|r| r.out.is_none()).count();
    if failed == units.len() && !units.is_empty() {
        let last = results.iter().rev().find_map(|r| r.error.clone()).unwrap_or_default();
        return Err(IngestError::Failed(format!("không đọc được trang nào ({last})")));
    }
    let escalated = results.iter().filter(|r| r.escalated).count();
    let cached = results.iter().filter(|r| r.cached).count();
    tracing::info!(pages = units.len(), escalated, cached, failed, "pages read");

    // --- stitch + records
    let run = input.previous.as_ref().map(|p| p.run).unwrap_or(0) + 1;
    let outs: Vec<Option<pages::PageOut>> = results.iter().map(|r| r.out.clone()).collect();
    let new = stitch(units, &outs, run);
    let rec = combine_records(input.previous.clone(), new);
    if rec.questions.is_empty() {
        return Err(IngestError::Failed("không tìm thấy câu hỏi nào trong tệp đã tải".into()));
    }
    write_atomic(&ingest_dir.join("records.json"), &serde_json::to_vec(&rec).unwrap_or_default())
        .map_err(|e| IngestError::Failed(format!("ghi records: {e}")))?;

    // --- merge: known groups in Rust, new ones by one model call
    let mut tree = merge::Tree::from_knowledge(input.existing.as_ref());
    let mut assign: merge::Assign =
        std::fs::read(ingest_dir.join("assign.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let raws = merge::raw_triples(&rec.questions);
    let open = merge::place_known(&tree, &raws, &mut assign);
    if open.is_empty() {
        ctx.send("merging", &format!("Gộp {} câu vào giáo trình (mọi dạng bài đã có sẵn, không cần gọi model)", rec.questions.len()));
    } else {
        ctx.send(
            "merging",
            &format!(
                "Đang xếp {} nhóm câu mới vào giáo trình ({} nhóm đã biết) bằng {}",
                open.len(),
                raws.len() - open.len(),
                model_label(&opts.merge_model)
            ),
        );
        let mut req = LlmRequest::new(
            "merge",
            &opts.merge_model,
            opts.merge_effort.clone(),
            merge::MERGE_SYSTEM,
            vec![Part::Text(merge::merge_prompt(&input.subject, input.goal.as_deref(), &tree, &open, &rec.questions))],
            opts.max_tokens_merge,
        );
        req.schema = Some(("curriculum".into(), merge::merge_schema()));
        req.timeout = Some(std::time::Duration::from_secs(opts.merge_timeout_secs));
        req.temperature = Some(0.0);
        let policy = RetryPolicy { retries: opts.retries.max(1), base_ms: opts.retry_base_ms };
        let open_ref = &open;
        match call_json(llm.as_ref(), req, policy, &ctx.spent, cancel.clone(), &|v: &Value| merge::check_merge(v, open_ref)).await {
            Ok(reply) => merge::apply_merge(&mut tree, &reply.value, &open, &mut assign),
            Err(CallError::Cancelled) => return Err(IngestError::Cancelled),
            Err(CallError::Failed(m)) => {
                tracing::warn!("merge failed, placing by names: {m}");
                ctx.send("merging", "Model gộp lỗi, xếp các câu mới theo tên chương/chủ đề đã đọc");
                merge::apply_merge(&mut tree, &json!({}), &open, &mut assign);
            }
        }
    }
    if *cancel.borrow() {
        return Err(IngestError::Cancelled);
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let k = merge::build_knowledge(input, &rec, &tree, &assign, &now);
    let errs = validate_knowledge(&k);
    if !errs.is_empty() {
        tracing::warn!("knowledge validation: {}", errs.join("; "));
    }
    write_atomic(&ingest_dir.join("assign.json"), &serde_json::to_vec_pretty(&assign).unwrap_or_default())
        .map_err(|e| IngestError::Failed(format!("ghi assign: {e}")))?;
    write_atomic(&folder_dir.join("knowledge.json"), &serde_json::to_vec_pretty(&k).unwrap_or_default())
        .map_err(|e| IngestError::Failed(format!("ghi knowledge.json: {e}")))?;
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::pages::PageOut;

    fn unit(i: usize, file: &str) -> PageUnit {
        PageUnit {
            index: i,
            first_page: i + 1,
            pages: vec![PageRef { file_id: file.into(), file_name: format!("{file}.jpg"), page_in_file: 1 }],
            part: Part::Text(String::new()),
            sha: format!("sha-{file}"),
            bytes: 0,
        }
    }

    fn q(n: &str, text: &str) -> QRec {
        QRec { n: n.into(), text: text.into(), kind: "mcq".into(), level: "th".into(), ..Default::default() }
    }

    fn page(title: &str, qs: Vec<QRec>) -> PageOut {
        PageOut { doc: SrcRec { title: title.into(), kind: "exam".into(), full_exam: true, ..Default::default() }, questions: qs, ..Default::default() }
    }

    #[test]
    fn stitch_joins_pages_without_header_and_dedupes_the_border() {
        let u = vec![unit(0, "p1"), unit(1, "p2"), unit(2, "p3")];
        let o = vec![
            Some(page("Đề 1", vec![q("1", "Câu một"), q("6", "Căn bậc ba")])),
            Some(page("", vec![q("6", "Căn bậc ba của -27 là A. 3 B. -3"), q("7", "Cặp số nghiệm")])),
            Some(page("Phiếu bài tập", vec![q("1", "Bài một")])),
        ];
        let r = stitch(&u, &o, 1);
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.sources[0].id, "p1#1");
        assert_eq!(r.sources[0].file_ids, vec!["p1", "p2"]);
        assert_eq!(r.sources[0].shas, vec!["sha-p1", "sha-p2"]);
        let ns: Vec<&str> = r.questions.iter().filter(|q| q.src == "p1#1").map(|q| q.n.as_str()).collect();
        assert_eq!(ns, ["1", "6", "7"]);
        assert!(r.questions[1].text.contains("A. 3"), "the longer reading of the border question wins");
        assert_eq!(r.questions[1].file, "p2");
        assert_eq!(r.questions[2].id, "p1#1/q3");
        // A failed page in between breaks the chain.
        let r = stitch(&u, &[Some(page("Đề", vec![q("1", "x")])), None, Some(page("", vec![q("9", "y")]))], 1);
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.sources[1].id, "p3#1");
    }

    #[test]
    fn combine_replaces_reuploaded_files_and_same_bytes() {
        let mut old = Records { run: 1, ..Default::default() };
        old.sources.push(SrcRec { id: "a#1".into(), file_ids: vec!["a".into()], shas: vec!["h1".into()], ..Default::default() });
        old.sources.push(SrcRec { id: "b#1".into(), file_ids: vec!["b".into()], shas: vec!["h2".into()], ..Default::default() });
        old.questions.push(QRec { id: "a#1/q1".into(), src: "a#1".into(), ..Default::default() });
        old.questions.push(QRec { id: "b#1/q1".into(), src: "b#1".into(), ..Default::default() });
        let mut new = Records { run: 2, ..Default::default() };
        new.sources.push(SrcRec { id: "b#1".into(), file_ids: vec!["b".into()], ..Default::default() });
        new.sources.push(SrcRec { id: "c#1".into(), file_ids: vec!["c".into()], shas: vec!["h1".into()], ..Default::default() });
        new.questions.push(QRec { id: "b#1/q1".into(), src: "b#1".into(), text: "new".into(), ..Default::default() });
        let r = combine_records(Some(old), new);
        assert_eq!(r.run, 2);
        assert_eq!(r.sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b#1", "c#1"], "a (same bytes as c) and old b are gone");
        assert_eq!(r.questions.iter().find(|q| q.src == "b#1").unwrap().text, "new");
    }

    #[test]
    fn points_are_filled_sensibly() {
        let mk = |k: &str, p: f64| QRec { kind: k.into(), points: p, ..Default::default() };
        let qs = [mk("mcq", 0.0), mk("mcq", 0.0), mk("essay", 0.0)];
        let refs: Vec<&QRec> = qs.iter().collect();
        assert_eq!(fill_points(&refs, 0.0), vec![0.25, 0.25, 9.5]);
        let qs = [mk("mcq", 0.25), mk("mcq", 0.25), mk("essay", 0.0)];
        let refs: Vec<&QRec> = qs.iter().collect();
        assert_eq!(fill_points(&refs, 2.0), vec![0.25, 0.25, 1.5]);
    }

    #[test]
    fn levels_and_kinds_normalise() {
        assert_eq!(norm_level("Vận dụng cao"), "vdc");
        assert_eq!(norm_level("NB"), "nb");
        assert_eq!(norm_level("?"), "th");
        assert_eq!(norm_kind("Trắc nghiệm"), "mcq");
        assert_eq!(norm_kind("whatever"), "essay");
        assert_eq!(norm("  Rút gọn   Biểu thức. "), "rút gọn biểu thức");
    }

    #[test]
    fn validation_catches_inconsistent_matrix() {
        let k = json!({"v": 1, "folderId": "f", "subject": "toan", "updatedAt": "", "sources": [],
            "curriculum": [{"id": "c1", "title": "C", "topics": [{"id": "c1.t1", "title": "T", "count": 2, "share": 1.0,
                "forms": [{"id": "c1.t1.f1", "title": "F", "count": 2, "levels": {"nb": 1, "th": 1, "vd": 0, "vdc": 0}}]}]}],
            "structures": [{"id": "s1", "totalPoints": 1.0, "sections": [{"id": "A", "kind": "mcq", "count": 3}],
                "matrix": [{"section": "A", "topicId": "c1.t9", "formId": "", "level": "th", "count": 2, "points": 0.5}]}],
            "schoolProgress": {"coveredTopicIds": ["c1.t1"]}, "stats": {"questions": 2}});
        let e = validate_knowledge(&k);
        assert!(e.iter().any(|x| x.contains("matrix count 2")), "{e:?}");
        assert!(e.iter().any(|x| x.contains("unknown topic c1.t9")), "{e:?}");
        assert!(e.iter().any(|x| x.contains("totalPoints")), "{e:?}");
    }

    #[test]
    fn events_have_protocol_fields() {
        let e = ingest_event("f", "j", "reading", 3, 10, "Đang đọc", Some((5, 2)), Some(0.012345));
        assert_eq!(e, json!({"t": "ingest", "folderId": "f", "jobId": "j", "state": "reading", "done": 3, "total": 10,
            "note": "Đang đọc", "found": {"questions": 5, "topics": 2}, "costUsd": 0.0123}));
    }
}
