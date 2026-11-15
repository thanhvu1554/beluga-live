//! Speech providers behind small interfaces: GenVoice (preferred), ElevenLabs direct, or none
//! (the phone then listens and speaks with the system voice). Fakes exist for tests.
//!
//! Status: GenVoice TTS/STT was measured against the real service by the project owner; this code
//! was exercised against a local mock plus one real TTS run (see NOTES). ElevenLabs direct is
//! unverified beyond its error path (the free plan answers 402).

use crate::config::{Config, ElevenCfg, GenvoiceCfg};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// `ready.audioWaitMs` per provider: only the phone-side safety net. The server normally reports
/// `audio_skip` earlier, using the per-sentence deadline below.
pub const GENVOICE_WAIT_MS: u64 = 20_000;
pub const ELEVEN_WAIT_MS: u64 = 8000;
/// Server-side deadline for one sentence: base + per character.
pub const DEFAULT_TTS_BASE_MS: u64 = 8000;
pub const DEFAULT_TTS_PER_CHAR_MS: u64 = 60;
/// Server-side STT deadline: base + per second of audio.
pub const DEFAULT_STT_BASE_MS: u64 = 8000;
pub const DEFAULT_STT_PER_SEC_MS: u64 = 1500;
/// Parallel sentence syntheses per server.
pub const TTS_CONCURRENCY: usize = 4;
/// This many failed sentences in a row switch TTS off for the rest of the run.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// Terms that speech recognition should lean towards (Vietnamese school maths + mixed English).
pub const STT_KEYTERMS: &[&str] = &[
    "bất phương trình",
    "tam thức bậc hai",
    "nghiệm",
    "delta",
    "parabol",
    "hàm số",
    "next step",
    "ok",
];

#[derive(Debug, Clone)]
pub struct VoiceError {
    /// HTTP status; 0 for network errors and timeouts; 520 for a task the provider marked failed.
    pub status: u16,
    pub message: String,
}

impl VoiceError {
    /// Auth/plan problems: no point trying more sentences.
    pub fn fatal(&self) -> bool {
        matches!(self.status, 401 | 402 | 403)
    }

    pub fn timeout() -> VoiceError {
        VoiceError { status: 0, message: "timeout".into() }
    }
}

pub trait Tts: Send + Sync {
    /// Returns mp3 bytes.
    fn synth<'a>(&'a self, text: &'a str, lang: &'a str) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>>;

    /// Like [`Tts::synth`] with another voice than the configured one (`None` = configured).
    /// Providers without voices ignore it.
    fn synth_voice<'a>(&'a self, text: &'a str, lang: &'a str, voice: Option<&'a str>) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        let _ = voice;
        self.synth(text, lang)
    }

    /// Startup check. Default: synthesize a short phrase. GenVoice overrides it with a free call.
    fn probe<'a>(&'a self) -> BoxFuture<'a, Result<(), VoiceError>> {
        Box::pin(async move { self.synth("Xin chào.", "vi").await.map(|_| ()) })
    }
}

pub trait Stt: Send + Sync {
    /// `wav` is a complete WAV file. Returns the transcript.
    fn transcribe<'a>(&'a self, wav: Vec<u8>, lang: &'a str) -> BoxFuture<'a, Result<String, VoiceError>>;
}

fn net_err(e: reqwest::Error) -> VoiceError {
    VoiceError { status: 0, message: format!("network: {}", e.without_url()) }
}

async fn http_err(resp: reqwest::Response, what: &str) -> VoiceError {
    let status = resp.status().as_u16();
    let detail = resp.text().await.unwrap_or_default();
    // Only a short, non-secret hint goes into logs.
    let hint: String = detail.chars().take(160).collect();
    VoiceError { status, message: format!("{what} http {status}: {hint}") }
}

// ---------------------------------------------------------------- GenVoice

pub struct GenVoice {
    client: reqwest::Client,
    cfg: GenvoiceCfg,
}

