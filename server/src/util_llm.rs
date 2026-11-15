//! The "utility model": a cheap, fast LLM for small server-side jobs (STT correction, session
//! titles, compaction summaries, weekly summaries, greetings). Never the tutor itself.
//!
//! Primary: DeepSeek's OpenAI-compatible `/chat/completions` (`deepseek-flash`, thinking disabled:
//! about 1 s for a short answer, measured 15/11/2026). Fallback: OpenRouter (same API shape).
//! Behind a trait so tests use [`FakeUtil`].

use crate::usage::{Usage, UsageRec};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct UtilReq {
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
    /// Short tag for logs and the usage journal (`stt_fix`, `title`, ...).
    pub purpose: &'static str,
}

impl UtilReq {
    pub fn new(purpose: &'static str, system: impl Into<String>, user: impl Into<String>, max_tokens: u32) -> UtilReq {
        UtilReq { system: system.into(), user: user.into(), max_tokens, purpose }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UtilOut {
    pub text: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

pub trait UtilLlm: Send + Sync {
    fn complete<'a>(&'a self, req: &'a UtilReq) -> BoxFuture<'a, Result<UtilOut, String>>;
    /// False when no provider is configured (callers then skip the job and use their fallback).
    fn available(&self) -> bool {
        true
    }
}

/// An OpenAI-compatible chat completions endpoint.
pub struct OpenAiCompat {
    client: reqwest::Client,
    pub name: &'static str,
    base_url: String,
    key: String,
    pub model: String,
    /// USD per million tokens, used when the response carries no cost.
    price_in: f64,
    price_out: f64,
    /// Provider-specific body fields (turn reasoning off).
    extra: Value,
}

impl OpenAiCompat {
    pub fn deepseek(key: String, model: Option<String>) -> OpenAiCompat {
        OpenAiCompat {
            client: reqwest::Client::new(),
            name: "deepseek",
            base_url: "https://api.deepseek.com".into(),
            key,
            model: model.unwrap_or_else(|| "deepseek-flash".into()),
            price_in: 0.05,
            price_out: 0.2,
            extra: json!({"thinking": {"type": "disabled"}}),
        }
    }

    pub fn openrouter(key: String, model: Option<String>) -> OpenAiCompat {
        OpenAiCompat {
            client: reqwest::Client::new(),
            name: "openrouter",
            base_url: "https://openrouter.ai/api/v1".into(),
            key,
            model: model.unwrap_or_else(|| "deepseek/deepseek-v4.1-flash".into()),
            price_in: 0.05,
            price_out: 0.2,
            extra: json!({"reasoning": {"enabled": false}, "usage": {"include": true}}),
        }
    }

    /// Any endpoint (tests point this at a local fake).
    pub fn custom(name: &'static str, base_url: String, key: String, model: String) -> OpenAiCompat {
        OpenAiCompat { client: reqwest::Client::new(), name, base_url, key, model, price_in: 0.0, price_out: 0.0, extra: json!({}) }
    }
}

impl UtilLlm for OpenAiCompat {
    fn complete<'a>(&'a self, req: &'a UtilReq) -> BoxFuture<'a, Result<UtilOut, String>> {
        Box::pin(async move {
            let mut body = json!({
                "model": self.model,
                "messages": [{"role": "system", "content": req.system}, {"role": "user", "content": req.user}],
                "max_tokens": req.max_tokens,
                "temperature": 0.3,
                "stream": false,
            });
            if let (Some(b), Some(e)) = (body.as_object_mut(), self.extra.as_object()) {
                for (k, v) in e {
                    b.insert(k.clone(), v.clone());
                }
            }
            let resp = self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.key)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("{}: network: {}", self.name, e.without_url()))?;
            let status = resp.status().as_u16();
            let v: Value = resp.json().await.map_err(|e| format!("{}: bad body: {}", self.name, e.without_url()))?;
            if status >= 400 {
                let msg: String = v["error"]["message"].as_str().unwrap_or("").chars().take(160).collect();
                return Err(format!("{}: http {status}: {msg}", self.name));
            }
            let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
            if text.is_empty() {
                return Err(format!("{}: empty answer", self.name));
            }
            let input_tokens = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            let output_tokens = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            let cost_usd = v["usage"]["cost"]
                .as_f64()
                .unwrap_or((input_tokens as f64 * self.price_in + output_tokens as f64 * self.price_out) / 1e6);
            Ok(UtilOut { text, model: format!("{}/{}", self.name, self.model), input_tokens, output_tokens, cost_usd })
        })
    }
}

/// Try each provider in order.
pub struct Chain(pub Vec<Arc<dyn UtilLlm>>);

impl UtilLlm for Chain {
    fn complete<'a>(&'a self, req: &'a UtilReq) -> BoxFuture<'a, Result<UtilOut, String>> {
        Box::pin(async move {
            let mut last = "no utility model configured".to_string();
            for p in &self.0 {
                match p.complete(req).await {
                    Ok(o) => return Ok(o),
                    Err(e) => {
                        tracing::warn!("util {}: {e}", req.purpose);
                        last = e;
                    }
                }
            }
            Err(last)
        })
    }

