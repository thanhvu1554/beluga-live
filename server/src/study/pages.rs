//! Page reading: every page (image, or one page of a PDF) is an independent unit.
//!
//! - **Plan**: one unit per image, one per PDF page (lopdf split; an unsplittable PDF stays one unit).
//! - **Cache**: a unit's reading is stored under `ingest/pages/<sha256(page sha | prompt version | model)>.json`,
//!   so re-ingesting, adding files or resuming a cancelled job never pays twice for a page.
//! - **Read**: the cheap model reads with a strict JSON shape; the reply is checked locally (shape,
//!   truncation, zero questions on a non-empty page, numbering jumps or wrong reading order, unbalanced
//!   `$`/braces, Vietnamese without diacritics, runaway repetition). A broken reply is retried once; a
//!   page that still fails or looks suspicious goes to the stronger model (without one: one more try).
//! - **Neighbours**: after all pages, a numbering gap between consecutive pages of one document sends
//!   both pages to the stronger model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::harness::{call_json, CallError, RetryPolicy};
use super::ingest::{
    image_mime, norm_kind, norm_level, num_of, pdf_page_count, s_of, split_pdf, write_atomic, Ctx, FileKind, IngestFile, PageRef, QRec,
    SecRec, SrcRec, KINDS, LEVELS,
};
use super::llm::{Llm, LlmRequest, Part};

/// Bump when the page prompt or schema changes: old cache entries are then ignored.
pub const PROMPT_VERSION: &str = "page-v1";

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Clone, Debug)]
pub struct PageUnit {
    pub index: usize,
    /// 1-based global number of the first page (for notes).
    pub first_page: usize,
    pub pages: Vec<PageRef>,
    pub part: Part,
    /// sha256 of the unit's bytes (the image, or the one-page PDF).
    pub sha: String,
    pub bytes: usize,
}

impl PageUnit {
    pub fn is_pdf(&self) -> bool {
        matches!(self.part, Part::Pdf { .. })
    }
    pub fn file_id(&self) -> &str {
        &self.pages[0].file_id
    }
    pub fn label(&self, total: usize) -> String {
        let last = self.first_page + self.pages.len() - 1;
        let span = if last == self.first_page { format!("trang {}/{total}", self.first_page) } else { format!("trang {}–{last}/{total}", self.first_page) };
        format!("{span} ({})", self.pages[0].file_name)
    }
}

/// Read the files and cut them into page units (file order kept).
pub fn plan_pages(files: &[IngestFile]) -> Result<Vec<PageUnit>, String> {
    let mut units: Vec<PageUnit> = Vec::new();
    let mut page_no = 1;
    let mut push = |pages: Vec<PageRef>, part: Part, bytes: &[u8], units: &mut Vec<PageUnit>| {
        let n = pages.len();
        units.push(PageUnit { index: units.len(), first_page: page_no, pages, part, sha: sha256_hex(bytes), bytes: bytes.len() });
        page_no += n;
    };
    for f in files {
        let data = std::fs::read(&f.path).map_err(|e| format!("không đọc được tệp {}: {e}", f.name))?;
        let pref = |p: usize| PageRef { file_id: f.file_id.clone(), file_name: f.name.clone(), page_in_file: p };
        match f.kind {
            FileKind::Image => {
                let mime = image_mime(&f.path, &data).to_string();
                push(vec![pref(1)], Part::Image { mime, data: data.clone() }, &data, &mut units);
            }
            FileKind::Pdf => {
                let n = pdf_page_count(&data);
                match (n > 1).then(|| split_pdf(&data, 1)).flatten() {
                    Some(chunks) => {
                        for (a, _, bytes) in chunks {
                            push(vec![pref(a)], Part::Pdf { name: f.name.clone(), data: bytes.clone() }, &bytes, &mut units);
                        }
                    }
                    None => push((1..=n).map(pref).collect(), Part::Pdf { name: f.name.clone(), data: data.clone() }, &data, &mut units),
                }
            }
        }
    }
    Ok(units)
}

// ---------------------------------------------------------------- prompt + schema

