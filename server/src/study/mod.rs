//! Study engine: exam matrix ("ma trận đề") per folder (PROTOCOL §11, §13).
//!
//! Façade for the server — everything takes/returns `serde_json::Value` events with `t` already set:
//!
//! ```text
//! StudyConfig::from_env(server_dir: &Path) -> StudyConfig            // .env + process env, see below
//! StudyService::new(data_dir: PathBuf, cfg: StudyConfig) -> StudyService   // DeepSeek / OpenRouter by the keys present
//! StudyService::with_llm(data_dir, cfg, llm: Arc<dyn Llm>) -> StudyService // tests / other providers
//! svc.available() -> bool                                              // an LLM is configured
//! svc.start_ingest(IngestRequest{folder_id, subject, goal, files}) -> Result<IngestHandle, String>
//!     IngestHandle { job_id, events: mpsc::UnboundedReceiver<Value /* t:"ingest" */>, task: JoinHandle<..> }
//!     events: queued → reading… → merging → done|error|cancelled (exact §11 fields). Err = busy / no key.
//! svc.cancel(folder_id: &str) -> bool                                  // §11 ingest_cancel
//! svc.ingest_status(folder_id: &str) -> Option<Value>                  // last `ingest` event (for folders/knowledge)
//! svc.knowledge(folder_id: &str) -> Option<Value>                      // knowledge.json
//! svc.gen_exam(folder_id, structure_id, note: Option<&str>, emit: impl Fn(Value)) -> Result<Value, String>
//!     emits t:"exam" writing → checking → done|error; Ok = list item {examId,title,structureId,createdAt,questions}
//! svc.list_exams(folder_id) -> Vec<Value>                              // §13 exams.items, newest first
//! svc.exam(exam_id) -> Option<Value>; svc.delete_exam(exam_id) -> bool
//! svc.exam_html(exam_id) -> Option<String>                             // GET /exam/<examId> body
//! svc.delete_folder_data(folder_id) -> io::Result<()>                  // cancels a job, removes the folder dir
//! svc.usage_summary() -> Value  // {ingest:{today:{costUsd},total:{costUsd}}, exam:{…}} for §10 `usage.ingest`
//! ```
//!
//! Files: `<data_dir>/folders/<folderId>/knowledge.json`, `…/ingest/records.json` (compact question
//! records), `…/ingest/assign.json` (question group → form id), `…/ingest/pages/*.json` (page cache:
//! sha256 of the page + prompt version + model), `…/exams/<examId>.json`, `<data_dir>/study/usage.jsonl`.
//!
//! Modules: `pages` (page units, checks, cache, escalation), `merge` (incremental tree + deterministic
//! knowledge), `ingest` (the job), `exam`, `harness` (one call: retry, jitter, cancel), `llm` (DeepSeek,
//! OpenRouter, failover, routing).
//!
//! Env (defaults chosen by `examples/ingest_bench.rs`, see NOTES.md): `DEEPSEEK_API_KEY`,
//! `DEEPSEEK_BASE_URL`, `OPENROUTER_API_KEY`, `OPENROUTER_BASE_URL`, `BELUGA_INGEST_MODEL`
//! (`deepseek-flash`), `BELUGA_INGEST_EFFORT` (`none`), `BELUGA_ESCALATE_MODEL`
//! (`anthropic/claude-haiku-5.5`, `none` = never), `BELUGA_ESCALATE_EFFORT` (`none`), `BELUGA_PDF_MODEL`
//! (escalation model), `BELUGA_MERGE_MODEL` (`anthropic/claude-haiku-5.5`), `BELUGA_MERGE_EFFORT` (`none`),
//! `BELUGA_EXAM_MODEL` (`anthropic/claude-haiku-5.5`), `BELUGA_EXAM_EFFORT` (`high`),
//! `BELUGA_INGEST_CONCURRENCY` (8), `BELUGA_DEEPSEEK_PRICE` (`in,cached,out` USD per million).

