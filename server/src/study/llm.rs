//! OpenRouter chat-completions client for the study engine (vision extraction, merge, exam writing).
//!
//! One request = system prompt + user content parts (text, images as base64 data URLs, PDFs as `file`
//! parts that OpenRouter parses natively). JSON output is requested through `response_format`
//! (`json_schema`, strict); if the provider rejects the schema the call is repeated once without it and
//! the JSON is pulled out of the text. Usage and the real `usage.cost` (USD) come back with every reply.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::future::BoxFuture;
use serde_json::{json, Value};

/// One piece of user content.
#[derive(Clone, Debug)]
pub enum Part {
    Text(String),
    /// `mime` is `image/jpeg` or `image/png`.
    Image { mime: String, data: Vec<u8> },
    Pdf { name: String, data: Vec<u8> },
}

#[derive(Clone, Debug)]
pub struct LlmRequest {
    /// What the call is for (`extract`, `merge`, `exam`, `repair`); used by logs and the fake.
    pub tag: &'static str,
    pub model: String,
    /// `low|medium|high|xhigh|max`; `None` or `none` = no reasoning.
    pub effort: Option<String>,
    pub system: String,
    pub parts: Vec<Part>,
    /// `(name, JSON schema)` for structured output.
    pub schema: Option<(String, Value)>,
    pub max_tokens: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.reasoning_tokens += o.reasoning_tokens;
        self.cost_usd += o.cost_usd;
    }
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub text: String,
    pub usage: Usage,
    /// Provider's `finish_reason` (`stop`, `length`, …).
    pub finish: String,
    pub model: String,
    pub elapsed_ms: u64,
}

impl LlmResponse {
    /// Parse the reply as JSON (tolerates code fences and prose around one object).
    pub fn json(&self) -> Result<Value, LlmError> {
        parse_json_loose(&self.text).ok_or_else(|| LlmError {
            status: 0,
            message: format!(
                "reply is not JSON (finish={}): {}",
                self.finish,
                self.text.chars().take(200).collect::<String>()
            ),
            retryable: true,
        })
    }
}

#[derive(Clone, Debug)]
pub struct LlmError {
    /// HTTP status, 0 for network/parse errors.
    pub status: u16,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "llm error {}: {}", self.status, self.message)
    }
}

pub trait Llm: Send + Sync {
    fn complete<'a>(&'a self, req: LlmRequest) -> BoxFuture<'a, Result<LlmResponse, LlmError>>;
}

/// Pull one JSON object out of model text: plain, fenced (```json … ```), or embedded in prose.
pub fn parse_json_loose(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if v.is_object() || v.is_array() {
            return Some(v);
        }
    }
    let stripped = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|s| s.trim_end().trim_end_matches("```").trim());
    if let Some(s) = stripped {
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            return Some(v);
        }
    }
    let (a, b) = (t.find('{')?, t.rfind('}')?);
    (b > a).then(|| serde_json::from_str::<Value>(&t[a..=b]).ok()).flatten()
}

// ---------------------------------------------------------------- OpenRouter

pub struct OpenRouter {
    client: reqwest::Client,
    key: String,
    base_url: String,
}

impl OpenRouter {
    pub fn new(key: String, base_url: Option<String>, timeout: Duration) -> Self {
        let client = reqwest::Client::builder().timeout(timeout).build().expect("http client");
        let base_url = base_url
            .unwrap_or_else(|| "https://openrouter.ai/api/v1".into())
            .trim_end_matches('/')
            .to_string();
        OpenRouter { client, key, base_url }
    }

