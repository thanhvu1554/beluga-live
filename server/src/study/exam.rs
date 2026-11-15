//! Exam generation (PROTOCOL §13): a new exam that follows one `structures[]` entry of the folder's
//! knowledge exactly, plus the printable A4 HTML (exam, then the answer key on its own page).
//!
//! Rust owns the blueprint: one slot per question with section, kind, level, points, topic and form
//! taken from the structure's matrix. The model only writes the content of each slot; Rust checks every
//! slot is filled once with the right shape (choices, answer, solution) and asks for one repair if not.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use super::ingest::{norm, KINDS, LEVELS};
use super::llm::{Llm, LlmRequest, Part, Usage};

#[derive(Clone, Debug)]
pub struct ExamOptions {
    pub model: String,
    pub effort: Option<String>,
    pub max_tokens: u32,
    /// Retries for transport errors (not counting the one content repair).
    pub retries: usize,
}

impl Default for ExamOptions {
    fn default() -> Self {
        ExamOptions { model: "anthropic/claude-sonnet-5.5".into(), effort: Some("medium".into()), max_tokens: 32_000, retries: 1 }
    }
}

/// One question to write.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// 1-based over the whole exam.
    pub slot: usize,
    pub section: String,
    pub kind: String,
    pub level: String,
    pub points: f64,
    pub topic_id: String,
    pub form_id: String,
    pub topic: String,
    pub form: String,
    pub method: String,
    pub example: String,
}

fn s_of(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn find_form<'a>(k: &'a Value, form_id: &str) -> Option<(&'a Value, &'a Value)> {
    for c in k["curriculum"].as_array()? {
        for t in c["topics"].as_array().into_iter().flatten() {
            for f in t["forms"].as_array().into_iter().flatten() {
                if f["id"] == form_id {
                    return Some((t, f));
                }
            }
        }
    }
    None
}

fn find_topic<'a>(k: &'a Value, topic_id: &str) -> Option<&'a Value> {
    k["curriculum"].as_array()?.iter().flat_map(|c| c["topics"].as_array().into_iter().flatten()).find(|t| t["id"] == topic_id)
}

/// Topics to use when a section has more questions than matrix rows: the most frequent ones.
fn filler_forms(k: &Value) -> Vec<(String, String)> {
    let mut v: Vec<(u64, String, String)> = Vec::new();
    for c in k["curriculum"].as_array().into_iter().flatten() {
        for t in c["topics"].as_array().into_iter().flatten() {
            for f in t["forms"].as_array().into_iter().flatten() {
                v.push((f["count"].as_u64().unwrap_or(0), s_of(&t["id"]), s_of(&f["id"])));
            }
        }
    }
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, t, f)| (t, f)).collect()
}

/// Expand a structure into slots. Errors if the structure is unknown or empty.
pub fn blueprint(k: &Value, structure_id: &str) -> Result<(Value, Vec<Slot>), String> {
    let st = k["structures"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["id"] == structure_id))
        .cloned()
        .ok_or_else(|| format!("không có cấu trúc đề {structure_id}"))?;
    let fillers = filler_forms(k);
    let mut slots: Vec<Slot> = Vec::new();
    for sec in st["sections"].as_array().into_iter().flatten() {
        let sid = s_of(&sec["id"]);
        let kind = s_of(&sec["kind"]);
        let kind = if KINDS.contains(&kind.as_str()) { kind } else { "essay".into() };
        let want = sec["count"].as_u64().unwrap_or(0) as usize;
        let mut here: Vec<Slot> = Vec::new();
        for m in st["matrix"].as_array().into_iter().flatten().filter(|m| m["section"] == sid.as_str()) {
            let n = m["count"].as_u64().unwrap_or(0) as usize;
            if n == 0 {
                continue;
            }
            let pts = m["points"].as_f64().unwrap_or(0.0);
            let each = round2(pts / n as f64);
            for i in 0..n {
                // Last item takes the rounding remainder so the row's points stay exact.
                let p = if i + 1 == n { round2(pts - each * (n - 1) as f64) } else { each };
                here.push(Slot {
                    slot: 0,
                    section: sid.clone(),
                    kind: kind.clone(),
                    level: m["level"].as_str().filter(|l| LEVELS.contains(l)).unwrap_or("th").to_string(),
                    points: p,
                    topic_id: s_of(&m["topicId"]),
                    form_id: s_of(&m["formId"]),
                    topic: String::new(),
                    form: String::new(),
                    method: String::new(),
                    example: String::new(),
                });
            }
        }
        let want = if want == 0 { here.len() } else { want };
        here.truncate(want);
        let each = sec["pointsEach"].as_f64().unwrap_or(0.25);
        let mut fi = 0;
        while here.len() < want && !fillers.is_empty() {
            let (t, f) = &fillers[fi % fillers.len()];
            fi += 1;
            here.push(Slot {
                slot: 0,
                section: sid.clone(),
                kind: kind.clone(),
                level: "th".into(),
                points: each,
                topic_id: t.clone(),
                form_id: f.clone(),
                topic: String::new(),
                form: String::new(),
                method: String::new(),
                example: String::new(),
            });
        }
        slots.extend(here);
    }
    if slots.is_empty() {
        return Err("cấu trúc đề không có câu nào".into());
    }
    for (i, s) in slots.iter_mut().enumerate() {
        s.slot = i + 1;
        if let Some((t, f)) = find_form(k, &s.form_id) {
            s.topic = s_of(&t["title"]);
            s.form = s_of(&f["title"]);
            s.method = s_of(&f["method"]);
            s.example = s_of(&f["example"]["md"]);
        } else if let Some(t) = find_topic(k, &s.topic_id) {
            s.topic = s_of(&t["title"]);
        }
    }
    Ok((st, slots))
}