pub mod exam;
pub mod harness;
pub mod ingest;
pub mod llm;
pub mod merge;
pub mod pages;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

pub use exam::{render_html, ExamOptions};
pub use ingest::{FileKind, IngestError, IngestFile, IngestOptions, Records};
pub use llm::{DeepSeek, FakeLlm, Llm, OpenRouter, Price, Providers};

#[derive(Clone, Debug)]
pub struct StudyConfig {
    /// OpenRouter key.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub deepseek_key: Option<String>,
    pub deepseek_base_url: Option<String>,
    pub deepseek_price: Price,
    pub ingest: IngestOptions,
    pub exam: ExamOptions,
    pub timeout_secs: u64,
}

impl Default for StudyConfig {
    fn default() -> Self {
        StudyConfig {
            api_key: None,
            base_url: None,
            deepseek_key: None,
            deepseek_base_url: None,
            deepseek_price: Price::DEEPSEEK_FLASH,
            ingest: IngestOptions::default(),
            exam: ExamOptions { model: "anthropic/claude-haiku-5.5".into(), effort: Some("high".into()), ..ExamOptions::default() },
            timeout_secs: 300,
        }
    }
}

fn parse_env_file(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                .unwrap_or(v);
            m.insert(k.trim().to_string(), v.to_string());
        }
    }
    m
}

impl StudyConfig {
    /// `<server_dir>/.env`, overridden by process env. Key values are never logged.
    pub fn from_env(server_dir: &Path) -> StudyConfig {
        let mut env = std::fs::read_to_string(server_dir.join(".env")).map(|t| parse_env_file(&t)).unwrap_or_default();
        for (k, v) in std::env::vars() {
            if k.starts_with("BELUGA_") || k.starts_with("OPENROUTER_") || k.starts_with("DEEPSEEK_") {
                env.insert(k, v);
            }
        }
        Self::from_map(&env)
    }

    pub fn from_map(env: &HashMap<String, String>) -> StudyConfig {
        let get = |k: &str| env.get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let effort = |k: &str, d: &str| -> Option<String> {
            let e = get(k).unwrap_or_else(|| d.to_string()).to_lowercase();
            (e != "none" && e != "off").then_some(e)
        };
        let model = |k: &str| get(k).map(|m| (m.to_lowercase() != "none" && m.to_lowercase() != "off").then_some(m));
        let mut c = StudyConfig::default();
        c.api_key = get("OPENROUTER_API_KEY");
        c.base_url = get("OPENROUTER_BASE_URL");
        c.deepseek_key = get("DEEPSEEK_API_KEY");
        c.deepseek_base_url = get("DEEPSEEK_BASE_URL");
        if let Some(p) = get("BELUGA_DEEPSEEK_PRICE").and_then(|p| Price::parse(&p)) {
            c.deepseek_price = p;
        }
        if let Some(m) = get("BELUGA_INGEST_MODEL") {
            c.ingest.model = m;
        }
        c.ingest.effort = effort("BELUGA_INGEST_EFFORT", "none");
        if let Some(m) = model("BELUGA_ESCALATE_MODEL") {
            c.ingest.escalate_model = m;
        }
        c.ingest.escalate_effort = effort("BELUGA_ESCALATE_EFFORT", "none");
        if let Some(m) = model("BELUGA_PDF_MODEL") {
            c.ingest.pdf_model = m;
        }
        if let Some(m) = get("BELUGA_MERGE_MODEL") {
            c.ingest.merge_model = m;
        }
        c.ingest.merge_effort = effort("BELUGA_MERGE_EFFORT", "none");
        if let Some(n) = get("BELUGA_INGEST_CONCURRENCY").and_then(|v| v.parse().ok()) {
            c.ingest.concurrency = n;
        }
        if let Some(m) = get("BELUGA_EXAM_MODEL") {
            c.exam.model = m;
        }
        c.exam.effort = effort("BELUGA_EXAM_EFFORT", "high");
        c
    }
}