impl GenVoice {
    pub fn new(cfg: GenvoiceCfg) -> Self {
        // One client for everything: connection reuse (keep-alive) matters for latency.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(60))
            .tcp_nodelay(true)
            .build()
            .expect("http client");
        GenVoice { client, cfg }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.cfg.base_url.trim_end_matches('/'), path)
    }

    fn poll(&self) -> Duration {
        Duration::from_millis(self.cfg.poll_ms.max(10))
    }

    /// `credit_balance` from `GET /v1/auth/me` (free); -1 when the field is missing.
    pub async fn credit_balance(&self) -> Result<f64, VoiceError> {
        let resp = self
            .client
            .get(self.url("/v1/auth/me"))
            .header("xi-api-key", &self.cfg.api_key)
            .send()
            .await
            .map_err(net_err)?;
        if !resp.status().is_success() {
            return Err(http_err(resp, "auth/me").await);
        }
        let v: Value = resp.json().await.map_err(net_err)?;
        Ok(v["credit_balance"].as_f64().unwrap_or(-1.0))
    }
}

fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Tts for GenVoice {
    fn synth<'a>(&'a self, text: &'a str, lang: &'a str) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        self.synth_voice(text, lang, None)
    }

    fn synth_voice<'a>(&'a self, text: &'a str, lang: &'a str, voice: Option<&'a str>) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        Box::pin(async move {
            let key = &self.cfg.api_key;
            let voice = voice.unwrap_or(&self.cfg.voice_id);
            // 1. submit (202 + id)
            let resp = self
                .client
                .post(self.url(&format!("/v1/text-to-speech/{voice}")))
                .header("xi-api-key", key)
                .json(&json!({"text": text, "model_id": self.cfg.tts_model, "language_code": lang}))
                .send()
                .await
                .map_err(net_err)?;
            if !resp.status().is_success() {
                return Err(http_err(resp, "tts submit").await);
            }
            let v: Value = resp.json().await.map_err(net_err)?;
            let id = id_string(&v["id"]).ok_or_else(|| VoiceError { status: 0, message: "tts submit: no id".into() })?;

            // 2. poll until completed / failed
            let started = tokio::time::Instant::now();
            let audio_url = loop {
                tokio::time::sleep(self.poll()).await;
                if started.elapsed() > Duration::from_secs(40) {
                    return Err(VoiceError::timeout());
                }
                let resp = self
                    .client
                    .get(self.url(&format!("/v1/history/{id}")))
                    .header("xi-api-key", key)
                    .send()
                    .await
                    .map_err(net_err)?;
                if !resp.status().is_success() {
                    return Err(http_err(resp, "tts poll").await);
                }
                let v: Value = resp.json().await.map_err(net_err)?;
                match v["status"].as_str().unwrap_or("") {
                    "completed" => match v["result"]["audio_url"].as_str() {
                        Some(u) => break u.to_string(),
                        None => return Err(VoiceError { status: 520, message: "completed without audio_url".into() }),
                    },
                    "failed" => {
                        let why = v["error"].as_str().unwrap_or("failed");
                        return Err(VoiceError { status: 520, message: format!("tts task failed: {why}") });
                    }
                    _ => {}
                }
            };

            // 3. download (send the key only to our own provider host)
            let mut req = self.client.get(&audio_url);
            if audio_url.starts_with(self.cfg.base_url.trim_end_matches('/')) {
                req = req.header("xi-api-key", key);
            }
            let resp = req.send().await.map_err(net_err)?;
            if !resp.status().is_success() {
                return Err(http_err(resp, "tts download").await);
            }
            Ok(resp.bytes().await.map_err(net_err)?.to_vec())
        })
    }

    /// `GET /v1/auth/me` costs no credits; logs the balance.
    fn probe<'a>(&'a self) -> BoxFuture<'a, Result<(), VoiceError>> {
        Box::pin(async move {
            let credits = self.credit_balance().await?;
            if (0.0..2000.0).contains(&credits) {
                tracing::warn!("genvoice credit_balance is low: {credits}");
            } else {
                tracing::info!("genvoice credit_balance: {credits}");
            }
            Ok(())
        })
    }
}

