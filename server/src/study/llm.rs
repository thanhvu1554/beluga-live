//! Model clients for the study engine (page reading, merge, exam writing).
//!
//! - [`OpenRouter`]: OpenAI-style `/chat/completions`; images as data URLs, PDFs as `file` parts (parsed
//!   natively), strict `json_schema` output (repeated once without it if the provider rejects it), real
//!   `usage.cost`.
//! - [`DeepSeek`]: `https://api.deepseek.com/chat/completions` (`deepseek-flash` reads images); JSON via
//!   `response_format: json_object` (the API has no `json_schema`), thinking off unless an effort is
//!   given, automatic prefix caching (`prompt_cache_hit_tokens`). The API returns no cost, so it is
//!   computed from [`Price`] (estimate, see `NOTES.md`). PDFs are refused here (non-retryable) so a
//!   [`Failover`] moves them on.
//! - [`Failover`]: one model on several endpoints (DeepSeek direct ↔ OpenRouter). Every attempt has its
//!   own timeout; an endpoint that fails is skipped for a cool-down so one flaky provider does not slow
//!   every call down.
//! - [`Providers`]: picks the [`Failover`] for a model id (`deepseek-*` → DeepSeek then OpenRouter,
//!   anything else → OpenRouter).
//! - [`FakeLlm`]: test double.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;

/// One piece of user content.
#[derive(Clone, Debug)]
pub enum Part {
    Text(String),
    /// `mime` is `image/jpeg`, `image/png` or `image/webp`.
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
    /// Hard limit for one attempt on one endpoint (`None`: the client's default).
    pub timeout: Option<Duration>,
    /// Sampling temperature; only sent without reasoning (Anthropic needs 1 when thinking).
    pub temperature: Option<f32>,
}

impl LlmRequest {
    pub fn new(tag: &'static str, model: &str, effort: Option<String>, system: &str, parts: Vec<Part>, max_tokens: u32) -> Self {
        LlmRequest {
            tag,
            model: model.to_string(),
            effort,
            system: system.to_string(),
            parts,
            schema: None,
            max_tokens,
            timeout: None,
            temperature: None,
        }
    }

