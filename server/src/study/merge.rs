//! Merge: question records → `knowledge.json`, incrementally.
//!
//! Only the semantic part uses a model: placing NEW question groups ("raws" = distinct
//! chapter > topic > form as read from the pages) into the curriculum tree, naming new nodes and writing
//! a one-line summary per new topic and a short method per new form. The model sees a compact outline
//! of the existing tree (ids + titles) and only the raws not placed yet, never the whole file.
//!
//! Everything else is deterministic Rust:
//! - placement of raws already seen (`ingest/assign.json`: raw key → form id) or whose form title matches
//!   an existing form;
//! - ids: existing nodes keep their ids and are never removed (topicId references in memory stay valid);
//!   new nodes take the next free number;
//! - counts, level histograms, shares, examples, refs, stats;
//! - exam structures (full exams grouped by section kinds, counts and timing) and their matrix;
//! - school progress (topics seen so far, the newest document's furthest topic) and the grade.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use super::ingest::{clip, fill_points, level_counts, norm, round, s_of, IngestInput, QRec, Records, SrcRec, LEVELS};

// ---------------------------------------------------------------- tree

#[derive(Clone, Debug, Default)]
pub struct Form {
    pub id: String,
    pub title: String,
    pub method: String,
}

#[derive(Clone, Debug, Default)]
pub struct Topic {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub forms: Vec<Form>,
}

#[derive(Clone, Debug, Default)]
pub struct Chapter {
    pub id: String,
    pub title: String,
    pub topics: Vec<Topic>,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub chapters: Vec<Chapter>,
}

fn num_after(id: &str, marker: char) -> usize {
    id.rsplit('.').next().and_then(|s| s.strip_prefix(marker)).and_then(|n| n.parse().ok()).unwrap_or(0)
}

impl Tree {
    pub fn from_knowledge(k: Option<&Value>) -> Tree {
        let mut t = Tree::default();
        for c in k.and_then(|k| k["curriculum"].as_array()).into_iter().flatten() {
            t.chapters.push(Chapter {
                id: s_of(&c["id"]),
                title: s_of(&c["title"]),
                topics: c["topics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|tp| Topic {
                        id: s_of(&tp["id"]),
                        title: s_of(&tp["title"]),
                        summary: s_of(&tp["summary"]),
                        forms: tp["forms"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|f| Form { id: s_of(&f["id"]), title: s_of(&f["title"]), method: s_of(&f["method"]) })
                            .collect(),
                    })
                    .collect(),
            });
        }
        t
    }

    pub fn has_form(&self, id: &str) -> bool {
        self.chapters.iter().flat_map(|c| &c.topics).flat_map(|t| &t.forms).any(|f| f.id == id)
    }

    fn new_chapter_id(&self) -> String {
        format!("c{}", self.chapters.iter().map(|c| c.id.strip_prefix('c').and_then(|n| n.parse().ok()).unwrap_or(0)).max().unwrap_or(0) + 1)
    }

    fn new_topic_id(c: &Chapter) -> String {
        format!("{}.t{}", c.id, c.topics.iter().map(|t| num_after(&t.id, 't')).max().unwrap_or(0) + 1)
    }

    fn new_form_id(t: &Topic) -> String {
        format!("{}.f{}", t.id, t.forms.iter().map(|f| num_after(&f.id, 'f')).max().unwrap_or(0) + 1)
    }

    /// Compact outline for the merge prompt.
    pub fn outline(&self) -> String {
        let mut s = String::new();
        for c in &self.chapters {
            s.push_str(&format!("{} {}\n", c.id, c.title));
            for t in &c.topics {
                s.push_str(&format!("  {} {}\n", t.id, t.title));
                for f in &t.forms {
                    s.push_str(&format!("    {} {}\n", f.id, f.title));
                }
            }
        }
        s
    }

    /// Form whose title matches (same topic title preferred).
    fn find_form_by_title(&self, topic: &str, form: &str) -> Option<String> {
        let (nt, nf) = (norm(topic), norm(form));
        if nf.is_empty() {
            return None;
        }
        let mut any = None;
        for t in self.chapters.iter().flat_map(|c| &c.topics) {
            for f in &t.forms {
                if norm(&f.title) == nf {
                    if norm(&t.title) == nt {
                        return Some(f.id.clone());
                    }
                    any.get_or_insert(f.id.clone());
                }
            }
        }
        any
    }

    /// Place a raw without the model: under a topic or chapter with the same title, else in a new chapter.
    fn place_fallback(&mut self, r: &Raw) -> String {
        let form_title = if r.form.is_empty() { if r.topic.is_empty() { "Khác".to_string() } else { r.topic.clone() } } else { r.form.clone() };
        if let Some(id) = self.find_form_by_title(&r.topic, &form_title) {
            return id;
        }
        let topic_title = if r.topic.is_empty() { "Khác".to_string() } else { r.topic.clone() };
        let chapter_title = if r.chapter.is_empty() { "Khác".to_string() } else { r.chapter.clone() };
        let ci = match self.chapters.iter().position(|c| c.topics.iter().any(|t| norm(&t.title) == norm(&topic_title))) {
            Some(i) => i,
            None => match self.chapters.iter().position(|c| norm(&c.title) == norm(&chapter_title)) {
                Some(i) => i,
                None => {
                    let id = self.new_chapter_id();
                    self.chapters.push(Chapter { id, title: chapter_title, topics: Vec::new() });
                    self.chapters.len() - 1
                }
            },
        };
        let ch = &mut self.chapters[ci];
        let ti = match ch.topics.iter().position(|t| norm(&t.title) == norm(&topic_title)) {
            Some(i) => i,
            None => {
                let id = Tree::new_topic_id(ch);
                ch.topics.push(Topic { id, title: topic_title, summary: String::new(), forms: Vec::new() });
                ch.topics.len() - 1
            }
        };
        let tp = &mut ch.topics[ti];
        let id = Tree::new_form_id(tp);
        tp.forms.push(Form { id: id.clone(), title: form_title, method: String::new() });
        id
    }
}

