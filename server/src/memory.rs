//! The student's memory: `tutor/memory/profile.md` plus one markdown file per subject.
//!
//! The files are meant to be read and edited by a human. Every entry is one list line in a fixed
//! shape (the server parses it back); any other line (headings, comments, free text, lines whose
//! shape is broken by a hand edit) is kept untouched when the file is written again.
//!
//! ```text
//! - [yếu] Xét dấu tam thức khi a âm | sai 3 | đúng 1 | bậc 1 | lần cuối 2026-11-14 | ôn 2026-11-17 | hay quên đổi chiều
//! - [vững] Giải phương trình bậc hai | ngày 2026-11-20 | tự làm đúng hai lần ôn
//! - [học] Bất phương trình bậc hai | ngày 2026-11-14 | xong phần xét dấu
//! - [cách] Hiểu nhanh hơn khi có ví dụ số | môn toan | ngày 2026-11-14
//! ```
//!
//! Review schedule (pure functions below, they take "today" so tests can pin the date):
//! a mistake always puts the item back to "review tomorrow"; a correct review moves it one rung up
//! the ladder 1, 3, 7, 14 days (the rung is kept across mistakes); two correct reviews in a row make
//! the item "đã vững".

use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Days until the next review, by rung.
pub const REVIEW_INTERVALS: [i64; 4] = [1, 3, 7, 14];
/// Consecutive correct reviews that make an item mastered.
pub const MASTERED_STREAK: u32 = 2;

/// Subject ids (= skill names under `tutor/.claude/skills/`) and their display names.
pub const SUBJECTS: &[(&str, &str)] = &[
    ("toan", "Toán"),
    ("ly", "Lý"),
    ("hoa", "Hóa"),
    ("van", "Văn"),
    ("anh-ssat", "Anh · SSAT"),
    ("duc", "Đức"),
    ("tin-cpp", "Tin · C++"),
    ("ngoai-ngu", "Ngoại ngữ"),
    ("chung", "Khác"),
];

const MAX_TOPIC: usize = 120;
const MAX_NOTE: usize = 240;
const MAX_SUMMARY_WEAK: usize = 6;

static LOCK: Mutex<()> = Mutex::new(());

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

pub fn subject_name(id: &str) -> String {
    SUBJECTS.iter().find(|(i, _)| *i == id).map(|(_, n)| n.to_string()).unwrap_or_else(|| id.to_string())
}

// ------------------------------------------------------------------ text helpers

/// Lowercase and strip Vietnamese diacritics.
pub fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'ả' | 'ã' | 'ạ' | 'ă' | 'ằ' | 'ắ' | 'ẳ' | 'ẵ' | 'ặ' | 'â' | 'ầ' | 'ấ' | 'ẩ' | 'ẫ' | 'ậ' => 'a',
            'è' | 'é' | 'ẻ' | 'ẽ' | 'ẹ' | 'ê' | 'ề' | 'ế' | 'ể' | 'ễ' | 'ệ' => 'e',
            'ì' | 'í' | 'ỉ' | 'ĩ' | 'ị' => 'i',
            'ò' | 'ó' | 'ỏ' | 'õ' | 'ọ' | 'ô' | 'ồ' | 'ố' | 'ổ' | 'ỗ' | 'ộ' | 'ơ' | 'ờ' | 'ớ' | 'ở' | 'ỡ' | 'ợ' => 'o',
            'ù' | 'ú' | 'ủ' | 'ũ' | 'ụ' | 'ư' | 'ừ' | 'ứ' | 'ử' | 'ữ' | 'ự' => 'u',
            'ỳ' | 'ý' | 'ỷ' | 'ỹ' | 'ỵ' => 'y',
            'đ' => 'd',
            c => c,
        })
        .collect()
}

/// Map whatever Claude wrote as `subject` ("Toán", "vật lý", "math", "Sinh"...) to a subject id.
/// Anything unknown (Sinh, Sử, Địa...) goes to `chung`.
pub fn normalize_subject(s: &str) -> String {
    let f = fold(s);
    let f = f.trim();
    if let Some((id, _)) = SUBJECTS.iter().find(|(id, _)| *id == f) {
        return id.to_string();
    }
    let has = |ks: &[&str]| ks.iter().any(|k| f.contains(k));
    let is = |ks: &[&str]| ks.iter().any(|k| f == *k);
    let id = if has(&["toan", "math"]) {
        "toan"
    } else if has(&["vat ly", "vat li", "physic"]) || is(&["ly", "li"]) {
        "ly"
    } else if has(&["tin hoc", "lap trinh", "c++", "cpp", "coding", "code"]) || is(&["tin"]) {
        "tin-cpp"
    } else if has(&["hoa", "chem"]) {
        "hoa"
    } else if has(&["ngu van", "literat"]) || is(&["van"]) {
        "van"
    } else if has(&["ssat", "english", "tieng anh"]) || is(&["anh"]) {
        "anh-ssat"
    } else if has(&["tieng duc", "german", "deutsch"]) || is(&["duc"]) {
        "duc"
    } else if has(&["ngoai ngu", "tieng phap", "tieng nhat", "tieng han", "tieng trung", "french", "japanese", "korean", "chinese"]) {
        "ngoai-ngu"
    } else {
        "chung"
    };
    id.to_string()
}

/// Key for matching topics: case-insensitive, whitespace collapsed.
pub fn topic_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// One line of plain text that cannot break the `|`-separated entry shape.
fn clean_field(s: &str, max: usize) -> String {
    let one: String = s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' || c == '|' { ' ' } else { c }).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(max).collect::<String>().trim().to_string()
}

fn fmt_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// `14/11` for spoken or summary text.
fn short_date(d: NaiveDate) -> String {
    d.format("%d/%m").to_string()
}

// ------------------------------------------------------------------ schedule (pure)

/// Date of the next review when the item sits on `stage` (rung, clamped to the ladder).
pub fn next_review(stage: u32, from: NaiveDate) -> NaiveDate {
    let i = (stage as usize).min(REVIEW_INTERVALS.len() - 1);
    from + Duration::days(REVIEW_INTERVALS[i])
}

/// The scheduling state of a weak item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sched {
    pub count: u32,
    pub streak: u32,
    pub stage: u32,
    pub due: NaiveDate,
}

#[derive(Debug, PartialEq)]
pub enum Reviewed {
    Weak(Sched),
    Mastered,
}

/// A new or repeated mistake. `prev` is the existing state if the topic was already weak.
pub fn after_mistake(prev: Option<Sched>, today: NaiveDate) -> Sched {
    let (count, stage) = prev.map(|p| (p.count + 1, p.stage)).unwrap_or((1, 0));
    Sched { count, streak: 0, stage, due: next_review(0, today) }
}

