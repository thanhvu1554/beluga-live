//! Streaming parser for the lesson line format (PROTOCOL section 2).
//!
//! Claude writes plain lines (speech) and `@command {json}` lines (board directives).
//! The parser accepts chunks of any size (even a chunk that splits a UTF-8 character
//! when fed as bytes) and emits events only when a full line is available, or when
//! `finish` flushes the last unterminated line at the end of a turn.

use serde_json::{json, Value};

/// Speech lines longer than this are split on punctuation.
pub const MAX_SAY_CHARS: usize = 220;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Session title (no `seq`; the session layer adds the date).
    Title { text: String },
    Say { seq: u64, turn: u64, text: String },
    Step { seq: u64, turn: u64, index: usize, title: String },
    Show { seq: u64, turn: u64, step: usize, block: Value },
    Ask { seq: u64, turn: u64, kind: String, prompt: String },
    Done { seq: u64, turn: u64 },
}

impl Event {
    pub fn seq(&self) -> Option<u64> {
        match self {
            Event::Title { .. } => None,
            Event::Say { seq, .. }
            | Event::Step { seq, .. }
            | Event::Show { seq, .. }
            | Event::Ask { seq, .. }
            | Event::Done { seq, .. } => Some(*seq),
        }
    }

    /// The wire frame defined in PROTOCOL section 4.
    pub fn to_json(&self) -> Value {
        match self {
            Event::Title { text } => json!({"t": "title", "text": text}),
            Event::Say { seq, turn, text } => {
                json!({"t": "say", "seq": seq, "turn": turn, "text": text, "audio": false})
            }
            Event::Step { seq, turn, index, title } => {
                json!({"t": "step", "seq": seq, "turn": turn, "index": index, "title": title})
            }
            Event::Show { seq, turn, step, block } => {
                json!({"t": "show", "seq": seq, "turn": turn, "step": step, "block": block})
            }
            Event::Ask { seq, turn, kind, prompt } => {
                json!({"t": "ask", "seq": seq, "turn": turn, "kind": kind, "prompt": prompt})
            }
            Event::Done { seq, turn } => json!({"t": "done", "seq": seq, "turn": turn}),
        }
    }
}

pub struct LessonParser {
    turn: u64,
    next_seq: u64,
    steps: usize,
    current_step: Option<usize>,
    line: String,
    pending_bytes: Vec<u8>,
    stopped: bool,
    dropped: Vec<String>,
}

impl LessonParser {
    /// `next_seq` is the first seq to hand out; `steps` is how many steps the session
    /// already has (a `@show` in a later turn attaches to the last step).
    pub fn new(turn: u64, next_seq: u64, steps: usize) -> Self {
        LessonParser {
            turn,
            next_seq,
            steps,
            current_step: if steps > 0 { Some(steps - 1) } else { None },
            line: String::new(),
            pending_bytes: Vec::new(),
            stopped: false,
            dropped: Vec::new(),
        }
    }

    /// Directive lines that were dropped (bad JSON, unknown command...), with the reason.
    pub fn take_dropped(&mut self) -> Vec<String> {
        std::mem::take(&mut self.dropped)
    }

    fn drop_line(&mut self, line: &str, why: &str) {
        tracing::warn!("lesson parser: dropped line ({why}): {}", truncate(line, 120));
        self.dropped.push(format!("{why}: {}", truncate(line, 120)));
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn steps(&self) -> usize {
        self.steps
    }

    /// True once an `@ask` was seen: everything after it is discarded.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Feed raw bytes; a multi-byte character split across chunks is held back.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.pending_bytes.extend_from_slice(bytes);
        let valid_up_to = match std::str::from_utf8(&self.pending_bytes) {
            Ok(_) => self.pending_bytes.len(),
            Err(e) => {
                if e.error_len().is_none() {
                    e.valid_up_to() // incomplete sequence at the end: wait for more
                } else {
                    // genuinely invalid bytes: decode lossily and move on
                    let s = String::from_utf8_lossy(&self.pending_bytes).into_owned();
                    self.pending_bytes.clear();
                    return self.push_str(&s);
                }
            }
        };
        let rest = self.pending_bytes.split_off(valid_up_to);
        let head = std::mem::replace(&mut self.pending_bytes, rest);
        let s = String::from_utf8(head).unwrap_or_default();
        self.push_str(&s)
    }