// ---------------------------------------------------------------- raws

/// A distinct `(chapter, topic, form)` as read from the pages.
#[derive(Clone, Debug)]
pub struct Raw {
    pub id: String,
    pub key: String,
    pub chapter: String,
    pub topic: String,
    pub form: String,
    pub qids: Vec<usize>,
}

pub fn raw_key(q: &QRec) -> String {
    format!("{}|{}|{}", norm(&q.chapter), norm(&q.topic), norm(&q.form))
}

pub fn raw_triples(qs: &[QRec]) -> Vec<Raw> {
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    let mut out: Vec<Raw> = Vec::new();
    for (i, q) in qs.iter().enumerate() {
        let key = raw_key(q);
        let idx = *map.entry(key.clone()).or_insert_with(|| {
            out.push(Raw { id: format!("r{}", out.len() + 1), key, chapter: q.chapter.clone(), topic: q.topic.clone(), form: q.form.clone(), qids: Vec::new() });
            out.len() - 1
        });
        out[idx].qids.push(i);
    }
    out
}

/// raw key → form id, persisted in `ingest/assign.json`.
pub type Assign = BTreeMap<String, String>;

/// Deterministic placement: remembered keys, then exact form titles. Returns the raws still open.
pub fn place_known(tree: &Tree, raws: &[Raw], assign: &mut Assign) -> Vec<Raw> {
    let mut open = Vec::new();
    for r in raws {
        if assign.get(&r.key).is_some_and(|id| tree.has_form(id)) {
            continue;
        }
        match tree.find_form_by_title(&r.topic, &r.form) {
            Some(id) => {
                assign.insert(r.key.clone(), id);
            }
            None => open.push(r.clone()),
        }
    }
    open
}

// ---------------------------------------------------------------- the model's part