    pub fn reasoning(&self) -> bool {
        !matches!(self.effort.as_deref().map(str::trim), None | Some("") | Some("none") | Some("off"))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Input tokens served from the provider's prompt cache.
    pub cached_tokens: u64,
    pub cost_usd: f64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.reasoning_tokens += o.reasoning_tokens;
        self.cached_tokens += o.cached_tokens;
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
    /// Endpoint that answered (`openrouter`, `deepseek`, `fake`).
    pub provider: String,
}

impl LlmResponse {
    /// Parse the reply as JSON (tolerates code fences and prose around one object).
    pub fn json(&self) -> Result<Value, LlmError> {
        if self.finish == "length" {
            return Err(LlmError { status: 0, message: "reply cut at max_tokens".into(), retryable: true });
        }
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
    /// HTTP status, 0 for network/parse errors, 408 for our own timeout.
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

/// Models sometimes write LaTeX into JSON strings without doubling the backslash: `\sqrt` is then
/// invalid JSON and `\frac` silently becomes a form feed + "rac". Inside strings, a backslash that
/// starts a LaTeX command (or any invalid escape) is doubled; real escapes (`\"`, `\\`, `\n` before a
/// non-command, `\u…`) are kept.
pub fn fix_latex_escapes(s: &str) -> String {
    const CMDS: [&str; 26] = [
        "frac", "beta", "bar", "begin", "big", "boxed", "binom", "neq", "ne ", "nu", "not", "nabla", "times", "text", "tan", "theta", "to",
        "tfrac", "tau", "right", "rho", "rangle", "rceil", "rfloor", "forall", "bf",
    ];
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut in_str = false;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if !in_str {
            if ch == '"' {
                in_str = true;
            }
            out.push(ch);
            i += 1;
            continue;
        }
        match ch {
            '"' => {
                in_str = false;
                out.push(ch);
                i += 1;
            }
            '\\' => {
                let next = c.get(i + 1).copied().unwrap_or(' ');
                let rest: String = c[i + 1..(i + 9).min(c.len())].iter().collect();
                let latex = match next {
                    '"' | '\\' | '/' => false,
                    // `\u` + 4 hex digits is a real escape (indexing by char: `rest` may hold non-ASCII).
                    'u' => !(i + 6 <= c.len() && c[i + 2..i + 6].iter().all(|x| x.is_ascii_hexdigit())),
                    'b' | 'f' | 'n' | 'r' | 't' => {
                        CMDS.iter().any(|cmd| rest.starts_with(cmd))
                            || (matches!(next, 'b' | 'f') && c.get(i + 2).is_some_and(|x| x.is_ascii_alphabetic()))
                    }
                    _ => true,
                };
                if latex {
                    out.push_str("\\\\");
                    i += 1;
                } else {
                    out.push('\\');
                    out.push(next);
                    i += 2;
                }
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// Pull one JSON object out of model text: plain, fenced (```json … ```), or embedded in prose. LaTeX
/// backslashes the model forgot to escape are repaired first.
pub fn parse_json_loose(text: &str) -> Option<Value> {
    let fixed = fix_latex_escapes(text);
    let t = fixed.trim();
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

fn content_parts(req: &LlmRequest, allow_pdf: bool) -> Result<Vec<Value>, LlmError> {
    let b64 = base64::engine::general_purpose::STANDARD;
    req.parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => Ok(json!({"type": "text", "text": t})),
            Part::Image { mime, data } => Ok(json!({"type": "image_url",
                "image_url": {"url": format!("data:{mime};base64,{}", b64.encode(data))}})),
            Part::Pdf { name, data } if allow_pdf => Ok(json!({"type": "file",
                "file": {"filename": name, "file_data": format!("data:application/pdf;base64,{}", b64.encode(data))}})),
            Part::Pdf { .. } => Err(LlmError { status: 415, message: "endpoint cannot read PDF".into(), retryable: false }),
        })
        .collect()
}

fn http_error(status: u16, body: &str) -> LlmError {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(300).collect());
    LlmError { status, message: msg.chars().take(300).collect(), retryable: status == 408 || status == 429 || status >= 500 }
}

async fn post_json(client: &reqwest::Client, url: &str, key: &str, body: &Value) -> Result<(Value, Duration), LlmError> {
    let started = Instant::now();
    let resp = client
        .post(url)
        .bearer_auth(key)
        .header("X-Title", "Beluga Live")
        .json(body)
        .send()
        .await
        .map_err(|e| LlmError { status: 0, message: e.without_url().to_string(), retryable: true })?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| LlmError { status: 0, message: e.without_url().to_string(), retryable: true })?;
    if !(200..300).contains(&status) {
        return Err(http_error(status, &text));
    }
    let v: Value =
        serde_json::from_str(&text).map_err(|e| LlmError { status: 0, message: format!("bad response json: {e}"), retryable: true })?;
    Ok((v, started.elapsed()))
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
        let content = content_parts(req, true).unwrap_or_default();
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": content},
            ],
            "usage": {"include": true},
        });
        if req.reasoning() {
            body["reasoning"] = json!({"effort": req.effort.as_deref().unwrap_or("medium").trim(), "exclude": true});
        } else {
            // Some models (DeepSeek on OpenRouter) think by default: say no explicitly.
            body["reasoning"] = json!({"enabled": false});
            if let Some(t) = req.temperature {
                body["temperature"] = json!(t);
            }
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
        let (v, elapsed) = post_json(&self.client, &format!("{}/chat/completions", self.base_url), &self.key, &Self::body(req, with_schema)).await?;
        let mut r = parse_completion(&v, elapsed)?;
        r.provider = "openrouter".into();
        Ok(r)
    }
}

/// Turn an OpenAI-style completion (OpenRouter or DeepSeek flavour) into an [`LlmResponse`].
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
        cached_tokens: u["prompt_cache_hit_tokens"]
            .as_u64()
            .or_else(|| u["prompt_tokens_details"]["cached_tokens"].as_u64())
            .unwrap_or(0),
        cost_usd: u["cost"].as_f64().unwrap_or(0.0),
    };
    Ok(LlmResponse {
        text,
        usage,
        finish: choice["finish_reason"].as_str().unwrap_or("").to_string(),
        model: v["model"].as_str().unwrap_or("").to_string(),
        elapsed_ms: elapsed.as_millis() as u64,
        provider: String::new(),
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

// ---------------------------------------------------------------- DeepSeek

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
}

impl Price {
    /// `deepseek-flash` estimate (the direct API returns no cost): OpenRouter's listed price for the same
    /// model (0,0507 / 0,2028 USD per million), cache hits at a tenth of the input price.
    pub const DEEPSEEK_FLASH: Price = Price { input: 0.05, cached: 0.005, output: 0.2 };

    pub fn cost(&self, u: &Usage) -> f64 {
        let miss = u.input_tokens.saturating_sub(u.cached_tokens) as f64;
        (miss * self.input + u.cached_tokens as f64 * self.cached + u.output_tokens as f64 * self.output) / 1e6
    }

    /// `in,cached,out` (e.g. from `BELUGA_DEEPSEEK_PRICE`).
    pub fn parse(s: &str) -> Option<Price> {
        let v: Vec<f64> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 3).then(|| Price { input: v[0], cached: v[1], output: v[2] })
    }
}