impl Stt for GenVoice {
    fn transcribe<'a>(&'a self, wav: Vec<u8>, lang: &'a str) -> BoxFuture<'a, Result<String, VoiceError>> {
        Box::pin(async move {
            let key = &self.cfg.api_key;
            let part = reqwest::multipart::Part::bytes(wav)
                .file_name("speech.wav")
                .mime_str("audio/wav")
                .map_err(net_err)?;
            let mut form = reqwest::multipart::Form::new()
                .part("file", part)
                .text("language_code", lang.to_string())
                .text("timestamps_granularity", "none")
                .text("tag_audio_events", "false");
            for k in STT_KEYTERMS {
                form = form.text("keyterms", k.to_string());
            }
            let resp = self
                .client
                .post(self.url("/v1/speech-to-text"))
                .header("xi-api-key", key)
                .multipart(form)
                .send()
                .await
                .map_err(net_err)?;
            if !resp.status().is_success() {
                return Err(http_err(resp, "stt submit").await);
            }
            let v: Value = resp.json().await.map_err(net_err)?;
            let id = id_string(&v["task_id"]).ok_or_else(|| VoiceError { status: 0, message: "stt submit: no task_id".into() })?;
            let started = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(self.poll()).await;
                if started.elapsed() > Duration::from_secs(30) {
                    return Err(VoiceError::timeout());
                }
                let resp = self
                    .client
                    .get(self.url(&format!("/v1/speech-to-text/{id}")))
                    .header("xi-api-key", key)
                    .send()
                    .await
                    .map_err(net_err)?;
                if !resp.status().is_success() {
                    return Err(http_err(resp, "stt poll").await);
                }
                let v: Value = resp.json().await.map_err(net_err)?;
                match v["status"].as_str().unwrap_or("") {
                    "completed" => return Ok(v["result"]["text"].as_str().unwrap_or("").trim().to_string()),
                    "failed" => {
                        let why = v["detail_error"].as_str().unwrap_or("failed");
                        return Err(VoiceError { status: 520, message: format!("stt task failed: {why}") });
                    }
                    _ => {}
                }
            }
        })
    }
}

// ---------------------------------------------------------------- ElevenLabs direct

pub struct ElevenLabs {
    client: reqwest::Client,
    cfg: ElevenCfg,
}

impl ElevenLabs {
    pub fn new(cfg: ElevenCfg) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .tcp_nodelay(true)
            .build()
            .expect("http client");
        ElevenLabs { client, cfg }
    }
}

impl Tts for ElevenLabs {
    fn synth<'a>(&'a self, text: &'a str, lang: &'a str) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        self.synth_voice(text, lang, None)
    }

    fn synth_voice<'a>(&'a self, text: &'a str, lang: &'a str, voice: Option<&'a str>) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        Box::pin(async move {
            let voice = voice.unwrap_or(&self.cfg.voice_id);
            let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=mp3_44100_64");
            let mut body = json!({"text": text, "model_id": self.cfg.tts_model});
            if self.cfg.tts_model.contains("v2_5") {
                body["language_code"] = Value::String(lang.to_string());
            }
            let resp = self
                .client
                .post(url)
                .header("xi-api-key", &self.cfg.api_key)
                .header("accept", "audio/mpeg")
                .json(&body)
                .send()
                .await
                .map_err(net_err)?;
            if !resp.status().is_success() {
                return Err(http_err(resp, "tts").await);
            }
            Ok(resp.bytes().await.map_err(net_err)?.to_vec())
        })
    }
}

impl Stt for ElevenLabs {
    fn transcribe<'a>(&'a self, wav: Vec<u8>, lang: &'a str) -> BoxFuture<'a, Result<String, VoiceError>> {
        Box::pin(async move {
            let model = self.cfg.stt_model.clone().unwrap_or_else(|| "scribe_v1".into());
            let part = reqwest::multipart::Part::bytes(wav)
                .file_name("speech.wav")
                .mime_str("audio/wav")
                .map_err(net_err)?;
            let form = reqwest::multipart::Form::new()
                .text("model_id", model)
                .text("language_code", lang.to_string())
                .part("file", part);
            let resp = self
                .client
                .post("https://api.elevenlabs.io/v1/speech-to-text")
                .header("xi-api-key", &self.cfg.api_key)
                .multipart(form)
                .send()
                .await
                .map_err(net_err)?;
            if !resp.status().is_success() {
                return Err(http_err(resp, "stt").await);
            }
            let v: Value = resp.json().await.map_err(net_err)?;
            Ok(v["text"].as_str().unwrap_or("").trim().to_string())
        })
    }
}

// ---------------------------------------------------------------- fake

/// Test double: returns fixed bytes, or fails with a fixed status.
pub struct FakeVoice {
    pub fail_status: Option<u16>,
    pub transcript: String,
}