pub const MERGE_SYSTEM: &str = r#"You organise questions from Vietnamese school test papers into a curriculum tree. Output JSON only; all titles in Vietnamese.
Input: the EXISTING tree (ids and titles, may be empty) and RAW groups "chapter > topic > form" as read from the pages, with counts and a sample question.
Return {"chapters":[{"id":"","title":"","topics":[{"id":"","title":"","summary":"","forms":[{"id":"","title":"","method":"","raw":["r1"]}]}]}]}
- Every RAW id goes into exactly one form. Put RAWs that are the same task (the same dạng bài written differently) into one form; keep forms specific and useful for practice. Fix vague names; chapters and topics as in the Vietnamese textbook.
- To use an EXISTING chapter, topic or form give its id and leave its title, summary and method "". For a new item give id "" and a clear title; a new topic gets summary = one short sentence; a new form gets method = the standard solving method in 2-4 short numbered steps, LaTeX in $...$.
- Chapters and topics in textbook order. Include existing items only when they receive RAWs."#;

pub fn merge_schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object", "additionalProperties": false, "required": ["chapters"],
        "properties": {"chapters": {"type": "array", "items": {"type": "object", "additionalProperties": false,
            "required": ["id", "title", "topics"],
            "properties": {"id": s, "title": s, "topics": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["id", "title", "summary", "forms"],
                "properties": {"id": s, "title": s, "summary": s, "forms": {"type": "array", "items": {"type": "object",
                    "additionalProperties": false, "required": ["id", "title", "method", "raw"],
                    "properties": {"id": s, "title": s, "method": s, "raw": {"type": "array", "items": s}}}}}}}}}}}
    })
}

/// The text the merge model reads (stable prefix first: subject, goal, existing outline; then the raws).
pub fn merge_prompt(subject: &str, goal: Option<&str>, tree: &Tree, open: &[Raw], qs: &[QRec]) -> String {
    let mut p = format!("SUBJECT: {subject}\n");
    if let Some(g) = goal.filter(|g| !g.trim().is_empty()) {
        p.push_str(&format!("GOAL: {g}\n"));
    }
    if !tree.chapters.is_empty() {
        p.push_str("EXISTING:\n");
        p.push_str(&tree.outline());
    }
    p.push_str("RAW (id | chapter > topic > form | count | levels nb/th/vd/vdc | sample):\n");
    for r in open {
        let rq: Vec<&QRec> = r.qids.iter().map(|i| &qs[*i]).collect();
        let lc = level_counts(&rq);
        let sample = rq.iter().find(|q| !q.has_figure && q.text.chars().count() > 15).or(rq.first()).map(|q| q.text.as_str()).unwrap_or("");
        p.push_str(&format!(
            "{} | {} > {} > {} | {} | {}/{}/{}/{} | {}\n",
            r.id, r.chapter, r.topic, r.form, rq.len(), lc["nb"], lc["th"], lc["vd"], lc["vdc"], clip(sample, 110)
        ));
    }
    p
}

/// At least half of the open raws must be placed for a reply to count.
pub fn check_merge(v: &Value, open: &[Raw]) -> Result<(), String> {
    let ids: HashSet<&str> = open.iter().map(|r| r.id.as_str()).collect();
    let chapters = v["chapters"].as_array().ok_or("thiếu chapters")?;
    let used = chapters
        .iter()
        .flat_map(|c| c["topics"].as_array().into_iter().flatten())
        .flat_map(|t| t["forms"].as_array().into_iter().flatten())
        .flat_map(|f| f["raw"].as_array().into_iter().flatten())
        .filter(|r| r.as_str().is_some_and(|r| ids.contains(r)))
        .count();
    if used * 2 < ids.len() {
        return Err(format!("cây mới dùng {used}/{} nhóm câu", ids.len()));
    }
    Ok(())
}