    pub fn push_str(&mut self, chunk: &str) -> Vec<Event> {
        let mut out = Vec::new();
        for ch in chunk.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.line);
                self.handle_line(&line, &mut out);
            } else {
                self.line.push(ch);
            }
        }
        out
    }

    /// End of turn: process the last line even without a trailing newline.
    pub fn finish(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if !self.pending_bytes.is_empty() {
            let s = String::from_utf8_lossy(&self.pending_bytes).into_owned();
            self.pending_bytes.clear();
            out.extend(self.push_str(&s));
        }
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.handle_line(&line, &mut out);
        }
        out
    }

    /// Drop everything still buffered (used on interrupt).
    pub fn discard(&mut self) {
        self.line.clear();
        self.pending_bytes.clear();
        self.stopped = true;
    }

    fn alloc_seq(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    fn handle_line(&mut self, raw: &str, out: &mut Vec<Event>) {
        if self.stopped {
            return;
        }
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            return;
        }
        if line.starts_with('@') {
            self.handle_directive(line, out);
        } else {
            self.handle_say(line, out);
        }
    }

    fn handle_say(&mut self, line: &str, out: &mut Vec<Event>) {
        let clean = sanitize_say(line);
        if clean.is_empty() {
            return;
        }
        for piece in split_long(&clean) {
            let seq = self.alloc_seq();
            out.push(Event::Say { seq, turn: self.turn, text: piece });
        }
    }

    fn open_step(&mut self, title: String, out: &mut Vec<Event>) -> usize {
        let index = self.steps;
        self.steps += 1;
        self.current_step = Some(index);
        let seq = self.alloc_seq();
        out.push(Event::Step { seq, turn: self.turn, index, title });
        index
    }

    fn handle_directive(&mut self, line: &str, out: &mut Vec<Event>) {
        let body = &line[1..];
        let name_end = body
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(body.len());
        let name = &body[..name_end];
        let rest = body[name_end..].trim();

        let parsed: Option<Value> = if rest.is_empty() {
            if name == "done" {
                Some(json!({}))
            } else {
                None
            }
        } else {
            parse_json_lenient(rest)
        };
        let Some(data) = parsed else {
            self.drop_line(line, "bad or missing JSON");
            return;
        };
        let str_field = |k: &str| data.get(k).and_then(|v| v.as_str()).map(|s| s.trim().to_string());

        match name {
            "title" => match str_field("text") {
                Some(t) if !t.is_empty() => out.push(Event::Title { text: t }),
                _ => self.drop_line(line, "@title without text"),
            },
            "step" => {
                let title = str_field("title").unwrap_or_default();
                self.open_step(title, out);
            }
            "show" => {
                let Some(mut obj) = data.as_object().cloned() else { return };
                if !obj.get("type").map(|t| t.is_string()).unwrap_or(false) {
                    self.drop_line(line, "@show without type");
                    return;
                }
                let step = match self.current_step {
                    Some(s) => s,
                    None => self.open_step(String::new(), out),
                };
                let seq = self.alloc_seq();
                let has_id = obj.get("id").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
                if !has_id {
                    obj.insert("id".into(), Value::String(format!("b{seq}")));
                }
                out.push(Event::Show { seq, turn: self.turn, step, block: Value::Object(obj) });
            }
            "ask" => {
                let prompt = str_field("prompt").unwrap_or_default();
                let kind = match str_field("kind").as_deref() {
                    Some(k @ ("next" | "why" | "check")) => k.to_string(),
                    _ => "check".to_string(),
                };
                let seq = self.alloc_seq();
                out.push(Event::Ask { seq, turn: self.turn, kind, prompt });
                self.stopped = true;
            }
            "done" => {
                let seq = self.alloc_seq();
                out.push(Event::Done { seq, turn: self.turn });
            }
            other => self.drop_line(line, &format!("unknown directive @{other}")),
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Parse a JSON object. If that fails, retry after doubling backslashes that do not form
/// a valid JSON escape (LaTeX such as `\(` or `\l` written with a single backslash).
fn parse_json_lenient(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return v.is_object().then_some(v);
    }
    let mut fixed = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') => {
                    fixed.push('\\');
                    fixed.push(chars.next().unwrap());
                }
                _ => fixed.push_str("\\\\"),
            }
        } else {
            fixed.push(c);
        }
    }
    serde_json::from_str::<Value>(&fixed).ok().filter(|v| v.is_object())
}