pub struct DeepSeek {
    client: reqwest::Client,
    key: String,
    base_url: String,
    pub price: Price,
}

impl DeepSeek {
    pub fn new(key: String, base_url: Option<String>, timeout: Duration, price: Price) -> Self {
        let client = reqwest::Client::builder().timeout(timeout).build().expect("http client");
        let base_url = base_url.unwrap_or_else(|| "https://api.deepseek.com".into()).trim_end_matches('/').to_string();
        DeepSeek { client, key, base_url, price }
    }

    /// The JSON body sent to `/chat/completions` (public for tests). Errors on PDF parts.
    pub fn body(req: &LlmRequest) -> Result<Value, LlmError> {
        let content = content_parts(req, false)?;
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "messages": [{"role": "system", "content": req.system}, {"role": "user", "content": content}],
            "stream": false,
            "thinking": {"type": if req.reasoning() { "enabled" } else { "disabled" }},
        });
        if !req.reasoning() {
            if let Some(t) = req.temperature {
                body["temperature"] = json!(t);
            }
        }
        if req.schema.is_some() {
            // No json_schema on this API; the system prompt describes the shape and says "JSON".
            body["response_format"] = json!({"type": "json_object"});
        }
        Ok(body)
    }
}

impl Llm for DeepSeek {
    fn complete<'a>(&'a self, req: LlmRequest) -> BoxFuture<'a, Result<LlmResponse, LlmError>> {
        Box::pin(async move {
            let body = Self::body(&req)?;
            let (v, elapsed) = post_json(&self.client, &format!("{}/chat/completions", self.base_url), &self.key, &body).await?;
            let mut r = parse_completion(&v, elapsed)?;
            if r.usage.cost_usd == 0.0 {
                r.usage.cost_usd = self.price.cost(&r.usage);
            }
            r.provider = "deepseek".into();
            Ok(r)
        })
    }
}

// ---------------------------------------------------------------- failover + routing

/// One way to reach a model: a client and the model id that client knows it by.
pub struct Endpoint {
    pub name: String,
    pub llm: Arc<dyn Llm>,
    pub model: String,
}

/// The same model on several endpoints, tried in order. Each attempt gets `req.timeout` (or the
/// default); an endpoint that errs (timeout, 429, 5xx, network, auth) is put behind the others for
/// `cooldown`. The error of the last endpoint is returned; it is retryable if any endpoint's was.
pub struct Failover {
    endpoints: Vec<Endpoint>,
    down_until: Mutex<Vec<Option<Instant>>>,
    pub cooldown: Duration,
    pub default_timeout: Duration,
}

impl Failover {
    pub fn new(endpoints: Vec<Endpoint>, default_timeout: Duration) -> Self {
        let n = endpoints.len();
        Failover { endpoints, down_until: Mutex::new(vec![None; n]), cooldown: Duration::from_secs(60), default_timeout }
    }

    pub fn names(&self) -> Vec<String> {
        self.endpoints.iter().map(|e| e.name.clone()).collect()
    }

    fn order(&self) -> Vec<usize> {
        let now = Instant::now();
        let down = self.down_until.lock().unwrap();
        let mut up: Vec<usize> = (0..self.endpoints.len()).filter(|i| down[*i].is_none_or(|t| t <= now)).collect();
        let mut rest: Vec<usize> = (0..self.endpoints.len()).filter(|i| !up.contains(i)).collect();
        // The one that comes back first goes first among the resting ones.
        rest.sort_by_key(|i| down[*i]);
        up.extend(rest);
        up
    }
}

