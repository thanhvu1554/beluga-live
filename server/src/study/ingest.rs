//! Ingest job: photos/PDFs of exams → compact question records → one `knowledge.json` per folder.
//!
//! Pipeline (PROTOCOL §11):
//! 1. **Batch** pages: images in groups of `batch_images`; a PDF is sent whole when short, otherwise split
//!    into chunks of `pdf_pages_per_batch` pages (lopdf).
//! 2. **Extract** each batch with the vision model (bounded concurrency, retries) into compact JSON: one
//!    record per source document and per question.
//! 3. **Stitch** sources that continue across batch borders, dedupe overlapping photos, store all records
//!    in `ingest/records.json` (so a later ingest re-merges old + new records).
//! 4. **Merge** (one text-only call on the distinct chapter/topic/form triples, never on the images): a
//!    normalized curriculum with solving methods, which sources share an exam structure, and the school
//!    progress. If the call fails the raw triples become the curriculum.
//! 5. **Build** everything countable in Rust: ids (stable across re-ingests), counts, levels, shares,
//!    examples, refs, structure sections/points/matrix, stats; validate; write atomically.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::llm::{Llm, LlmError, LlmRequest, Part, Usage};

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
    pub model: String,
    pub effort: Option<String>,
    pub merge_model: String,
    pub merge_effort: Option<String>,
    pub batch_images: usize,
    /// PDFs with at most this many pages go in one request.
    pub pdf_whole_max_pages: usize,
    pub pdf_pages_per_batch: usize,
    pub concurrency: usize,
    /// Extra attempts after the first failure.
    pub retries: usize,
    pub retry_base_ms: u64,
    pub max_tokens_extract: u32,
    pub max_tokens_merge: u32,
}

impl Default for IngestOptions {
    fn default() -> Self {
        IngestOptions {
            model: "anthropic/claude-haiku-5.5".into(),
            effort: Some("high".into()),
            merge_model: "anthropic/claude-haiku-5.5".into(),
            merge_effort: Some("high".into()),
            batch_images: 3,
            pdf_whole_max_pages: 6,
            pdf_pages_per_batch: 5,
            concurrency: 4,
            retries: 2,
            retry_base_ms: 1500,
            max_tokens_extract: 24_000,
            max_tokens_merge: 24_000,
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

struct Progress {
    done: usize,
    questions: usize,
    topics: HashSet<String>,
    usage: Usage,
}

struct Ctx {
    folder_id: String,
    job_id: String,
    total: usize,
    emit: Emit,
    progress: Mutex<Progress>,
}

impl Ctx {
    fn send(&self, state: &str, note: &str) {
        let p = self.progress.lock().unwrap();
        (self.emit)(ingest_event(
            &self.folder_id,
            &self.job_id,
            state,
            p.done.min(self.total),
            self.total,
            note,
            Some((p.questions, p.topics.len())),
            Some(p.usage.cost_usd),
        ));
    }
    fn cost(&self) -> f64 {
        self.progress.lock().unwrap().usage.cost_usd
    }
}

// ---------------------------------------------------------------- batching

#[derive(Clone, Debug)]
pub struct PageRef {
    pub file_id: String,
    pub file_name: String,
    /// 1-based page inside its file.
    pub page_in_file: usize,
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub index: usize,
    /// 1-based global number of the first page (for notes).
    pub first_page: usize,
    pub pages: Vec<PageRef>,
    pub parts: Vec<Part>,
}

fn image_mime(path: &Path, data: &[u8]) -> &'static str {
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

/// Read the files and cut them into request batches (file order is kept).
pub fn plan_batches(files: &[IngestFile], opts: &IngestOptions) -> Result<Vec<Batch>, String> {
    let mut batches: Vec<Batch> = Vec::new();
    let mut page_no = 1;
    let mut pending: Vec<(PageRef, Part)> = Vec::new();
    let flush = |pending: &mut Vec<(PageRef, Part)>, batches: &mut Vec<Batch>, page_no: &mut usize| {
        if pending.is_empty() {
            return;
        }
        let first = *page_no;
        let mut parts = Vec::new();
        let mut pages = Vec::new();
        for (i, (pr, part)) in pending.drain(..).enumerate() {
            parts.push(Part::Text(format!("Page P{} (photo \"{}\"):", i + 1, pr.file_name)));
            parts.push(part);
            pages.push(pr);
        }
        *page_no += pages.len();
        batches.push(Batch { index: batches.len(), first_page: first, pages, parts });
    };
    for f in files {
        let data = std::fs::read(&f.path).map_err(|e| format!("không đọc được tệp {}: {e}", f.name))?;
        match f.kind {
            FileKind::Image => {
                let mime = image_mime(&f.path, &data).to_string();
                pending.push((
                    PageRef { file_id: f.file_id.clone(), file_name: f.name.clone(), page_in_file: 1 },
                    Part::Image { mime, data },
                ));
                if pending.len() >= opts.batch_images.max(1) {
                    flush(&mut pending, &mut batches, &mut page_no);
                }
            }
            FileKind::Pdf => {
                flush(&mut pending, &mut batches, &mut page_no);
                let n = pdf_page_count(&data);
                let chunks = if n > opts.pdf_whole_max_pages.max(1) {
                    split_pdf(&data, opts.pdf_pages_per_batch.max(1))
                } else {
                    None
                };
                let chunks = chunks.unwrap_or_else(|| vec![(1, n, data)]);
                for (a, b, bytes) in chunks {
                    let pages: Vec<PageRef> = (a..=b)
                        .map(|p| PageRef { file_id: f.file_id.clone(), file_name: f.name.clone(), page_in_file: p })
                        .collect();
                    let parts = vec![
                        Part::Text(format!(
                            "PDF \"{}\", pages {a}-{b} of {n}. Number them P1..P{} in order.",
                            f.name,
                            b - a + 1
                        )),
                        Part::Pdf { name: f.name.clone(), data: bytes },
                    ];
                    let first = page_no;
                    page_no += pages.len();
                    batches.push(Batch { index: batches.len(), first_page: first, pages, parts });
                }
            }
        }
    }
    flush(&mut pending, &mut batches, &mut page_no);
    Ok(batches)
}

// ---------------------------------------------------------------- extraction

pub const EXTRACT_SYSTEM: &str = r#"You read photos or scans of Vietnamese school test papers (teacher-made review exams, tests, worksheets) and transcribe them into compact JSON. Output JSON only.

Pages are labelled P1..Pn in the order given. Several different documents may appear; one document may span several pages.

sources: one entry per document seen in this batch.
- key: "A", "B", ... (local to this batch). pages: the P-numbers it occupies.
- title: the heading as printed, Vietnamese (e.g. "Đề ôn tập giữa kỳ I - Đề 3"); "" if none visible.
- kind: "exam" (test paper), "review" (review sheet / đề cương), "worksheet" (practice exercises).
- school, date, grade: as printed, else "". durationMin, totalPoints: as printed, else 0.
- fullExam: true only if this is a complete test paper (exam header and/or time limit/point scheme), not a list of exercises.
- continuesPrevious: true only for the document on P1 when P1 has no header and clearly continues a document started before this batch (numbering does not restart).
- sections: the parts as printed, in order: label ("I", "A", "Phần 1"), title (Vietnamese), kind (mcq|truefalse|short|essay), instructions (short, Vietnamese, "" if none).

questions: one entry per gradable item, in reading order. Split essay problems into their printed sub-items (1a, 1b, ...); a multiple-choice question is one item; a true/false cluster (a,b,c,d statements) is one item.
- src: source key. page: P-number. section: section label ("" if none). n: printed number ("3", "2b").
- kind: mcq (choices A-D), truefalse (Đúng/Sai statements), short (short numeric answer), essay (written solution).
- level (Vietnamese standard): nb = recall a definition/formula; th = one-step direct application; vd = multi-step problem; vdc = hard, non-routine or olympiad-style. Use the printed level when shown.
- points: as printed (sub-item points if given), else 0.
- chapter, topic, form: Vietnamese, as in the Vietnamese textbook for this subject (chapter = chương, topic = bài/chủ đề, form = dạng bài, the specific kind of task, e.g. "Rút gọn biểu thức chứa căn rồi tính giá trị").
- text: the question, shortened to at most ~250 characters, math in LaTeX between $...$; for mcq append the choices as " A. … B. … C. … D. …".
- hasFigure: true if it needs a figure/graph/table shown on the page.
- answer: only if printed on the page (answer key, "Đáp án"), else "".

Skip non-questions (headers, student name lines, decorations). If a page is unreadable, return what you can."#;

fn extract_schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    json!({
        "type": "object", "additionalProperties": false, "required": ["sources", "questions"],
        "properties": {
            "sources": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["key", "pages", "title", "kind", "school", "date", "grade", "durationMin", "totalPoints",
                             "fullExam", "continuesPrevious", "sections"],
                "properties": {
                    "key": s("A, B, ..."), "pages": {"type": "array", "items": {"type": "integer"}},
                    "title": s("heading"), "kind": {"type": "string", "enum": ["exam", "review", "worksheet"]},
                    "school": s(""), "date": s(""), "grade": s(""),
                    "durationMin": {"type": "number"}, "totalPoints": {"type": "number"},
                    "fullExam": {"type": "boolean"}, "continuesPrevious": {"type": "boolean"},
                    "sections": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                        "required": ["label", "title", "kind", "instructions"],
                        "properties": {"label": s(""), "title": s(""),
                            "kind": {"type": "string", "enum": KINDS}, "instructions": s("")}}}
                }}},
            "questions": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["src", "page", "section", "n", "kind", "level", "points", "chapter", "topic", "form",
                             "text", "hasFigure", "answer"],
                "properties": {
                    "src": s("source key"), "page": {"type": "integer"}, "section": s(""), "n": s(""),
                    "kind": {"type": "string", "enum": KINDS}, "level": {"type": "string", "enum": LEVELS},
                    "points": {"type": "number"}, "chapter": s(""), "topic": s(""), "form": s(""),
                    "text": s("LaTeX in $...$"), "hasFigure": {"type": "boolean"}, "answer": s("")
                }}}
        }
    })
}

