//! "Fake Claude": plays a scripted lesson in small chunks with delays, through the real parser.

use super::{BrainEvent, BrainMsg, Sink, TurnInput};
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
            let mut queue: std::collections::VecDeque<Step> = script(&input.user_text).into();
            // Tests can see what context the server attached: echo the memory block and review note.
            for l in context_echo(&input.text).into_iter().rev() {
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