/// Strip markdown/LaTeX leftovers from a spoken line.
pub fn sanitize_say(line: &str) -> String {
    let mut s = line.trim().to_string();
    // leading markdown markers: headings, quotes, bullets
    loop {
        let t = s.trim_start();
        let stripped = t
            .strip_prefix('#')
            .map(|r| r.trim_start_matches('#'))
            .or_else(|| t.strip_prefix("> "))
            .or_else(|| t.strip_prefix("- "))
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("• "));
        match stripped {
            Some(r) => s = r.to_string(),
            None => break,
        }
    }
    // [text](url) -> text
    let chars: Vec<char> = s.chars().collect();
    let mut tmp = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            if let Some(close) = chars[i..].iter().position(|&c| c == ']') {
                let close = i + close;
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close..].iter().position(|&c| c == ')') {
                        tmp.extend(&chars[i + 1..close]);
                        i = close + end + 1;
                        continue;
                    }
                }
            }
        }
        tmp.push(chars[i]);
        i += 1;
    }
    let tmp = tmp.replace("__", "");
    let cleaned: String = tmp.chars().filter(|c| !matches!(c, '*' | '`' | '$' | '\\')).collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().any(|c| c.is_alphanumeric()) {
        collapsed
    } else {
        String::new()
    }
}