    /// The JSON body sent to `/chat/completions` (public for tests).
    pub fn body(req: &LlmRequest, with_schema: bool) -> Value {
        let b64 = base64::engine::general_purpose::STANDARD;
        let content: Vec<Value> = req
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => json!({"type": "text", "text": t}),
                Part::Image { mime, data } => json!({"type": "image_url",
                    "image_url": {"url": format!("data:{mime};base64,{}", b64.encode(data))}}),
                Part::Pdf { name, data } => json!({"type": "file",
                    "file": {"filename": name, "file_data": format!("data:application/pdf;base64,{}", b64.encode(data))}}),
            })
            .collect();
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": content},
            ],
            "usage": {"include": true},
        });
        match req.effort.as_deref().map(str::trim) {
            None | Some("") | Some("none") | Some("off") => {}
            Some(e) => body["reasoning"] = json!({"effort": e, "exclude": true}),
        }
        if req.parts.iter().any(|p| matches!(p, Part::Pdf { .. })) {
            // Let the model read the PDF pages itself (text + images), not an OCR text dump.
            body["plugins"] = json!([{"id": "file-parser", "pdf": {"engine": "native"}}]);
        }
        if with_schema {
            if let Some((name, schema)) = &req.schema {
                body["response_format"] = json!({"type": "json_schema",
                    "json_schema": {"name": name, "strict": true, "schema": schema}});
            }
        }
        body
    }

    async fn send(&self, req: &LlmRequest, with_schema: bool) -> Result<LlmResponse, LlmError> {
        let started = Instant::now();
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.key)
            .header("X-Title", "Beluga Live")
            .json(&Self::body(req, with_schema))
            .send()
            .await
            .map_err(|e| LlmError { status: 0, message: e.to_string(), retryable: true })?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| LlmError { status: 0, message: e.to_string(), retryable: true })?;
        if !(200..300).contains(&status) {
            return Err(LlmError {
                status,
                message: text.chars().take(400).collect(),
                retryable: status == 408 || status == 429 || status >= 500,
            });
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| LlmError { status: 0, message: format!("bad response json: {e}"), retryable: true })?;
        parse_completion(&v, started.elapsed())
    }
}

/// Turn an OpenAI-style completion (OpenRouter flavour) into an [`LlmResponse`].
pub fn parse_completion(v: &Value, elapsed: Duration) -> Result<LlmResponse, LlmError> {
    if let Some(err) = v.get("error") {
        let code = err["code"].as_u64().unwrap_or(0) as u16;
        return Err(LlmError {
            status: code,
            message: err["message"].as_str().unwrap_or("provider error").to_string(),
            retryable: code == 0 || code == 429 || code >= 500,
        });
    }
    let choice = &v["choices"][0];
    let msg = &choice["message"];
    let text = match &msg["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
        _ => String::new(),
    };
    let u = &v["usage"];
    let usage = Usage {
        input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
        reasoning_tokens: u["completion_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0),
        cost_usd: u["cost"].as_f64().unwrap_or(0.0),
    };
    Ok(LlmResponse {
        text,
        usage,
        finish: choice["finish_reason"].as_str().unwrap_or("").to_string(),
        model: v["model"].as_str().unwrap_or("").to_string(),
        elapsed_ms: elapsed.as_millis() as u64,
    })
}

impl Llm for OpenRouter {
    fn complete<'a>(&'a self, req: LlmRequest) -> BoxFuture<'a, Result<LlmResponse, LlmError>> {
        Box::pin(async move {
            match self.send(&req, true).await {
                // Strict schema rejected (unsupported keyword, provider without structured output):
                // the prompt also describes the shape, so retry once without it.
                Err(e) if req.schema.is_some() && e.status == 400 => {
                    tracing::warn!(tag = req.tag, "structured output rejected, retrying without schema: {}", e.message);
                    self.send(&req, false).await
                }
                r => r,
            }
        })
    }
}

// ---------------------------------------------------------------- fake

type Handler = dyn Fn(&LlmRequest, usize) -> Result<String, LlmError> + Send + Sync;

/// Test double. The handler gets the request and the 0-based call index for its tag and returns the
/// reply text (or an error). Every call costs `cost_each` USD.
pub struct FakeLlm {
    handler: Box<Handler>,
    pub cost_each: f64,
    pub delay: Duration,
    pub calls: Mutex<Vec<LlmRequest>>,
}

impl FakeLlm {
    pub fn new(handler: impl Fn(&LlmRequest, usize) -> Result<String, LlmError> + Send + Sync + 'static) -> Self {
        FakeLlm { handler: Box::new(handler), cost_each: 0.001, delay: Duration::ZERO, calls: Mutex::new(Vec::new()) }
    }

    pub fn calls_with_tag(&self, tag: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|r| r.tag == tag).count()
    }
}