/// One batch, parsed.
#[derive(Clone, Debug, Default)]
pub struct BatchOut {
    /// `(local key, record, continues previous, local pages)`.
    pub sources: Vec<(String, SrcRec, bool, Vec<usize>)>,
    /// `(local source key, local page, record)`.
    pub questions: Vec<(String, usize, QRec)>,
}

fn s_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn num_of(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().replace(',', ".").parse().unwrap_or(0.0),
        _ => 0.0,
    }
    .max(0.0)
}

pub fn norm_level(s: &str) -> String {
    let l = s.trim().to_lowercase();
    let l = l.as_str();
    match l {
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

pub fn parse_batch(v: &Value) -> Result<BatchOut, String> {
    let qs = v.get("questions").and_then(Value::as_array).ok_or("thiếu questions")?;
    let mut out = BatchOut::default();
    for s in v.get("sources").and_then(Value::as_array).cloned().unwrap_or_default() {
        let key = s_of(&s["key"]);
        let rec = SrcRec {
            kind: match s_of(&s["kind"]).as_str() {
                k @ ("exam" | "review" | "worksheet") => k.to_string(),
                _ => "exam".into(),
            },
            title: s_of(&s["title"]),
            school: s_of(&s["school"]),
            date: s_of(&s["date"]),
            grade: s_of(&s["grade"]),
            duration_min: num_of(&s["durationMin"]).round() as u32,
            total_points: num_of(&s["totalPoints"]),
            full_exam: s["fullExam"].as_bool().unwrap_or(false),
            sections: s["sections"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| SecRec {
                            label: s_of(&x["label"]),
                            title: s_of(&x["title"]),
                            kind: norm_kind(&s_of(&x["kind"])),
                            instructions: s_of(&x["instructions"]),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ..Default::default()
        };
        let pages: Vec<usize> =
            s["pages"].as_array().map(|a| a.iter().filter_map(|p| p.as_u64().map(|x| x as usize)).collect()).unwrap_or_default();
        out.sources.push((key, rec, s["continuesPrevious"].as_bool().unwrap_or(false), pages));
    }
    for q in qs {
        let rec = QRec {
            n: s_of(&q["n"]),
            section: s_of(&q["section"]),
            kind: norm_kind(&s_of(&q["kind"])),
            level: norm_level(&s_of(&q["level"])),
            points: num_of(&q["points"]),
            chapter: s_of(&q["chapter"]),
            topic: s_of(&q["topic"]),
            form: s_of(&q["form"]),
            text: s_of(&q["text"]),
            has_figure: q["hasFigure"].as_bool().unwrap_or(false),
            answer: s_of(&q["answer"]),
            ..Default::default()
        };
        if rec.text.is_empty() && rec.n.is_empty() {
            continue;
        }
        out.questions.push((s_of(&q["src"]), q["page"].as_u64().unwrap_or(1) as usize, rec));
    }
    Ok(out)
}

async fn cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// One LLM call with retries; JSON parse failures count as retryable. Usage is added to `ctx` for every
/// attempt (failed attempts cost money too).
async fn call_json(
    llm: &dyn Llm,
    req: LlmRequest,
    retries: usize,
    retry_base_ms: u64,
    ctx: &Ctx,
    mut cancel: watch::Receiver<bool>,
    validate: &(dyn Fn(&Value) -> Result<(), String> + Send + Sync),
) -> Result<Value, IngestError> {
    let mut last = String::new();
    for attempt in 0..=retries {
        if *cancel.borrow() {
            return Err(IngestError::Cancelled);
        }
        if attempt > 0 {
            let wait = Duration::from_millis(retry_base_ms * attempt as u64);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = cancelled(&mut cancel) => return Err(IngestError::Cancelled),
            }
        }
        let r = tokio::select! {
            r = llm.complete(req.clone()) => r,
            _ = cancelled(&mut cancel) => return Err(IngestError::Cancelled),
        };
        let r: Result<Value, LlmError> = match r {
            Ok(resp) => {
                ctx.progress.lock().unwrap().usage.add(&resp.usage);
                tracing::info!(tag = req.tag, ms = resp.elapsed_ms, cost = resp.usage.cost_usd,
                    inp = resp.usage.input_tokens, out = resp.usage.output_tokens, "study llm call");
                resp.json().and_then(|v| {
                    validate(&v).map(|_| v).map_err(|m| LlmError { status: 0, message: m, retryable: true })
                })
            }
            Err(e) => Err(e),
        };
        match r {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::warn!(tag = req.tag, attempt, "study llm call failed: {e}");
                last = e.to_string();
                if !e.retryable {
                    break;
                }
            }
        }
    }
    Err(IngestError::Failed(last))
}

// ---------------------------------------------------------------- stitching

/// Turn per-batch outputs (in batch order; `None` = batch failed) into global records.
pub fn stitch(batches: &[Batch], outs: &[Option<BatchOut>], run: u32) -> Records {
    let mut rec = Records { run, ..Default::default() };
    let mut per_file_count: HashMap<String, usize> = HashMap::new();
    let mut prev_last: Option<usize> = None; // index into rec.sources of the last source of the previous batch
    for (b, out) in batches.iter().zip(outs) {
        let Some(out) = out else {
            prev_last = None;
            continue;
        };
        let file_of = |p: usize| b.pages.get(p.saturating_sub(1)).or(b.pages.first());
        let mut alias: HashMap<String, usize> = HashMap::new();
        let mut last_here: Option<usize> = None;
        for (key, src, cont, pages) in &out.sources {
            let starts_at_p1 = pages.is_empty() || pages.contains(&1);
            let files: Vec<String> = {
                let mut v: Vec<String> = Vec::new();
                let ps = if pages.is_empty() { vec![1] } else { pages.clone() };
                for p in ps {
                    if let Some(pr) = file_of(p) {
                        if !v.contains(&pr.file_id) {
                            v.push(pr.file_id.clone());
                        }
                    }
                }
                v
            };
            let same_title = |i: usize| {
                let t = norm(&src.title);
                !t.is_empty() && norm(&rec.sources[i].title) == t
            };
            let join = match prev_last {
                Some(i) if starts_at_p1 && (*cont || same_title(i)) => Some(i),
                _ => None,
            };
            let idx = if let Some(i) = join {
                let s = &mut rec.sources[i];
                for f in files {
                    if !s.file_ids.contains(&f) {
                        s.file_ids.push(f);
                    }
                }
                s.pages += pages.len().max(1) as u32;
                for sec in &src.sections {
                    if !s.sections.iter().any(|x| x.label == sec.label) {
                        s.sections.push(sec.clone());
                    }
                }
                fill(&mut s.title, &src.title);
                fill(&mut s.school, &src.school);
                fill(&mut s.date, &src.date);
                fill(&mut s.grade, &src.grade);
                if s.duration_min == 0 {
                    s.duration_min = src.duration_min;
                }
                if s.total_points == 0.0 {
                    s.total_points = src.total_points;
                }
                s.full_exam |= src.full_exam;
                i
            } else {
                let first = files.first().cloned().unwrap_or_else(|| b.pages[0].file_id.clone());
                let k = per_file_count.entry(first.clone()).or_insert(0);
                *k += 1;
                let name = b.pages.iter().find(|p| p.file_id == first).map(|p| p.file_name.clone()).unwrap_or_default();
                let mut s = src.clone();
                s.id = format!("{first}#{k}");
                s.file_ids = if files.is_empty() { vec![first] } else { files };
                s.name = name;
                s.pages = pages.len().max(1) as u32;
                s.run = run;
                rec.sources.push(s);
                rec.sources.len() - 1
            };
            alias.insert(key.clone(), idx);
            last_here = Some(idx);
        }
        for (key, page, q) in &out.questions {
            let idx = match alias.get(key).copied().or(last_here) {
                Some(i) => i,
                None => {
                    // Questions without any source: invent one for this batch.
                    let first = file_of(*page).map(|p| p.file_id.clone()).unwrap_or_default();
                    let k = per_file_count.entry(first.clone()).or_insert(0);
                    *k += 1;
                    rec.sources.push(SrcRec {
                        id: format!("{first}#{k}"),
                        file_ids: vec![first],
                        name: b.pages[0].file_name.clone(),
                        kind: "worksheet".into(),
                        pages: b.pages.len() as u32,
                        run,
                        ..Default::default()
                    });
                    let i = rec.sources.len() - 1;
                    alias.insert(key.clone(), i);
                    last_here = Some(i);
                    i
                }
            };
            let sid = rec.sources[idx].id.clone();
            // Overlapping photos: same number and text in the same source → keep one.
            if rec.questions.iter().any(|x| x.src == sid && x.n == q.n && norm(&x.text) == norm(&q.text)) {
                continue;
            }
            let mut q = q.clone();
            q.src = sid.clone();
            q.id = format!("{sid}/q{}", rec.questions.iter().filter(|x| x.src == sid).count() + 1);
            rec.questions.push(q);
        }
        prev_last = last_here;
    }
    rec
}

fn fill(dst: &mut String, src: &str) {
    if dst.is_empty() && !src.is_empty() {
        *dst = src.to_string();
    }
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

/// Old records + new: sources whose files were uploaded again are replaced by the new reading.
pub fn combine_records(prev: Option<Records>, new: Records) -> Records {
    let Some(mut prev) = prev else { return new };
    let new_files: HashSet<&String> = new.sources.iter().flat_map(|s| s.file_ids.iter()).collect();
    let dropped: HashSet<String> =
        prev.sources.iter().filter(|s| s.file_ids.iter().any(|f| new_files.contains(f))).map(|s| s.id.clone()).collect();
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

// ---------------------------------------------------------------- merge

/// A distinct `(chapter, topic, form)` as extracted.
#[derive(Clone, Debug)]
pub struct Raw {
    pub id: String,
    pub chapter: String,
    pub topic: String,
    pub form: String,
    pub qids: Vec<usize>,
}

pub fn raw_triples(qs: &[QRec]) -> Vec<Raw> {
    let mut map: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut out: Vec<Raw> = Vec::new();
    for (i, q) in qs.iter().enumerate() {
        let key = (norm(&q.chapter), norm(&q.topic), norm(&q.form));
        let idx = *map.entry(key).or_insert_with(|| {
            out.push(Raw {
                id: format!("r{}", out.len() + 1),
                chapter: q.chapter.clone(),
                topic: q.topic.clone(),
                form: q.form.clone(),
                qids: Vec::new(),
            });
            out.len() - 1
        });
        out[idx].qids.push(i);
    }
    out
}

pub const MERGE_SYSTEM: &str = r#"You organise exam questions extracted from Vietnamese school test papers into a curriculum and exam structures. Output JSON only; all human-readable fields in Vietnamese.

Input: subject, grade hints, the student's goal, optional existing curriculum (keep its titles and order wherever they still fit), the source documents, and RAW triples "chapter > topic > form" with question counts, levels and a sample question.

1. curriculum: chapters (chương) in textbook order; each has topics (bài/chủ đề) in order; each topic has forms (dạng bài). Merge duplicates and near-duplicates (same task written differently), fix vague names, keep forms specific and useful for practice (2-8 per topic is typical). Every RAW id must appear in exactly one form's "raw" list. summary: one sentence on what the topic covers. method: the standard solving method for the form as 2-5 short numbered steps, LaTeX in $...$.
2. structures: group the full exam papers (fullExam=yes) that share the same format (same kinds of sections, similar counts and timing) into one structure each. title: short Vietnamese description, e.g. "Đề giữa kỳ I (trắc nghiệm + tự luận, 90 phút)". sources: the S-ids. Ignore non-exam documents. [] if there are no full exams.
3. progress: from the topics covered by the most recent documents (higher run, later dates, "giữa kỳ"/"cuối kỳ" in titles) infer what the school has already taught. Refer to topics by position "chapter.topic", 1-based in YOUR curriculum (e.g. "2.3"). covered: all topics taught so far; current: the topic being taught now (the last one covered) or ""; confidence 0..1; note: one Vietnamese sentence explaining the inference.
4. grade: the school grade ("9"), "" if unknown."#;

fn merge_schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["grade", "curriculum", "structures", "progress"],
        "properties": {
            "grade": s,
            "curriculum": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["title", "topics"],
                "properties": {"title": s, "topics": {"type": "array", "items": {"type": "object",
                    "additionalProperties": false, "required": ["title", "summary", "forms"],
                    "properties": {"title": s, "summary": s, "forms": {"type": "array", "items": {"type": "object",
                        "additionalProperties": false, "required": ["title", "method", "raw"],
                        "properties": {"title": s, "method": s, "raw": {"type": "array", "items": s}}}}}}}}}},
            "structures": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["title", "sources"],
                "properties": {"title": s, "sources": {"type": "array", "items": s}}}},
            "progress": {"type": "object", "additionalProperties": false,
                "required": ["covered", "current", "confidence", "note"],
                "properties": {"covered": {"type": "array", "items": s}, "current": s,
                    "confidence": {"type": "number"}, "note": s}}
        }
    })
}