impl Llm for Failover {
    fn complete<'a>(&'a self, req: LlmRequest) -> BoxFuture<'a, Result<LlmResponse, LlmError>> {
        Box::pin(async move {
            let mut last = LlmError { status: 0, message: "no endpoint".into(), retryable: false };
            let mut any_retryable = false;
            for i in self.order() {
                let ep = &self.endpoints[i];
                let mut r = req.clone();
                r.model = ep.model.clone();
                let limit = req.timeout.unwrap_or(self.default_timeout);
                let res = match tokio::time::timeout(limit, ep.llm.complete(r)).await {
                    Ok(x) => x,
                    Err(_) => Err(LlmError { status: 408, message: format!("timed out after {} s", limit.as_secs()), retryable: true }),
                };
                match res {
                    Ok(mut resp) => {
                        self.down_until.lock().unwrap()[i] = None;
                        if resp.provider.is_empty() {
                            resp.provider = ep.name.clone();
                        }
                        return Ok(resp);
                    }
                    Err(e) => {
                        tracing::warn!(tag = req.tag, endpoint = %ep.name, "llm endpoint failed: {e}");
                        // A request this endpoint cannot take (PDF on DeepSeek) says nothing about its health.
                        if e.status != 415 {
                            self.down_until.lock().unwrap()[i] = Some(Instant::now() + self.cooldown);
                        }
                        any_retryable |= e.retryable;
                        last = e;
                    }
                }
            }
            last.retryable = any_retryable;
            Err(last)
        })
    }
}

/// OpenRouter id of a DeepSeek-direct model and back.
fn openrouter_id(deepseek_model: &str) -> Option<&'static str> {
    match deepseek_model {
        "deepseek-flash" => Some("deepseek/deepseek-v4.1-flash"),
        "deepseek-v4-pro" => Some("deepseek/deepseek-v4-pro"),
        _ => None,
    }
}

fn deepseek_id(openrouter_model: &str) -> Option<&'static str> {
    match openrouter_model {
        "deepseek/deepseek-v4.1-flash" | "deepseek/deepseek-flash-latest" => Some("deepseek-flash"),
        "deepseek/deepseek-v4-pro" => Some("deepseek-v4-pro"),
        _ => None,
    }
}

/// Routes each request by model id to a [`Failover`] (built lazily, cached per model).
pub struct Providers {
    deepseek: Option<Arc<dyn Llm>>,
    openrouter: Option<Arc<dyn Llm>>,
    default_timeout: Duration,
    routes: Mutex<Vec<(String, Arc<Failover>)>>,
}

impl Providers {
    pub fn new(deepseek: Option<Arc<dyn Llm>>, openrouter: Option<Arc<dyn Llm>>, default_timeout: Duration) -> Self {
        Providers { deepseek, openrouter, default_timeout, routes: Mutex::new(Vec::new()) }
    }

    pub fn is_empty(&self) -> bool {
        self.deepseek.is_none() && self.openrouter.is_none()
    }

    /// Endpoints for a model id, in preference order.
    pub fn endpoints(&self, model: &str) -> Vec<Endpoint> {
        let mut v = Vec::new();
        let ds = |m: &str| self.deepseek.clone().map(|l| Endpoint { name: "deepseek".into(), llm: l, model: m.to_string() });
        let or = |m: &str| self.openrouter.clone().map(|l| Endpoint { name: "openrouter".into(), llm: l, model: m.to_string() });
        if !model.contains('/') {
            // DeepSeek's own id: direct first, OpenRouter as the spare.
            v.extend(ds(model));
            v.extend(openrouter_id(model).and_then(or));
        } else {
            v.extend(or(model));
            v.extend(deepseek_id(model).and_then(ds));
        }
        v
    }

    pub fn route(&self, model: &str) -> Arc<Failover> {
        let mut routes = self.routes.lock().unwrap();
        if let Some((_, f)) = routes.iter().find(|(m, _)| m == model) {
            return f.clone();
        }
        let f = Arc::new(Failover::new(self.endpoints(model), self.default_timeout));
        routes.push((model.to_string(), f.clone()));
        f
    }
}