pub const EXAM_SYSTEM: &str = r#"You write NEW Vietnamese school exam questions that follow a fixed blueprint exactly. Output JSON only; all question text in Vietnamese, math in LaTeX between $...$ (display math $$...$$).

For every blueprint slot write exactly one item, with the same "slot" number, testing the given form (dạng bài) of the given topic at the given level:
- nb: recall/recognise; th: direct one-step application; vd: multi-step problem; vdc: hard, non-routine.
- Do NOT copy the sample; change numbers, functions and context. Keep numbers friendly so the answer is clean.
- mcq: "choices" = exactly 4 options WITHOUT "A." prefixes, exactly one correct; "answer" = "A"|"B"|"C"|"D". Spread correct letters.
- truefalse: "text" = shared stem, "statements" = exactly 4 statements; "answer" = 4 letters Đ/S separated by commas, e.g. "Đ,S,S,Đ".
- short: "answer" = the final value only.
- essay: may have its own sub-steps; "answer" = final result in one line; "solution" = concise marking guide with the key steps (and point split if > 0.5 points).
- For mcq/truefalse/short "solution" = one short line of justification.
- Never require a figure that is not described in words; describe any figure precisely in the text.
- KaTeX-compatible LaTeX only: systems as $\begin{cases}x+y=5\\x-y=1\end{cases}$, fractions \dfrac in text, decimals with a comma ($0{,}5$).
- Check every answer yourself before writing it.
"title": a short Vietnamese exam title (e.g. "Đề kiểm tra giữa kỳ I – Toán 9 (đề luyện)")."#;

fn exam_schema() -> Value {
    let s = json!({"type": "string"});
    json!({
        "type": "object", "additionalProperties": false, "required": ["title", "questions"],
        "properties": {
            "title": s,
            "questions": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                "required": ["slot", "text", "choices", "statements", "answer", "solution"],
                "properties": {"slot": {"type": "integer"}, "text": s, "choices": {"type": "array", "items": s},
                    "statements": {"type": "array", "items": s}, "answer": s, "solution": s}}}
        }
    })
}

pub fn exam_prompt(k: &Value, st: &Value, slots: &[Slot], note: Option<&str>) -> String {
    let mut p = format!(
        "SUBJECT: {} · GRADE: {} · STRUCTURE: {} · {} minutes · {} points\n",
        s_of(&k["subject"]),
        s_of(&k["grade"]),
        s_of(&st["title"]),
        st["durationMin"].as_u64().unwrap_or(90),
        st["totalPoints"].as_f64().unwrap_or(10.0)
    );
    if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
        p.push_str(&format!(
            "STUDENT REQUEST (apply within the fixed blueprint; never change kinds, levels, counts): {n}\n"
        ));
    }
    p.push_str("SECTIONS:\n");
    for sec in st["sections"].as_array().into_iter().flatten() {
        p.push_str(&format!(
            "{} | {} | {} | {} items | {}\n",
            s_of(&sec["id"]),
            s_of(&sec["title"]),
            s_of(&sec["kind"]),
            sec["count"],
            s_of(&sec["instructions"])
        ));
    }
    p.push_str("BLUEPRINT (slot | section | kind | level | points | topic > form | method | sample to NOT copy):\n");
    for s in slots {
        p.push_str(&format!(
            "{} | {} | {} | {} | {} | {} > {} | {} | {}\n",
            s.slot,
            s.section,
            s.kind,
            s.level,
            s.points,
            s.topic,
            s.form,
            s.method.replace('\n', " "),
            s.example.replace('\n', " ")
        ));
    }
    p
}

fn balanced_dollars(s: &str) -> bool {
    let mut n = 0;
    let mut prev = ' ';
    for c in s.chars() {
        if c == '$' && prev != '\\' {
            n += 1;
        }
        prev = c;
    }
    n % 2 == 0
}

/// Parse `Đ,S,S,Đ` (also `D/S`, `T/F`, spaces) into 4 booleans.
pub fn parse_tf(a: &str) -> Option<Vec<bool>> {
    let v: Vec<bool> = a
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace() || c == '-')
        .filter(|x| !x.is_empty())
        .map(|x| {
            let mut x = x.to_lowercase();
            // "a)Đ", "b.S", "c:Đ"
            let cs: Vec<char> = x.chars().collect();
            if cs.len() > 2 && "abcd".contains(cs[0]) && ").:".contains(cs[1]) {
                x = cs[2..].iter().collect();
            }
            let x = x.trim_matches(|c: char| !c.is_alphanumeric());
            match x {
                "đ" | "d" | "đúng" | "t" | "true" => Some(true),
                "s" | "sai" | "f" | "false" => Some(false),
                _ => None,
            }
        })
        .collect::<Option<Vec<bool>>>()?;
    (v.len() == 4).then_some(v)
}