/// Apply the model's tree to `tree`: existing nodes by id (or same title), new ones get fresh ids and
/// go after the item the model listed before them. Open raws the model skipped are placed by title.
pub fn apply_merge(tree: &mut Tree, v: &Value, open: &[Raw], assign: &mut Assign) {
    let raw_by_id: HashMap<&str, &Raw> = open.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut done: HashSet<String> = HashSet::new();
    let mut c_anchor: Option<usize> = None;
    for c in v["chapters"].as_array().into_iter().flatten() {
        let (cid_in, ctitle) = (s_of(&c["id"]), s_of(&c["title"]));
        let ci = match tree.chapters.iter().position(|x| (!cid_in.is_empty() && x.id == cid_in) || (!ctitle.is_empty() && norm(&x.title) == norm(&ctitle))) {
            Some(i) => i,
            None => {
                let at = c_anchor.map(|a| a + 1).unwrap_or(0).min(tree.chapters.len());
                let id = tree.new_chapter_id();
                let title = if ctitle.is_empty() { "Khác".to_string() } else { ctitle };
                tree.chapters.insert(at, Chapter { id, title, topics: Vec::new() });
                at
            }
        };
        c_anchor = Some(ci);
        let mut t_anchor: Option<usize> = None;
        for t in c["topics"].as_array().into_iter().flatten() {
            let (tid_in, ttitle) = (s_of(&t["id"]), s_of(&t["title"]));
            let ch = &mut tree.chapters[ci];
            let ti = match ch.topics.iter().position(|x| (!tid_in.is_empty() && x.id == tid_in) || (!ttitle.is_empty() && norm(&x.title) == norm(&ttitle))) {
                Some(i) => i,
                None => {
                    let at = t_anchor.map(|a| a + 1).unwrap_or(ch.topics.len()).min(ch.topics.len());
                    let id = Tree::new_topic_id(ch);
                    let title = if ttitle.is_empty() { "Khác".to_string() } else { ttitle };
                    ch.topics.insert(at, Topic { id, title, summary: s_of(&t["summary"]), forms: Vec::new() });
                    at
                }
            };
            t_anchor = Some(ti);
            for f in t["forms"].as_array().into_iter().flatten() {
                let raws: Vec<&Raw> = f["raw"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str())
                    .filter_map(|r| raw_by_id.get(r).copied())
                    .filter(|r| !done.contains(&r.id))
                    .collect();
                if raws.is_empty() {
                    continue;
                }
                let (fid_in, ftitle) = (s_of(&f["id"]), s_of(&f["title"]));
                let fid = if !fid_in.is_empty() && tree.has_form(&fid_in) {
                    fid_in
                } else {
                    let tp = &mut tree.chapters[ci].topics[ti];
                    match tp.forms.iter().find(|x| !ftitle.is_empty() && norm(&x.title) == norm(&ftitle)) {
                        Some(x) => x.id.clone(),
                        None => {
                            let id = Tree::new_form_id(tp);
                            let title = if ftitle.is_empty() { raws[0].form.clone() } else { ftitle };
                            tp.forms.push(Form { id: id.clone(), title, method: s_of(&f["method"]) });
                            id
                        }
                    }
                };
                for r in raws {
                    done.insert(r.id.clone());
                    assign.insert(r.key.clone(), fid.clone());
                }
            }
        }
    }
    for r in open.iter().filter(|r| !done.contains(&r.id)) {
        let id = tree.place_fallback(r);
        assign.insert(r.key.clone(), id);
    }
}

// ---------------------------------------------------------------- structures, progress, knowledge

fn section_default_title(kind: &str) -> &'static str {
    match kind {
        "mcq" => "Trắc nghiệm",
        "truefalse" => "Trắc nghiệm đúng sai",
        "short" => "Trả lời ngắn",
        _ => "Tự luận",
    }
}

fn kind_words(kind: &str) -> &'static str {
    match kind {
        "mcq" => "trắc nghiệm",
        "truefalse" => "đúng sai",
        "short" => "trả lời ngắn",
        _ => "tự luận",
    }
}

/// `[(section label, majority kind, count)]` of one source, in order of appearance.
fn sections_of(qs: &[&QRec]) -> Vec<(String, String, usize)> {
    let mut v: Vec<(String, BTreeMap<String, usize>)> = Vec::new();
    for q in qs {
        let i = match v.iter().position(|x| norm(&x.0) == norm(&q.section)) {
            Some(i) => i,
            None => {
                v.push((q.section.clone(), BTreeMap::new()));
                v.len() - 1
            }
        };
        *v[i].1.entry(q.kind.clone()).or_insert(0) += 1;
    }
    v.into_iter()
        .map(|(l, k)| {
            let n = k.values().sum();
            let kind = k.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k).unwrap_or_else(|| "essay".into());
            (l, kind, n)
        })
        .collect()
}