impl Llm for Providers {
    fn complete<'a>(&'a self, req: LlmRequest) -> BoxFuture<'a, Result<LlmResponse, LlmError>> {
        Box::pin(async move {
            let route = self.route(&req.model);
            if route.endpoints.is_empty() {
                return Err(LlmError { status: 401, message: format!("no API key for model {}", req.model), retryable: false });
            }
            route.complete(req).await
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

    /// Calls with this tag made for this model.
    pub fn calls_with_model(&self, tag: &str, model: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|r| r.tag == tag && r.model == model).count()
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
                usage: Usage { input_tokens: 100, output_tokens: 50, cost_usd: self.cost_each, ..Default::default() },
                finish: "stop".into(),
                model: req.model.clone(),
                elapsed_ms: self.delay.as_millis() as u64,
                provider: "fake".into(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req(parts: Vec<Part>, schema: bool) -> LlmRequest {
        let mut r = LlmRequest::new("extract", "anthropic/claude-haiku-5.5", Some("high".into()), "sys", parts, 100);
        r.schema = schema.then(|| ("x".to_string(), json!({"type": "object"})));
        r
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
        r2.temperature = Some(0.0);
        let b2 = OpenRouter::body(&r2, true);
        assert_eq!(b2["reasoning"], json!({"enabled": false}));
        assert_eq!(b2["temperature"], 0.0);
        assert!(b2.get("plugins").is_none() && b2.get("response_format").is_none());
    }

    #[test]
    fn deepseek_body_uses_json_object_thinking_switch_and_refuses_pdf() {
        let mut r = req(vec![Part::Text("x".into()), Part::Image { mime: "image/jpeg".into(), data: vec![9] }], true);
        r.model = "deepseek-flash".into();
        r.effort = None;
        r.temperature = Some(0.0);
        let b = DeepSeek::body(&r).unwrap();
        assert_eq!(b["thinking"]["type"], "disabled");
        assert_eq!(b["response_format"]["type"], "json_object");
        assert_eq!(b["messages"][1]["content"][1]["image_url"]["url"], "data:image/jpeg;base64,CQ==");
        assert_eq!(b["temperature"], 0.0);
        r.effort = Some("high".into());
        assert_eq!(DeepSeek::body(&r).unwrap()["thinking"]["type"], "enabled");
        r.parts.push(Part::Pdf { name: "a.pdf".into(), data: vec![1] });
        assert_eq!(DeepSeek::body(&r).unwrap_err().status, 415);
        let u = Usage { input_tokens: 1_000_000, cached_tokens: 400_000, output_tokens: 1_000_000, ..Default::default() };
        let p = Price::DEEPSEEK_FLASH;
        assert!((p.cost(&u) - (0.6 * 0.05 + 0.4 * 0.005 + 0.2)).abs() < 1e-9);
        assert_eq!(Price::parse("0.1, 0.01,0.4"), Some(Price { input: 0.1, cached: 0.01, output: 0.4 }));
    }

    #[test]
    fn loose_json() {
        assert_eq!(parse_json_loose("{\"a\":1}").unwrap()["a"], 1);
        assert_eq!(parse_json_loose("```json\n{\"a\":2}\n```").unwrap()["a"], 2);
        assert_eq!(parse_json_loose("Here: {\"a\":3} done").unwrap()["a"], 3);
        assert!(parse_json_loose("nope").is_none());
        // Unescaped LaTeX: invalid escapes and escapes that are really commands.
        let v = parse_json_loose(r#"{"t":"$\frac{1}{2}+\sqrt{x}\neq \times\text{a}$","ok":"a\nb \"q\" \\frac é"}"#).unwrap();
        assert_eq!(v["t"], r"$\frac{1}{2}+\sqrt{x}\neq \times\text{a}$");
        assert_eq!(v["ok"], "a\nb \"q\" \\frac é");
        // Non-ASCII right after a backslash must not panic; `é` stays a real escape.
        let v = parse_json_loose(r#"{"t":"\uée é \ứ"}"#).unwrap();
        assert_eq!(v["t"], "\\uée é \\ứ");
        let cut = LlmResponse { text: "{\"a\":1}".into(), usage: Usage::default(), finish: "length".into(), model: String::new(),
            elapsed_ms: 0, provider: String::new() };
        assert!(cut.json().unwrap_err().message.contains("max_tokens"));
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
                                      "prompt_tokens_details": {"cached_tokens": 4},
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
        assert_eq!(r.usage, Usage { input_tokens: 10, output_tokens: 5, reasoning_tokens: 2, cached_tokens: 4, cost_usd: 0.0123 });
        assert_eq!(r.provider, "openrouter");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn deepseek_against_fake_service_prices_cache_hits() {
        let app = Router::new().route(
            "/chat/completions",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["thinking"]["type"], "disabled");
                Json(json!({"model": "deepseek-flash", "choices": [{"finish_reason": "stop", "message": {"content": "{\"n\":1}"}}],
                    "usage": {"prompt_tokens": 1000, "completion_tokens": 100, "prompt_cache_hit_tokens": 800, "prompt_cache_miss_tokens": 200}}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let ds = DeepSeek::new("k".into(), Some(format!("http://{addr}")), Duration::from_secs(5), Price::DEEPSEEK_FLASH);
        let mut r = req(vec![Part::Text("x".into())], true);
        r.model = "deepseek-flash".into();
        r.effort = None;
        let resp = ds.complete(r).await.unwrap();
        assert_eq!(resp.usage.cached_tokens, 800);
        assert!((resp.usage.cost_usd - (200.0 * 0.05 + 800.0 * 0.005 + 100.0 * 0.2) / 1e6).abs() < 1e-12);
        assert_eq!(resp.provider, "deepseek");
    }

    #[test]
    fn provider_error_in_body_is_an_error() {
        let v = json!({"error": {"code": 502, "message": "upstream"}});
        let e = parse_completion(&v, Duration::ZERO).unwrap_err();
        assert!(e.retryable && e.status == 502);
    }

    fn ep(name: &str, f: FakeLlm) -> (Endpoint, Arc<FakeLlm>) {
        let f = Arc::new(f);
        (Endpoint { name: name.into(), llm: f.clone(), model: format!("{name}-model") }, f)
    }

    #[tokio::test]
    async fn failover_moves_to_the_next_endpoint_and_rests_the_failed_one() {
        let flaky = AtomicUsize::new(0);
        let (a, fa) = ep("deepseek", FakeLlm::new(move |_, _| {
            flaky.fetch_add(1, Ordering::SeqCst);
            Err(LlmError { status: 503, message: "overloaded".into(), retryable: true })
        }));
        let (b, fb) = ep("openrouter", FakeLlm::new(|r, _| Ok(format!("{{\"model\":\"{}\"}}", r.model))));
        let f = Failover::new(vec![a, b], Duration::from_secs(5));
        let r = f.complete(req(vec![], false)).await.unwrap();
        assert_eq!(r.provider, "fake");
        assert_eq!(r.json().unwrap()["model"], "openrouter-model", "the spare gets its own model id");
        // The failed endpoint now rests: the next call goes straight to the spare.
        f.complete(req(vec![], false)).await.unwrap();
        assert_eq!(fa.calls.lock().unwrap().len(), 1);
        assert_eq!(fb.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn failover_times_out_a_hanging_endpoint() {
        let mut slow = FakeLlm::new(|_, _| Ok("{}".into()));
        slow.delay = Duration::from_secs(30);
        let (a, _) = ep("deepseek", slow);
        let (b, _) = ep("openrouter", FakeLlm::new(|_, _| Ok("{\"ok\":1}".into())));
        let f = Failover::new(vec![a, b], Duration::from_secs(5));
        let mut r = req(vec![], false);
        r.timeout = Some(Duration::from_millis(50));
        let t = Instant::now();
        assert_eq!(f.complete(r).await.unwrap().json().unwrap()["ok"], 1);
        assert!(t.elapsed() < Duration::from_secs(2));
        // Both down: the error is retryable (timeout / 5xx), not a hard failure.
        let (a, _) = ep("x", FakeLlm::new(|_, _| Err(LlmError { status: 429, message: "slow".into(), retryable: true })));
        let (b, _) = ep("y", FakeLlm::new(|_, _| Err(LlmError { status: 401, message: "key".into(), retryable: false })));
        let e = Failover::new(vec![a, b], Duration::from_secs(5)).complete(req(vec![], false)).await.unwrap_err();
        assert!(e.retryable && e.status == 401);
    }

    #[test]
    fn providers_route_models_to_endpoints() {
        let ds: Arc<dyn Llm> = Arc::new(FakeLlm::new(|_, _| Ok("{}".into())));
        let or: Arc<dyn Llm> = Arc::new(FakeLlm::new(|_, _| Ok("{}".into())));
        let p = Providers::new(Some(ds.clone()), Some(or.clone()), Duration::from_secs(5));
        let e = p.endpoints("deepseek-flash");
        assert_eq!(e.iter().map(|x| (x.name.as_str(), x.model.as_str())).collect::<Vec<_>>(),
            [("deepseek", "deepseek-flash"), ("openrouter", "deepseek/deepseek-v4.1-flash")]);
        assert_eq!(p.endpoints("anthropic/claude-haiku-5.5").len(), 1);
        assert_eq!(p.endpoints("deepseek/deepseek-v4.1-flash")[1].model, "deepseek-flash");
        let only_or = Providers::new(None, Some(or), Duration::from_secs(5));
        assert_eq!(only_or.endpoints("deepseek-flash")[0].name, "openrouter");
        assert!(Providers::new(None, None, Duration::from_secs(5)).is_empty());
    }
}