/// Check the model's items against the blueprint. Returns the problems (empty = valid).
pub fn validate_items(v: &Value, slots: &[Slot], banned: &HashSet<String>) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(items) = v["questions"].as_array() else {
        return vec!["missing questions array".into()];
    };
    let mut seen = HashSet::new();
    for it in items {
        let n = it["slot"].as_u64().unwrap_or(0) as usize;
        let Some(s) = slots.iter().find(|s| s.slot == n) else {
            errs.push(format!("slot {n} is not in the blueprint"));
            continue;
        };
        if !seen.insert(n) {
            errs.push(format!("slot {n} written twice"));
            continue;
        }
        let text = s_of(&it["text"]);
        let answer = s_of(&it["answer"]);
        if text.chars().count() < 8 {
            errs.push(format!("slot {n}: text missing"));
        }
        if banned.contains(&norm(&text)) {
            errs.push(format!("slot {n}: copied from the sample, write a new question"));
        }
        if !balanced_dollars(&text) {
            errs.push(format!("slot {n}: unbalanced $ in text"));
        }
        let arr = |k: &str| -> Vec<String> { it[k].as_array().map(|a| a.iter().map(s_of).collect()).unwrap_or_default() };
        match s.kind.as_str() {
            "mcq" => {
                let ch = arr("choices");
                if ch.len() != 4 || ch.iter().any(|c| c.is_empty()) {
                    errs.push(format!("slot {n}: mcq needs exactly 4 non-empty choices"));
                } else if ch.iter().map(|c| c.split_whitespace().collect::<String>()).collect::<HashSet<_>>().len() != 4 {
                    errs.push(format!("slot {n}: mcq choices must be distinct"));
                }
                if !["A", "B", "C", "D"].contains(&answer.trim_end_matches('.').to_uppercase().as_str()) {
                    errs.push(format!("slot {n}: mcq answer must be A, B, C or D"));
                }
            }
            "truefalse" => {
                if arr("statements").len() != 4 || arr("statements").iter().any(|c| c.is_empty()) {
                    errs.push(format!("slot {n}: truefalse needs exactly 4 statements"));
                }
                if parse_tf(&answer).is_none() {
                    errs.push(format!("slot {n}: truefalse answer must be like Đ,S,S,Đ"));
                }
            }
            "short" => {
                if answer.is_empty() {
                    errs.push(format!("slot {n}: short answer missing"));
                }
            }
            _ => {
                if s_of(&it["solution"]).chars().count() < 10 {
                    errs.push(format!("slot {n}: essay needs a solution / marking guide"));
                }
            }
        }
    }
    let missing: Vec<String> = slots.iter().filter(|s| !seen.contains(&s.slot)).map(|s| s.slot.to_string()).collect();
    if !missing.is_empty() {
        errs.push(format!("missing slots: {}", missing.join(", ")));
    }
    errs
}

fn section_label(kind: &str) -> &'static str {
    if kind == "essay" {
        "Bài"
    } else {
        "Câu"
    }
}

/// Final exam document from blueprint + model items (assumes they validated).
pub fn assemble(
    k: &Value,
    st: &Value,
    slots: &[Slot],
    items: &Value,
    exam_id: &str,
    note: Option<&str>,
    model: &str,
    usage: &Usage,
    now: &str,
) -> Value {
    let item = |n: usize| items["questions"].as_array().and_then(|a| a.iter().find(|i| i["slot"].as_u64() == Some(n as u64)));
    let mut sections = Vec::new();
    let (mut n_obj, mut n_essay) = (0, 0);
    for sec in st["sections"].as_array().into_iter().flatten() {
        let sid = s_of(&sec["id"]);
        let mut qs = Vec::new();
        for s in slots.iter().filter(|s| s.section == sid) {
            let it = item(s.slot).cloned().unwrap_or(Value::Null);
            let label = if s.kind == "essay" {
                n_essay += 1;
                format!("{} {}", section_label(&s.kind), n_essay)
            } else {
                n_obj += 1;
                format!("{} {}", section_label(&s.kind), n_obj)
            };
            let mut q = json!({
                "no": s.slot, "label": label, "kind": s.kind, "level": s.level, "points": s.points,
                "topicId": s.topic_id, "formId": s.form_id, "text": s_of(&it["text"]),
                "answer": s_of(&it["answer"]).trim_end_matches('.').to_string(), "solution": s_of(&it["solution"]),
            });
            if s.kind == "mcq" {
                q["answer"] = json!(s_of(&it["answer"]).trim_end_matches('.').to_uppercase());
                q["choices"] = it["choices"].clone();
            }
            if s.kind == "truefalse" {
                q["statements"] = it["statements"].clone();
            }
            qs.push(q);
        }
        sections.push(json!({"id": sid, "title": s_of(&sec["title"]), "kind": s_of(&sec["kind"]),
            "instructions": s_of(&sec["instructions"]), "points": round2(slots.iter().filter(|s| s.section == sid).map(|s| s.points).sum()),
            "questions": qs}));
    }
    let total: f64 = slots.iter().map(|s| s.points).sum();
    let title = s_of(&items["title"]);
    json!({
        "v": 1, "examId": exam_id, "folderId": s_of(&k["folderId"]), "structureId": s_of(&st["id"]),
        "title": if title.is_empty() { s_of(&st["title"]) } else { title },
        "subject": s_of(&k["subject"]), "grade": s_of(&k["grade"]),
        "durationMin": st["durationMin"].as_u64().unwrap_or(90), "totalPoints": round2(total),
        "createdAt": now, "note": note, "model": model, "costUsd": (usage.cost_usd * 10000.0).round() / 10000.0,
        "questions": slots.len(), "sections": sections,
    })
}