pub const PAGE_SYSTEM: &str = r#"You transcribe ONE page (a photo or a PDF page) of a Vietnamese school test paper or worksheet into JSON. Output JSON only, in exactly this shape:
{"doc":{"title":"","kind":"exam","school":"","date":"","grade":"","minutes":0,"points":0,"fullExam":false,"continues":false},"empty":false,"sections":[{"label":"","title":"","kind":"mcq","note":""}],"questions":[{"sec":"","n":"","kind":"mcq","level":"th","pts":0,"chapter":"","topic":"","form":"","text":"","fig":false,"ans":""}]}

doc: the header if this page shows one (title as printed; school; date; grade like "9"; time limit in minutes; total points), else "" and 0. kind: exam (test paper), review (đề cương / ôn tập), worksheet (exercise sheet). fullExam: a complete test paper (header with a time limit or point scheme). continues: the page has no header and continues a document begun on an earlier page.
empty: true only if the page has no questions at all (cover, answer sheet, blank).
sections: the parts printed on this page, in order (label as printed: "I", "PHẦN II", "A"; kind mcq|truefalse|short|essay; note: short instructions or "").
questions: every gradable item on the page, in reading order (two columns: the whole left column, then the right one). Split essay problems into their printed sub-items (1a, 1b); one multiple-choice question is one item; a true/false cluster (statements a-d) is one item.
- sec: label of its section, "" if none. n: printed number ("3", "2b"). kind: mcq|truefalse|short|essay. pts: printed points, else 0.
- level: nb = recall; th = one-step application; vd = multi-step problem; vdc = hard, non-routine.
- chapter, topic, form: Vietnamese textbook names (chương; bài or chủ đề; dạng bài = the specific task type).
- text: the question copied exactly: Vietnamese with every diacritic, every formula in LaTeX between $...$ (\frac, \sqrt{}, chemistry in \ce{}). mcq: append " A. … B. … C. … D. …"; truefalse: append " a) … b) … c) … d) …". Describe a table or figure briefly in words. At most ~400 characters: shorten words, never formulas.
- fig: needs a figure, graph or table. ans: only if an answer is printed on the page, else "".
Ignore handwriting, pen marks and ticks."#;

pub fn page_schema() -> Value {
    let s = json!({"type": "string"});
    let n = json!({"type": "number"});
    let b = json!({"type": "boolean"});
    json!({
        "type": "object", "additionalProperties": false, "required": ["doc", "empty", "sections", "questions"],
        "properties": {
            "doc": {"type": "object", "additionalProperties": false,
                "required": ["title", "kind", "school", "date", "grade", "minutes", "points", "fullExam", "continues"],
                "properties": {"title": s, "kind": {"type": "string", "enum": ["exam", "review", "worksheet"]}, "school": s, "date": s,
                    "grade": s, "minutes": n, "points": n, "fullExam": b, "continues": b}},
            "empty": b,
            "sections": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["label", "title", "kind", "note"],
                "properties": {"label": s, "title": s, "kind": {"type": "string", "enum": KINDS}, "note": s}}},
            "questions": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["sec", "n", "kind", "level", "pts", "chapter", "topic", "form", "text", "fig", "ans"],
                "properties": {"sec": s, "n": s, "kind": {"type": "string", "enum": KINDS}, "level": {"type": "string", "enum": LEVELS},
                    "pts": n, "chapter": s, "topic": s, "form": s, "text": s, "fig": b, "ans": s}}}
        }
    })
}

// ---------------------------------------------------------------- parse + checks

#[derive(Clone, Debug, Default)]
pub struct PageOut {
    /// Header fields (title "" when the page shows none) and the sections printed on the page.
    pub doc: SrcRec,
    pub continues: bool,
    pub empty: bool,
    pub questions: Vec<QRec>,
}