impl Tts for FakeVoice {
    fn synth<'a>(&'a self, text: &'a str, _lang: &'a str) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        Box::pin(async move {
            match self.fail_status {
                Some(s) => Err(VoiceError { status: s, message: "fake failure".into() }),
                None => Ok(format!("MP3:{text}").into_bytes()),
            }
        })
    }
}

impl Stt for FakeVoice {
    fn transcribe<'a>(&'a self, _wav: Vec<u8>, _lang: &'a str) -> BoxFuture<'a, Result<String, VoiceError>> {
        Box::pin(async move { Ok(self.transcript.clone()) })
    }
}

// ---------------------------------------------------------------- provider choice

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Provider {
    GenVoice,
    Eleven,
    Device,
}

/// `requested` is `auto|genvoice|elevenlabs|device`. `auto` prefers GenVoice, then ElevenLabs.
pub fn resolve_provider(requested: &str, has_genvoice: bool, has_eleven: bool) -> Provider {
    match requested.trim().to_lowercase().as_str() {
        "device" | "off" | "none" => Provider::Device,
        "genvoice" => {
            if has_genvoice {
                Provider::GenVoice
            } else {
                tracing::warn!("provider genvoice requested but GENVOICE_API_KEY is missing; using device voice");
                Provider::Device
            }
        }
        "elevenlabs" => {
            if has_eleven {
                Provider::Eleven
            } else {
                tracing::warn!("provider elevenlabs requested but its key/voice is missing; using device voice");
                Provider::Device
            }
        }
        _ => {
            if has_genvoice {
                Provider::GenVoice
            } else if has_eleven {
                Provider::Eleven
            } else {
                Provider::Device
            }
        }
    }
}

/// Wrap raw 16-bit mono PCM in a WAV header; data that already starts with `RIFF` passes through.
pub fn ensure_wav(data: Vec<u8>, sample_rate: u32) -> Vec<u8> {
    if data.starts_with(b"RIFF") {
        return data;
    }
    let n = data.len() as u32;
    let mut h = Vec::with_capacity(44 + data.len());
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + n).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&1u16.to_le_bytes()); // mono
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&n.to_le_bytes());
    h.extend_from_slice(&data);
    h
}

// ---------------------------------------------------------------- shared state

/// Shared voice state: which backends exist and whether TTS is still considered usable.
pub struct Voice {
    pub tts: Option<Arc<dyn Tts>>,
    pub stt: Option<Arc<dyn Stt>>,
    /// Sent as `ready.audioWaitMs`; also the per-sentence synthesis timeout.
    pub tts_wait_ms: u64,
    pub tts_provider: &'static str,
    pub stt_provider: &'static str,
    /// Voice id from `.env` for the active provider (default until the owner picks one).
    pub default_voice_id: String,
    tts_base_ms: u64,
    tts_per_char_ms: u64,
    stt_base_ms: u64,
    stt_per_sec_ms: u64,
    tts_on: AtomicBool,
    /// Whether the phone was already told about a fallback (one `notice` per run).
    announced: AtomicBool,
    failures: AtomicU32,
    /// Limits parallel sentence syntheses.
    pub gate: Semaphore,
}

impl Voice {
    pub fn none() -> Arc<Voice> {
        Arc::new(Voice::build(None, None, ELEVEN_WAIT_MS))
    }

    pub fn build(tts: Option<Arc<dyn Tts>>, stt: Option<Arc<dyn Stt>>, tts_wait_ms: u64) -> Voice {
        let on = tts.is_some();
        Voice {
            tts,
            stt,
            tts_wait_ms,
            tts_provider: if on { "custom" } else { "device" },
            stt_provider: "custom",
            default_voice_id: crate::voices::DEFAULT_VOICE_ID.into(),
            tts_base_ms: DEFAULT_TTS_BASE_MS,
            tts_per_char_ms: DEFAULT_TTS_PER_CHAR_MS,
            stt_base_ms: DEFAULT_STT_BASE_MS,
            stt_per_sec_ms: DEFAULT_STT_PER_SEC_MS,
            tts_on: AtomicBool::new(on),
            announced: AtomicBool::new(true),
            failures: AtomicU32::new(0),
            gate: Semaphore::new(TTS_CONCURRENCY),
        }
    }