fn level_counts(qs: &[&QRec]) -> BTreeMap<&'static str, usize> {
    let mut m: BTreeMap<&'static str, usize> = LEVELS.iter().map(|l| (*l, 0)).collect();
    for q in qs {
        let l = LEVELS.iter().find(|l| **l == q.level).copied().unwrap_or("th");
        *m.get_mut(l).unwrap() += 1;
    }
    m
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// The compact text the merge model reads. `sids` maps source id → `S<n>`.
pub fn merge_prompt(input: &IngestInput, rec: &Records, raws: &[Raw], sids: &HashMap<String, String>) -> String {
    let mut p = String::new();
    p.push_str(&format!("SUBJECT: {}\n", input.subject));
    if let Some(g) = input.goal.as_deref().filter(|g| !g.trim().is_empty()) {
        p.push_str(&format!("GOAL: {g}\n"));
    }
    let grades: HashSet<&str> = rec.sources.iter().map(|s| s.grade.as_str()).filter(|g| !g.is_empty()).collect();
    if !grades.is_empty() {
        p.push_str(&format!("GRADES SEEN: {}\n", grades.into_iter().collect::<Vec<_>>().join(", ")));
    }
    if let Some(cur) = input.existing.as_ref().and_then(|k| k["curriculum"].as_array()).filter(|c| !c.is_empty()) {
        p.push_str("EXISTING CURRICULUM:\n");
        for c in cur {
            p.push_str(&format!("- {}\n", s_of(&c["title"])));
            for t in c["topics"].as_array().into_iter().flatten() {
                let forms: Vec<String> =
                    t["forms"].as_array().into_iter().flatten().map(|f| s_of(&f["title"])).collect();
                p.push_str(&format!("  - {} [{}]\n", s_of(&t["title"]), forms.join("; ")));
            }
        }
    }
    p.push_str("SOURCES (id | kind | fullExam | run | title | school | date | minutes | points | sections: label kind count):\n");
    for s in &rec.sources {
        let qs: Vec<&QRec> = rec.questions.iter().filter(|q| q.src == s.id).collect();
        let mut secs: Vec<(String, String, usize)> = Vec::new();
        for q in &qs {
            match secs.iter_mut().find(|x| x.0 == q.section) {
                Some(x) => x.2 += 1,
                None => secs.push((q.section.clone(), q.kind.clone(), 1)),
            }
        }
        let secs: Vec<String> = secs.iter().map(|(l, k, n)| format!("{} {k} {n}", if l.is_empty() { "-" } else { l })).collect();
        p.push_str(&format!(
            "{} | {} | {} | {} | {} | {} | {} | {} | {} | {}\n",
            sids[&s.id],
            s.kind,
            if s.full_exam { "yes" } else { "no" },
            s.run,
            clip(&s.title, 80),
            s.school,
            s.date,
            s.duration_min,
            s.total_points,
            secs.join("; ")
        ));
    }
    p.push_str("RAW (id | chapter > topic > form | count | levels nb/th/vd/vdc | sample):\n");
    for r in raws {
        let qs: Vec<&QRec> = r.qids.iter().map(|i| &rec.questions[*i]).collect();
        let lc = level_counts(&qs);
        let sample = qs.iter().find(|q| !q.has_figure && q.text.chars().count() > 15).or(qs.first()).map(|q| q.text.as_str()).unwrap_or("");
        p.push_str(&format!(
            "{} | {} > {} > {} | {} | {}/{}/{}/{} | {}\n",
            r.id,
            r.chapter,
            r.topic,
            r.form,
            qs.len(),
            lc["nb"],
            lc["th"],
            lc["vd"],
            lc["vdc"],
            clip(sample, 110)
        ));
    }
    p
}