/// Lenient parse of one page reply. Errors only when there is no `questions` array.
pub fn parse_page(v: &Value) -> Result<PageOut, String> {
    let qs = v.get("questions").and_then(Value::as_array).ok_or("thiếu questions")?;
    let d = &v["doc"];
    let doc = SrcRec {
        kind: match s_of(&d["kind"]).as_str() {
            k @ ("exam" | "review" | "worksheet") => k.to_string(),
            _ => "exam".into(),
        },
        title: s_of(&d["title"]),
        school: s_of(&d["school"]),
        date: s_of(&d["date"]),
        grade: s_of(&d["grade"]),
        duration_min: num_of(&d["minutes"]).round() as u32,
        total_points: num_of(&d["points"]),
        full_exam: d["fullExam"].as_bool().unwrap_or(false),
        sections: v["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| SecRec {
                label: s_of(&x["label"]),
                title: s_of(&x["title"]),
                kind: norm_kind(&s_of(&x["kind"])),
                instructions: s_of(&x["note"]),
            })
            .collect(),
        ..Default::default()
    };
    let mut out = PageOut { doc, continues: d["continues"].as_bool().unwrap_or(false), empty: v["empty"].as_bool().unwrap_or(false), questions: Vec::new() };
    for q in qs {
        let rec = QRec {
            n: s_of(&q["n"]),
            section: s_of(&q["sec"]),
            kind: norm_kind(&s_of(&q["kind"])),
            level: norm_level(&s_of(&q["level"])),
            points: num_of(&q["pts"]),
            chapter: s_of(&q["chapter"]),
            topic: s_of(&q["topic"]),
            form: s_of(&q["form"]),
            text: s_of(&q["text"]),
            has_figure: q["fig"].as_bool().unwrap_or(false),
            answer: s_of(&q["ans"]),
            ..Default::default()
        };
        if rec.text.is_empty() && rec.n.is_empty() {
            continue;
        }
        out.questions.push(rec);
    }
    Ok(out)
}