    /// Pick providers from the config (`TTS_PROVIDER` / `STT_PROVIDER`, default `auto`).
    pub fn from_config(cfg: &Config) -> Voice {
        let gv = cfg.genvoice.as_ref().map(|c| Arc::new(GenVoice::new(c.clone())));
        let el = cfg.eleven.as_ref().map(|c| Arc::new(ElevenLabs::new(c.clone())));
        let tts_p = resolve_provider(&cfg.tts_provider, gv.is_some(), el.is_some());
        let stt_p = resolve_provider(&cfg.stt_provider, gv.is_some(), el.is_some());
        let (tts, wait, name): (Option<Arc<dyn Tts>>, u64, &'static str) = match tts_p {
            Provider::GenVoice => (
                gv.clone().map(|g| g as Arc<dyn Tts>),
                cfg.genvoice.as_ref().map(|c| c.audio_wait_ms).unwrap_or(6000),
                "genvoice",
            ),
            Provider::Eleven => (el.clone().map(|e| e as Arc<dyn Tts>), ELEVEN_WAIT_MS, "elevenlabs"),
            Provider::Device => (None, ELEVEN_WAIT_MS, "device"),
        };
        let stt: Option<Arc<dyn Stt>> = match stt_p {
            Provider::GenVoice => gv.map(|g| g as Arc<dyn Stt>),
            Provider::Eleven => el.map(|e| e as Arc<dyn Stt>),
            Provider::Device => None,
        };
        let mut v = Voice::build(tts, stt, wait);
        v.tts_provider = name;
        v.default_voice_id = match tts_p {
            Provider::Eleven => cfg.eleven.as_ref().map(|c| c.voice_id.clone()),
            _ => cfg.genvoice.as_ref().map(|c| c.voice_id.clone()),
        }
        .unwrap_or_else(|| crate::voices::DEFAULT_VOICE_ID.into());
        v.stt_provider = match stt_p {
            Provider::GenVoice => "genvoice",
            Provider::Eleven => "elevenlabs",
            Provider::Device => "device",
        };
        v.set_deadlines(cfg.tts_base_ms, cfg.tts_per_char_ms, cfg.stt_base_ms, cfg.stt_per_sec_ms);
        tracing::info!("voice: tts={name}, stt={}", if v.stt.is_some() { "server" } else { "device" });
        v
    }

    pub fn set_deadlines(&mut self, tts_base: u64, tts_per_char: u64, stt_base: u64, stt_per_sec: u64) {
        self.tts_base_ms = tts_base;
        self.tts_per_char_ms = tts_per_char;
        self.stt_base_ms = stt_base;
        self.stt_per_sec_ms = stt_per_sec;
    }

    /// Time one sentence may take to synthesize before the server gives up (`audio_skip`).
    pub fn tts_deadline(&self, text: &str) -> Duration {
        Duration::from_millis(self.tts_base_ms + self.tts_per_char_ms * text.chars().count() as u64)
    }

    /// Time speech recognition may take for `audio_secs` seconds of speech.
    pub fn stt_deadline(&self, audio_secs: f64) -> Duration {
        Duration::from_millis(self.stt_base_ms + (self.stt_per_sec_ms as f64 * audio_secs.max(0.0)) as u64)
    }

    pub fn tts_enabled(&self) -> bool {
        self.tts_on.load(Ordering::Relaxed)
    }