/// A correct answer to a review of a weak item.
pub fn after_correct(prev: Sched, today: NaiveDate) -> Reviewed {
    let streak = prev.streak + 1;
    if streak >= MASTERED_STREAK {
        return Reviewed::Mastered;
    }
    let stage = (prev.stage + 1).min(REVIEW_INTERVALS.len() as u32 - 1);
    Reviewed::Weak(Sched { count: prev.count, streak, stage, due: next_review(stage, today) })
}

// ------------------------------------------------------------------ entries

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tag {
    Weak,
    Mastered,
    Learned,
    Style,
    /// Understood after a re-explanation; the note is the way of explaining that worked.
    Understood,
}

impl Tag {
    fn word(self) -> &'static str {
        match self {
            Tag::Weak => "yếu",
            Tag::Mastered => "vững",
            Tag::Learned => "học",
            Tag::Style => "cách",
            Tag::Understood => "hiểu",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Tag::Weak => "Đang yếu",
            Tag::Mastered => "Đã vững",
            Tag::Learned => "Đã học",
            Tag::Style => "Cách giảng hợp",
            Tag::Understood => "Đã hiểu sau khi giảng lại",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub tag: Tag,
    pub topic: String,
    pub count: u32,
    pub streak: u32,
    pub stage: u32,
    /// "lần cuối" (weak) or "ngày" (the other kinds).
    pub last: NaiveDate,
    pub due: Option<NaiveDate>,
    /// Subject id, used by style entries in `profile.md`.
    pub subject: Option<String>,
    /// Topic id in the folder's curriculum (`c1.t2`), when known.
    pub topic_id: Option<String>,
    pub note: String,
}

impl Entry {
    fn sched(&self) -> Sched {
        Sched { count: self.count, streak: self.streak, stage: self.stage, due: self.due.unwrap_or(self.last + Duration::days(1)) }
    }

    fn with_sched(mut self, s: Sched) -> Entry {
        self.count = s.count;
        self.streak = s.streak;
        self.stage = s.stage;
        self.due = Some(s.due);
        self
    }

    fn plain(tag: Tag, topic: &str, note: &str, today: NaiveDate) -> Entry {
        Entry {
            tag,
            topic: clean_field(topic, MAX_TOPIC),
            count: 0,
            streak: 0,
            stage: 0,
            last: today,
            due: None,
            subject: None,
            topic_id: None,
            note: clean_field(note, MAX_NOTE),
        }
    }

    pub fn render(&self) -> String {
        let mut parts = vec![format!("- [{}] {}", self.tag.word(), self.topic)];
        match self.tag {
            Tag::Weak => {
                parts.push(format!("sai {}", self.count));
                parts.push(format!("đúng {}", self.streak));
                parts.push(format!("bậc {}", self.stage));
                parts.push(format!("lần cuối {}", fmt_date(self.last)));
                parts.push(format!("ôn {}", fmt_date(self.due.unwrap_or(self.last + Duration::days(1)))));
            }
            Tag::Style => {
                if let Some(s) = &self.subject {
                    parts.push(format!("môn {s}"));
                }
                parts.push(format!("ngày {}", fmt_date(self.last)));
            }
            Tag::Mastered | Tag::Learned | Tag::Understood => parts.push(format!("ngày {}", fmt_date(self.last))),
        }
        if let Some(id) = &self.topic_id {
            parts.push(format!("id {id}"));
        }
        if !self.note.is_empty() {
            parts.push(self.note.clone());
        }
        parts.join(" | ")
    }
}

/// Parse one line; `None` means "not an entry" (the line is then kept as it is).
pub fn parse_entry(line: &str) -> Option<Entry> {
    let t = line.trim_start();
    let t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
    let t = t.trim_start().strip_prefix('[')?;
    let (tag, rest) = t.split_once(']')?;
    let tag = match fold(tag).trim() {
        "yeu" => Tag::Weak,
        "vung" => Tag::Mastered,
        "hoc" => Tag::Learned,
        "cach" => Tag::Style,
        "hieu" => Tag::Understood,
        _ => return None,
    };
    let mut parts = rest.split(" | ");
    let topic = parts.next()?.trim().to_string();
    if topic.is_empty() {
        return None;
    }
    let (mut count, mut streak, mut stage) = (None, None, None);
    let (mut last, mut due, mut subject, mut topic_id) = (None, None, None, None);
    let mut note_parts: Vec<&str> = Vec::new();
    for p in parts {
        if !note_parts.is_empty() {
            note_parts.push(p);
            continue;
        }
        let f = fold(p.trim());
        let num = |k: &str| f.strip_prefix(k).and_then(|v| v.trim().parse::<u32>().ok());
        let date = |k: &str| f.strip_prefix(k).and_then(parse_date);
        if let Some(n) = num("sai ") {
            count = Some(n);
        } else if let Some(n) = num("dung ") {
            streak = Some(n);
        } else if let Some(n) = num("bac ") {
            stage = Some(n);
        } else if let Some(d) = date("lan cuoi ").or_else(|| date("ngay ")) {
            last = Some(d);
        } else if let Some(d) = date("on ") {
            due = Some(d);
        } else if let Some(s) = f.strip_prefix("mon ").filter(|s| !s.trim().is_empty() && !s.contains(' ')) {
            subject = Some(s.trim().to_string());
        } else if let Some(s) = p.trim().strip_prefix("id ").filter(|s| !s.trim().is_empty() && !s.trim().contains(' ')) {
            topic_id = Some(s.trim().to_string());
        } else {
            note_parts.push(p);
        }
    }
    let last = last?;
    let weak = tag == Tag::Weak;
    Some(Entry {
        tag,
        topic,
        count: count.unwrap_or(if weak { 1 } else { 0 }),
        streak: streak.unwrap_or(0),
        stage: stage.unwrap_or(0),
        last,
        due: if weak { Some(due.unwrap_or(last + Duration::days(1))) } else { due },
        subject,
        topic_id,
        note: note_parts.join(" | ").trim().to_string(),
    })
}

// ------------------------------------------------------------------ one markdown file

/// A memory file as a list of lines. Entry lines are replaced in place; everything else survives.
#[derive(Debug, Clone, Default)]
pub struct MemFile {
    lines: Vec<String>,
}

impl MemFile {
    pub fn parse(text: &str) -> MemFile {
        MemFile { lines: text.lines().map(|l| l.to_string()).collect() }
    }