impl Llm for FakeLlm {
    fn complete<'a>(&'a self, req: LlmRequest) -> BoxFuture<'a, Result<LlmResponse, LlmError>> {
        Box::pin(async move {
            let n = {
                let mut c = self.calls.lock().unwrap();
                let n = c.iter().filter(|r| r.tag == req.tag).count();
                c.push(req.clone());
                n
            };
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let text = (self.handler)(&req, n)?;
            Ok(LlmResponse {
                text,
                usage: Usage { input_tokens: 100, output_tokens: 50, reasoning_tokens: 0, cost_usd: self.cost_each },
                finish: "stop".into(),
                model: req.model.clone(),
                elapsed_ms: self.delay.as_millis() as u64,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use std::sync::Arc;

    fn req(parts: Vec<Part>, schema: bool) -> LlmRequest {
        LlmRequest {
            tag: "extract",
            model: "anthropic/claude-haiku-5.5".into(),
            effort: Some("high".into()),
            system: "sys".into(),
            parts,
            schema: schema.then(|| ("x".to_string(), json!({"type": "object"}))),
            max_tokens: 100,
        }
    }

    #[test]
    fn body_has_images_pdf_reasoning_and_schema() {
        let r = req(
            vec![
                Part::Text("hi".into()),
                Part::Image { mime: "image/png".into(), data: vec![1, 2, 3] },
                Part::Pdf { name: "a.pdf".into(), data: b"%PDF".to_vec() },
            ],
            true,
        );
        let b = OpenRouter::body(&r, true);
        let c = &b["messages"][1]["content"];
        assert_eq!(c[1]["image_url"]["url"], "data:image/png;base64,AQID");
        assert_eq!(c[2]["type"], "file");
        assert_eq!(c[2]["file"]["file_data"], "data:application/pdf;base64,JVBERg==");
        assert_eq!(b["reasoning"]["effort"], "high");
        assert_eq!(b["plugins"][0]["pdf"]["engine"], "native");
        assert_eq!(b["response_format"]["json_schema"]["strict"], true);
        assert!(OpenRouter::body(&r, false).get("response_format").is_none());
        let mut r2 = req(vec![Part::Text("x".into())], false);
        r2.effort = Some("none".into());
        let b2 = OpenRouter::body(&r2, true);
        assert!(b2.get("reasoning").is_none() && b2.get("plugins").is_none() && b2.get("response_format").is_none());
    }

    #[test]
    fn loose_json() {
        assert_eq!(parse_json_loose("{\"a\":1}").unwrap()["a"], 1);
        assert_eq!(parse_json_loose("```json\n{\"a\":2}\n```").unwrap()["a"], 2);
        assert_eq!(parse_json_loose("Here: {\"a\":3} done").unwrap()["a"], 3);
        assert!(parse_json_loose("nope").is_none());
    }

    #[tokio::test]
    async fn openrouter_against_fake_service_parses_usage_and_falls_back_without_schema() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let app = Router::new().route(
            "/api/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let s2 = s2.clone();
                async move {
                    let had_schema = body.get("response_format").is_some();
                    s2.lock().unwrap().push(body);
                    if had_schema {
                        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "schema"}})));
                    }
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"model": "m", "choices": [{"finish_reason": "stop",
                            "message": {"content": "```json\n{\"ok\":true}\n```"}}],
                            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "cost": 0.0123,
                                      "completion_tokens_details": {"reasoning_tokens": 2}}})),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let or = OpenRouter::new("k".into(), Some(format!("http://{addr}/api/v1")), Duration::from_secs(5));
        let r = or.complete(req(vec![Part::Text("x".into())], true)).await.unwrap();
        assert_eq!(r.json().unwrap()["ok"], true);
        assert_eq!(r.usage, Usage { input_tokens: 10, output_tokens: 5, reasoning_tokens: 2, cost_usd: 0.0123 });
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn provider_error_in_body_is_an_error() {
        let v = json!({"error": {"code": 502, "message": "upstream"}});
        let e = parse_completion(&v, Duration::ZERO).unwrap_err();
        assert!(e.retryable && e.status == 502);
    }
}