    /// Provider named in the `voices` frame: `device` once server-side speech is off.
    pub fn active_tts_provider(&self) -> &'static str {
        if !self.tts_enabled() {
            "device"
        } else if self.tts_provider == "elevenlabs" {
            "elevenlabs"
        } else {
            "genvoice"
        }
    }

    pub fn stt_enabled(&self) -> bool {
        self.stt.is_some()
    }

    /// Value for `ready.audioWaitMs`; `None` (field omitted) when TTS is not server-side.
    pub fn audio_wait_ms(&self) -> Option<u64> {
        self.tts_enabled().then_some(self.tts_wait_ms)
    }

    /// Switch TTS off. Returns true the first time (caller sends the `notice`).
    pub fn disable_tts(&self) -> bool {
        self.tts_on.swap(false, Ordering::Relaxed)
    }

    pub fn record_ok(&self) {
        self.failures.store(0, Ordering::Relaxed);
    }

    /// Count a failed sentence. Returns true if this failure just switched TTS off
    /// (auth/plan error, or too many failures in a row): the caller announces it once.
    pub fn record_failure(&self, e: &VoiceError) -> bool {
        if e.fatal() {
            return self.disable_tts();
        }
        let n = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= MAX_CONSECUTIVE_FAILURES {
            return self.disable_tts();
        }
        false
    }

    /// Probe once at startup; on failure TTS is off and the first phone gets a notice.
    pub async fn probe(&self) {
        let Some(tts) = &self.tts else { return };
        match tts.probe().await {
            Ok(()) => tracing::info!("tts probe ok"),
            Err(e) => {
                tracing::warn!("tts probe failed, falling back to device voice: {}", e.message);
                self.tts_on.store(false, Ordering::Relaxed);
                self.announced.store(false, Ordering::Relaxed);
            }
        }
    }

    /// True once if a startup fallback has not been announced to any phone yet.
    pub fn take_unannounced(&self) -> bool {
        !self.announced.swap(true, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(fail: Option<u16>) -> Voice {
        Voice::build(Some(Arc::new(FakeVoice { fail_status: fail, transcript: String::new() })), None, 3000)
    }

    #[tokio::test]
    async fn probe_failure_turns_tts_off_once() {
        let v = fake(Some(402));
        assert!(v.tts_enabled());
        v.probe().await;
        assert!(!v.tts_enabled());
        assert!(v.take_unannounced());
        assert!(!v.take_unannounced());
        assert_eq!(v.audio_wait_ms(), None);
    }

    #[tokio::test]
    async fn probe_success_keeps_tts_on() {
        let v = fake(None);
        v.probe().await;
        assert!(v.tts_enabled());
        assert!(!v.take_unannounced());
        assert_eq!(v.audio_wait_ms(), Some(3000));
    }

    fn err(status: u16) -> VoiceError {
        VoiceError { status, message: String::new() }
    }

    #[test]
    fn auth_and_plan_errors_disable_immediately() {
        for s in [401u16, 402] {
            let v = fake(None);
            assert!(v.record_failure(&err(s)));
            assert!(!v.tts_enabled());
            assert!(!v.record_failure(&err(s)), "announce only once");
        }
    }

    #[test]
    fn three_failures_in_a_row_disable_but_success_resets() {
        let v = fake(None);
        assert!(!v.record_failure(&err(429)));
        assert!(!v.record_failure(&err(0)));
        assert!(v.record_failure(&err(520)));
        assert!(!v.tts_enabled());

        let v = fake(None);
        assert!(!v.record_failure(&err(429)));
        assert!(!v.record_failure(&err(0)));
        v.record_ok();
        assert!(!v.record_failure(&err(520)));
        assert!(!v.record_failure(&err(520)));
        assert!(v.tts_enabled());
        assert!(v.record_failure(&err(520)));
    }

    #[test]
    fn deadlines_scale_with_length() {
        let v = fake(None);
        assert_eq!(v.tts_deadline("").as_millis(), 8000);
        assert_eq!(v.tts_deadline(&"a".repeat(100)).as_millis(), 14_000);
        assert_eq!(v.tts_deadline(&"ạ".repeat(100)).as_millis(), 14_000, "counts characters, not bytes");
        assert_eq!(v.stt_deadline(0.0).as_millis(), 8000);
        assert_eq!(v.stt_deadline(4.0).as_millis(), 14_000);
        let mut v = fake(None);
        v.set_deadlines(100, 1, 200, 10);
        assert_eq!(v.tts_deadline("abc").as_millis(), 103);
        assert_eq!(v.stt_deadline(2.0).as_millis(), 220);
    }

    #[test]
    fn provider_resolution() {
        use Provider::*;
        assert_eq!(resolve_provider("auto", true, true), GenVoice);
        assert_eq!(resolve_provider("auto", false, true), Eleven);
        assert_eq!(resolve_provider("auto", false, false), Device);
        assert_eq!(resolve_provider("", true, false), GenVoice);
        assert_eq!(resolve_provider("elevenlabs", true, true), Eleven);
        assert_eq!(resolve_provider("elevenlabs", true, false), Device);
        assert_eq!(resolve_provider("genvoice", false, true), Device);
        assert_eq!(resolve_provider("DEVICE", true, true), Device);
    }

    #[test]
    fn wav_header_wraps_raw_pcm_only() {
        let pcm = vec![1u8, 2, 3, 4];
        let w = ensure_wav(pcm.clone(), 16000);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 4);
        assert_eq!(&w[44..], &pcm[..]);
        assert_eq!(ensure_wav(w.clone(), 8000), w);
    }
}