/// Leading number of a printed question number: "12" → 12, "2b" → 2, "Câu 3" → 3.
pub fn base_num(n: &str) -> Option<u32> {
    let d: String = n.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

fn vi_letter(c: char) -> bool {
    "àáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵđ".contains(c.to_lowercase().next().unwrap_or(c))
}

/// Text outside `$…$`.
fn prose(s: &str) -> String {
    s.split('$').step_by(2).collect::<Vec<_>>().join(" ")
}

fn balanced(s: &str) -> bool {
    let mut dollars = 0;
    let mut depth: i32 = 0;
    let mut prev = ' ';
    for c in s.chars() {
        if prev != '\\' {
            match c {
                '$' => dollars += 1,
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth < 0 {
            return false;
        }
        prev = if prev == '\\' && c == '\\' { ' ' } else { c };
    }
    dollars % 2 == 0 && depth == 0
}

fn runaway(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    if c.len() < 120 {
        return false;
    }
    // A 12-char chunk repeated 6+ times = the model looping.
    let mut i = 0;
    while i + 12 <= c.len() {
        let chunk: String = c[i..i + 12].iter().collect();
        if chunk.trim().len() >= 6 && s.matches(&chunk).count() >= 6 {
            return true;
        }
        i += 12;
    }
    false
}

/// Local checks of one page reading. Each issue is a short Vietnamese phrase (shown in notes).
pub fn check_page(p: &PageOut, subject: &str) -> Vec<String> {
    let mut issues = Vec::new();
    if p.questions.is_empty() {
        if !p.empty || !p.doc.sections.is_empty() {
            issues.push("không thấy câu nào".to_string());
        }
        return issues;
    }
    // Numbering inside each section: increasing by at most 1 (sub-items share a number).
    let mut last: Option<(String, u32)> = None;
    let mut seen = std::collections::HashSet::new();
    for q in &p.questions {
        if !seen.insert((q.section.to_lowercase(), q.n.to_lowercase())) {
            issues.push(format!("câu {} lặp lại", q.n));
        }
        let Some(b) = base_num(&q.n) else { continue };
        if let Some((sec, l)) = &last {
            let same_sec = *sec == q.section;
            if b == 1 && (!same_sec || l >= &1) && b <= *l {
                // numbering restarts (new part)
            } else if same_sec && b < *l {
                issues.push(format!("thứ tự câu lộn xộn ({l} rồi {b})"));
            } else if same_sec && b > l + 1 {
                issues.push(format!("đánh số nhảy {l} → {b}"));
            }
        }
        last = Some((q.section.clone(), b));
    }
    let mut prose_text = String::new();
    for q in &p.questions {
        if !balanced(&q.text) {
            issues.push(format!("câu {}: công thức hỏng ($ hoặc ngoặc lệch)", q.n));
        }
        if runaway(&q.text) || q.text.chars().count() > 1500 {
            issues.push(format!("câu {}: chữ lặp vô tận", q.n));
        }
        prose_text.push_str(&prose(&q.text));
        prose_text.push(' ');
    }
    if !matches!(subject, "anh-ssat" | "duc" | "tin-cpp") {
        let letters: Vec<char> = prose_text.chars().filter(|c| c.is_alphabetic()).collect();
        let vi = letters.iter().filter(|c| vi_letter(**c)).count();
        if letters.len() >= 80 && (vi as f64) < 0.04 * letters.len() as f64 {
            issues.push("tiếng Việt mất dấu".to_string());
        }
    }
    issues.dedup();
    issues
}

/// Numbering gaps between consecutive pages of one document: `(unit index, reason)` for both pages.
pub fn neighbour_gaps(outs: &[Option<PageOut>]) -> Vec<(usize, String)> {
    let mut v = Vec::new();
    for k in 1..outs.len() {
        let (Some(a), Some(b)) = (&outs[k - 1], &outs[k]) else { continue };
        if !continues(b) {
            continue;
        }
        let (Some(la), Some(fb)) = (a.questions.iter().rev().find_map(|q| base_num(&q.n)), b.questions.iter().find_map(|q| base_num(&q.n))) else {
            continue;
        };
        if fb > la + 1 {
            let why = format!("giữa hai trang thiếu câu {}", if fb == la + 2 { (la + 1).to_string() } else { format!("{}–{}", la + 1, fb - 1) });
            v.push((k - 1, why.clone()));
            v.push((k, why));
        }
    }
    v
}

/// The page carries on the document of the page before it (no header of its own).
pub fn continues(p: &PageOut) -> bool {
    p.continues || p.doc.title.trim().is_empty()
}

// ---------------------------------------------------------------- cache

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPage {
    pub v: u32,
    pub sha: String,
    pub prompt: String,
    pub model: String,
    pub provider: String,
    pub out: Value,
    pub cost_usd: f64,
    pub ms: u64,
}

pub fn cache_path(dir: &Path, sha: &str, model: &str) -> PathBuf {
    let key = sha256_hex(format!("{sha}|{PROMPT_VERSION}|{model}").as_bytes());
    dir.join(format!("{}.json", &key[..40]))
}

fn cache_get(dir: &Path, sha: &str, model: &str) -> Option<CachedPage> {
    let c: CachedPage = serde_json::from_slice(&std::fs::read(cache_path(dir, sha, model)).ok()?).ok()?;
    (c.sha == sha && c.model == model && c.prompt == PROMPT_VERSION).then_some(c)
}

// ---------------------------------------------------------------- reading

#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub model: String,
    pub effort: Option<String>,
    /// Stronger model for pages the cheap one gets wrong (`None`: no escalation).
    pub escalate: Option<String>,
    pub escalate_effort: Option<String>,
    /// Model for PDF pages when the reading model cannot take PDFs.
    pub pdf_model: String,
    pub concurrency: usize,
    pub retries: usize,
    pub retry_base_ms: u64,
    pub max_tokens: u32,
    pub timeout_secs: u64,
    pub subject: String,
}

/// Models that read PDF files natively (through OpenRouter).
pub fn reads_pdf(model: &str) -> bool {
    model.starts_with("anthropic/") || model.starts_with("google/") || model.starts_with("openai/")
}

/// Short human name of a model id for notes.
pub fn model_label(model: &str) -> String {
    let m = model.rsplit('/').next().unwrap_or(model);
    if m.contains("haiku") {
        "Haiku".into()
    } else if m.contains("flash") {
        "DeepSeek Flash".into()
    } else if m.contains("sonnet") {
        "Sonnet".into()
    } else {
        m.to_string()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PageResult {
    pub out: Option<PageOut>,
    pub model: String,
    pub issues: Vec<String>,
    pub cached: bool,
    pub escalated: bool,
    pub error: Option<String>,
}

pub(crate) struct Reader {
    pub llm: Arc<dyn Llm>,
    pub opts: ReadOptions,
    pub cache_dir: PathBuf,
    pub ctx: Arc<Ctx>,
    pub cancel: watch::Receiver<bool>,
}

struct Got {
    out: PageOut,
    issues: Vec<String>,
    cached: bool,
}

impl Reader {
    fn primary_for(&self, u: &PageUnit) -> String {
        if u.is_pdf() && !reads_pdf(&self.opts.model) {
            self.opts.pdf_model.clone()
        } else {
            self.opts.model.clone()
        }
    }

    fn strong_for(&self, primary: &str) -> Option<String> {
        self.opts.escalate.clone().filter(|m| m != primary && !m.is_empty())
    }

    fn got_from(&self, out: PageOut, cached: bool) -> Got {
        let issues = check_page(&out, &self.opts.subject);
        Got { out, issues, cached }
    }

    /// One reading by `model` (cache first). `fresh` skips the cache and uses a warmer temperature.
    async fn read(&self, u: &PageUnit, model: &str, strong: bool, fresh: bool) -> Result<Got, CallError> {
        if !fresh {
            if let Some(c) = cache_get(&self.cache_dir, &u.sha, model) {
                if let Ok(out) = parse_page(&c.out) {
                    return Ok(self.got_from(out, true));
                }
            }
        }
        let effort = if strong { self.opts.escalate_effort.clone() } else { self.opts.effort.clone() };
        let mut parts = Vec::new();
        if u.pages.len() > 1 {
            parts.push(Part::Text(format!("PDF pages 1-{}: read them all as one page list.", u.pages.len())));
        }
        parts.push(u.part.clone());
        let mut req = LlmRequest::new("extract", model, effort, PAGE_SYSTEM, parts, self.opts.max_tokens * u.pages.len().min(4) as u32);
        req.schema = Some(("exam_page".into(), page_schema()));
        req.timeout = Some(std::time::Duration::from_secs(self.opts.timeout_secs));
        req.temperature = Some(if fresh { 0.4 } else { 0.0 });
        let policy = RetryPolicy { retries: self.opts.retries, base_ms: self.opts.retry_base_ms };
        let reply = call_json(self.llm.as_ref(), req, policy, &self.ctx.spent, self.cancel.clone(), &|v: &Value| parse_page(v).map(|_| ())).await?;
        let out = parse_page(&reply.value).map_err(CallError::Failed)?;
        let rec = CachedPage {
            v: 1,
            sha: u.sha.clone(),
            prompt: PROMPT_VERSION.into(),
            model: model.to_string(),
            provider: reply.provider,
            out: reply.value,
            cost_usd: reply.usage.cost_usd,
            ms: reply.elapsed_ms,
        };
        if let Err(e) = write_atomic(&cache_path(&self.cache_dir, &u.sha, model), &serde_json::to_vec(&rec).unwrap_or_default()) {
            tracing::warn!("page cache write: {e}");
        }
        Ok(self.got_from(out, false))
    }

    fn pick(a: Option<Got>, b: Option<Got>) -> Option<Got> {
        match (a, b) {
            (Some(a), Some(b)) => Some(if b.issues.len() <= a.issues.len() { b } else { a }),
            (a, b) => b.or(a),
        }
    }

    /// First pass for one unit: cache, cheap read, retry/escalation on trouble.
    pub async fn resolve(&self, u: &PageUnit) -> Result<PageResult, CallError> {
        let total = self.ctx.total;
        let primary = self.primary_for(u);
        let strong = self.strong_for(&primary);
        // A page escalated in an earlier job: its strong reading is final.
        if let Some(s) = &strong {
            if let Some(c) = cache_get(&self.cache_dir, &u.sha, s).and_then(|c| parse_page(&c.out).ok()) {
                let g = self.got_from(c, true);
                return Ok(PageResult { issues: g.issues, out: Some(g.out), model: s.clone(), cached: true, escalated: true, error: None });
            }
        }
        let first = match self.read(u, &primary, false, false).await {
            Ok(g) => Ok(g),
            Err(CallError::Cancelled) => return Err(CallError::Cancelled),
            Err(CallError::Failed(m)) => Err(m),
        };
        if let Ok(g) = &first {
            if g.issues.is_empty() {
                let g = first.unwrap();
                return Ok(PageResult { out: Some(g.out), model: primary, issues: vec![], cached: g.cached, escalated: false, error: None });
            }
        }
        let why = match &first {
            Ok(g) => g.issues.join("; "),
            Err(_) => "đọc lỗi".to_string(),
        };
        let first = first.ok();
        match strong {
            Some(s) => {
                self.ctx.send("reading", &format!("{} chưa chắc ({why}), đọc lại bằng {}", cap(&u.label(total)), model_label(&s)));
                let second = match self.read(u, &s, true, false).await {
                    Ok(g) => Some(g),
                    Err(CallError::Cancelled) => return Err(CallError::Cancelled),
                    Err(CallError::Failed(m)) => {
                        tracing::warn!("escalation failed: {m}");
                        None
                    }
                };
                let used_strong = second.as_ref().is_some_and(|b| first.as_ref().is_none_or(|a| b.issues.len() <= a.issues.len()));
                Ok(self.result(Self::pick(first, second), if used_strong { s } else { primary }, used_strong))
            }
            None => {
                // No stronger model: one more (fresh) try of the same one.
                let second = match self.read(u, &primary, false, true).await {
                    Ok(g) => Some(g),
                    Err(CallError::Cancelled) => return Err(CallError::Cancelled),
                    Err(_) => None,
                };
                Ok(self.result(Self::pick(first, second), primary, false))
            }
        }
    }

    fn result(&self, g: Option<Got>, model: String, escalated: bool) -> PageResult {
        match g {
            Some(g) => PageResult { out: Some(g.out), model, issues: g.issues, cached: g.cached, escalated, error: None },
            None => PageResult { model, error: Some("không đọc được".into()), ..Default::default() },
        }
    }

    /// Second pass: re-read one unit with the strong model (neighbour gap), keep the better reading.
    pub async fn escalate(&self, u: &PageUnit, cur: PageResult) -> Result<PageResult, CallError> {
        let primary = self.primary_for(u);
        let Some(s) = self.strong_for(&primary) else { return Ok(cur) };
        if cur.model == s {
            return Ok(cur);
        }
        let a = cur.out.clone().map(|out| Got { issues: cur.issues.clone(), out, cached: cur.cached });
        let b = match self.read(u, &s, true, false).await {
            Ok(g) => Some(g),
            Err(CallError::Cancelled) => return Err(CallError::Cancelled),
            Err(_) => None,
        };
        // The gap is evidence against the cheap reading: the strong one wins ties and near-ties.
        Ok(match b {
            Some(b) if a.as_ref().is_none_or(|a| b.issues.len() <= a.issues.len() + 1) => {
                PageResult { out: Some(b.out), model: s, issues: b.issues, cached: b.cached, escalated: true, error: None }
            }
            _ => cur,
        })
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(sec: &str, n: &str, text: &str) -> QRec {
        QRec { section: sec.into(), n: n.into(), text: text.into(), kind: "mcq".into(), ..Default::default() }
    }

    fn page(qs: Vec<QRec>) -> PageOut {
        PageOut { questions: qs, ..Default::default() }
    }

    #[test]
    fn checks_catch_suspicious_pages() {
        let ok = page(vec![q("I", "1", "Căn bậc hai của $49$ là"), q("I", "2", "Tính $\\sqrt{4}$"), q("II", "1a", "Rút gọn"), q("II", "1b", "Tìm")]);
        assert!(check_page(&ok, "toan").is_empty(), "{:?}", check_page(&ok, "toan"));
        let gap = page(vec![q("I", "3", "a"), q("I", "5", "b")]);
        assert!(check_page(&gap, "toan")[0].contains("3 → 5"));
        let order = page(vec![q("", "1", "a"), q("", "9", "b"), q("", "2", "c")]);
        assert!(check_page(&order, "toan").iter().any(|i| i.contains("lộn xộn") || i.contains("nhảy")));
        let broken = page(vec![q("", "1", "Tính $\\frac{1}{2$")]);
        assert!(check_page(&broken, "toan")[0].contains("công thức"));
        let plain = page(vec![q("", "1", &"Tinh gia tri cua bieu thuc sau va rut gon bieu thuc cho truoc voi dieu kien xac dinh ".repeat(2))]);
        assert!(check_page(&plain, "toan").iter().any(|i| i.contains("mất dấu")));
        assert!(check_page(&plain, "anh-ssat").is_empty());
        let empty = PageOut { empty: false, ..Default::default() };
        assert_eq!(check_page(&empty, "toan"), ["không thấy câu nào"]);
        let blank = PageOut { empty: true, ..Default::default() };
        assert!(check_page(&blank, "toan").is_empty());
        let looping = page(vec![q("", "1", &"x = 1 + 1 = 2, ".repeat(20))]);
        assert!(check_page(&looping, "toan").iter().any(|i| i.contains("lặp")));
    }

    #[test]
    fn gaps_between_pages_and_restarts() {
        let a = page(vec![q("I", "4", "a"), q("I", "5", "b")]);
        let b = page(vec![q("I", "7", "c")]);
        let c = page(vec![q("II", "1", "d")]);
        let g = neighbour_gaps(&[Some(a.clone()), Some(b), Some(c)]);
        assert_eq!(g.iter().map(|x| x.0).collect::<Vec<_>>(), [0, 1]);
        assert!(g[0].1.contains("thiếu câu 6"));
        let mut headed = page(vec![q("", "1", "x")]);
        headed.doc.title = "Phiếu bài tập".into();
        assert!(neighbour_gaps(&[Some(a.clone()), Some(page(vec![q("I", "6", "x")]))]).is_empty(), "continuous");
        assert!(neighbour_gaps(&[Some(a), Some(headed)]).is_empty(), "new document");
    }

    #[test]
    fn parse_is_lenient_and_numbers_parse() {
        let v = json!({"doc": {"title": "Đề 1", "kind": "?", "minutes": "90", "fullExam": true}, "questions": [
            {"sec": "I", "n": "1", "kind": "Trắc nghiệm", "level": "Vận dụng", "pts": "0,25", "text": "Câu $x$", "fig": false, "ans": ""},
            {"n": "", "text": ""}]});
        let p = parse_page(&v).unwrap();
        assert_eq!(p.doc.duration_min, 90);
        assert_eq!(p.doc.kind, "exam");
        assert_eq!(p.questions.len(), 1);
        assert_eq!((p.questions[0].kind.as_str(), p.questions[0].level.as_str(), p.questions[0].points), ("mcq", "vd", 0.25));
        assert!(parse_page(&json!({"doc": {}})).is_err());
        assert_eq!(base_num("Câu 12"), Some(12));
        assert_eq!(base_num("2b"), Some(2));
        assert_eq!(base_num("b"), None);
        assert_eq!(model_label("anthropic/claude-haiku-5.5"), "Haiku");
        assert_eq!(model_label("deepseek-flash"), "DeepSeek Flash");
    }

    #[test]
    fn cache_key_depends_on_bytes_prompt_and_model() {
        let d = Path::new("x");
        assert_ne!(cache_path(d, "aa", "m1"), cache_path(d, "aa", "m2"));
        assert_ne!(cache_path(d, "aa", "m1"), cache_path(d, "bb", "m1"));
        assert_eq!(cache_path(d, "aa", "m1"), cache_path(d, "aa", "m1"));
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
