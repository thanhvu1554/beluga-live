//! Configuration: CLI flags, `.env` (never logged) and repo layout discovery.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct ElevenCfg {
    pub api_key: String,
    pub voice_id: String,
    pub tts_model: String,
    /// `None` = server-side STT disabled (the phone recognises speech itself).
    pub stt_model: Option<String>,
}

/// GenVoice (https://genvoice.pro): async ElevenLabs-compatible TTS and STT.
#[derive(Clone)]
pub struct GenvoiceCfg {
    pub api_key: String,
    pub voice_id: String,
    pub tts_model: String,
    pub base_url: String,
    /// Poll interval for async tasks.
    pub poll_ms: u64,
    /// Max time the phone waits for one sentence's audio (sent as `ready.audioWaitMs`).
    pub audio_wait_ms: u64,
}

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub mock: bool,
    /// Delay between mock lesson lines.
    pub mock_delay_ms: u64,
    /// How long the `capture_photo` tool waits for the phone (PROTOCOL: 8 seconds).
    pub capture_timeout_ms: u64,
    pub data_dir: PathBuf,
    pub board_dir: PathBuf,
    pub tutor_dir: PathBuf,
    pub claude_bin: String,
    pub claude_effort: Option<String>,
    /// `BELUGA_MODEL_MAP`: alias → model, e.g. `opus=ds/deepseek-flash` (temporary tests).
    pub claude_model_map: Vec<(String, String)>,
    pub eleven: Option<ElevenCfg>,
    pub genvoice: Option<GenvoiceCfg>,
    /// `auto`, `genvoice`, `elevenlabs` or `device`.
    pub tts_provider: String,
    pub stt_provider: String,
    /// Per-sentence TTS deadline: `tts_base_ms + tts_per_char_ms * chars`.
    pub tts_base_ms: u64,
    pub tts_per_char_ms: u64,
    /// Server-side STT deadline: `stt_base_ms + stt_per_sec_ms * seconds of audio`.
    pub stt_base_ms: u64,
    pub stt_per_sec_ms: u64,
    /// Fixed pairing code (tests); normally generated and persisted.
    pub fixed_pair_code: Option<String>,
}

/// Minimal `.env` parser. Values are never printed.
pub fn parse_dotenv(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                .unwrap_or(v);
            m.insert(k.trim().to_string(), v.to_string());
        }
    }
    m
}

/// Parse `BELUGA_MODEL_MAP`: comma-separated `from=to` pairs. A phone setting (`opus`) can
/// then run a different Claude Code model without changing the app.
pub fn parse_model_map(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .filter_map(|p| {
            let (a, b) = p.split_once('=')?;
            let (a, b) = (a.trim(), b.trim());
            (!a.is_empty() && !b.is_empty()).then(|| (a.to_string(), b.to_string()))
        })
        .collect()
}