/// Event `exam` as PROTOCOL §13.
pub fn exam_event(folder_id: &str, exam_id: &str, state: &str, note: &str, title: Option<&str>) -> Value {
    let mut v = json!({"t": "exam", "folderId": folder_id, "examId": exam_id, "state": state, "note": note});
    if let Some(t) = title {
        v["title"] = json!(t);
    }
    v
}

/// Generate one exam. `emit` receives `exam` events (writing → checking → [writing → checking] → done |
/// error is emitted by the caller on `Err`). Returns the exam document (not yet stored).
pub async fn gen_exam(
    llm: Arc<dyn Llm>,
    opts: &ExamOptions,
    k: &Value,
    structure_id: &str,
    note: Option<&str>,
    exam_id: &str,
    emit: &(dyn Fn(Value) + Send + Sync),
    spent: &std::sync::Mutex<Usage>,
) -> Result<(Value, Usage), String> {
    let folder = s_of(&k["folderId"]);
    let (st, slots) = blueprint(k, structure_id)?;
    let banned: HashSet<String> = slots.iter().map(|s| norm(&s.example)).filter(|s| !s.is_empty()).collect();
    emit(exam_event(&folder, exam_id, "writing", &format!("Đang viết {} câu theo cấu trúc “{}”", slots.len(), s_of(&st["title"])), None));
    let prompt = exam_prompt(k, &st, &slots, note);
    let mut usage = Usage::default();
    let mut req = LlmRequest {
        tag: "exam",
        model: opts.model.clone(),
        effort: opts.effort.clone(),
        system: EXAM_SYSTEM.to_string(),
        parts: vec![Part::Text(prompt.clone())],
        schema: Some(("exam".into(), exam_schema())),
        max_tokens: opts.max_tokens,
    };
    let call = |req: LlmRequest| {
        let llm = llm.clone();
        async move {
            let mut last = String::new();
            for _ in 0..=opts.retries {
                match llm.complete(req.clone()).await {
                    Ok(r) => {
                        tracing::info!(tag = req.tag, ms = r.elapsed_ms, cost = r.usage.cost_usd, out = r.usage.output_tokens, "study llm call");
                        return Ok((r.json(), r.usage));
                    }
                    Err(e) => {
                        last = e.to_string();
                        if !e.retryable {
                            break;
                        }
                    }
                }
            }
            Err(last)
        }
    };
    let (first, u) = call(req.clone()).await?;
    usage.add(&u);
    spent.lock().unwrap().add(&u);
    emit(exam_event(&folder, exam_id, "checking", "Đang kiểm tra số câu, loại câu, điểm và đáp án", None));
    let mut items = first.unwrap_or(Value::Null);
    let mut errs = validate_items(&items, &slots, &banned);
    let repairs = errs.clone();
    if !errs.is_empty() {
        tracing::warn!("exam needs repair: {}", errs.join("; "));
        emit(exam_event(&folder, exam_id, "writing", &format!("Đang sửa {} lỗi trong đề", errs.len()), None));
        req.tag = "repair";
        req.parts = vec![
            Part::Text(prompt),
            Part::Text(format!(
                "YOUR PREVIOUS OUTPUT:\n{}\nPROBLEMS:\n- {}\nReturn the complete corrected JSON (all slots).",
                serde_json::to_string(&items).unwrap_or_default(),
                errs.join("\n- ")
            )),
        ];
        let (second, u) = call(req).await?;
        usage.add(&u);
        spent.lock().unwrap().add(&u);
        emit(exam_event(&folder, exam_id, "checking", "Đang kiểm tra lại đề đã sửa", None));
        items = second.unwrap_or(Value::Null);
        errs = validate_items(&items, &slots, &banned);
        if !errs.is_empty() {
            return Err(format!("đề sinh ra chưa đúng cấu trúc: {}", errs.into_iter().take(4).collect::<Vec<_>>().join("; ")));
        }
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut exam = assemble(k, &st, &slots, &items, exam_id, note, &opts.model, &usage, &now);
    if !repairs.is_empty() {
        // What the first draft got wrong (diagnostics for prompt tuning).
        exam["repairs"] = json!(repairs);
    }
    Ok((exam, usage))
}

/// Check an assembled exam against its structure: counts per section and kind, total points.
pub fn check_exam(exam: &Value, st: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    for sec in st["sections"].as_array().into_iter().flatten() {
        let got = exam["sections"].as_array().and_then(|a| a.iter().find(|s| s["id"] == sec["id"]));
        let n = got.and_then(|s| s["questions"].as_array()).map(|a| a.len()).unwrap_or(0) as u64;
        if Some(n) != sec["count"].as_u64() {
            errs.push(format!("section {}: {} questions, structure says {}", sec["id"], n, sec["count"]));
        }
        for q in got.and_then(|s| s["questions"].as_array()).into_iter().flatten() {
            if q["kind"] != sec["kind"] {
                errs.push(format!("{}: kind {} != {}", q["label"], q["kind"], sec["kind"]));
            }
        }
    }
    let sum: f64 = exam["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["questions"].as_array().into_iter().flatten())
        .map(|q| q["points"].as_f64().unwrap_or(0.0))
        .sum();
    let want = st["totalPoints"].as_f64().unwrap_or(sum);
    if (sum - want).abs() > 0.02 {
        errs.push(format!("points {sum} != {want}"));
    }
    errs
}

// ---------------------------------------------------------------- HTML

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// Markdown-ish text with LaTeX → HTML. Math becomes `<span class="m|mb" data-tex="…">` that the page
/// renders with KaTeX; the raw TeX stays visible if KaTeX is missing.
pub fn rich(s: &str) -> String {
    let mut out = String::new();
    let mut text = String::new();
    let flush_text = |text: &mut String, out: &mut String| {
        if text.is_empty() {
            return;
        }
        let mut e = esc(text);
        // **bold**
        while let Some(a) = e.find("**") {
            match e[a + 2..].find("**") {
                Some(b) => {
                    let inner = e[a + 2..a + 2 + b].to_string();
                    e = format!("{}<b>{}</b>{}", &e[..a], inner, &e[a + 4 + b..]);
                }
                None => break,
            }
        }
        out.push_str(&e.replace('\n', "<br>"));
        text.clear();
    };
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let find = |from: usize, pat: &[char]| -> Option<usize> {
        let mut j = from;
        while j + pat.len() <= chars.len() {
            if chars[j..j + pat.len()] == *pat && (j == 0 || chars[j - 1] != '\\' || pat[0] == '\\') {
                return Some(j);
            }
            j += 1;
        }
        None
    };
    while i < chars.len() {
        let c = chars[i];
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let (open, close, display): (usize, &[char], bool) = match two.as_str() {
            "$$" => (2, &['$', '$'], true),
            "\\[" => (2, &['\\', ']'], true),
            "\\(" => (2, &['\\', ')'], false),
            "\\$" => {
                text.push('$');
                i += 2;
                continue;
            }
            _ if c == '$' => (1, &['$'], false),
            _ => {
                text.push(c);
                i += 1;
                continue;
            }
        };
        match find(i + open, close) {
            Some(j) => {
                let tex: String = chars[i + open..j].iter().collect();
                flush_text(&mut text, &mut out);
                out.push_str(&format!(
                    "<span class=\"{}\" data-tex=\"{}\">{}</span>",
                    if display { "mb" } else { "m" },
                    esc(tex.trim()),
                    esc(tex.trim())
                ));
                i = j + close.len();
            }
            None => {
                text.push(c);
                i += 1;
            }
        }
    }
    flush_text(&mut text, &mut out);
    out
}

/// Approximate printed width of a choice (TeX commands count less than letters).
fn visual_len(s: &str) -> usize {
    let mut n = 0;
    let mut in_cmd = false;
    for c in s.chars() {
        if c == '\\' {
            in_cmd = true;
            continue;
        }
        if in_cmd && c.is_ascii_alphabetic() {
            continue;
        }
        in_cmd = false;
        if !"${}^_".contains(c) {
            n += 1;
        }
    }
    n
}

pub fn subject_name(id: &str) -> &str {
    match id {
        "toan" => "Toán",
        "ly" => "Vật lí",
        "hoa" => "Hóa học",
        "van" => "Ngữ văn",
        "anh-ssat" => "Tiếng Anh",
        "duc" => "Tiếng Đức",
        "tin-cpp" => "Tin học",
        "" => "",
        other => other,
    }
}

fn pts(p: f64) -> String {
    let s = format!("{p:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

const CSS: &str = r#"
@page { size: A4; margin: 14mm 16mm 16mm 16mm; }
* { box-sizing: border-box; }
html, body { background: #fff; color: #000; }
body { font-family: "Times New Roman", "Noto Serif", "DejaVu Serif", serif; font-size: 12.5pt; line-height: 1.4; margin: 0; }
.page { max-width: 178mm; margin: 0 auto; padding: 6mm 0; }
@media print { .page { padding: 0; max-width: none; } }
table.hd { width: 100%; border-collapse: collapse; margin-bottom: 3mm; }
table.hd td { vertical-align: top; text-align: center; padding: 0 2mm; }
.hd .l { width: 42%; } .hd .r { width: 58%; }
.hd b { display: block; } .hd .rule { width: 35%; margin: 1mm auto; border-top: 1px solid #000; }
.title { font-size: 14pt; font-weight: bold; text-transform: uppercase; }
.student { margin: 2mm 0 4mm; white-space: nowrap; overflow: hidden; }
.sec { margin-top: 4mm; }
.sec h2 { font-size: 12.5pt; margin: 0 0 1mm; }
.ins { font-style: italic; margin-bottom: 2mm; }
.q { margin: 0 0 2.5mm; break-inside: avoid; page-break-inside: avoid; }
.q .lb { font-weight: bold; }
.choices { display: grid; column-gap: 4mm; row-gap: 0.5mm; margin: 1mm 0 0 6mm; }
.ch4 { grid-template-columns: repeat(4, 1fr); } .ch2 { grid-template-columns: repeat(2, 1fr); } .ch1 { grid-template-columns: 1fr; }
.choices b, .tf b { margin-right: 1.5mm; }
.tf { margin: 1mm 0 0 6mm; }
.end { text-align: center; margin-top: 6mm; font-weight: bold; }
.key { page-break-before: always; break-before: page; }
.key h1 { font-size: 14pt; text-align: center; margin: 0 0 4mm; text-transform: uppercase; }
table.grid { border-collapse: collapse; margin: 1mm 0 3mm; }
table.grid td, table.grid th { border: 1px solid #000; padding: 0.8mm 2.2mm; text-align: center; min-width: 9mm; }
.sol { margin: 0 0 3mm; break-inside: avoid; }
.sol .lb { font-weight: bold; }
.mb { display: block; text-align: center; margin: 1mm 0; }
.katex { font-size: 1.05em; }
"#;

const JS: &str = r#"
(function () {
  function ready() {
    document.documentElement.dataset.ready = "1";
    try { window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.board &&
          window.webkit.messageHandlers.board.postMessage({ type: "exam_ready" }); } catch (e) {}
  }
  function run() {
    if (window.katex) {
      var els = document.querySelectorAll("[data-tex]");
      for (var i = 0; i < els.length; i++) {
        var el = els[i];
        try { katex.render(el.getAttribute("data-tex"), el, { displayMode: el.classList.contains("mb"), throwOnError: false }); }
        catch (e) { el.textContent = el.getAttribute("data-tex"); }
      }
    }
    var f = (document.fonts && document.fonts.ready) ? document.fonts.ready : Promise.resolve();
    f.then(function () { requestAnimationFrame(function () { requestAnimationFrame(ready); }); });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run); else run();
})();
"#;

/// Printable page with KaTeX from `/board/vendor/katex/`.
pub fn render_html(exam: &Value) -> String {
    render_html_with_base(exam, "/board/vendor/katex/")
}

/// Same, with the KaTeX folder at `katex_base` (must end with `/`).
pub fn render_html_with_base(exam: &Value, katex_base: &str) -> String {
    let subject = subject_name(exam["subject"].as_str().unwrap_or(""));
    let grade = s_of(&exam["grade"]);
    let title = s_of(&exam["title"]);
    let mut h = String::new();
    h.push_str("<!doctype html><html lang=\"vi\"><head><meta charset=\"utf-8\">");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    h.push_str(&format!("<title>{}</title>", esc(&title)));
    h.push_str(&format!("<link rel=\"stylesheet\" href=\"{}katex.min.css\">", esc(katex_base)));
    h.push_str(&format!("<style>{CSS}</style></head><body><main class=\"page\">"));
    // Header
    let subj_line = if grade.is_empty() { format!("Môn: {subject}") } else { format!("Môn: {subject} – Lớp {grade}") };
    h.push_str(&format!(
        "<table class=\"hd\"><tr><td class=\"l\"><b>TRƯỜNG ………………………</b><span>Năm học ……… – ………</span><div class=\"rule\"></div></td>\
         <td class=\"r\"><b class=\"title\">{}</b><b>{}</b><span>Thời gian làm bài: {} phút (không kể thời gian giao đề)</span><div class=\"rule\"></div></td></tr></table>",
        esc(&title),
        esc(&subj_line),
        exam["durationMin"].as_u64().unwrap_or(90)
    ));
    h.push_str("<div class=\"student\">Họ và tên: ……………………………………………… Lớp: ………… Số báo danh: …………</div>");
    let roman = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII"];
    let secs: Vec<&Value> = exam["sections"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    for (si, sec) in secs.iter().enumerate() {
        h.push_str("<section class=\"sec\">");
        h.push_str(&format!(
            "<h2>PHẦN {}. {} ({} điểm)</h2>",
            roman.get(si).copied().unwrap_or("·"),
            esc(&s_of(&sec["title"]).to_uppercase()),
            pts(sec["points"].as_f64().unwrap_or(0.0))
        ));
        let ins = s_of(&sec["instructions"]);
        if !ins.is_empty() {
            h.push_str(&format!("<div class=\"ins\">{}</div>", rich(&ins)));
        }
        for q in sec["questions"].as_array().into_iter().flatten() {
            let kind = q["kind"].as_str().unwrap_or("essay");
            h.push_str(&format!("<div class=\"q\" data-no=\"{}\">", q["no"]));
            let p = if kind == "essay" { format!(" ({} điểm)", pts(q["points"].as_f64().unwrap_or(0.0))) } else { String::new() };
            h.push_str(&format!("<span class=\"lb\">{}{}.</span> {}", esc(&s_of(&q["label"])), p, rich(&s_of(&q["text"]))));
            if kind == "mcq" {
                let ch: Vec<String> = q["choices"].as_array().map(|a| a.iter().map(s_of).collect()).unwrap_or_default();
                let w = ch.iter().map(|c| visual_len(c)).max().unwrap_or(0);
                let cls = if w <= 12 { "ch4" } else if w <= 34 { "ch2" } else { "ch1" };
                h.push_str(&format!("<div class=\"choices {cls}\">"));
                for (i, c) in ch.iter().enumerate() {
                    h.push_str(&format!("<div><b>{}.</b>{}</div>", (b'A' + i as u8) as char, rich(c)));
                }
                h.push_str("</div>");
            } else if kind == "truefalse" {
                h.push_str("<div class=\"tf\">");
                for (i, st) in q["statements"].as_array().into_iter().flatten().enumerate() {
                    h.push_str(&format!("<div><b>{})</b>{}</div>", (b'a' + i as u8) as char, rich(&s_of(st))));
                }
                h.push_str("</div>");
            }
            h.push_str("</div>");
        }
        h.push_str("</section>");
    }
    h.push_str("<div class=\"end\">––––––– HẾT –––––––</div>");

    // Answer key
    h.push_str("<section class=\"key\"><h1>Đáp án và hướng dẫn chấm</h1>");
    h.push_str(&format!("<p style=\"text-align:center;margin-top:-2mm\">{}</p>", esc(&title)));
    for (si, sec) in secs.iter().enumerate() {
        let kind = sec["kind"].as_str().unwrap_or("essay");
        let qs: Vec<&Value> = sec["questions"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        h.push_str(&format!("<h2 style=\"font-size:12.5pt\">PHẦN {}. {}</h2>", roman.get(si).copied().unwrap_or("·"), esc(&s_of(&sec["title"]))));
        match kind {
            "mcq" | "short" => {
                for chunk in qs.chunks(10) {
                    h.push_str("<table class=\"grid\"><tr><th>Câu</th>");
                    for q in chunk {
                        h.push_str(&format!("<td>{}</td>", esc(s_of(&q["label"]).trim_start_matches("Câu ").trim_start_matches("Bài "))));
                    }
                    h.push_str("</tr><tr><th>Đáp án</th>");
                    for q in chunk {
                        h.push_str(&format!("<td>{}</td>", rich(&s_of(&q["answer"]))));
                    }
                    h.push_str("</tr></table>");
                }
                for q in &qs {
                    let sol = s_of(&q["solution"]);
                    if !sol.is_empty() {
                        h.push_str(&format!("<div class=\"sol\"><span class=\"lb\">{}:</span> {}</div>", esc(&s_of(&q["label"])), rich(&sol)));
                    }
                }
            }
            "truefalse" => {
                for q in &qs {
                    let marks = parse_tf(&s_of(&q["answer"]))
                        .map(|v| v.iter().enumerate().map(|(i, b)| format!("{}) {}", (b'a' + i as u8) as char, if *b { "Đ" } else { "S" })).collect::<Vec<_>>().join("&nbsp;&nbsp; "))
                        .unwrap_or_else(|| esc(&s_of(&q["answer"])));
                    h.push_str(&format!("<div class=\"sol\"><span class=\"lb\">{}:</span> {}", esc(&s_of(&q["label"])), marks));
                    let sol = s_of(&q["solution"]);
                    if !sol.is_empty() {
                        h.push_str(&format!("<br>{}", rich(&sol)));
                    }
                    h.push_str("</div>");
                }
            }
            _ => {
                for q in &qs {
                    h.push_str(&format!(
                        "<div class=\"sol\"><span class=\"lb\">{} ({} điểm).</span> ",
                        esc(&s_of(&q["label"])),
                        pts(q["points"].as_f64().unwrap_or(0.0))
                    ));
                    let ans = s_of(&q["answer"]);
                    if !ans.is_empty() {
                        h.push_str(&format!("<i>Đáp số:</i> {}<br>", rich(&ans)));
                    }
                    h.push_str(&format!("{}</div>", rich(&s_of(&q["solution"]))));
                }
            }
        }
    }
    h.push_str("</section></main>");
    h.push_str(&format!(
        "<script src=\"{0}katex.min.js\"></script><script src=\"{0}mhchem.min.js\"></script>",
        esc(katex_base)
    ));
    h.push_str(&format!("<script>{JS}</script></body></html>"));
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knowledge() -> Value {
        json!({"folderId": "f", "subject": "toan", "grade": "9",
            "curriculum": [{"id": "c1", "title": "Căn", "topics": [{"id": "c1.t1", "title": "Rút gọn", "count": 3,
                "forms": [{"id": "c1.t1.f1", "title": "Rút gọn biểu thức", "method": "1. Đặt ĐK", "count": 3,
                           "example": {"md": "Rút gọn $A=\\sqrt{8}$"}}]}]}],
            "structures": [{"id": "s1", "title": "Đề", "durationMin": 45, "totalPoints": 2.0,
                "sections": [{"id": "A", "kind": "mcq", "count": 3, "pointsEach": 0.25},
                             {"id": "B", "kind": "essay", "count": 1, "pointsEach": 1.25}],
                "matrix": [{"section": "A", "topicId": "c1.t1", "formId": "c1.t1.f1", "level": "nb", "count": 2, "points": 0.5},
                           {"section": "B", "topicId": "c1.t1", "formId": "c1.t1.f1", "level": "vd", "count": 1, "points": 1.25}]}]})
    }

    #[test]
    fn blueprint_follows_matrix_and_fills_gaps() {
        let (st, slots) = blueprint(&knowledge(), "s1").unwrap();
        assert_eq!(st["id"], "s1");
        assert_eq!(slots.len(), 4);
        assert_eq!(slots.iter().map(|s| s.slot).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(slots[0].level, "nb");
        assert_eq!(slots[2].level, "th", "filler slot for the missing matrix row");
        assert_eq!(slots[2].points, 0.25);
        assert_eq!(slots[3].kind, "essay");
        assert_eq!(slots[0].form, "Rút gọn biểu thức");
        assert_eq!(slots[0].example, "Rút gọn $A=\\sqrt{8}$");
        let total: f64 = slots.iter().map(|s| s.points).sum();
        assert!((total - 2.0).abs() < 1e-9);
        assert!(blueprint(&knowledge(), "nope").is_err());
    }

    #[test]
    fn points_remainder_goes_to_last_item() {
        let mut k = knowledge();
        k["structures"][0]["matrix"][0]["count"] = json!(3);
        k["structures"][0]["matrix"][0]["points"] = json!(1.0);
        let (_, slots) = blueprint(&k, "s1").unwrap();
        assert_eq!(slots[..3].iter().map(|s| s.points).collect::<Vec<_>>(), [0.33, 0.33, 0.34]);
    }

    #[test]
    fn item_validation() {
        let (_, slots) = blueprint(&knowledge(), "s1").unwrap();
        let banned: HashSet<String> = [norm("Rút gọn $A=\\sqrt{8}$")].into_iter().collect();
        let v = json!({"questions": [
            {"slot": 1, "text": "Rút gọn $A=\\sqrt{8}$", "choices": ["1", "2", "3", "4"], "answer": "E"},
            {"slot": 2, "text": "Tính $\\sqrt{9}$ nhé", "choices": ["1", "1", "3", "4"], "answer": "C"},
            {"slot": 2, "text": "dup dup dup", "choices": [], "answer": "A"},
            {"slot": 4, "text": "Bài tự luận $x", "solution": "", "answer": ""},
            {"slot": 9, "text": "extra"}]});
        let e = validate_items(&v, &slots, &banned).join("\n");
        for needle in ["slot 1: copied", "slot 1: mcq answer", "slot 2: mcq choices must be distinct", "slot 2 written twice",
                       "slot 4: unbalanced $", "slot 4: essay needs", "slot 9 is not in the blueprint", "missing slots: 3"] {
            assert!(e.contains(needle), "{needle} in {e}");
        }
    }

    #[test]
    fn true_false_answers_parse() {
        assert_eq!(parse_tf("Đ,S,S,Đ"), Some(vec![true, false, false, true]));
        assert_eq!(parse_tf("a)Đ; b)S; c)Đ; d)S"), Some(vec![true, false, true, false]));
        assert_eq!(parse_tf("D S D S"), Some(vec![true, false, true, false]));
        assert_eq!(parse_tf("Đ,S"), None);
    }

    #[test]
    fn rich_text_marks_math_and_escapes() {
        assert_eq!(rich("a<b $x^2$"), "a&lt;b <span class=\"m\" data-tex=\"x^2\">x^2</span>");
        assert!(rich("$$\\frac{1}{2}$$").starts_with("<span class=\"mb\""));
        assert!(rich("\\(a\\) và").contains("class=\"m\" data-tex=\"a\""));
        assert!(rich("\\[b\\]").contains("class=\"mb\""));
        assert_eq!(rich("giá 5\\$ và **đậm**\ndòng"), "giá 5$ và <b>đậm</b><br>dòng");
        assert_eq!(rich("lẻ $ không đóng"), "lẻ $ không đóng");
        assert_eq!(pts(0.5), "0,5");
        assert_eq!(pts(1.0), "1");
    }
}
