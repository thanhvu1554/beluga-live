//! One model call as a unit of work: retries with jittered exponential backoff on retryable errors
//! (429, 5xx, timeouts, network, unparsable or truncated JSON), local validation of the reply, usage
//! accounting for every attempt (failed attempts cost money too), and immediate cancellation.

use std::sync::Mutex;
use std::time::Duration;

use rand::Rng;
use serde_json::Value;
use tokio::sync::watch;

use super::llm::{Llm, LlmError, LlmRequest, Usage};

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Extra attempts after the first.
    pub retries: usize,
    pub base_ms: u64,
}

/// `base · 2^(attempt-1)`, times a random factor in `[0.5, 1.5)` so parallel units do not retry in step.
pub fn backoff(base_ms: u64, attempt: usize) -> Duration {
    let exp = base_ms.saturating_mul(1u64 << (attempt.saturating_sub(1)).min(6));
    let f: f64 = rand::rng().random_range(0.5..1.5);
    Duration::from_millis((exp as f64 * f) as u64)
}

pub async fn cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// What a successful call returns: the parsed JSON, the finish reason and who answered.
#[derive(Clone, Debug)]
pub struct Reply {
    pub value: Value,
    pub provider: String,
    pub usage: Usage,
    pub elapsed_ms: u64,
}

/// Call until the reply parses and `validate` accepts it. `spent` gets the usage of every attempt.
pub async fn call_json(
    llm: &dyn Llm,
    req: LlmRequest,
    policy: RetryPolicy,
    spent: &Mutex<Usage>,
    mut cancel: watch::Receiver<bool>,
    validate: &(dyn Fn(&Value) -> Result<(), String> + Send + Sync),
) -> Result<Reply, CallError> {
    let mut last = String::new();
    for attempt in 0..=policy.retries {
        if *cancel.borrow() {
            return Err(CallError::Cancelled);
        }
        if attempt > 0 {
            tokio::select! {
                _ = tokio::time::sleep(backoff(policy.base_ms, attempt)) => {}
                _ = cancelled(&mut cancel) => return Err(CallError::Cancelled),
            }
        }
        let r = tokio::select! {
            r = llm.complete(req.clone()) => r,
            _ = cancelled(&mut cancel) => return Err(CallError::Cancelled),
        };
        let r: Result<Reply, LlmError> = match r {
            Ok(resp) => {
                spent.lock().unwrap().add(&resp.usage);
                tracing::info!(tag = req.tag, model = %req.model, provider = %resp.provider, ms = resp.elapsed_ms,
                    cost = resp.usage.cost_usd, inp = resp.usage.input_tokens, cached = resp.usage.cached_tokens,
                    out = resp.usage.output_tokens, think = resp.usage.reasoning_tokens, "study llm call");
                resp.json().and_then(|v| {
                    validate(&v).map_err(|m| {
                        tracing::warn!(tag = req.tag, "reply rejected ({m}): {}", resp.text.chars().take(400).collect::<String>());
                        LlmError { status: 0, message: m, retryable: true }
                    })?;
                    Ok(Reply { value: v, provider: resp.provider.clone(), usage: resp.usage.clone(), elapsed_ms: resp.elapsed_ms })
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
    Err(CallError::Failed(last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::llm::FakeLlm;

    #[test]
    fn backoff_grows_with_jitter() {
        for _ in 0..50 {
            let a = backoff(100, 1).as_millis();
            let b = backoff(100, 3).as_millis();
            assert!((50..150).contains(&a), "{a}");
            assert!((200..600).contains(&b), "{b}");
        }
    }

    #[tokio::test]
    async fn retries_bad_replies_then_gives_up_and_counts_every_attempt() {
        let fake = FakeLlm::new(|_, n| Ok(if n < 2 { "garbage".into() } else { "{\"ok\":true}".into() }));
        let spent = Mutex::new(Usage::default());
        let (_tx, rx) = watch::channel(false);
        let req = LlmRequest::new("extract", "m", None, "s", vec![], 10);
        let ok = call_json(&fake, req.clone(), RetryPolicy { retries: 2, base_ms: 1 }, &spent, rx.clone(), &|_| Ok(())).await.unwrap();
        assert_eq!(ok.value["ok"], true);
        assert_eq!(spent.lock().unwrap().cost_usd, 0.003);
        let r = call_json(&fake, req, RetryPolicy { retries: 1, base_ms: 1 }, &spent, rx, &|_| Err("nope".into())).await;
        assert!(matches!(r, Err(CallError::Failed(m)) if m.contains("nope")));
    }

    #[tokio::test]
    async fn cancel_is_immediate() {
        let mut fake = FakeLlm::new(|_, _| Ok("{}".into()));
        fake.delay = Duration::from_secs(30);
        let (tx, rx) = watch::channel(false);
        let spent = Mutex::new(Usage::default());
        let t = std::time::Instant::now();
        let h = async {
            call_json(&fake, LlmRequest::new("x", "m", None, "s", vec![], 1), RetryPolicy { retries: 0, base_ms: 1 }, &spent, rx, &|_| Ok(()))
                .await
        };
        let (r, _) = tokio::join!(h, async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).unwrap();
        });
        assert_eq!(r.unwrap_err(), CallError::Cancelled);
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