    pub fn render(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    fn ensure_header(&mut self, title: &str) {
        if self.is_blank() {
            self.lines = vec![
                format!("# {title}"),
                String::new(),
                "> Server ghi file này, cậu sửa tay được. Mỗi mục một dòng, ví dụ:".to_string(),
                "> `- [yếu] Chủ đề | sai 3 | đúng 0 | bậc 0 | lần cuối 2026-11-14 | ôn 2026-11-15 | ghi chú`".to_string(),
                "> Dòng nào không theo khuôn thì server giữ nguyên và bỏ qua.".to_string(),
            ];
        }
    }

    pub fn entries(&self) -> Vec<(usize, Entry)> {
        self.lines.iter().enumerate().filter_map(|(i, l)| parse_entry(l).map(|e| (i, e))).collect()
    }

    pub fn of_tag(&self, tag: Tag) -> Vec<Entry> {
        self.entries().into_iter().map(|(_, e)| e).filter(|e| e.tag == tag).collect()
    }

    fn find(&self, tag: Tag, topic: &str) -> Option<(usize, Entry)> {
        let key = topic_key(topic);
        self.entries().into_iter().find(|(_, e)| e.tag == tag && topic_key(&e.topic) == key)
    }

    fn replace(&mut self, idx: usize, e: &Entry) {
        self.lines[idx] = e.render();
    }

    fn remove(&mut self, idx: usize) {
        self.lines.remove(idx);
    }

    /// Append `e` at the end of its section, creating the heading if the file has none.
    fn insert(&mut self, e: &Entry) {
        let want = fold(e.tag.heading());
        let head = self.lines.iter().position(|l| {
            let t = l.trim_start();
            t.starts_with("##") && !t.starts_with("###") && fold(t.trim_start_matches('#')).trim() == want
        });
        match head {
            Some(h) => {
                let end = self.lines[h + 1..].iter().position(|l| l.trim_start().starts_with('#')).map(|p| h + 1 + p).unwrap_or(self.lines.len());
                let mut pos = end;
                while pos > h + 1 && self.lines[pos - 1].trim().is_empty() {
                    pos -= 1;
                }
                self.lines.insert(pos, e.render());
            }
            None => {
                if self.lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("## {}", e.tag.heading()));
                self.lines.push(e.render());
            }
        }
    }
}

// ------------------------------------------------------------------ notes

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Mistake,
    Mastered,
    Style,
    Progress,
    Understood,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_lowercase().as_str() {
            "mistake" => Some(Kind::Mistake),
            "mastered" => Some(Kind::Mastered),
            "style" => Some(Kind::Style),
            "progress" => Some(Kind::Progress),
            "understood" => Some(Kind::Understood),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteResult {
    pub subject: String,
    pub topic: String,
    /// `created`, `updated`, `reviewed` (correct, comes back later), `mastered`, `reopened`.
    pub action: &'static str,
    pub due: Option<NaiveDate>,
}

impl NoteResult {
    pub fn to_json(&self) -> Value {
        json!({"saved": true, "subject": self.subject, "topic": self.topic, "action": self.action,
               "nextReview": self.due.map(fmt_date)})
    }
}

/// Apply one note to a file (pure apart from the file model). `subject` is a normalized id.
pub fn apply_note(f: &mut MemFile, subject: &str, kind: Kind, topic: &str, note: &str, today: NaiveDate) -> Result<NoteResult, String> {
    apply_note_id(f, subject, kind, topic, note, None, today)
}

/// Same, with the curriculum topic id (folder sessions). The id is kept on the entry.
pub fn apply_note_id(f: &mut MemFile, subject: &str, kind: Kind, topic: &str, note: &str, topic_id: Option<&str>, today: NaiveDate) -> Result<NoteResult, String> {
    let r = apply_note_inner(f, subject, kind, topic, note, today)?;
    if let Some(id) = topic_id.map(|s| clean_field(s, 40).replace(' ', "")).filter(|s| !s.is_empty()) {
        let tag = match kind {
            Kind::Mistake => Tag::Weak,
            Kind::Mastered => if r.action == "reviewed" { Tag::Weak } else { Tag::Mastered },
            Kind::Progress => Tag::Learned,
            Kind::Style => Tag::Style,
            Kind::Understood => Tag::Understood,
        };
        if let Some((i, mut e)) = f.find(tag, &r.topic) {
            e.topic_id = Some(id);
            f.replace(i, &e);
        }
    }
    Ok(r)
}

fn apply_note_inner(f: &mut MemFile, subject: &str, kind: Kind, topic: &str, note: &str, today: NaiveDate) -> Result<NoteResult, String> {
    let topic = clean_field(topic, MAX_TOPIC);
    if topic.is_empty() {
        return Err("Thiếu topic.".into());
    }
    let note = clean_field(note, MAX_NOTE);
    let done = |action: &'static str, due: Option<NaiveDate>| NoteResult { subject: subject.to_string(), topic: topic.clone(), action, due };
    match kind {
        Kind::Mistake => {
            if let Some((i, mut e)) = f.find(Tag::Weak, &topic) {
                let s = after_mistake(Some(e.sched()), today);
                e.last = today;
                if !note.is_empty() {
                    e.note = note;
                }
                let e = e.with_sched(s);
                f.replace(i, &e);
                return Ok(done("updated", Some(s.due)));
            }
            let s = after_mistake(None, today);
            let mut e = Entry::plain(Tag::Weak, &topic, &note, today).with_sched(s);
            // A topic that was mastered and went wrong again comes back as weak.
            let reopened = if let Some((i, old)) = f.find(Tag::Mastered, &topic) {
                f.remove(i);
                if e.note.is_empty() {
                    e.note = old.note;
                }
                true
            } else {
                false
            };
            f.insert(&e);
            Ok(done(if reopened { "reopened" } else { "created" }, Some(s.due)))
        }
        Kind::Mastered => {
            if let Some((i, mut e)) = f.find(Tag::Weak, &topic) {
                match after_correct(e.sched(), today) {
                    Reviewed::Weak(s) => {
                        e.last = today;
                        if !note.is_empty() {
                            e.note = note;
                        }
                        let e = e.with_sched(s);
                        f.replace(i, &e);
                        return Ok(done("reviewed", Some(s.due)));
                    }
                    Reviewed::Mastered => {
                        f.remove(i);
                        let keep = if note.is_empty() { e.note.clone() } else { note };
                        f.insert(&Entry::plain(Tag::Mastered, &topic, &keep, today));
                        return Ok(done("mastered", None));
                    }
                }
            }
            if let Some((i, mut e)) = f.find(Tag::Mastered, &topic) {
                e.last = today;
                if !note.is_empty() {
                    e.note = note;
                }
                f.replace(i, &e);
                return Ok(done("updated", None));
            }
            f.insert(&Entry::plain(Tag::Mastered, &topic, &note, today));
            Ok(done("mastered", None))
        }
        Kind::Progress => upsert_plain(f, Tag::Learned, &topic, &note, None, today).map(|a| done(a, None)),
        Kind::Understood => upsert_plain(f, Tag::Understood, &topic, &note, None, today).map(|a| done(a, None)),
        Kind::Style => {
            let sub = (subject != "chung").then(|| subject.to_string());
            upsert_plain(f, Tag::Style, &topic, &note, sub, today).map(|a| done(a, None))
        }
    }
}

fn upsert_plain(f: &mut MemFile, tag: Tag, topic: &str, note: &str, subject: Option<String>, today: NaiveDate) -> Result<&'static str, String> {
    if let Some((i, mut e)) = f.find(tag, topic) {
        e.last = today;
        if !note.is_empty() {
            e.note = note.to_string();
        }
        f.replace(i, &e);
        return Ok("updated");
    }
    let mut e = Entry::plain(tag, topic, note, today);
    e.subject = subject;
    f.insert(&e);
    Ok("created")
}

// ------------------------------------------------------------------ review picking

#[derive(Debug, Clone, PartialEq)]
pub struct DueItem {
    pub subject: String,
    pub topic: String,
    pub note: String,
    pub last: NaiveDate,
    pub due: NaiveDate,
    pub count: u32,
}

/// The item to offer: the one overdue the longest (ties: more mistakes first, then name).
pub fn pick_review(mut items: Vec<DueItem>, today: NaiveDate) -> Option<DueItem> {
    items.retain(|i| i.due <= today);
    items.sort_by(|a, b| a.due.cmp(&b.due).then(b.count.cmp(&a.count)).then(a.topic.cmp(&b.topic)));
    items.into_iter().next()
}

/// The `[memory]` block put in front of the first turn of a session (at most 12 lines).
/// Pure: the inputs are the style lines (oldest first), weak items and learned items.
pub fn build_summary(style: &[String], weak: &[(String, Entry)], learned: &[(String, Entry)], today: NaiveDate) -> Option<String> {
    if style.is_empty() && weak.is_empty() && learned.is_empty() {
        return None;
    }
    let mut out = vec!["[memory]".to_string()];
    if !style.is_empty() {
        let recent: Vec<&str> = style.iter().rev().take(3).map(|s| s.as_str()).collect();
        out.push(format!("Cách giảng hợp: {}", recent.join("; ")));
    }
    if !weak.is_empty() {
        let mut w: Vec<&(String, Entry)> = weak.iter().collect();
        w.sort_by(|a, b| a.1.due.cmp(&b.1.due).then(b.1.count.cmp(&a.1.count)).then(a.1.topic.cmp(&b.1.topic)));
        out.push("Đang yếu (môn · chủ đề · số lần sai · ghi chú):".to_string());
        for (sub, e) in w.into_iter().take(MAX_SUMMARY_WEAK) {
            let due = if e.due.map(|d| d <= today).unwrap_or(false) { " · tới hạn ôn" } else { "" };
            let note = if e.note.is_empty() { String::new() } else { format!(" · {}", e.note) };
            out.push(format!("- {sub} · {} · sai {}{due}{note}", e.topic, e.count));
        }
    }
    if !learned.is_empty() {
        let mut l: Vec<&(String, Entry)> = learned.iter().collect();
        l.sort_by(|a, b| b.1.last.cmp(&a.1.last));
        let items: Vec<String> = l.into_iter().take(2).map(|(s, e)| format!("{s} · {} ({})", e.topic, short_date(e.last))).collect();
        out.push(format!("Đã học gần đây: {}", items.join("; ")));
    }
    out.push("[/memory]".to_string());
    Some(out.join("\n"))
}

/// Where the class is at school, from `knowledge.json.schoolProgress` (at most 3 lines).
/// Topics are written `title [id]` so the tutor can pass the id to `memory_note`.
pub fn school_lines(k: &Value) -> Vec<String> {
    let topics = crate::folders::curriculum_topics(k);
    if topics.is_empty() {
        return vec![];
    }
    let name = |id: &str| topics.iter().find(|(i, _, _)| i == id).map(|(i, t, _)| format!("{t} [{i}]"));
    let sp = &k["schoolProgress"];
    let covered: Vec<String> = sp["coveredTopicIds"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(String::from).collect();
    let current = sp["currentTopicId"].as_str().map(String::from);
    if covered.is_empty() && current.is_none() {
        let all: Vec<String> = topics.iter().take(10).map(|(i, t, _)| format!("{t} [{i}]")).collect();
        return vec![format!("Giáo trình folder (chưa rõ trên trường học tới đâu): {}", all.join("; "))];
    }
    let mut out = Vec::new();
    if let Some(c) = current.as_deref().and_then(name) {
        let conf = sp["confidence"].as_f64().map(|x| format!(" (độ tin {:.1})", x)).unwrap_or_default();
        out.push(format!("Trên trường đang học: {c}{conf}"));
    }
    let done: Vec<String> = covered.iter().filter_map(|i| name(i)).take(10).collect();
    if !done.is_empty() {
        out.push(format!("Trên trường đã học: {}", done.join("; ")));
    }
    let not_yet: Vec<String> = topics
        .iter()
        .filter(|(i, _, _)| !covered.contains(i) && current.as_deref() != Some(i.as_str()))
        .take(10)
        .map(|(i, t, _)| format!("{t} [{i}]"))
        .collect();
    if !not_yet.is_empty() {
        out.push(format!("Trên trường CHƯA học (muốn dùng thì phải dạy từ đầu): {}", not_yet.join("; ")));
    }
    out
}

// ------------------------------------------------------------------ the store (files)

/// Reads and writes `tutor/memory/`.
pub struct Memory {
    dir: PathBuf,
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("md.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // e.g. the file is open in an editor on Windows: fall back to a plain overwrite.
        let r = std::fs::write(path, text);
        let _ = std::fs::remove_file(&tmp);
        return r.map_err(|_| e);
    }
    Ok(())
}

impl Memory {
    /// `tutor_dir` is the `tutor/` folder; files live in `tutor_dir/memory/`.
    pub fn new(tutor_dir: &Path) -> Memory {
        Memory { dir: tutor_dir.join("memory") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.md"))
    }

    fn read(&self, name: &str) -> MemFile {
        MemFile::parse(&std::fs::read_to_string(self.path(name)).unwrap_or_default())
    }

    fn subject_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = SUBJECTS.iter().map(|(i, _)| i.to_string()).collect();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut extra: Vec<String> = rd
                .filter_map(|e| e.ok())
                .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()).filter(|_| e.path().extension().map(|x| x == "md").unwrap_or(false)))
                .filter(|s| s != "profile" && !ids.contains(s))
                .collect();
            extra.sort();
            ids.extend(extra);
        }
        ids
    }

    /// Record one note. Style goes to `profile.md`, the rest to the subject's file.
    pub fn note(&self, subject: &str, kind: Kind, topic: &str, note: &str, today: NaiveDate) -> Result<NoteResult, String> {
        let subject = normalize_subject(subject);
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("Không tạo được thư mục memory: {e}"))?;
        let name = if kind == Kind::Style { "profile".to_string() } else { subject.clone() };
        let mut f = self.read(&name);
        let title = if name == "profile" { "Hồ sơ học sinh".to_string() } else { format!("Trí nhớ · {}", subject_name(&name)) };
        f.ensure_header(&title);
        let r = apply_note(&mut f, &subject, kind, topic, note, today)?;
        write_atomic(&self.path(&name), &f.render()).map_err(|e| format!("Không ghi được memory: {e}"))?;
        Ok(r)
    }

    fn folder_name(folder: &str) -> Option<String> {
        crate::store::is_valid_session_id(folder).then(|| format!("folders/{folder}"))
    }

    /// Record a note for a session: style always goes to `profile.md`; the rest to the folder's
    /// file (`memory/folders/<id>.md`) when the session has a folder, else to the subject's file.
    #[allow(clippy::too_many_arguments)]
    pub fn note_at(&self, folder: Option<&str>, subject: &str, kind: Kind, topic: &str, note: &str, topic_id: Option<&str>, today: NaiveDate) -> Result<NoteResult, String> {
        let subject = normalize_subject(subject);
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let folder_file = folder.and_then(Memory::folder_name);
        let name = match (&folder_file, kind) {
            (_, Kind::Style) => "profile".to_string(),
            (Some(f), _) => f.clone(),
            (None, _) => subject.clone(),
        };
        std::fs::create_dir_all(self.dir.join("folders")).map_err(|e| format!("Không tạo được thư mục memory: {e}"))?;
        let mut f = self.read(&name);
        let title = if name == "profile" {
            "Hồ sơ học sinh".to_string()
        } else if folder_file.as_deref() == Some(name.as_str()) {
            format!("Trí nhớ · folder {}", folder.unwrap_or(""))
        } else {
            format!("Trí nhớ · {}", subject_name(&name))
        };
        f.ensure_header(&title);
        let r = apply_note_id(&mut f, &subject, kind, topic, note, topic_id, today)?;
        write_atomic(&self.path(&name), &f.render()).map_err(|e| format!("Không ghi được memory: {e}"))?;
        Ok(r)
    }

    /// The folder's memory file (empty when it does not exist).
    pub fn folder_file(&self, folder: &str) -> MemFile {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Memory::folder_name(folder).map(|n| self.read(&n)).unwrap_or_default()
    }

    /// Weak items of a folder, due or not, as review items.
    pub fn folder_weak(&self, folder: &str, subject: &str) -> Vec<DueItem> {
        self.folder_file(folder)
            .of_tag(Tag::Weak)
            .into_iter()
            .map(|e| DueItem { subject: subject.to_string(), due: e.due.unwrap_or(e.last + Duration::days(1)), topic: e.topic, note: e.note, last: e.last, count: e.count })
            .collect()
    }

    pub fn folder_review_candidate(&self, folder: &str, subject: &str, today: NaiveDate) -> Option<DueItem> {
        pick_review(self.folder_weak(folder, subject), today)
    }

    /// The `[memory]` block of a folder session: teaching style, the folder's weak points, what
    /// worked when re-explaining, and where the class is at school (from `knowledge.json`).
    pub fn folder_summary(&self, folder: &str, subject: &str, knowledge: Option<&Value>, today: NaiveDate) -> Option<String> {
        let style = {
            let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            self.style_lines()
        };
        let f = self.folder_file(folder);
        let tagged = |t: Tag| f.of_tag(t).into_iter().map(|e| (subject.to_string(), e)).collect::<Vec<_>>();
        let base = build_summary(&style, &tagged(Tag::Weak), &tagged(Tag::Learned), today);
        let mut extra = Vec::new();
        let mut understood = f.of_tag(Tag::Understood);
        understood.sort_by(|a, b| b.last.cmp(&a.last));
        if !understood.is_empty() {
            let items: Vec<String> = understood.iter().take(3).map(|e| if e.note.is_empty() { e.topic.clone() } else { format!("{} ({})", e.topic, e.note) }).collect();
            extra.push(format!("Đã hiểu sau khi giảng lại: {}", items.join("; ")));
        }
        if let Some(k) = knowledge {
            extra.extend(school_lines(k));
        }
        let mut lines: Vec<String> = match base {
            Some(b) => b.lines().filter(|l| *l != "[/memory]").map(String::from).collect(),
            None if extra.is_empty() => return None,
            None => vec!["[memory]".to_string()],
        };
        lines.extend(extra);
        lines.push("[/memory]".to_string());
        Some(lines.join("\n"))
    }

    /// Weak items that are due, across all subjects.
    pub fn due_items(&self, today: NaiveDate) -> Vec<DueItem> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for id in self.subject_ids() {
            for e in self.read(&id).of_tag(Tag::Weak) {
                let due = e.due.unwrap_or(e.last + Duration::days(1));
                if due <= today {
                    out.push(DueItem { subject: id.clone(), topic: e.topic, note: e.note, last: e.last, due, count: e.count });
                }
            }
        }
        out
    }

    pub fn review_candidate(&self, today: NaiveDate) -> Option<DueItem> {
        pick_review(self.due_items(today), today)
    }

    fn style_lines(&self) -> Vec<String> {
        self.read("profile")
            .of_tag(Tag::Style)
            .into_iter()
            .map(|e| if e.note.is_empty() { e.topic } else { format!("{} ({})", e.topic, e.note) })
            .collect()
    }

    /// The `[memory]` block, or `None` when nothing is known yet.
    pub fn summary(&self, today: NaiveDate) -> Option<String> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let style = self.style_lines();
        let (mut weak, mut learned) = (Vec::new(), Vec::new());
        for id in self.subject_ids() {
            let f = self.read(&id);
            weak.extend(f.of_tag(Tag::Weak).into_iter().map(|e| (id.clone(), e)));
            learned.extend(f.of_tag(Tag::Learned).into_iter().map(|e| (id.clone(), e)));
        }
        build_summary(&style, &weak, &learned, today)
    }

    /// The `progress` frame (PROTOCOL section 6).
    pub fn progress(&self) -> Value {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut subjects = Vec::new();
        for id in self.subject_ids() {
            let f = self.read(&id);
            let mut weak = f.of_tag(Tag::Weak);
            weak.sort_by(|a, b| a.due.cmp(&b.due).then(b.count.cmp(&a.count)).then(a.topic.cmp(&b.topic)));
            let mut mastered = f.of_tag(Tag::Mastered);
            mastered.sort_by(|a, b| b.last.cmp(&a.last));
            let mut recent = f.of_tag(Tag::Learned);
            recent.sort_by(|a, b| b.last.cmp(&a.last));
            if weak.is_empty() && mastered.is_empty() && recent.is_empty() {
                continue;
            }
            subjects.push(json!({
                "id": id,
                "name": subject_name(&id),
                "weak": weak.iter().map(|e| json!({
                    "topic": e.topic, "note": e.note, "count": e.count,
                    "lastSeen": fmt_date(e.last), "due": fmt_date(e.due.unwrap_or(e.last + Duration::days(1)))})).collect::<Vec<_>>(),
                "mastered": mastered.iter().map(|e| json!({"topic": e.topic, "date": fmt_date(e.last)})).collect::<Vec<_>>(),
                "recent": recent.iter().take(10).map(|e| json!({"topic": e.topic, "date": fmt_date(e.last)})).collect::<Vec<_>>(),
            }));
        }
        json!({"t": "progress", "subjects": subjects, "style": self.style_lines(), "updatedAt": crate::store::now_rfc3339()})
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("beluga-mem-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn schedule_ladder_and_mastery() {
        let t0 = d("2026-11-14");
        let s1 = after_mistake(None, t0);
        assert_eq!((s1.count, s1.streak, s1.stage, s1.due), (1, 0, 0, d("2026-11-15")));
        // first correct review: one rung up, three days later
        let Reviewed::Weak(s2) = after_correct(s1, d("2026-11-15")) else { panic!("should stay weak") };
        assert_eq!((s2.streak, s2.stage, s2.due), (1, 1, d("2026-11-18")));
        // a mistake in between: back to tomorrow, streak reset, rung and count kept/raised
        let s3 = after_mistake(Some(s2), d("2026-11-18"));
        assert_eq!((s3.count, s3.streak, s3.stage, s3.due), (2, 0, 1, d("2026-11-19")));
        // two correct reviews in a row make it mastered; the rung keeps climbing meanwhile
        let Reviewed::Weak(s4) = after_correct(s3, d("2026-11-19")) else { panic!() };
        assert_eq!((s4.stage, s4.due), (2, d("2026-11-26")));
        assert_eq!(after_correct(s4, d("2026-11-26")), Reviewed::Mastered);
    }

    #[test]
    fn ladder_is_one_three_seven_fourteen_and_clamped() {
        let t = d("2026-01-01");
        let days: Vec<i64> = (0..6).map(|s| (next_review(s, t) - t).num_days()).collect();
        assert_eq!(days, vec![1, 3, 7, 14, 14, 14]);
    }

    #[test]
    fn entry_roundtrip_all_tags() {
        let lines = [
            "- [yếu] Xét dấu tam thức khi a âm | sai 3 | đúng 1 | bậc 1 | lần cuối 2026-11-14 | ôn 2026-11-17 | hay quên đổi chiều",
            "- [vững] Giải phương trình bậc hai | ngày 2026-11-20 | tự làm đúng",
            "- [học] Bất phương trình bậc hai | ngày 2026-11-14",
            "- [cách] Hiểu nhanh hơn khi có ví dụ số | môn toan | ngày 2026-11-14 | ghi chú",
        ];
        for l in lines {
            let e = parse_entry(l).unwrap_or_else(|| panic!("parse {l}"));
            assert_eq!(e.render(), l);
        }
    }

    #[test]
    fn hand_edited_lines_are_parsed_leniently_and_broken_ones_ignored() {
        // hand-written: ascii tag, missing optional fields, `*` bullet, note with a pipe in it
        let e = parse_entry("* [yeu] Chủ đề | lần cuối 2026-11-14 | ghi | chú").unwrap();
        assert_eq!((e.count, e.streak, e.stage), (1, 0, 0));
        assert_eq!(e.due, Some(d("2026-11-15")));
        assert_eq!(e.note, "ghi | chú");
        assert!(parse_entry("- [yếu] Thiếu ngày | sai 2").is_none());
        assert!(parse_entry("- [yếu] Ngày hỏng | lần cuối 14/11/2026").is_none());
        assert!(parse_entry("- [lạ] Gì đó | ngày 2026-11-14").is_none());
        assert!(parse_entry("Một câu thường").is_none());
        assert!(parse_entry("## Đang yếu").is_none());
    }

    #[test]
    fn unknown_lines_survive_a_rewrite() {
        let text = "# Trí nhớ · Toán\n\nGhi chú tay của chủ nhà.\n\n## Đang yếu\n- [yếu] Cũ | sai 1 | đúng 0 | bậc 0 | lần cuối 2026-11-10 | ôn 2026-11-11\n- dòng tự viết không theo khuôn\n- [yếu] Hỏng | sai 2\n\n## Đã học\n- [học] Bài 1 | ngày 2026-11-01\n\nCuối file.\n";
        let mut f = MemFile::parse(text);
        apply_note(&mut f, "toan", Kind::Mistake, "Mới", "n", d("2026-11-14")).unwrap();
        apply_note(&mut f, "toan", Kind::Progress, "Bài 2", "", d("2026-11-14")).unwrap();
        apply_note(&mut f, "toan", Kind::Mistake, "cũ", "", d("2026-11-14")).unwrap();
        let out = f.render();
        for keep in ["Ghi chú tay của chủ nhà.", "- dòng tự viết không theo khuôn", "- [yếu] Hỏng | sai 2", "Cuối file."] {
            assert!(out.contains(keep), "{keep} lost in:\n{out}");
        }
        // new weak entry is inside the weak section (before "## Đã học"), new learned one after "Bài 1"
        let pos = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing in:\n{out}"));
        assert!(pos("- [yếu] Mới") < pos("## Đã học"));
        assert!(pos("- [học] Bài 2") > pos("- [học] Bài 1"));
        // (free text under a heading belongs to that section, so a new line goes after it)
        // "cũ" matched "Cũ" ignoring case: still one entry, count raised to 2
        assert_eq!(out.matches("] Cũ |").count(), 1);
        assert!(out.contains("- [yếu] Cũ | sai 2 | đúng 0 | bậc 0 | lần cuối 2026-11-14 | ôn 2026-11-15"), "{out}");
    }

    #[test]
    fn topics_match_ignoring_case_and_extra_spaces() {
        let mut f = MemFile::default();
        f.ensure_header("T");
        let t = d("2026-11-14");
        let a = apply_note(&mut f, "toan", Kind::Mistake, "Xét dấu  tam thức", "một", t).unwrap();
        assert_eq!(a.action, "created");
        let b = apply_note(&mut f, "toan", Kind::Mistake, "  xét DẤU tam   thức ", "hai", t).unwrap();
        assert_eq!(b.action, "updated");
        let weak = f.of_tag(Tag::Weak);
        assert_eq!(weak.len(), 1);
        assert_eq!((weak[0].count, weak[0].note.as_str()), (2, "hai"));
    }

    #[test]
    fn mastered_after_two_correct_reviews_moves_the_line() {
        let mut f = MemFile::default();
        f.ensure_header("T");
        apply_note(&mut f, "toan", Kind::Mistake, "Dạng A", "lỗi", d("2026-11-14")).unwrap();
        let r1 = apply_note(&mut f, "toan", Kind::Mastered, "dạng a", "", d("2026-11-15")).unwrap();
        assert_eq!((r1.action, r1.due), ("reviewed", Some(d("2026-11-18"))));
        assert_eq!(f.of_tag(Tag::Weak).len(), 1);
        let r2 = apply_note(&mut f, "toan", Kind::Mastered, "Dạng A", "", d("2026-11-18")).unwrap();
        assert_eq!(r2.action, "mastered");
        assert!(f.of_tag(Tag::Weak).is_empty());
        let m = f.of_tag(Tag::Mastered);
        assert_eq!((m.len(), m[0].note.as_str(), m[0].last), (1, "lỗi", d("2026-11-18")), "note is kept");
        // going wrong again reopens it
        let r3 = apply_note(&mut f, "toan", Kind::Mistake, "Dạng A", "", d("2026-12-01")).unwrap();
        assert_eq!(r3.action, "reopened");
        assert!(f.of_tag(Tag::Mastered).is_empty());
        assert_eq!(f.of_tag(Tag::Weak)[0].count, 1);
    }

    #[test]
    fn mastered_without_a_weak_entry_is_recorded_directly() {
        let mut f = MemFile::default();
        f.ensure_header("T");
        let r = apply_note(&mut f, "toan", Kind::Mastered, "Dạng B", "tự làm", d("2026-11-14")).unwrap();
        assert_eq!(r.action, "mastered");
        assert_eq!(f.of_tag(Tag::Mastered).len(), 1);
    }

    #[test]
    fn fields_cannot_break_the_line_shape() {
        let mut f = MemFile::default();
        f.ensure_header("T");
        apply_note(&mut f, "toan", Kind::Mistake, "A | B\nC", "x | y\n- [vững] giả", d("2026-11-14")).unwrap();
        let out = f.render();
        assert_eq!(out.lines().filter(|l| l.starts_with("- [")).count(), 1, "{out}");
        let e = &f.of_tag(Tag::Weak)[0];
        assert_eq!(e.topic, "A B C");
        assert!(apply_note(&mut f, "toan", Kind::Mistake, "   ", "", d("2026-11-14")).is_err());
        let long = "x".repeat(500);
        apply_note(&mut f, "toan", Kind::Mistake, &long, &long, d("2026-11-14")).unwrap();
        assert!(f.of_tag(Tag::Weak).iter().all(|e| e.topic.chars().count() <= MAX_TOPIC && e.note.chars().count() <= MAX_NOTE));
    }

    #[test]
    fn subject_normalization() {
        for (s, id) in [
            ("toan", "toan"), ("Toán", "toan"), ("TOÁN 10", "toan"), ("math", "toan"),
            ("lý", "ly"), ("Vật lý", "ly"), ("physics", "ly"),
            ("hóa", "hoa"), ("hoá học", "hoa"), ("Chemistry", "hoa"),
            ("văn", "van"), ("Ngữ văn", "van"),
            ("anh", "anh-ssat"), ("SSAT", "anh-ssat"), ("Tiếng Anh", "anh-ssat"), ("anh-ssat", "anh-ssat"),
            ("đức", "duc"), ("Tiếng Đức", "duc"), ("German", "duc"),
            ("tin", "tin-cpp"), ("C++", "tin-cpp"), ("Tin học", "tin-cpp"), ("tin-cpp", "tin-cpp"),
            ("ngoai-ngu", "ngoai-ngu"), ("Ngoại ngữ", "ngoai-ngu"), ("tiếng Pháp", "ngoai-ngu"),
            ("sinh", "chung"), ("Lịch sử", "chung"), ("", "chung"), ("../etc", "chung"),
        ] {
            assert_eq!(normalize_subject(s), id, "{s}");
        }
    }

    #[test]
    fn pick_review_takes_the_longest_overdue() {
        let t = d("2026-11-14");
        let item = |topic: &str, due: &str, count: u32| DueItem {
            subject: "toan".into(), topic: topic.into(), note: String::new(), last: d("2026-11-01"), due: d(due), count,
        };
        assert!(pick_review(vec![item("a", "2026-11-15", 5)], t).is_none(), "not due yet");
        let p = pick_review(vec![item("a", "2026-11-13", 1), item("b", "2026-11-10", 1), item("c", "2026-11-10", 4)], t).unwrap();
        assert_eq!(p.topic, "c", "oldest due, then most mistakes");
        assert_eq!(pick_review(vec![item("a", "2026-11-14", 1)], t).unwrap().topic, "a", "due today counts");
    }

    #[test]
    fn summary_is_short_and_ordered() {
        let t = d("2026-11-14");
        assert!(build_summary(&[], &[], &[], t).is_none());
        let weak_entry = |topic: &str, due: &str, count: u32, note: &str| {
            let mut e = Entry::plain(Tag::Weak, topic, note, d("2026-11-10"));
            e.count = count;
            e.due = Some(d(due));
            e
        };
        let weak: Vec<(String, Entry)> = (0..10).map(|i| ("toan".to_string(), weak_entry(&format!("Dạng {i}"), &format!("2026-11-{:02}", 10 + i), 1, "ghi chú"))).collect();
        let learned = vec![("ly".to_string(), Entry::plain(Tag::Learned, "Chuyển động thẳng đều", "", d("2026-11-12")))];
        let style: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let s = build_summary(&style, &weak, &learned, t).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines.len() <= 12, "{} lines:\n{s}", lines.len());
        assert_eq!(lines[0], "[memory]");
        assert_eq!(*lines.last().unwrap(), "[/memory]");
        assert_eq!(lines[1], "Cách giảng hợp: d; c; b", "newest three style lines");
        assert!(lines[3].contains("Dạng 0") && lines[3].contains("tới hạn ôn"), "most overdue first: {}", lines[3]);
        assert_eq!(lines.iter().filter(|l| l.starts_with("- toan")).count(), MAX_SUMMARY_WEAK);
        assert!(s.contains("Đã học gần đây: ly · Chuyển động thẳng đều (12/11)"));
        assert!(!s.contains("Dạng 9"), "least urgent items are left out");
    }

    #[test]
    fn understood_and_topic_ids_roundtrip() {
        let l = "- [hiểu] Rút gọn căn | ngày 2026-11-14 | id c1.t1 | ví dụ số 4 và 9";
        let e = parse_entry(l).unwrap();
        assert_eq!((e.tag, e.topic_id.as_deref(), e.note.as_str()), (Tag::Understood, Some("c1.t1"), "ví dụ số 4 và 9"));
        assert_eq!(e.render(), l);
        let mut f = MemFile::default();
        f.ensure_header("T");
        apply_note_id(&mut f, "toan", Kind::Mistake, "Rút gọn căn", "quên điều kiện", Some("c1.t1"), d("2026-11-14")).unwrap();
        apply_note_id(&mut f, "toan", Kind::Understood, "Rút gọn căn", "so sánh với chia bánh", Some("c1.t1"), d("2026-11-14")).unwrap();
        assert_eq!(f.of_tag(Tag::Weak)[0].topic_id.as_deref(), Some("c1.t1"));
        assert_eq!(f.of_tag(Tag::Understood)[0].note, "so sánh với chia bánh");
        assert!(f.render().contains("## Đã hiểu sau khi giảng lại"));
    }

    #[test]
    fn folder_notes_and_summary_with_school_progress() {
        let dir = tmp();
        let m = Memory::new(&dir);
        let t = d("2026-11-14");
        m.note_at(Some("fold-1"), "toan", Kind::Mistake, "Rút gọn căn", "quên điều kiện", Some("c1.t1"), t).unwrap();
        m.note_at(Some("fold-1"), "toan", Kind::Understood, "Rút gọn căn", "ví dụ số", None, t).unwrap();
        m.note_at(Some("fold-1"), "toan", Kind::Style, "Thích hình vẽ", "", None, t).unwrap();
        assert!(dir.join("memory").join("folders").join("fold-1.md").exists());
        assert!(!dir.join("memory").join("toan.md").exists(), "folder notes stay in the folder file");
        assert!(std::fs::read_to_string(dir.join("memory").join("profile.md")).unwrap().contains("Thích hình vẽ"));
        let k = serde_json::json!({"curriculum":[{"id":"c1","title":"Căn","topics":[
            {"id":"c1.t1","title":"Rút gọn"},{"id":"c1.t2","title":"Trục căn"},{"id":"c1.t3","title":"Phương trình chứa căn"}]}],
            "schoolProgress":{"coveredTopicIds":["c1.t1"],"currentTopicId":"c1.t2","confidence":0.7}});
        let s = m.folder_summary("fold-1", "toan", Some(&k), t).unwrap();
        assert!(s.starts_with("[memory]") && s.ends_with("[/memory]"), "{s}");
        assert!(s.contains("Cách giảng hợp: Thích hình vẽ"), "{s}");
        assert!(s.contains("- toan · Rút gọn căn · sai 1"), "{s}");
        assert!(s.contains("Đã hiểu sau khi giảng lại: Rút gọn căn (ví dụ số)"), "{s}");
        assert!(s.contains("Trên trường đang học: Trục căn [c1.t2] (độ tin 0.7)"), "{s}");
        assert!(s.contains("Trên trường đã học: Rút gọn [c1.t1]"), "{s}");
        assert!(s.contains("CHƯA học (muốn dùng thì phải dạy từ đầu): Phương trình chứa căn [c1.t3]"), "{s}");
        assert!(m.folder_summary("empty", "toan", None, t).is_some(), "style alone is worth a summary");
        assert_eq!(m.folder_weak("fold-1", "toan").len(), 1);
        assert!(m.folder_review_candidate("fold-1", "toan", d("2026-11-20")).is_some());
        assert!(m.note_at(Some("../x"), "toan", Kind::Mistake, "a", "", None, t).is_ok(), "bad folder id falls back to the subject file");
        assert!(dir.join("memory").join("toan.md").exists());
    }

    #[test]
    fn store_writes_files_reads_hand_edits_and_builds_progress() {
        let dir = tmp();
        let m = Memory::new(&dir);
        let t = d("2026-11-14");
        let r = m.note("Toán", Kind::Mistake, "Xét dấu tam thức khi a âm", "quên đổi chiều", t).unwrap();
        assert_eq!((r.subject.as_str(), r.action), ("toan", "created"));
        m.note("toán", Kind::Progress, "Bất phương trình bậc hai", "xong phần xét dấu", t).unwrap();
        m.note("toan", Kind::Style, "Hiểu nhanh hơn khi có ví dụ số", "", t).unwrap();
        m.note("Sinh", Kind::Mistake, "Phân biệt ADN và ARN", "", t).unwrap();
        let toan = std::fs::read_to_string(dir.join("memory").join("toan.md")).unwrap();
        assert!(toan.contains("## Đang yếu\n- [yếu] Xét dấu tam thức khi a âm | sai 1 | đúng 0 | bậc 0 | lần cuối 2026-11-14 | ôn 2026-11-15 | quên đổi chiều"), "{toan}");
        assert!(toan.contains("## Đã học\n- [học] Bất phương trình bậc hai | ngày 2026-11-14 | xong phần xét dấu"), "{toan}");
        let profile = std::fs::read_to_string(dir.join("memory").join("profile.md")).unwrap();
        assert!(profile.contains("## Cách giảng hợp\n- [cách] Hiểu nhanh hơn khi có ví dụ số | môn toan | ngày 2026-11-14"), "{profile}");
        assert!(dir.join("memory").join("chung.md").exists());
        assert!(std::fs::read_dir(dir.join("memory")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")), "no temp files left");

        // a human edits the file: change a date and the note, add a free line
        let edited = toan.replace("lần cuối 2026-11-14 | ôn 2026-11-15 | quên đổi chiều", "lần cuối 2026-11-01 | ôn 2026-11-02 | sửa tay") + "\nTớ tự thêm dòng này.\n";
        std::fs::write(dir.join("memory").join("toan.md"), edited).unwrap();
        let due = m.due_items(t);
        assert_eq!(due.len(), 1, "only the hand-edited toan item is due; the chung one is due tomorrow");
        assert_eq!((due[0].subject.as_str(), due[0].note.as_str(), due[0].due), ("toan", "sửa tay", d("2026-11-02")));
        // a later server write keeps the hand-added line
        m.note("toan", Kind::Mistake, "Dạng khác", "", t).unwrap();
        assert!(std::fs::read_to_string(dir.join("memory").join("toan.md")).unwrap().contains("Tớ tự thêm dòng này."));

        let p = m.progress();
        let subs = p["subjects"].as_array().unwrap();
        assert_eq!(subs.iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(), ["toan", "chung"]);
        assert_eq!(subs[0]["name"], "Toán");
        assert_eq!(subs[0]["weak"][0]["topic"], "Xét dấu tam thức khi a âm", "most overdue first");
        assert_eq!(subs[0]["weak"][0]["due"], "2026-11-02");
        assert_eq!(subs[0]["weak"][0]["lastSeen"], "2026-11-01");
        assert_eq!(subs[0]["recent"][0]["date"], "2026-11-14");
        assert_eq!(p["style"][0], "Hiểu nhanh hơn khi có ví dụ số");
    }
}