#[derive(Clone, Debug)]
pub struct IngestRequest {
    pub folder_id: String,
    pub subject: String,
    pub goal: Option<String>,
    pub files: Vec<IngestFile>,
}

pub struct IngestHandle {
    pub job_id: String,
    pub events: mpsc::UnboundedReceiver<Value>,
    pub task: JoinHandle<Result<Value, IngestError>>,
}

struct Job {
    job_id: String,
    cancel: watch::Sender<bool>,
}

struct Inner {
    data_dir: PathBuf,
    cfg: StudyConfig,
    llm: Option<Arc<dyn Llm>>,
    jobs: Mutex<HashMap<String, Job>>,
    status: Mutex<HashMap<String, Value>>,
}

#[derive(Clone)]
pub struct StudyService {
    inner: Arc<Inner>,
}

/// Ids become path components: only `[A-Za-z0-9_-]`, 1..=80 chars.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl StudyService {
    pub fn new(data_dir: PathBuf, cfg: StudyConfig) -> Self {
        let t = Duration::from_secs(cfg.timeout_secs);
        let or: Option<Arc<dyn Llm>> = cfg.api_key.clone().map(|k| Arc::new(OpenRouter::new(k, cfg.base_url.clone(), t)) as Arc<dyn Llm>);
        let ds: Option<Arc<dyn Llm>> = cfg
            .deepseek_key
            .clone()
            .map(|k| Arc::new(DeepSeek::new(k, cfg.deepseek_base_url.clone(), t, cfg.deepseek_price)) as Arc<dyn Llm>);
        let p = Providers::new(ds, or, t);
        let llm: Option<Arc<dyn Llm>> = (!p.is_empty()).then(|| Arc::new(p) as Arc<dyn Llm>);
        Self::build(data_dir, cfg, llm)
    }

    pub fn with_llm(data_dir: PathBuf, cfg: StudyConfig, llm: Arc<dyn Llm>) -> Self {
        Self::build(data_dir, cfg, Some(llm))
    }

    fn build(data_dir: PathBuf, cfg: StudyConfig, llm: Option<Arc<dyn Llm>>) -> Self {
        StudyService {
            inner: Arc::new(Inner { data_dir, cfg, llm, jobs: Mutex::new(HashMap::new()), status: Mutex::new(HashMap::new()) }),
        }
    }

    pub fn available(&self) -> bool {
        self.inner.llm.is_some()
    }

    pub fn folder_dir(&self, folder_id: &str) -> PathBuf {
        self.inner.data_dir.join("folders").join(folder_id)
    }

    fn log_usage(&self, kind: &str, folder_id: &str, model: &str, u: &llm::Usage) {
        use std::io::Write;
        let dir = self.inner.data_dir.join("study");
        let _ = std::fs::create_dir_all(&dir);
        let line = json!({"ts": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(), "kind": kind,
            "folderId": folder_id, "model": model, "costUsd": u.cost_usd,
            "inputTokens": u.input_tokens, "outputTokens": u.output_tokens});
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("usage.jsonl")) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// `{ingest:{today:{costUsd},total:{costUsd}}, exam:{today,total}}` (today = UTC date).
    pub fn usage_summary(&self) -> Value {
        let text = std::fs::read_to_string(self.inner.data_dir.join("study").join("usage.jsonl")).unwrap_or_default();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut acc: HashMap<(String, bool), f64> = HashMap::new();
        for l in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
            let kind = v["kind"].as_str().unwrap_or("ingest").to_string();
            let c = v["costUsd"].as_f64().unwrap_or(0.0);
            *acc.entry((kind.clone(), false)).or_insert(0.0) += c;
            if v["ts"].as_str().is_some_and(|t| t.starts_with(&today)) {
                *acc.entry((kind, true)).or_insert(0.0) += c;
            }
        }
        let r = |k: &str, t: bool| (acc.get(&(k.to_string(), t)).copied().unwrap_or(0.0) * 10000.0).round() / 10000.0;
        json!({"ingest": {"today": {"costUsd": r("ingest", true)}, "total": {"costUsd": r("ingest", false)}},
               "exam": {"today": {"costUsd": r("exam", true)}, "total": {"costUsd": r("exam", false)}}})
    }

    /// Start an ingest job for one folder (one job per folder at a time).
    pub fn start_ingest(&self, req: IngestRequest) -> Result<IngestHandle, String> {
        if !safe_id(&req.folder_id) {
            return Err("folderId không hợp lệ".into());
        }
        let llm = self.inner.llm.clone().ok_or("thiếu khóa API (OPENROUTER_API_KEY hoặc DEEPSEEK_API_KEY)")?;
        if req.files.is_empty() {
            return Err("chưa có tệp nào".into());
        }
        let job_id = format!("ing-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        {
            let mut jobs = self.inner.jobs.lock().unwrap();
            if jobs.contains_key(&req.folder_id) {
                return Err("folder này đang nạp đề".into());
            }
            jobs.insert(req.folder_id.clone(), Job { job_id: job_id.clone(), cancel: cancel_tx });
        }
        let (tx, rx) = mpsc::unbounded_channel::<Value>();
        let me = self.clone();
        let folder = req.folder_id.clone();
        let emit: ingest::Emit = {
            let me = me.clone();
            let folder = folder.clone();
            Arc::new(move |ev: Value| {
                me.inner.status.lock().unwrap().insert(folder.clone(), ev.clone());
                let _ = tx.send(ev);
            })
        };
        emit(ingest::ingest_event(&folder, &job_id, "queued", 0, 0, "Đang xếp hàng nạp đề", None, Some(0.0)));
        let dir = self.folder_dir(&folder);
        let existing = self.knowledge(&folder);
        let previous: Option<Records> =
            std::fs::read(dir.join("ingest").join("records.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let input = ingest::IngestInput {
            folder_id: folder.clone(),
            subject: req.subject,
            goal: req.goal,
            files: req.files,
            existing,
            previous,
        };
        let opts = self.inner.cfg.ingest.clone();
        let jid = job_id.clone();
        let task = tokio::spawn(async move {
            let usage_before = Arc::new(Mutex::new(0.0f64));
            let ub = usage_before.clone();
            let emit2: ingest::Emit = Arc::new(move |ev: Value| {
                if let Some(c) = ev["costUsd"].as_f64() {
                    *ub.lock().unwrap() = c;
                }
                emit(ev)
            });
            let r = ingest::run_ingest(llm, opts.clone(), input, dir, jid.clone(), emit2, cancel_rx).await;
            let cost = *usage_before.lock().unwrap();
            me.log_usage("ingest", &folder, &opts.model, &llm::Usage { cost_usd: cost, ..Default::default() });
            let mut jobs = me.inner.jobs.lock().unwrap();
            if jobs.get(&folder).is_some_and(|j| j.job_id == jid) {
                jobs.remove(&folder);
            }
            r
        });
        Ok(IngestHandle { job_id, events: rx, task })
    }

    /// Cancel the running ingest of a folder. `false` if none is running.
    pub fn cancel(&self, folder_id: &str) -> bool {
        match self.inner.jobs.lock().unwrap().get(folder_id) {
            Some(j) => {
                let _ = j.cancel.send(true);
                true
            }
            None => false,
        }
    }

    /// Last `ingest` event of the folder (running or finished in this process).
    pub fn ingest_status(&self, folder_id: &str) -> Option<Value> {
        self.inner.status.lock().unwrap().get(folder_id).cloned()
    }

    pub fn knowledge(&self, folder_id: &str) -> Option<Value> {
        if !safe_id(folder_id) {
            return None;
        }
        let b = std::fs::read(self.folder_dir(folder_id).join("knowledge.json")).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// Generate and store a new exam. `emit` gets `exam` events; the final one is `done` or `error`.
    pub async fn gen_exam(
        &self,
        folder_id: &str,
        structure_id: &str,
        note: Option<&str>,
        emit: impl Fn(Value) + Send + Sync,
    ) -> Result<Value, String> {
        let exam_id = format!("ex-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let fail = |m: String| {
            emit(exam::exam_event(folder_id, &exam_id, "error", &m, None));
            Err(m)
        };
        let Some(llm) = self.inner.llm.clone() else { return fail("thiếu khóa API (OPENROUTER_API_KEY hoặc DEEPSEEK_API_KEY)".into()) };
        let Some(k) = self.knowledge(folder_id) else { return fail("folder chưa có kiến thức (hãy nạp đề trước)".into()) };
        let opts = self.inner.cfg.exam.clone();
        let spent = Mutex::new(llm::Usage::default());
        let t0 = std::time::Instant::now();
        let r = exam::gen_exam(llm, &opts, &k, structure_id, note, &exam_id, &emit, &spent).await;
        tracing::info!(ms = t0.elapsed().as_millis() as u64, cost = spent.lock().unwrap().cost_usd, ok = r.is_ok(), "gen_exam");
        match r {
            Ok((doc, usage)) => {
                self.log_usage("exam", folder_id, &opts.model, &usage);
                let path = self.folder_dir(folder_id).join("exams").join(format!("{exam_id}.json"));
                if let Err(e) = ingest::write_atomic(&path, &serde_json::to_vec_pretty(&doc).unwrap_or_default()) {
                    return fail(format!("không lưu được đề: {e}"));
                }
                let title = doc["title"].as_str().unwrap_or("");
                emit(exam::exam_event(folder_id, &exam_id, "done", &format!("Xong: {} câu", doc["questions"]), Some(title)));
                Ok(list_item(&doc))
            }
            Err(m) => {
                // Failed attempts cost money too.
                self.log_usage("exam", folder_id, &opts.model, &spent.lock().unwrap());
                fail(m)
            }
        }
    }

    pub fn list_exams(&self, folder_id: &str) -> Vec<Value> {
        if !safe_id(folder_id) {
            return Vec::new();
        }
        let mut v: Vec<Value> = std::fs::read_dir(self.folder_dir(folder_id).join("exams"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|d| list_item(&d))
            .collect();
        v.sort_by(|a, b| b["createdAt"].as_str().cmp(&a["createdAt"].as_str()));
        v
    }

    fn exam_path(&self, exam_id: &str) -> Option<PathBuf> {
        if !safe_id(exam_id) {
            return None;
        }
        std::fs::read_dir(self.inner.data_dir.join("folders"))
            .ok()?
            .flatten()
            .map(|d| d.path().join("exams").join(format!("{exam_id}.json")))
            .find(|p| p.exists())
    }

    pub fn exam(&self, exam_id: &str) -> Option<Value> {
        serde_json::from_slice(&std::fs::read(self.exam_path(exam_id)?).ok()?).ok()
    }

    pub fn delete_exam(&self, exam_id: &str) -> bool {
        self.exam_path(exam_id).map(|p| std::fs::remove_file(p).is_ok()).unwrap_or(false)
    }

    pub fn exam_html(&self, exam_id: &str) -> Option<String> {
        self.exam(exam_id).map(|e| render_html(&e))
    }

    /// Folder deleted (§8): stop its job, remove knowledge, records and exams.
    pub fn delete_folder_data(&self, folder_id: &str) -> std::io::Result<()> {
        if !safe_id(folder_id) {
            return Ok(());
        }
        self.cancel(folder_id);
        self.inner.status.lock().unwrap().remove(folder_id);
        let d = self.folder_dir(folder_id);
        if d.exists() {
            std::fs::remove_dir_all(d)?;
        }
        Ok(())
    }
}

fn list_item(d: &Value) -> Value {
    json!({"examId": d["examId"], "title": d["title"], "structureId": d["structureId"],
           "createdAt": d["createdAt"], "questions": d["questions"]})
}