/// "Đề giữa kỳ I (trắc nghiệm + tự luận, 90 phút)" from a source title and its sections.
pub fn structure_title(s: &SrcRec, secs: &[(String, String, usize)]) -> String {
    let t = norm(&s.title);
    let term = if t.contains(" ii") || t.contains("kì 2") || t.contains("kỳ 2") { "II" } else { "I" };
    let base = if t.contains("giữa") {
        format!("Đề giữa kỳ {term}")
    } else if t.contains("cuối") {
        format!("Đề cuối kỳ {term}")
    } else if t.contains("tuyển sinh") || t.contains("vào lớp 10") || t.contains("vào 10") {
        "Đề tuyển sinh vào 10".to_string()
    } else if t.contains("15 phút") {
        "Đề kiểm tra 15 phút".to_string()
    } else if t.contains("45 phút") || t.contains("1 tiết") || t.contains("một tiết") {
        "Đề kiểm tra 1 tiết".to_string()
    } else if t.contains("học sinh giỏi") || t.contains("hsg") {
        "Đề học sinh giỏi".to_string()
    } else {
        "Đề kiểm tra".to_string()
    };
    let mut kinds: Vec<&str> = Vec::new();
    for (_, k, _) in secs {
        let w = kind_words(k);
        if !kinds.contains(&w) {
            kinds.push(w);
        }
    }
    let mut detail = kinds.join(" + ");
    if s.duration_min > 0 {
        detail.push_str(&format!(", {} phút", s.duration_min));
    }
    if detail.is_empty() {
        base
    } else {
        format!("{base} ({detail})")
    }
}

/// Same format: same sequence of section kinds, counts within 25 %, same time (when both are known).
fn same_format(a: &(Vec<(String, String, usize)>, u32), b: &(Vec<(String, String, usize)>, u32)) -> bool {
    a.0.len() == b.0.len()
        && a.0.iter().zip(&b.0).all(|(x, y)| x.1 == y.1 && (x.2 as f64 - y.2 as f64).abs() <= 0.25 * x.2.max(y.2) as f64 + 0.5)
        && (a.1 == 0 || b.1 == 0 || a.1 == b.1)
}

fn date_key(d: &str) -> (u32, u32, u32) {
    let v: Vec<u32> = d.split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).collect();
    match v.as_slice() {
        [dd, mm, yy, ..] if *yy > 31 => (*yy, *mm, *dd),
        [yy, mm, dd, ..] if *yy > 31 => (*yy, *mm, *dd),
        _ => (0, 0, 0),
    }
}