/// Split text longer than `MAX_SAY_CHARS` at punctuation (sentence end, then comma-like,
/// then whitespace, then hard cut).
pub fn split_long(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > MAX_SAY_CHARS {
        let cut = find_cut(&rest);
        let head: String = rest[..cut].iter().collect();
        let head = head.trim();
        if !head.is_empty() {
            out.push(head.to_string());
        }
        let tail: String = rest[cut..].iter().collect();
        rest = tail.trim_start().chars().collect();
    }
    let last: String = rest.iter().collect();
    let last = last.trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

fn find_cut(rest: &[char]) -> usize {
    let is_ws = |i: usize| rest.get(i).map(|c| c.is_whitespace()).unwrap_or(true);
    for i in (0..MAX_SAY_CHARS).rev() {
        if matches!(rest[i], '.' | '!' | '?' | '…') && is_ws(i + 1) {
            return i + 1;
        }
    }
    for i in (0..MAX_SAY_CHARS).rev() {
        if matches!(rest[i], ',' | ';' | ':') && is_ws(i + 1) {
            return i + 1;
        }
    }
    for i in (0..MAX_SAY_CHARS).rev() {
        if rest[i].is_whitespace() {
            return i;
        }
    }
    MAX_SAY_CHARS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> Vec<Event> {
        let mut p = LessonParser::new(1, 1, 0);
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.push_str(c));
        }
        out.extend(p.finish());
        out
    }

    fn says(ev: &[Event]) -> Vec<String> {
        ev.iter()
            .filter_map(|e| if let Event::Say { text, .. } = e { Some(text.clone()) } else { None })
            .collect()
    }

    const SAMPLE: &str = "@title {\"text\":\"Toán · Bất phương trình bậc hai\"}\nCâu này là bất phương trình bậc hai, nhìn màn hình nhé.\n@step {\"title\":\"Đưa về dạng chuẩn\"}\n@show {\"type\":\"math\",\"tex\":\"x^2 - 5x + 6 > 0\"}\nViệc đầu tiên là tìm nghiệm của tam thức.\n@ask {\"kind\":\"next\",\"prompt\":\"Cậu tìm nghiệm bằng cách nào?\"}\n";

    #[test]
    fn protocol_example_in_order() {
        let ev = run(&[SAMPLE]);
        assert_eq!(ev.len(), 6);
        assert!(matches!(&ev[0], Event::Title { text } if text == "Toán · Bất phương trình bậc hai"));
        assert!(matches!(&ev[1], Event::Say { seq: 1, .. }));
        assert!(matches!(&ev[2], Event::Step { seq: 2, index: 0, .. }));
        match &ev[3] {
            Event::Show { seq: 3, step: 0, block, .. } => {
                assert_eq!(block["type"], "math");
                assert_eq!(block["id"], "b3");
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(&ev[4], Event::Say { seq: 4, .. }));
        assert!(matches!(&ev[5], Event::Ask { seq: 5, kind, .. } if kind == "next"));
    }

    #[test]
    fn any_chunking_gives_same_events() {
        let whole = run(&[SAMPLE]);
        let chars: Vec<String> = SAMPLE.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chars.iter().map(|s| s.as_str()).collect();
        assert_eq!(run(&refs), whole);
        // chunks of 7 chars, cutting through JSON and words
        let cs: Vec<char> = SAMPLE.chars().collect();
        let sevens: Vec<String> = cs.chunks(7).map(|c| c.iter().collect()).collect();
        let refs: Vec<&str> = sevens.iter().map(|s| s.as_str()).collect();
        assert_eq!(run(&refs), whole);
    }

    #[test]
    fn nothing_emitted_before_newline() {
        let mut p = LessonParser::new(1, 1, 0);
        assert!(p.push_str("Chào cậu").is_empty());
        assert!(p.push_str(", tớ là Minh.").is_empty());
        let e = p.push_str("\n");
        assert_eq!(e.len(), 1);
        assert!(p.finish().is_empty());
    }

    #[test]
    fn finish_flushes_unterminated_line() {
        let ev = run(&["Một câu không xuống dòng."]);
        assert_eq!(says(&ev), vec!["Một câu không xuống dòng."]);
        let ev = run(&["@done {}"]);
        assert!(matches!(ev[0], Event::Done { seq: 1, .. }));
    }

    #[test]
    fn utf8_split_across_byte_chunks() {
        let text = "Cậu làm đúng rồi, tuyệt vời!\n";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut p = LessonParser::new(1, 1, 0);
            let mut ev = p.push_bytes(&bytes[..split]);
            ev.extend(p.push_bytes(&bytes[split..]));
            ev.extend(p.finish());
            assert_eq!(says(&ev), vec!["Cậu làm đúng rồi, tuyệt vời!"], "split at {split}");
        }
        // byte at a time
        let mut p = LessonParser::new(1, 1, 0);
        let mut ev = Vec::new();
        for b in bytes {
            ev.extend(p.push_bytes(&[*b]));
        }
        assert_eq!(says(&ev), vec!["Cậu làm đúng rồi, tuyệt vời!"]);
    }

    #[test]
    fn crlf_lines() {
        let ev = run(&["Dòng một.\r\n@step {\"title\":\"A\"}\r\nDòng hai.\r\n"]);
        assert_eq!(says(&ev), vec!["Dòng một.", "Dòng hai."]);
        assert!(matches!(&ev[1], Event::Step { title, .. } if title == "A"));
    }

    #[test]
    fn latex_double_backslash_survives() {
        let line = r#"@show {"type":"math","tex":"\\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}"}"#;
        let ev = run(&[line, "\n"]);
        match &ev[1] {
            Event::Show { block, .. } => assert_eq!(block["tex"], r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn single_backslash_latex_is_repaired() {
        let line = r#"@show {"type":"math","tex":"\(x\) \le 3"}"#;
        let ev = run(&[line]);
        match &ev[1] {
            Event::Show { block, .. } => assert_eq!(block["tex"], r"\(x\) \le 3"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn show_before_first_step_opens_untitled_step() {
        let ev = run(&["@show {\"type\":\"text\",\"md\":\"hi\"}\n@show {\"type\":\"text\",\"md\":\"hi2\"}\n"]);
        assert!(matches!(&ev[0], Event::Step { index: 0, title, seq: 1, .. } if title.is_empty()));
        assert!(matches!(&ev[1], Event::Show { step: 0, seq: 2, .. }));
        assert!(matches!(&ev[2], Event::Show { step: 0, seq: 3, .. }));
        let mut p = LessonParser::new(1, 1, 0);
        p.push_str("@show {\"type\":\"text\",\"md\":\"x\"}\n@step {\"title\":\"B\"}\n");
        assert_eq!(p.steps(), 2);
    }

    #[test]
    fn show_in_later_turn_attaches_to_last_step() {
        let mut p = LessonParser::new(2, 10, 3);
        let ev = p.push_str("@show {\"type\":\"text\",\"md\":\"x\"}\n");
        assert!(matches!(&ev[0], Event::Show { step: 2, seq: 10, turn: 2, .. }));
    }

    #[test]
    fn bad_directives_are_dropped_without_breaking_turn() {
        let ev = run(&[
            "Trước.\n@show {\"type\":\"math\",\"tex\":\n@step not json\n@bogus {\"a\":1}\n@ lone at\n@\n@show {\"nottype\":1}\n@show [1,2]\n@ask\nSau.\n",
        ]);
        assert_eq!(says(&ev), vec!["Trước.", "Sau."]);
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn dropped_lines_are_reported() {
        let mut p = LessonParser::new(1, 1, 0);
        p.push_str("ok.
@bogus {}
@step nope
@show {\"x\":1}
");
        let d = p.take_dropped();
        assert_eq!(d.len(), 3);
        assert!(d[0].contains("unknown directive @bogus"));
        assert!(p.take_dropped().is_empty());
    }

    #[test]
    fn everything_after_ask_is_dropped() {
        let ev = run(&["Nói.\n@ask {\"kind\":\"why\",\"prompt\":\"Vì sao?\"}\nCòn nữa.\n@done {}\n@step {\"title\":\"x\"}\n"]);
        assert_eq!(ev.len(), 2);
        assert!(matches!(&ev[1], Event::Ask { kind, .. } if kind == "why"));
    }

    #[test]
    fn unknown_ask_kind_falls_back_to_check() {
        let ev = run(&["@ask {\"kind\":\"weird\",\"prompt\":\"?\"}\n"]);
        assert!(matches!(&ev[0], Event::Ask { kind, .. } if kind == "check"));
    }

    #[test]
    fn markdown_is_stripped() {
        let ev = run(&["## **Đáp án** là `x = 2` và $x^2$\n- gạch đầu dòng\n[xem](http://a.b) thêm\n---\n"]);
        assert_eq!(says(&ev), vec!["Đáp án là x = 2 và x^2", "gạch đầu dòng", "xem thêm"]);
    }

    #[test]
    fn long_say_is_split_on_punctuation() {
        let sentence = "Đây là một câu khá dài để thử việc cắt";
        let long = format!("{s}. {s}. {s}. {s}. {s}. {s}.", s = sentence);
        assert!(long.chars().count() > MAX_SAY_CHARS);
        let ev = run(&[&long, "\n"]);
        let s = says(&ev);
        assert!(s.len() >= 2);
        for part in &s {
            assert!(part.chars().count() <= MAX_SAY_CHARS, "{part}");
            assert!(part.ends_with('.'), "{part}");
        }
        assert_eq!(s.join(" "), long);
        // seq increments per piece
        let seqs: Vec<u64> = ev.iter().filter_map(|e| e.seq()).collect();
        assert_eq!(seqs, (1..=s.len() as u64).collect::<Vec<_>>());
    }

    #[test]
    fn long_say_without_punctuation_cuts_on_spaces_or_hard() {
        let words = "từ ".repeat(200);
        let s = split_long(words.trim());
        assert!(s.len() >= 2 && s.iter().all(|p| p.chars().count() <= MAX_SAY_CHARS));
        let blob = "a".repeat(500);
        let s = split_long(&blob);
        assert_eq!(s.len(), 3);
        assert_eq!(s.concat(), blob);
    }

    #[test]
    fn explicit_block_id_is_kept() {
        let ev = run(&["@step {\"title\":\"a\"}\n@show {\"type\":\"answer\",\"md\":\"4\",\"id\":\"ans\"}\n"]);
        assert!(matches!(&ev[1], Event::Show { block, .. } if block["id"] == "ans"));
    }

    #[test]
    fn discard_drops_partial_line() {
        let mut p = LessonParser::new(1, 1, 0);
        p.push_str("nửa dòng");
        p.discard();
        assert!(p.finish().is_empty());
        assert!(p.push_str("tiếp\n").is_empty());
    }

    #[test]
    fn wire_frames_match_protocol() {
        let ev = run(&[SAMPLE]);
        let say = ev[1].to_json();
        assert_eq!(say["t"], "say");
        assert_eq!(say["audio"], false);
        let step = ev[2].to_json();
        assert_eq!(step["index"], 0);
        assert_eq!(ev[0].to_json()["t"], "title");
        assert_eq!(ev[5].to_json()["kind"], "next");
    }
}