/// Find the repo root (the directory holding `tutor/CLAUDE.md`).
pub fn find_repo_root() -> PathBuf {
    if let Ok(v) = std::env::var("BELUGA_ROOT") {
        return PathBuf::from(v);
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            starts.push(d.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for s in starts {
        for anc in s.ancestors() {
            if anc.join("tutor").join("CLAUDE.md").exists() {
                return anc.to_path_buf();
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

impl Config {
    pub fn from_args(args: &[String]) -> Config {
        let root = find_repo_root();
        let server_dir = root.join("server");
        let mut env: HashMap<String, String> = std::fs::read_to_string(server_dir.join(".env"))
            .map(|t| parse_dotenv(&t))
            .unwrap_or_default();
        for (k, v) in std::env::vars() {
            if k.starts_with("ELEVENLABS_") || k.starts_with("GENVOICE_") || k == "TTS_PROVIDER" || k == "STT_PROVIDER" || k == "PORT" || k.starts_with("BELUGA_") {
                env.insert(k, v);
            }
        }
        let get = |k: &str| env.get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        let mut port: u16 = get("PORT").and_then(|p| p.parse().ok()).unwrap_or(8787);
        let mut mock = false;
        let mut data_dir = server_dir.join("data");
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--mock" => mock = true,
                "--port" => {
                    if let Some(p) = it.next().and_then(|p| p.parse().ok()) {
                        port = p;
                    }
                }
                "--data" => {
                    if let Some(p) = it.next() {
                        data_dir = PathBuf::from(p);
                    }
                }
                _ => {}
            }
        }

        let eleven = match (get("ELEVENLABS_API_KEY"), get("ELEVENLABS_VOICE_ID")) {
            (Some(api_key), Some(voice_id)) => Some(ElevenCfg {
                api_key,
                voice_id,
                tts_model: get("ELEVENLABS_TTS_MODEL").unwrap_or_else(|| "eleven_flash_v2_5".into()),
                stt_model: get("ELEVENLABS_STT_MODEL"),
            }),
            _ => None,
        };
        let genvoice = get("GENVOICE_API_KEY").map(|api_key| GenvoiceCfg {
            api_key,
            voice_id: get("GENVOICE_VOICE_ID").unwrap_or_else(|| "5yEPTrA0uAhZsBEhHjfi".into()),
            tts_model: get("GENVOICE_TTS_MODEL").unwrap_or_else(|| "eleven_flash_v2_5".into()),
            base_url: get("GENVOICE_BASE_URL").unwrap_or_else(|| "https://api.genvoice.pro".into()),
            poll_ms: 220,
            audio_wait_ms: crate::voice::GENVOICE_WAIT_MS,
        });
        // Mock mode must never spend voice credits unless explicitly asked.
        let voice_off = mock && get("BELUGA_VOICE_IN_MOCK").is_none();
        let (eleven, genvoice) = if voice_off { (None, None) } else { (eleven, genvoice) };

        Config {
            port,
            mock,
            capture_timeout_ms: 8000,
            mock_delay_ms: get("BELUGA_MOCK_DELAY_MS").and_then(|v| v.parse().ok()).unwrap_or(250),
            data_dir,
            board_dir: root.join("shared").join("board"),
            tutor_dir: root.join("tutor"),
            claude_bin: get("BELUGA_CLAUDE_BIN").unwrap_or_else(|| "claude".into()),
            claude_effort: get("BELUGA_EFFORT"),
            claude_model_map: parse_model_map(&get("BELUGA_MODEL_MAP").unwrap_or_default()),
            eleven,
            genvoice,
            tts_provider: get("TTS_PROVIDER").unwrap_or_else(|| "auto".into()),
            stt_provider: get("STT_PROVIDER").unwrap_or_else(|| "auto".into()),
            tts_base_ms: get("BELUGA_TTS_BASE_MS").and_then(|v| v.parse().ok()).unwrap_or(crate::voice::DEFAULT_TTS_BASE_MS),
            tts_per_char_ms: get("BELUGA_TTS_PER_CHAR_MS").and_then(|v| v.parse().ok()).unwrap_or(crate::voice::DEFAULT_TTS_PER_CHAR_MS),
            stt_base_ms: get("BELUGA_STT_BASE_MS").and_then(|v| v.parse().ok()).unwrap_or(crate::voice::DEFAULT_STT_BASE_MS),
            stt_per_sec_ms: get("BELUGA_STT_PER_SEC_MS").and_then(|v| v.parse().ok()).unwrap_or(crate::voice::DEFAULT_STT_PER_SEC_MS),
            fixed_pair_code: None,
        }
    }

    pub fn board_version(&self) -> u64 {
        read_board_version(&self.board_dir)
    }
}

pub fn read_board_version(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("version.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("version").and_then(|x| x.as_u64()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_map_parsing() {
        let m = parse_model_map(" opus=ds/deepseek-flash , sonnet=cc/claude-sonnet-5,,");
        assert_eq!(m, vec![("opus".into(), "ds/deepseek-flash".into()), ("sonnet".into(), "cc/claude-sonnet-5".into())]);
        assert!(parse_model_map("").is_empty());
        assert!(parse_model_map("opus").is_empty());
    }

    #[test]
    fn dotenv_parsing() {
        let m = parse_dotenv("# c\nA=1\nB = \"two\"\n\nC=\nD='x y'\n");
        assert_eq!(m["A"], "1");
        assert_eq!(m["B"], "two");
        assert_eq!(m["C"], "");
        assert_eq!(m["D"], "x y");
    }
}