fn grade_of(rec: &Records, existing: Option<&Value>) -> String {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for s in &rec.sources {
        let g: String = s.grade.chars().filter(|c| c.is_ascii_digit()).collect();
        if !g.is_empty() {
            *count.entry(g).or_insert(0) += 1;
        }
    }
    if count.is_empty() {
        // "TOÁN 9", "HÓA HỌC 10" in titles.
        for s in &rec.sources {
            let words: Vec<&str> = s.title.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
            for w in words.windows(2) {
                if matches!(norm(w[0]).as_str(), "toán" | "lý" | "lí" | "hóa" | "học" | "văn" | "anh" | "lớp") {
                    if let Ok(n) = w[1].parse::<u32>() {
                        if (1..=12).contains(&n) {
                            *count.entry(n.to_string()).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }
    count
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(g, _)| g)
        .or_else(|| existing.map(|k| s_of(&k["grade"])).filter(|g| !g.is_empty()))
        .unwrap_or_default()
}

/// Assemble `knowledge.json`. Pure function (tested directly).
pub fn build_knowledge(input: &IngestInput, rec: &Records, tree: &Tree, assign: &Assign, now: &str) -> Value {
    let existing = input.existing.as_ref();
    let fid_of: Vec<Option<String>> = rec.questions.iter().map(|q| assign.get(&raw_key(q)).filter(|id| tree.has_form(id)).cloned()).collect();
    let total_q = rec.questions.len().max(1);
    let src_by_id: HashMap<&str, &SrcRec> = rec.sources.iter().map(|s| (s.id.as_str(), s)).collect();
    let file_of = |q: &QRec| -> String {
        if !q.file.is_empty() {
            q.file.clone()
        } else {
            src_by_id.get(q.src.as_str()).and_then(|s| s.file_ids.first().cloned()).unwrap_or_default()
        }
    };
    let mut topic_of_form: HashMap<String, String> = HashMap::new();
    let mut curriculum = Vec::new();
    for c in &tree.chapters {
        let mut topics = Vec::new();
        for t in &c.topics {
            let mut forms = Vec::new();
            let mut tcount = 0;
            for f in &t.forms {
                topic_of_form.insert(f.id.clone(), t.id.clone());
                let qs: Vec<&QRec> = (0..rec.questions.len()).filter(|i| fid_of[*i].as_deref() == Some(f.id.as_str())).map(|i| &rec.questions[i]).collect();
                tcount += qs.len();
                let ex = qs
                    .iter()
                    .filter(|q| !q.has_figure && q.text.chars().count() > 15)
                    .max_by_key(|q| LEVELS.iter().position(|l| *l == q.level).unwrap_or(1) * 10 + q.text.len().min(9))
                    .or(qs.first());
                let refs: Vec<Value> = qs.iter().take(8).map(|q| json!({"fileId": file_of(q), "q": q.n})).collect();
                let mut fv = json!({"id": f.id, "title": f.title, "method": f.method, "count": qs.len(), "levels": level_counts(&qs), "refs": refs});
                if let Some(e) = ex {
                    fv["example"] = json!({"md": e.text});
                    if !e.answer.is_empty() {
                        fv["example"]["answer"] = json!(e.answer);
                    }
                }
                forms.push(fv);
            }
            topics.push(json!({"id": t.id, "title": t.title, "summary": t.summary, "count": tcount,
                "share": round(tcount as f64 / total_q as f64, 3), "forms": forms}));
        }
        curriculum.push(json!({"id": c.id, "title": c.title, "topics": topics}));
    }

    // --- structures: full exams grouped by format
    let mut groups: Vec<((Vec<(String, String, usize)>, u32), Vec<&SrcRec>)> = Vec::new();
    for s in rec.sources.iter().filter(|s| s.full_exam) {
        let qs: Vec<&QRec> = rec.questions.iter().filter(|q| q.src == s.id).collect();
        if qs.is_empty() {
            continue;
        }
        let sig = (sections_of(&qs), s.duration_min);
        match groups.iter_mut().find(|g| same_format(&g.0, &sig)) {
            Some(g) => g.1.push(s),
            None => groups.push((sig, vec![s])),
        }
    }
    let old_structs: Vec<Value> = existing.and_then(|k| k["structures"].as_array().cloned()).unwrap_or_default();
    let mut used_sids: HashSet<String> = HashSet::new();
    let mut next_s = old_structs.iter().filter_map(|s| s_of(&s["id"]).strip_prefix('s').and_then(|n| n.parse::<usize>().ok())).max().unwrap_or(0) + 1;
    let mut structures: Vec<Value> = Vec::new();
    for (_, members) in groups {
        let rep = *members.iter().max_by_key(|s| rec.questions.iter().filter(|q| q.src == s.id).count()).unwrap();
        let idx: Vec<usize> = (0..rec.questions.len()).filter(|i| rec.questions[*i].src == rep.id).collect();
        let qs: Vec<&QRec> = idx.iter().map(|i| &rec.questions[*i]).collect();
        let secs = sections_of(&qs);
        let pts = fill_points(&qs, rep.total_points);
        let mut sections = Vec::new();
        let mut matrix: BTreeMap<(usize, String, String, usize), (usize, f64)> = BTreeMap::new();
        for (si, (label, kind, _)) in secs.iter().enumerate() {
            let sid = ((b'A' + (si as u8).min(25)) as char).to_string();
            let members_q: Vec<usize> = (0..qs.len()).filter(|j| norm(&qs[*j].section) == norm(label)).collect();
            let sec_rec = rep.sections.iter().find(|s| norm(&s.label) == norm(label));
            let spoints: f64 = members_q.iter().map(|j| pts[*j]).sum();
            let title = sec_rec.map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| section_default_title(kind).to_string());
            sections.push(json!({"id": sid, "title": title, "kind": kind, "count": members_q.len(),
                "pointsEach": round(spoints / members_q.len().max(1) as f64, 2), "points": round(spoints, 2),
                "instructions": sec_rec.map(|s| s.instructions.clone()).unwrap_or_default()}));
            for j in members_q {
                let fid = fid_of[idx[j]].clone().unwrap_or_default();
                let tid = topic_of_form.get(&fid).cloned().unwrap_or_default();
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
        let mut based: Vec<String> = Vec::new();
        for s in &members {
            for f in &s.file_ids {
                if !based.contains(f) {
                    based.push(f.clone());
                }
            }
        }
        let title = structure_title(rep, &secs);
        // Stable id: the old structure built on the same files, else the one with the same title.
        let old = old_structs
            .iter()
            .find(|o| !used_sids.contains(&s_of(&o["id"])) && o["basedOn"].as_array().into_iter().flatten().any(|f| based.contains(&s_of(f))))
            .or_else(|| old_structs.iter().find(|o| !used_sids.contains(&s_of(&o["id"])) && norm(&s_of(&o["title"])) == norm(&title)));
        let id = match old {
            Some(o) => s_of(&o["id"]),
            None => {
                next_s += 1;
                format!("s{}", next_s - 1)
            }
        };
        used_sids.insert(id.clone());
        let duration = members.iter().map(|s| s.duration_min).find(|d| *d > 0).unwrap_or(90);
        structures.push(json!({"id": id, "title": title, "basedOn": based, "durationMin": duration,
            "totalPoints": round(pts.iter().sum(), 2), "sections": sections, "matrix": matrix}));
    }

    // --- school progress: topics seen so far; current = furthest topic of the newest document
    let order: Vec<String> = tree.chapters.iter().flat_map(|c| c.topics.iter().map(|t| t.id.clone())).collect();
    let topic_of_q = |i: usize| fid_of[i].as_ref().and_then(|f| topic_of_form.get(f)).cloned();
    let covered: Vec<String> = order.iter().filter(|t| (0..rec.questions.len()).any(|i| topic_of_q(i).as_deref() == Some(t.as_str()))).cloned().collect();
    let newest = rec.sources.iter().enumerate().filter(|(_, s)| rec.questions.iter().any(|q| q.src == s.id)).max_by_key(|(i, s)| (s.run, date_key(&s.date), *i)).map(|(_, s)| s);
    let current = newest.and_then(|s| {
        (0..rec.questions.len()).filter(|i| rec.questions[*i].src == s.id).filter_map(topic_of_q).max_by_key(|t| order.iter().position(|o| o == t))
    });
    let school_progress = if covered.is_empty() {
        existing.map(|k| k["schoolProgress"].clone()).filter(|v| v.is_object()).unwrap_or_else(|| json!({"coveredTopicIds": [], "currentTopicId": null, "confidence": 0.0, "note": ""}))
    } else {
        let title_of = |id: &str| tree.chapters.iter().flat_map(|c| &c.topics).find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default();
        let note = match (newest, &current) {
            (Some(s), Some(c)) => format!(
                "Suy từ {} tài liệu đã nạp: tài liệu mới nhất “{}” có tới chủ đề “{}”.",
                rec.sources.len(),
                clip(if s.title.is_empty() { &s.name } else { &s.title }, 60),
                title_of(c)
            ),
            _ => format!("Suy từ {} tài liệu đã nạp.", rec.sources.len()),
        };
        let conf = if newest.is_some_and(|s| s.full_exam || s.kind == "review") { 0.6 } else { 0.4 };
        json!({"coveredTopicIds": covered, "currentTopicId": current, "confidence": conf, "note": note})
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
    let goal = input.goal.clone().filter(|g| !g.trim().is_empty()).or_else(|| existing.and_then(|k| k["goal"].as_str().map(String::from)));
    json!({
        "v": 1, "folderId": input.folder_id, "subject": input.subject, "grade": grade_of(rec, existing), "goal": goal,
        "updatedAt": now, "sources": sources, "curriculum": curriculum, "structures": structures,
        "schoolProgress": school_progress,
        "stats": {"questions": rec.questions.len(), "byLevel": level_counts(&all), "byKind": by_kind},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(src: &str, sec: &str, kind: &str, ch: &str, tp: &str, fm: &str) -> QRec {
        QRec { src: src.into(), section: sec.into(), kind: kind.into(), level: "th".into(), chapter: ch.into(), topic: tp.into(), form: fm.into(),
            text: "Câu hỏi đủ dài để làm ví dụ".into(), n: "1".into(), ..Default::default() }
    }

    #[test]
    fn known_raws_skip_the_model_and_new_ones_get_fresh_ids() {
        let mut tree = Tree::default();
        let qs = vec![q("a", "", "mcq", "Căn", "Căn bậc hai", "Tìm ĐKXĐ"), q("a", "", "mcq", "Căn", "Căn bậc hai", "Rút gọn")];
        let raws = raw_triples(&qs);
        let mut assign = Assign::new();
        let open = place_known(&tree, &raws, &mut assign);
        assert_eq!(open.len(), 2, "empty tree: everything is new");
        let v = json!({"chapters": [{"id": "", "title": "Căn bậc hai, căn bậc ba", "topics": [{"id": "", "title": "Căn bậc hai", "summary": "s",
            "forms": [{"id": "", "title": "Tìm điều kiện xác định", "method": "1. …", "raw": ["r1"]}]}]}]});
        apply_merge(&mut tree, &v, &open, &mut assign);
        assert_eq!(tree.chapters[0].id, "c1");
        assert_eq!(tree.chapters[0].topics[0].forms[0].id, "c1.t1.f1");
        // r2 was skipped by the model: placed by its topic title.
        assert_eq!(assign[&raws[1].key], "c1.t1.f2");
        // Next ingest: both keys are known, nothing goes to the model.
        let again = place_known(&tree, &raws, &mut assign);
        assert!(again.is_empty());
        // A new chapter listed before c1 goes in front, with the next number.
        let more = vec![q("b", "", "mcq", "Mệnh đề", "Mệnh đề", "Phủ định")];
        let raws2 = raw_triples(&more);
        let open2 = place_known(&tree, &raws2, &mut assign);
        let v2 = json!({"chapters": [{"id": "", "title": "Mệnh đề", "topics": [{"id": "", "title": "Mệnh đề", "summary": "", "forms": [
            {"id": "", "title": "Phủ định mệnh đề", "method": "", "raw": ["r1"]}]}]}, {"id": "c1", "title": "", "topics": []}]});
        apply_merge(&mut tree, &v2, &open2, &mut assign);
        assert_eq!(tree.chapters.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c2", "c1"]);
    }

    #[test]
    fn structure_titles_and_grouping_are_deterministic() {
        let s = SrcRec { title: "ĐỀ ÔN TẬP KIỂM TRA GIỮA HỌC KÌ I".into(), duration_min: 90, ..Default::default() };
        let secs = vec![("I".to_string(), "mcq".to_string(), 12), ("II".to_string(), "essay".to_string(), 8)];
        assert_eq!(structure_title(&s, &secs), "Đề giữa kỳ I (trắc nghiệm + tự luận, 90 phút)");
        let a = (secs.clone(), 90);
        let b = (vec![("I".to_string(), "mcq".to_string(), 11), ("II".to_string(), "essay".to_string(), 9)], 90);
        let c = (vec![("I".to_string(), "mcq".to_string(), 40)], 90);
        assert!(same_format(&a, &b));
        assert!(!same_format(&a, &c));
        assert_eq!(date_key("03/11/2026"), (2026, 11, 3));
    }
}
