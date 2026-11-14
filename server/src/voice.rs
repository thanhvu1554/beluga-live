//! ElevenLabs text-to-speech / speech-to-text behind small interfaces (with fakes for tests).
//!
//! NOT yet exercised against a working paid key: only the "free plan returns 402" path
//! has been confirmed against the real API.

use crate::config::ElevenCfg;
use futures_util::future::BoxFuture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct VoiceError {
    /// HTTP status, 0 for network errors.
    pub status: u16,
    pub message: String,
}

impl VoiceError {
    /// Errors after which TTS should be disabled for the rest of the run
    /// (auth, plan, quota/rate limit, network).
    pub fn disables_tts(&self) -> bool {
        matches!(self.status, 0 | 401 | 402 | 403 | 429)
    }
}

pub trait Tts: Send + Sync {
    /// Returns mp3 bytes.
    fn synth<'a>(&'a self, text: &'a str, lang: &'a str) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>>;
}

pub trait Stt: Send + Sync {
    /// `wav` is a complete WAV file. Returns the transcript.
    fn transcribe<'a>(&'a self, wav: Vec<u8>, lang: &'a str) -> BoxFuture<'a, Result<String, VoiceError>>;
}

pub struct ElevenLabs {
    client: reqwest::Client,
    cfg: ElevenCfg,
}

impl ElevenLabs {
    pub fn new(cfg: ElevenCfg) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("http client");
        ElevenLabs { client, cfg }
    }
}

fn net_err(e: reqwest::Error) -> VoiceError {
    VoiceError { status: 0, message: format!("network: {}", e.without_url()) }
}

impl Tts for ElevenLabs {
    fn synth<'a>(&'a self, text: &'a str, lang: &'a str) -> BoxFuture<'a, Result<Vec<u8>, VoiceError>> {
        Box::pin(async move {
            let url = format!(
                "https://api.elevenlabs.io/v1/text-to-speech/{}?output_format=mp3_44100_64",
                self.cfg.voice_id
            );
            let mut body = serde_json::json!({"text": text, "model_id": self.cfg.tts_model});
            // flash/turbo v2.5 accept a language code, which keeps Vietnamese from being guessed wrong.
            if self.cfg.tts_model.contains("v2_5") {
                body["language_code"] = serde_json::Value::String(lang.to_string());
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
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                let detail = resp.text().await.unwrap_or_default();
                // Only a short, non-secret hint goes into logs.
                let hint: String = detail.chars().take(160).collect();
                return Err(VoiceError { status, message: format!("tts http {status}: {hint}") });
            }
            let bytes = resp.bytes().await.map_err(net_err)?;
            Ok(bytes.to_vec())
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
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                let detail = resp.text().await.unwrap_or_default();
                let hint: String = detail.chars().take(160).collect();
                return Err(VoiceError { status, message: format!("stt http {status}: {hint}") });
            }
            let v: serde_json::Value = resp.json().await.map_err(net_err)?;
            Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string())
        })
    }
}

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

/// Shared voice state: which backends exist and whether TTS is still considered usable.
pub struct Voice {
    pub tts: Option<Arc<dyn Tts>>,
    pub stt: Option<Arc<dyn Stt>>,
    tts_on: AtomicBool,
    /// Whether the phone was already told about a fallback (one `notice` per run).
    announced: AtomicBool,
    pub gate: Semaphore,
}

impl Voice {
    pub fn none() -> Arc<Voice> {
        Arc::new(Voice::build(None, None))
    }

    pub fn build(tts: Option<Arc<dyn Tts>>, stt: Option<Arc<dyn Stt>>) -> Voice {
        let on = tts.is_some();
        Voice { tts, stt, tts_on: AtomicBool::new(on), announced: AtomicBool::new(true), gate: Semaphore::new(2) }
    }

    pub fn from_eleven(cfg: &ElevenCfg) -> Voice {
        let e = Arc::new(ElevenLabs::new(cfg.clone()));
        let stt: Option<Arc<dyn Stt>> = if cfg.stt_model.is_some() { Some(e.clone()) } else { None };
        Voice::build(Some(e), stt)
    }

    pub fn tts_enabled(&self) -> bool {
        self.tts_on.load(Ordering::Relaxed)
    }

    pub fn stt_enabled(&self) -> bool {
        self.stt.is_some()
    }

    /// Switch TTS off. Returns true the first time (caller sends the `notice`).
    pub fn disable_tts(&self) -> bool {
        self.tts_on.swap(false, Ordering::Relaxed)
    }

    /// Probe once at startup; on failure TTS is off and the first phone gets a notice.
    pub async fn probe(&self) {
        let Some(tts) = &self.tts else { return };
        match tts.synth("Xin chào.", "vi").await {
            Ok(b) if !b.is_empty() => tracing::info!("tts probe ok ({} bytes)", b.len()),
            Ok(_) => self.fail_probe("empty audio".into()),
            Err(e) => self.fail_probe(e.message),
        }
    }

    fn fail_probe(&self, why: String) {
        tracing::warn!("tts probe failed, falling back to device voice: {why}");
        self.tts_on.store(false, Ordering::Relaxed);
        self.announced.store(false, Ordering::Relaxed);
    }

    /// True once if a startup fallback has not been announced to any phone yet.
    pub fn take_unannounced(&self) -> bool {
        !self.announced.swap(true, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_failure_turns_tts_off_once() {
        let v = Voice::build(Some(Arc::new(FakeVoice { fail_status: Some(402), transcript: String::new() })), None);
        assert!(v.tts_enabled());
        v.probe().await;
        assert!(!v.tts_enabled());
        assert!(v.take_unannounced());
        assert!(!v.take_unannounced());
    }

    #[tokio::test]
    async fn probe_success_keeps_tts_on() {
        let v = Voice::build(Some(Arc::new(FakeVoice { fail_status: None, transcript: String::new() })), None);
        v.probe().await;
        assert!(v.tts_enabled());
        assert!(!v.take_unannounced());
    }

    #[test]
    fn fallback_statuses() {
        for s in [0u16, 401, 402, 429] {
            assert!(VoiceError { status: s, message: String::new() }.disables_tts());
        }
        assert!(!VoiceError { status: 422, message: String::new() }.disables_tts());
    }
}