/// Curriculum built straight from the raw triples (when the merge call fails).
pub fn fallback_merge(raws: &[Raw]) -> Value {
    let mut chapters: Vec<(String, Vec<(String, Vec<(String, String)>)>)> = Vec::new();
    for r in raws {
        let ch = if r.chapter.is_empty() { "Khác".to_string() } else { r.chapter.clone() };
        let ci = match chapters.iter().position(|c| norm(&c.0) == norm(&ch)) {
            Some(i) => i,
            None => {
                chapters.push((ch, Vec::new()));
                chapters.len() - 1
            }
        };
        let tp = if r.topic.is_empty() { "Khác".to_string() } else { r.topic.clone() };
        let topics = &mut chapters[ci].1;
        let ti = match topics.iter().position(|t| norm(&t.0) == norm(&tp)) {
            Some(i) => i,
            None => {
                topics.push((tp, Vec::new()));
                topics.len() - 1
            }
        };
        let fm = if r.form.is_empty() { r.topic.clone() } else { r.form.clone() };
        topics[ti].1.push((fm, r.id.clone()));
    }
    json!({
        "grade": "",
        "curriculum": chapters.iter().map(|(c, ts)| json!({"title": c, "topics": ts.iter().map(|(t, fs)| json!({
            "title": t, "summary": "", "forms": fs.iter().map(|(f, r)| json!({"title": f, "method": "", "raw": [r]})).collect::<Vec<_>>()
        })).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "structures": [],
        "progress": {"covered": [], "current": "", "confidence": 0, "note": ""}
    })
}

// ---------------------------------------------------------------- build knowledge

fn round(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

/// Ids stable across re-ingests: an item whose normalized title matches the old one keeps its id.
struct IdPool {
    old: HashMap<String, String>,
    used: HashSet<String>,
    next: usize,
}

impl IdPool {
    fn new(old: Vec<(String, String)>, prefix: &str, num_of_id: impl Fn(&str) -> Option<usize>) -> Self {
        let next = old.iter().filter_map(|(_, id)| num_of_id(id)).max().unwrap_or(0) + 1;
        let _ = prefix;
        IdPool { old: old.into_iter().map(|(t, id)| (norm(&t), id)).collect(), used: HashSet::new(), next }
    }
    fn take(&mut self, title: &str, make: impl Fn(usize) -> String) -> String {
        if let Some(id) = self.old.get(&norm(title)) {
            if self.used.insert(id.clone()) {
                return id.clone();
            }
        }
        loop {
            let id = make(self.next);
            self.next += 1;
            if !self.old.values().any(|v| *v == id) && self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

fn last_num(id: &str, marker: char) -> Option<usize> {
    id.rsplit('.').next()?.strip_prefix(marker)?.parse().ok()
}

fn section_default_title(kind: &str) -> &'static str {
    match kind {
        "mcq" => "Trắc nghiệm",
        "truefalse" => "Trắc nghiệm đúng sai",
        "short" => "Trả lời ngắn",
        _ => "Tự luận",
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

/// Assemble `knowledge.json` from records + merge output. Pure function (tested directly).
pub fn build_knowledge(input: &IngestInput, rec: &Records, raws: &[Raw], merged: &Value, sids: &HashMap<String, String>, now: &str) -> Value {
    let existing = input.existing.as_ref();
    let raw_by_id: HashMap<&str, &Raw> = raws.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut q_form: Vec<Option<(usize, usize, usize)>> = vec![None; rec.questions.len()];

    // --- curriculum skeleton from the model
    struct F {
        title: String,
        method: String,
        qs: Vec<usize>,
    }
    struct T {
        title: String,
        summary: String,
        forms: Vec<F>,
    }
    struct C {
        title: String,
        topics: Vec<T>,
    }
    let mut chapters: Vec<C> = Vec::new();
    let mut seen_raw: HashSet<String> = HashSet::new();
    for c in merged["curriculum"].as_array().into_iter().flatten() {
        let mut ch = C { title: s_of(&c["title"]), topics: Vec::new() };
        for t in c["topics"].as_array().into_iter().flatten() {
            let mut tp = T { title: s_of(&t["title"]), summary: s_of(&t["summary"]), forms: Vec::new() };
            for f in t["forms"].as_array().into_iter().flatten() {
                let mut qs = Vec::new();
                for r in f["raw"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    if let Some(raw) = raw_by_id.get(r) {
                        if seen_raw.insert(r.to_string()) {
                            qs.extend(raw.qids.iter().copied());
                        }
                    }
                }
                tp.forms.push(F { title: s_of(&f["title"]), method: s_of(&f["method"]), qs });
            }
            ch.topics.push(tp);
        }
        chapters.push(ch);
    }
    // Raws the model forgot: attach by matching titles, else under "Khác".
    for r in raws.iter().filter(|r| !seen_raw.contains(&r.id)) {
        let mut placed = false;
        'outer: for ch in chapters.iter_mut() {
            for tp in ch.topics.iter_mut() {
                if let Some(f) = tp.forms.iter_mut().find(|f| norm(&f.title) == norm(&r.form)) {
                    f.qs.extend(r.qids.iter().copied());
                    placed = true;
                    break 'outer;
                }
                if norm(&tp.title) == norm(&r.topic) {
                    tp.forms.push(F { title: if r.form.is_empty() { r.topic.clone() } else { r.form.clone() }, method: String::new(), qs: r.qids.clone() });
                    placed = true;
                    break 'outer;
                }
            }
        }
        if !placed {
            let ci = match chapters.iter().position(|c| c.title == "Khác") {
                Some(i) => i,
                None => {
                    chapters.push(C { title: "Khác".into(), topics: Vec::new() });
                    chapters.len() - 1
                }
            };
            let title = if r.topic.is_empty() { "Khác".to_string() } else { r.topic.clone() };
            chapters[ci].topics.push(T {
                title,
                summary: String::new(),
                forms: vec![F { title: if r.form.is_empty() { r.topic.clone() } else { r.form.clone() }, method: String::new(), qs: r.qids.clone() }],
            });
        }
    }
    // Drop empty forms/topics/chapters.
    for ch in chapters.iter_mut() {
        for tp in ch.topics.iter_mut() {
            tp.forms.retain(|f| !f.qs.is_empty());
        }
        ch.topics.retain(|t| !t.forms.is_empty());
    }
    chapters.retain(|c| !c.topics.is_empty());

    // --- old ids
    let old_cur: Vec<Value> = existing.and_then(|k| k["curriculum"].as_array().cloned()).unwrap_or_default();
    let mut c_pool = IdPool::new(
        old_cur.iter().map(|c| (s_of(&c["title"]), s_of(&c["id"]))).collect(),
        "c",
        |id| id.strip_prefix('c').and_then(|n| n.parse().ok()),
    );
    let old_topics = |cid: &str| -> Vec<(String, String)> {
        old_cur
            .iter()
            .filter(|c| s_of(&c["id"]) == cid)
            .flat_map(|c| c["topics"].as_array().cloned().unwrap_or_default())
            .map(|t| (s_of(&t["title"]), s_of(&t["id"])))
            .collect()
    };
    let old_forms = |tid: &str| -> Vec<(String, String)> {
        old_cur
            .iter()
            .flat_map(|c| c["topics"].as_array().cloned().unwrap_or_default())
            .filter(|t| s_of(&t["id"]) == tid)
            .flat_map(|t| t["forms"].as_array().cloned().unwrap_or_default())
            .map(|f| (s_of(&f["title"]), s_of(&f["id"])))
            .collect()
    };

    let total_q = rec.questions.len().max(1);
    let src_by_id: HashMap<&str, &SrcRec> = rec.sources.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut topic_ids_by_pos: HashMap<String, String> = HashMap::new();
    let mut curriculum: Vec<Value> = Vec::new();
    let mut ids_tf: Vec<Vec<(String, Vec<String>)>> = Vec::new(); // [chapter][topic] = (topicId, formIds)
    for (ci, ch) in chapters.iter().enumerate() {
        let cid = c_pool.take(&ch.title, |n| format!("c{n}"));
        let mut t_pool = IdPool::new(old_topics(&cid), "t", |id| last_num(id, 't'));
        let mut topics = Vec::new();
        let mut row = Vec::new();
        for (ti, tp) in ch.topics.iter().enumerate() {
            let tid = t_pool.take(&tp.title, |n| format!("{cid}.t{n}"));
            topic_ids_by_pos.insert(format!("{}.{}", ci + 1, ti + 1), tid.clone());
            let mut f_pool = IdPool::new(old_forms(&tid), "f", |id| last_num(id, 'f'));
            let mut forms = Vec::new();
            let mut fids = Vec::new();
            let mut tcount = 0;
            for (fi, f) in tp.forms.iter().enumerate() {
                let fid = f_pool.take(&f.title, |n| format!("{tid}.f{n}"));
                let qs: Vec<&QRec> = f.qs.iter().map(|i| &rec.questions[*i]).collect();
                for i in &f.qs {
                    q_form[*i] = Some((ci, ti, fi));
                }
                tcount += qs.len();
                let ex = qs
                    .iter()
                    .filter(|q| !q.has_figure && q.text.chars().count() > 15)
                    .max_by_key(|q| LEVELS.iter().position(|l| *l == q.level).unwrap_or(1) * 10 + q.text.len().min(9))
                    .or(qs.first());
                let refs: Vec<Value> = qs
                    .iter()
                    .take(8)
                    .map(|q| {
                        let fid = src_by_id.get(q.src.as_str()).and_then(|s| s.file_ids.first().cloned()).unwrap_or_default();
                        json!({"fileId": fid, "q": q.n})
                    })
                    .collect();
                let mut fv = json!({
                    "id": fid, "title": f.title, "method": f.method, "count": qs.len(),
                    "levels": level_counts(&qs), "refs": refs,
                });
                if let Some(e) = ex {
                    fv["example"] = json!({"md": e.text});
                    if !e.answer.is_empty() {
                        fv["example"]["answer"] = json!(e.answer);
                    }
                }
                fids.push(fid);
                forms.push(fv);
            }
            topics.push(json!({
                "id": tid, "title": tp.title, "summary": tp.summary, "count": tcount,
                "share": round(tcount as f64 / total_q as f64, 3), "forms": forms,
            }));
            row.push((tid, fids));
        }
        ids_tf.push(row);
        curriculum.push(json!({"id": cid, "title": ch.title, "topics": topics}));
    }
    // Keep old curriculum items that no longer have records (knowledge made before records existed).
    if input.previous.as_ref().map(|p| p.questions.is_empty()).unwrap_or(true) {
        for oc in &old_cur {
            let oid = s_of(&oc["id"]);
            match curriculum.iter_mut().find(|c| c["id"] == oid) {
                None => curriculum.push(oc.clone()),
                Some(c) => {
                    for ot in oc["topics"].as_array().into_iter().flatten() {
                        let tid = &ot["id"];
                        let topics = c["topics"].as_array_mut().unwrap();
                        if !topics.iter().any(|t| &t["id"] == tid) {
                            topics.push(ot.clone());
                        }
                    }
                }
            }
        }
    }
    let qid = |i: usize| -> Option<(String, String)> {
        let (c, t, f) = q_form[i]?;
        let (tid, fids) = &ids_tf[c][t];
        Some((tid.clone(), fids[f].clone()))
    };

    // --- structures
    let old_structs: Vec<Value> = existing.and_then(|k| k["structures"].as_array().cloned()).unwrap_or_default();
    let mut s_pool = IdPool::new(
        old_structs.iter().map(|s| (s_of(&s["title"]), s_of(&s["id"]))).collect(),
        "s",
        |id| id.strip_prefix('s').and_then(|n| n.parse().ok()),
    );
    let by_sid: HashMap<&str, &str> = sids.iter().map(|(k, v)| (v.as_str(), k.as_str())).collect();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut covered: HashSet<String> = HashSet::new();
    for s in merged["structures"].as_array().into_iter().flatten() {
        let ids: Vec<String> = s["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str())
            .filter_map(|x| by_sid.get(x).map(|s| s.to_string()))
            .filter(|id| rec.questions.iter().any(|q| &q.src == id))
            .filter(|id| covered.insert(id.clone()))
            .collect();
        if !ids.is_empty() {
            groups.push((s_of(&s["title"]), ids));
        }
    }
    for s in rec.sources.iter().filter(|s| s.full_exam && !covered.contains(&s.id)) {
        if rec.questions.iter().any(|q| q.src == s.id) {
            let title = if s.title.is_empty() { "Đề kiểm tra".to_string() } else { s.title.clone() };
            groups.push((title, vec![s.id.clone()]));
        }
    }
    let mut structures: Vec<Value> = Vec::new();
    for (title, ids) in groups {
        // Representative: the source with the most questions.
        let rep = ids
            .iter()
            .max_by_key(|id| rec.questions.iter().filter(|q| &&q.src == id).count())
            .cloned()
            .unwrap();
        let srec = src_by_id[rep.as_str()];
        let idx: Vec<usize> = (0..rec.questions.len()).filter(|i| rec.questions[*i].src == rep).collect();
        let qs: Vec<&QRec> = idx.iter().map(|i| &rec.questions[*i]).collect();
        let pts = fill_points(&qs, srec.total_points);
        let mut labels: Vec<String> = Vec::new();
        for q in &qs {
            if !labels.contains(&q.section) {
                labels.push(q.section.clone());
            }
        }
        let mut sections = Vec::new();
        let mut matrix: BTreeMap<(usize, String, String, usize), (usize, f64)> = BTreeMap::new();
        for (si, label) in labels.iter().enumerate() {
            let sid = ((b'A' + (si as u8).min(25)) as char).to_string();
            let members: Vec<usize> = (0..qs.len()).filter(|j| &qs[*j].section == label).collect();
            let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            for j in &members {
                *kinds.entry(qs[*j].kind.as_str()).or_insert(0) += 1;
            }
            let kind = kinds.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.to_string()).unwrap_or("essay".into());
            let sec_rec = srec.sections.iter().find(|s| &s.label == label);
            let spoints: f64 = members.iter().map(|j| pts[*j]).sum();
            let title = sec_rec.map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| section_default_title(&kind).to_string());
            sections.push(json!({
                "id": sid, "title": title, "kind": kind, "count": members.len(),
                "pointsEach": round(spoints / members.len().max(1) as f64, 2),
                "points": round(spoints, 2),
                "instructions": sec_rec.map(|s| s.instructions.clone()).unwrap_or_default(),
            }));
            for j in members {
                let (tid, fid) = qid(idx[j]).unwrap_or_default();
                let lv = LEVELS.iter().position(|l| *l == qs[j].level).unwrap_or(1);
                let e = matrix.entry((si, tid, fid, lv)).or_insert((0, 0.0));
                e.0 += 1;
                e.1 += pts[j];
            }
        }
        let matrix: Vec<Value> = matrix
            .into_iter()
            .map(|((si, tid, fid, lv), (n, p))| {
                json!({"section": ((b'A' + (si as u8).min(25)) as char).to_string(), "topicId": tid, "formId": fid,
                       "level": LEVELS[lv], "count": n, "points": round(p, 2)})
            })
            .collect();
        let total: f64 = pts.iter().sum();
        let duration = ids.iter().filter_map(|id| src_by_id.get(id.as_str())).map(|s| s.duration_min).find(|d| *d > 0).unwrap_or(90);
        let mut based: Vec<String> = Vec::new();
        for id in &ids {
            if let Some(f) = src_by_id.get(id.as_str()).and_then(|s| s.file_ids.first()) {
                if !based.contains(f) {
                    based.push(f.clone());
                }
            }
        }
        structures.push(json!({
            "id": s_pool.take(&title, |n| format!("s{n}")), "title": title, "basedOn": based,
            "durationMin": duration, "totalPoints": round(total, 2), "sections": sections, "matrix": matrix,
        }));
    }
    if input.previous.as_ref().map(|p| p.questions.is_empty()).unwrap_or(true) {
        for os in &old_structs {
            if !structures.iter().any(|s| s["id"] == os["id"]) {
                structures.push(os.clone());
            }
        }
    }

    // --- progress
    let pr = &merged["progress"];
    let mut cov: Vec<String> = Vec::new();
    for p in pr["covered"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
        if let Some(id) = topic_ids_by_pos.get(p.trim()) {
            if !cov.contains(id) {
                cov.push(id.clone());
            }
        }
    }
    let current = pr["current"].as_str().and_then(|p| topic_ids_by_pos.get(p.trim()).cloned());
    let school_progress = if cov.is_empty() && current.is_none() {
        existing.map(|k| k["schoolProgress"].clone()).filter(|v| v.is_object()).unwrap_or_else(|| {
            json!({"coveredTopicIds": [], "currentTopicId": null, "confidence": 0.0, "note": ""})
        })
    } else {
        json!({"coveredTopicIds": cov, "currentTopicId": current,
               "confidence": round(pr["confidence"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0), 2),
               "note": s_of(&pr["note"])})
    };

    // --- sources + stats
    let sources: Vec<Value> = rec
        .sources
        .iter()
        .map(|s| {
            let n = rec.questions.iter().filter(|q| q.src == s.id).count();
            let opt = |x: &str| if x.is_empty() { Value::Null } else { json!(x) };
            json!({"fileId": s.file_ids.first().cloned().unwrap_or_default(), "fileIds": s.file_ids, "name": s.name,
                   "kind": s.kind, "title": if s.title.is_empty() { s.name.clone() } else { s.title.clone() },
                   "school": opt(&s.school), "date": opt(&s.date), "pages": s.pages, "questions": n})
        })
        .collect();
    let all: Vec<&QRec> = rec.questions.iter().collect();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for q in &all {
        *by_kind.entry(q.kind.clone()).or_insert(0) += 1;
    }
    let grade = {
        let g = s_of(&merged["grade"]);
        if !g.is_empty() {
            g
        } else {
            rec.sources
                .iter()
                .map(|s| s.grade.clone())
                .find(|g| !g.is_empty())
                .or_else(|| existing.map(|k| s_of(&k["grade"])))
                .unwrap_or_default()
        }
    };
    let goal = input
        .goal
        .clone()
        .filter(|g| !g.trim().is_empty())
        .or_else(|| existing.and_then(|k| k["goal"].as_str().map(String::from)));
    json!({
        "v": 1, "folderId": input.folder_id, "subject": input.subject, "grade": grade, "goal": goal,
        "updatedAt": now, "sources": sources, "curriculum": curriculum, "structures": structures,
        "schoolProgress": school_progress,
        "stats": {"questions": rec.questions.len(), "byLevel": level_counts(&all), "byKind": by_kind},
    })
}

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
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---------------------------------------------------------------- the job

fn page_span(b: &Batch, total: usize) -> String {
    let last = b.first_page + b.pages.len() - 1;
    if last == b.first_page {
        format!("trang {}/{total}", b.first_page)
    } else {
        format!("trang {}–{last}/{total}", b.first_page)
    }
}

/// Run one ingest. Emits `ingest` events (reading → merging → done | error | cancelled), writes
/// `<folder_dir>/knowledge.json` and `<folder_dir>/ingest/records.json`, returns the knowledge.
pub async fn run_ingest(
    llm: Arc<dyn Llm>,
    opts: IngestOptions,
    input: IngestInput,
    folder_dir: PathBuf,
    job_id: String,
    emit: Emit,
    cancel: watch::Receiver<bool>,
) -> Result<Value, IngestError> {
    let batches = match plan_batches(&input.files, &opts) {
        Ok(b) => b,
        Err(e) => {
            emit(ingest_event(&input.folder_id, &job_id, "error", 0, 0, &e, None, Some(0.0)));
            return Err(IngestError::Failed(e));
        }
    };
    let total: usize = batches.iter().map(|b| b.pages.len()).sum();
    let ctx = Arc::new(Ctx {
        folder_id: input.folder_id.clone(),
        job_id: job_id.clone(),
        total,
        emit: emit.clone(),
        progress: Mutex::new(Progress { done: 0, questions: 0, topics: HashSet::new(), usage: Usage::default() }),
    });
    let r = run_inner(llm, &opts, &input, &folder_dir, &batches, ctx.clone(), cancel).await;
    match &r {
        Ok(k) => {
            let n = k["stats"]["questions"].as_u64().unwrap_or(0) as usize;
            let topics: usize = k["curriculum"].as_array().into_iter().flatten().map(|c| c["topics"].as_array().map(|t| t.len()).unwrap_or(0)).sum();
            let ns = k["structures"].as_array().map(|a| a.len()).unwrap_or(0);
            ctx.progress.lock().unwrap().questions = n;
            let note = format!("Xong: {n} câu, {topics} chủ đề, {ns} cấu trúc đề");
            let mut ev = ingest_event(&ctx.folder_id, &ctx.job_id, "done", total, total, &note, Some((n, topics)), Some(ctx.cost()));
            ev["found"]["topics"] = json!(topics);
            emit(ev);
        }
        Err(IngestError::Cancelled) => ctx.send("cancelled", "Đã dừng nạp đề"),
        Err(IngestError::Failed(m)) => ctx.send("error", &format!("Lỗi: {}", clip(m, 160))),
    }
    r
}

async fn run_inner(
    llm: Arc<dyn Llm>,
    opts: &IngestOptions,
    input: &IngestInput,
    folder_dir: &Path,
    batches: &[Batch],
    ctx: Arc<Ctx>,
    cancel: watch::Receiver<bool>,
) -> Result<Value, IngestError> {
    let total = ctx.total;
    ctx.send("reading", &format!("Chuẩn bị đọc {total} trang"));
    let schema = extract_schema();
    // Index-based closure: a `|b: &Batch|` closure trips the higher-ranked lifetime check of tokio::spawn.
    let futs = (0..batches.len()).map(|i| {
        let b = &batches[i];
        let llm = llm.clone();
        let ctx = ctx.clone();
        let cancel = cancel.clone();
        let schema = schema.clone();
        async move {
            let first_name = b.pages.first().map(|p| p.file_name.clone()).unwrap_or_default();
            ctx.send("reading", &format!("Đang đọc {} ({}…)", page_span(b, total), first_name));
            let req = LlmRequest {
                tag: "extract",
                model: opts.model.clone(),
                effort: opts.effort.clone(),
                system: EXTRACT_SYSTEM.to_string(),
                parts: b.parts.clone(),
                schema: Some(("exam_pages".into(), schema)),
                max_tokens: opts.max_tokens_extract,
            };
            let r = call_json(llm.as_ref(), req, opts.retries, opts.retry_base_ms, &ctx, cancel, &|v: &Value| {
                parse_batch(v).map(|_| ())
            })
            .await;
            let out = match r {
                Ok(v) => Ok(parse_batch(&v).unwrap_or_default()),
                Err(e) => Err(e),
            };
            let note = {
                let mut p = ctx.progress.lock().unwrap();
                p.done += b.pages.len();
                match &out {
                    Ok(o) => {
                        p.questions += o.questions.len();
                        for (_, _, q) in &o.questions {
                            p.topics.insert(norm(&q.topic));
                        }
                        let title = o.sources.iter().map(|s| s.1.title.clone()).find(|t| !t.is_empty()).unwrap_or(first_name);
                        format!("Đã đọc {}: {} · {} câu", page_span(b, total), clip(&title, 60), o.questions.len())
                    }
                    Err(IngestError::Cancelled) => String::new(),
                    Err(IngestError::Failed(_)) => format!("Không đọc được {}, bỏ qua", page_span(b, total)),
                }
            };
            if !note.is_empty() {
                ctx.send("reading", &note);
            }
            (b.index, out)
        }
    });
    let mut stream = futures_util::stream::iter(futs).buffer_unordered(opts.concurrency.max(1));
    let mut outs: Vec<Option<BatchOut>> = vec![None; batches.len()];
    let mut failures = 0;
    let mut last_err = String::new();
    while let Some((i, r)) = stream.next().await {
        match r {
            Ok(o) => outs[i] = Some(o),
            Err(IngestError::Cancelled) => return Err(IngestError::Cancelled),
            Err(IngestError::Failed(m)) => {
                failures += 1;
                last_err = m;
            }
        }
    }
    drop(stream);
    if *cancel.borrow() {
        return Err(IngestError::Cancelled);
    }
    if failures == batches.len() && !batches.is_empty() {
        return Err(IngestError::Failed(format!("không đọc được trang nào ({last_err})")));
    }

    // --- stitch + records
    let run = input.previous.as_ref().map(|p| p.run).unwrap_or(0) + 1;
    let new = stitch(batches, &outs, run);
    let rec = combine_records(input.previous.clone(), new);
    if rec.questions.is_empty() {
        return Err(IngestError::Failed("không tìm thấy câu hỏi nào trong tệp đã tải".into()));
    }
    write_atomic(&folder_dir.join("ingest").join("records.json"), &serde_json::to_vec(&rec).unwrap_or_default())
        .map_err(|e| IngestError::Failed(format!("ghi records: {e}")))?;

    // --- merge
    let raws = raw_triples(&rec.questions);
    let sids: HashMap<String, String> = rec.sources.iter().enumerate().map(|(i, s)| (s.id.clone(), format!("S{}", i + 1))).collect();
    ctx.send(
        "merging",
        &format!("Đang gộp {} câu từ {} tài liệu thành giáo trình và ma trận đề", rec.questions.len(), rec.sources.len()),
    );
    let raw_ids: HashSet<String> = raws.iter().map(|r| r.id.clone()).collect();
    let req = LlmRequest {
        tag: "merge",
        model: opts.merge_model.clone(),
        effort: opts.merge_effort.clone(),
        system: MERGE_SYSTEM.to_string(),
        parts: vec![Part::Text(merge_prompt(input, &rec, &raws, &sids))],
        schema: Some(("curriculum".into(), merge_schema())),
        max_tokens: opts.max_tokens_merge,
    };
    let merged = match call_json(llm.as_ref(), req, opts.retries, opts.retry_base_ms, &ctx, cancel.clone(), &|v: &Value| {
        let cur = v["curriculum"].as_array().ok_or("thiếu curriculum")?;
        let mut used = 0;
        for f in cur.iter().flat_map(|c| c["topics"].as_array().into_iter().flatten()).flat_map(|t| t["forms"].as_array().into_iter().flatten()) {
            used += f["raw"].as_array().into_iter().flatten().filter(|r| r.as_str().is_some_and(|r| raw_ids.contains(r))).count();
        }
        // Accept small omissions (fixed in Rust), reject a curriculum that ignores most raws.
        if used * 2 < raw_ids.len() {
            return Err(format!("curriculum dùng {used}/{} raw", raw_ids.len()));
        }
        Ok(())
    })
    .await
    {
        Ok(v) => v,
        Err(IngestError::Cancelled) => return Err(IngestError::Cancelled),
        Err(IngestError::Failed(m)) => {
            tracing::warn!("merge failed, using raw curriculum: {m}");
            ctx.send("merging", "Model gộp lỗi, dựng giáo trình trực tiếp từ các câu đã đọc");
            fallback_merge(&raws)
        }
    };
    if *cancel.borrow() {
        return Err(IngestError::Cancelled);
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let k = build_knowledge(input, &rec, &raws, &merged, &sids, &now);
    let errs = validate_knowledge(&k);
    if !errs.is_empty() {
        tracing::warn!("knowledge validation: {}", errs.join("; "));
    }
    write_atomic(&folder_dir.join("knowledge.json"), &serde_json::to_vec_pretty(&k).unwrap_or_default())
        .map_err(|e| IngestError::Failed(format!("ghi knowledge.json: {e}")))?;
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(i: usize, files: &[&str]) -> Batch {
        Batch {
            index: i,
            first_page: 1,
            pages: files.iter().map(|f| PageRef { file_id: f.to_string(), file_name: format!("{f}.jpg"), page_in_file: 1 }).collect(),
            parts: vec![],
        }
    }

    fn q(n: &str, text: &str) -> QRec {
        QRec { n: n.into(), text: text.into(), kind: "mcq".into(), level: "th".into(), ..Default::default() }
    }

    fn src(title: &str) -> SrcRec {
        SrcRec { title: title.into(), kind: "exam".into(), full_exam: true, ..Default::default() }
    }

    #[test]
    fn stitch_joins_continued_documents_and_dedupes_overlap() {
        let b = vec![batch(0, &["p1", "p2"]), batch(1, &["p3", "p4"])];
        let o1 = BatchOut {
            sources: vec![("A".into(), src("Đề 1"), false, vec![1, 2])],
            questions: vec![("A".into(), 1, q("1", "Câu một")), ("A".into(), 2, q("6", "Căn bậc ba của -27"))],
        };
        let o2 = BatchOut {
            sources: vec![("A".into(), src(""), true, vec![1]), ("B".into(), src("Phiếu bài tập"), false, vec![2])],
            questions: vec![
                ("A".into(), 1, q("6", "Căn bậc ba của  -27")), // overlap of the two photos
                ("A".into(), 1, q("7", "Cặp số nghiệm")),
                ("B".into(), 2, q("1", "Bài một")),
            ],
        };
        let r = stitch(&b, &[Some(o1), Some(o2)], 1);
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.sources[0].id, "p1#1");
        assert_eq!(r.sources[0].file_ids, vec!["p1", "p2", "p3"]);
        assert_eq!(r.sources[0].pages, 3);
        assert_eq!(r.sources[1].id, "p4#1");
        let ns: Vec<&str> = r.questions.iter().filter(|q| q.src == "p1#1").map(|q| q.n.as_str()).collect();
        assert_eq!(ns, ["1", "6", "7"]);
        assert_eq!(r.questions[2].id, "p1#1/q3");
        // A failed batch in between breaks the chain.
        let o3 = BatchOut { sources: vec![("A".into(), src(""), true, vec![1])], questions: vec![("A".into(), 1, q("9", "x"))] };
        let b3 = vec![batch(0, &["p1"]), batch(1, &["p2"]), batch(2, &["p3"])];
        let r = stitch(&b3, &[Some(BatchOut::default()), None, Some(o3)], 1);
        assert_eq!(r.sources.len(), 1);
        assert_eq!(r.sources[0].id, "p3#1");
    }

    #[test]
    fn combine_replaces_reuploaded_files() {
        let mut old = Records { run: 1, ..Default::default() };
        old.sources.push(SrcRec { id: "a#1".into(), file_ids: vec!["a".into()], ..Default::default() });
        old.sources.push(SrcRec { id: "b#1".into(), file_ids: vec!["b".into()], ..Default::default() });
        old.questions.push(QRec { id: "a#1/q1".into(), src: "a#1".into(), ..Default::default() });
        old.questions.push(QRec { id: "b#1/q1".into(), src: "b#1".into(), ..Default::default() });
        let mut new = Records { run: 2, ..Default::default() };
        new.sources.push(SrcRec { id: "b#1".into(), file_ids: vec!["b".into()], ..Default::default() });
        new.questions.push(QRec { id: "b#1/q1".into(), src: "b#1".into(), text: "new".into(), ..Default::default() });
        let r = combine_records(Some(old), new);
        assert_eq!(r.run, 2);
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.questions.iter().filter(|q| q.src == "b#1").count(), 1);
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