    fn available(&self) -> bool {
        !self.0.is_empty()
    }
}

/// No provider: every job uses its built-in fallback.
pub struct NoUtil;

impl UtilLlm for NoUtil {
    fn complete<'a>(&'a self, _req: &'a UtilReq) -> BoxFuture<'a, Result<UtilOut, String>> {
        Box::pin(async { Err("no utility model configured".to_string()) })
    }
    fn available(&self) -> bool {
        false
    }
}

type FakeFn = dyn Fn(&UtilReq) -> Result<String, String> + Send + Sync;

/// Test double: answers with a closure after an optional delay.
pub struct FakeUtil {
    pub answer: Box<FakeFn>,
    pub delay_ms: u64,
}

impl FakeUtil {
    pub fn new(f: impl Fn(&UtilReq) -> Result<String, String> + Send + Sync + 'static) -> Arc<dyn UtilLlm> {
        Arc::new(FakeUtil { answer: Box::new(f), delay_ms: 0 })
    }

    pub fn slow(delay_ms: u64, f: impl Fn(&UtilReq) -> Result<String, String> + Send + Sync + 'static) -> Arc<dyn UtilLlm> {
        Arc::new(FakeUtil { answer: Box::new(f), delay_ms })
    }
}

impl UtilLlm for FakeUtil {
    fn complete<'a>(&'a self, req: &'a UtilReq) -> BoxFuture<'a, Result<UtilOut, String>> {
        Box::pin(async move {
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            (self.answer)(req).map(|text| UtilOut { text, model: "fake".into(), input_tokens: 10, output_tokens: 5, cost_usd: 0.00001 })
        })
    }
}

/// Providers from the keys in `server/.env` (DeepSeek first, then OpenRouter).
/// `BELUGA_UTIL_MODEL_DEEPSEEK` / `BELUGA_UTIL_MODEL_OPENROUTER` override the model names.
pub fn from_keys(get: impl Fn(&str) -> Option<String>) -> Arc<dyn UtilLlm> {
    let mut v: Vec<Arc<dyn UtilLlm>> = Vec::new();
    if let Some(k) = get("DEEPSEEK_API_KEY") {
        v.push(Arc::new(OpenAiCompat::deepseek(k, get("BELUGA_UTIL_MODEL_DEEPSEEK"))));
    }
    if let Some(k) = get("OPENROUTER_API_KEY") {
        v.push(Arc::new(OpenAiCompat::openrouter(k, get("BELUGA_UTIL_MODEL_OPENROUTER"))));
    }
    if v.is_empty() {
        Arc::new(NoUtil)
    } else {
        Arc::new(Chain(v))
    }
}

/// Run one job with a hard deadline and record its cost (`kind:"util"`) in the usage journal.
pub async fn ask(llm: &Arc<dyn UtilLlm>, usage: &Usage, req: UtilReq, timeout: Duration) -> Result<String, String> {
    if !llm.available() {
        return Err("no utility model configured".into());
    }
    let out = match tokio::time::timeout(timeout, llm.complete(&req)).await {
        Ok(r) => r?,
        Err(_) => return Err(format!("util {}: timed out after {} ms", req.purpose, timeout.as_millis())),
    };
    usage.record(&UsageRec {
        kind: "util".into(),
        model_id: out.model.clone(),
        provider: "util".into(),
        tracked: false,
        input_tokens: out.input_tokens,
        output_tokens: out.output_tokens,
        cost_usd: out.cost_usd,
        note: Some(req.purpose.to_string()),
        ..Default::default()
    });
    Ok(out.text)
}

/// First `{...}` object in a model answer (models sometimes wrap JSON in prose or fences).
pub fn extract_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| serde_json::from_str::<Value>(&text[start..=end]).ok()).flatten()
}

/// Strip quotes/fences a model may put around a one-line answer.
pub fn one_line(text: &str) -> String {
    let t = text.trim().trim_start_matches("```").trim_end_matches("```").trim();
    let t = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    t.trim_matches(|c| c == '"' || c == '“' || c == '”' || c == '\'' || c == '`').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_line_helpers() {
        assert_eq!(extract_json("Here: ```json\n{\"a\":1}\n```").unwrap()["a"], 1);
        assert!(extract_json("no json").is_none());
        assert_eq!(one_line("\"Toán · Căn bậc hai\"\n"), "Toán · Căn bậc hai");
        assert_eq!(one_line("```\nabc\n```"), "abc");
    }

    #[tokio::test]
    async fn chain_falls_back_and_ask_times_out() {
        let bad = FakeUtil::new(|_| Err("down".into()));
        let good = FakeUtil::new(|r| Ok(format!("ok {}", r.purpose)));
        let chain: Arc<dyn UtilLlm> = Arc::new(Chain(vec![bad, good]));
        let dir = std::env::temp_dir().join(format!("beluga-util-{}", uuid::Uuid::new_v4()));
        let usage = Usage::open(&dir);
        let r = ask(&chain, &usage, UtilReq::new("title", "s", "u", 10), Duration::from_secs(1)).await;
        assert_eq!(r.unwrap(), "ok title");
        let slow = FakeUtil::slow(300, |_| Ok("late".into()));
        let r = ask(&slow, &usage, UtilReq::new("stt_fix", "s", "u", 10), Duration::from_millis(50)).await;
        assert!(r.unwrap_err().contains("timed out"));
        assert!(ask(&(Arc::new(NoUtil) as Arc<dyn UtilLlm>), &usage, UtilReq::new("x", "", "", 1), Duration::from_secs(1)).await.is_err());
        let rep = usage.report(None);
        assert!(rep["other"]["today"]["costUsd"].as_f64().unwrap() > 0.0);
    }
}
