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

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub mock: bool,
    /// Delay between mock lesson lines.
    pub mock_delay_ms: u64,
    pub data_dir: PathBuf,
    pub board_dir: PathBuf,
    pub tutor_dir: PathBuf,
    pub claude_bin: String,
    pub claude_effort: Option<String>,
    pub eleven: Option<ElevenCfg>,
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
            if k.starts_with("ELEVENLABS_") || k == "PORT" || k.starts_with("BELUGA_") {
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
        // Mock mode must never spend ElevenLabs quota unless explicitly asked.
        let eleven = if mock && get("BELUGA_VOICE_IN_MOCK").is_none() { None } else { eleven };

        Config {
            port,
            mock,
            mock_delay_ms: get("BELUGA_MOCK_DELAY_MS").and_then(|v| v.parse().ok()).unwrap_or(250),
            data_dir,
            board_dir: root.join("shared").join("board"),
            tutor_dir: root.join("tutor"),
            claude_bin: get("BELUGA_CLAUDE_BIN").unwrap_or_else(|| "claude".into()),
            claude_effort: get("BELUGA_EFFORT"),
            eleven,
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
    fn dotenv_parsing() {
        let m = parse_dotenv("# c\nA=1\nB = \"two\"\n\nC=\nD='x y'\n");
        assert_eq!(m["A"], "1");
        assert_eq!(m["B"], "two");
        assert_eq!(m["C"], "");
        assert_eq!(m["D"], "x y");
    }
}
