//! "Fake Claude": plays a scripted lesson in small chunks with delays, through the real parser.

use super::{BrainEvent, BrainMsg, Sink, TurnInput, TurnUsage};
use crate::session::Cmd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

pub struct MockBrain {
    sink: Sink,
    cmd_tx: UnboundedSender<Cmd>,
    delay: Duration,
    /// Turn number to abort (0 = none).
    cancelled: Arc<AtomicU64>,
    current: Arc<AtomicU64>,
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Line(String),
    /// Ask the phone for a photo (same path as the real MCP tool).
    Capture { ok: Vec<String>, err: Vec<String> },
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Chat-mode answer: markdown with an inline block and a heading.
pub fn chat_script() -> Vec<Step> {
    ["Đây là **markdown** với $x^2$.", "", "- ý một", "@show {\"type\":\"math\",\"tex\":\"x^2-5x+6=0\"}", "@step {\"title\":\"Bước 2\"}", "Xong phần này."]
        .iter()
        .map(|s| Step::Line(s.to_string()))
        .collect()
}

pub fn script(user_text: &str) -> Vec<Step> {
    let t = user_text.to_lowercase();
    let line = |s: &str| Step::Line(s.to_string());
    if t.contains("chụp") || t.contains("chup") {
        return vec![
            line("Để tớ xem đề nhé."),
            Step::Capture {
                ok: lines(&[
                    "Tớ thấy đề rồi.",
                    "@step {\"title\":\"Đọc đề\"}",
                    "@show {\"type\":\"text\",\"md\":\"Đề bài trong ảnh\"}",
                    "@ask {\"kind\":\"check\",\"prompt\":\"Cậu thấy đề này hỏi gì?\"}",
                ]),
                err: lines(&["Tớ chưa chụp được ảnh.", "@ask {\"kind\":\"check\",\"prompt\":\"Cậu chụp lại giúp tớ nhé?\"}"]),
            },
        ];
    }
    if t.contains("dài") {
        let mut v: Vec<Step> = (1..=40).map(|i| Step::Line(format!("Đây là câu số {i} của một bài giảng dài."))).collect();
        v.push(line("@done {}"));
        return v;
    }
    if t.contains("xong") || t.contains("đáp án") || t.contains("tìm nghiệm") {
        return vec![
            line("Đúng rồi, tìm nghiệm trước."),
            line("@step {\"title\":\"Xét dấu\"}"),
            line("@show {\"type\":\"answer\",\"md\":\"$x<2$ hoặc $x>3$\"}"),
            line("Chốt lại: nghiệm là hai khoảng ngoài hai nghiệm."),
            line("@done {}"),
        ];
    }
    vec![
        line("@title {\"text\":\"Toán · Bất phương trình bậc hai\"}"),
        line("Câu này là bất phương trình bậc hai, nhìn màn hình nhé."),
        line("@step {\"title\":\"Đưa về dạng chuẩn\"}"),
        line("@show {\"type\":\"math\",\"tex\":\"x^2 - 5x + 6 > 0\"}"),
        line("Việc đầu tiên là tìm nghiệm của tam thức."),
        line("@ask {\"kind\":\"next\",\"prompt\":\"Cậu tìm nghiệm bằng cách nào?\"}"),
    ]
}

/// Spoken echo of the `[memory]` block lines and the `[ôn lại: ...]` note found in a turn's text.
pub fn context_echo(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_memory = false;
    for l in text.lines() {
        let l = l.trim();
        if l == "[memory]" {
            in_memory = true;
            out.push("Mock thấy memory.".to_string());
        } else if l == "[/memory]" {
            in_memory = false;
        } else if in_memory {
            out.push(format!("Mock nhớ: {}", l.trim_start_matches("- ")));
        } else if let Some(r) = l.strip_prefix("[ôn lại: ") {
            out.push(format!("Mock ôn lại: {}", r.trim_end_matches(']')));
        } else if let Some(i) = l.find("[ôn nhanh: ") {
            out.push(format!("Mock ôn nhanh: {}", l[i + "[ôn nhanh: ".len()..].trim_end_matches(']')));
        } else if l.starts_with("[tóm tắt phiên trước") {
            out.push("Mock thấy tóm tắt.".to_string());
        } else if let Some(f) = l.strip_prefix("[folder: ") {
            out.push(format!("Mock folder: {}", f.trim_end_matches(']')));
        }
        // (only chat is echoed, so live-mode tests keep their exact v1 frames)
        if l.contains("[mode: chat]") {
            out.push("Mock mode chat.".to_string());
        }
    }
    out
}

impl MockBrain {
    pub fn new(sink: Sink, cmd_tx: UnboundedSender<Cmd>, delay_ms: u64) -> Self {
        MockBrain {
            sink,
            cmd_tx,
            delay: Duration::from_millis(delay_ms),
            cancelled: Arc::new(AtomicU64::new(0)),
            current: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn interrupt(&self) {
        self.cancelled.store(self.current.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    pub fn send_turn(&self, input: TurnInput) {
        let sink = self.sink.clone();
        let cmd_tx = self.cmd_tx.clone();
        let delay = self.delay;
        let cancelled = self.cancelled.clone();
        self.current.store(input.turn, Ordering::SeqCst);
        tokio::spawn(async move {
            let turn = input.turn;
            let is_cancelled = || cancelled.load(Ordering::SeqCst) == turn;
            let send = |s: String| {
                let _ = sink.send(BrainMsg { turn, ev: BrainEvent::Text(s) });
            };
            // Emit a line in uneven chunks so the parser sees cuts inside words and JSON.
            let emit = |line: &str| {
                let chars: Vec<char> = line.chars().chain(std::iter::once('\n')).collect();
                for piece in chars.chunks(9) {
                    send(piece.iter().collect());
                }
            };
            let steps = if input.mode == "chat" && !input.user_text.contains("chụp") { chat_script() } else { script(&input.user_text) };
            let mut queue: std::collections::VecDeque<Step> = steps.into();
            // Tests can see what context the server attached: echo the memory block and review note.
            let mut echo = context_echo(&input.text);
            // "#raw" in the student's words: echo every line Claude would receive (isolation tests).
            if input.user_text.contains("#raw") {
                echo.extend(input.text.lines().filter(|l| !l.trim().is_empty()).map(|l| format!("Mock raw: {}", l.trim())));
            }
            if !input.attachments.is_empty() {
                let kinds: Vec<&str> = input.attachments.iter().map(|a| a.kind.as_str()).collect();
                echo.push(format!("Mock thấy tệp {} model {}.", kinds.join(" "), input.model));
            }
            for l in echo.into_iter().rev() {
                queue.push_front(Step::Line(l));
            }
            while let Some(step) = queue.pop_front() {
                if is_cancelled() {
                    break;
                }
                tokio::time::sleep(delay).await;
                if is_cancelled() {
                    break;
                }
                match step {
                    Step::Line(l) => emit(&l),
                    Step::Capture { ok, err } => {
                        let (tx, rx) = oneshot::channel();
                        let _ = cmd_tx.send(Cmd::Capture { reply: tx });
                        let res = rx.await.unwrap_or_else(|_| Err("session closed".into()));
                        let follow = if res.is_ok() { ok } else { err };
                        for l in follow.into_iter().rev() {
                            queue.push_front(Step::Line(l));
                        }
                    }
                }
            }
            // "#ctx" in the student's words simulates a context that is too big (compaction tests).
            let context_tokens = if input.user_text.contains("#ctx") { 500_000 } else { 1_000 };
            let usage = TurnUsage { input_tokens: 100, output_tokens: 50, cache_read_tokens: 900, cache_write_tokens: 0, cli_cost_usd: 0.001, context_tokens };
            let _ = sink.send(BrainMsg { turn, ev: BrainEvent::Usage(usage) });
            let _ = sink.send(BrainMsg { turn, ev: BrainEvent::End });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_end_with_ask_or_done() {
        for q in ["xin chào", "tìm nghiệm", "bài dài", "chụp giúp"] {
            let s = script(q);
            assert!(!s.is_empty());
        }
        assert!(matches!(script("x").last(), Some(Step::Line(l)) if l.starts_with("@ask")));
        assert!(matches!(script("xong").last(), Some(Step::Line(l)) if l.starts_with("@done")));
    }
}
