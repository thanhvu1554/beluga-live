use beluga_server::config::Config;
use beluga_server::store::Store;
use std::io::IsTerminal;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::from_args(&args);
    if args.first().map(String::as_str) == Some("gen-voice-previews") {
        std::process::exit(gen_voice_previews(&cfg).await);
    }
    if args.first().map(String::as_str) == Some("gen-voice-fillers") {
        std::process::exit(gen_voice_fillers(&cfg).await);
    }
    if let Some(code) = beluga_server::backup::cli(&cfg, &args).await {
        std::process::exit(code);
    }

    let listener = match TcpListener::bind(("0.0.0.0", cfg.port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Không mở được cổng {}: {e}", cfg.port);
            std::process::exit(1);
        }
    };
    // Open the store here only to print the pairing code; `start` opens it again (same files).
    match Store::open(&cfg.data_dir, cfg.fixed_pair_code.as_deref()) {
        Ok(s) => {
            println!("Beluga server cổng {} ({})", cfg.port, if cfg.mock { "MOCK" } else { "Claude Code" });
            println!("Mã ghép đôi: {}", s.pair_code());
        }
        Err(e) => {
            eprintln!("Không tạo được thư mục dữ liệu: {e}");
            std::process::exit(1);
        }
    }
    if !cfg.mock {
        check_claude(&cfg.claude_bin).await;
    }
    if let Err(e) = beluga_server::start(cfg, listener).await {
        eprintln!("Server dừng: {e}");
        std::process::exit(1);
    }
}

/// `beluga-server gen-voice-previews`: create `server/voices/<id>.mp3` for every voice in
/// `server/voices.json` that has no clip yet (one synthesis each; existing clips are kept, so
/// delete a file to regenerate it). Spends voice credits.
async fn gen_voice_previews(cfg: &Config) -> i32 {
    use beluga_server::voice::{ElevenLabs, GenVoice, Tts};
    use beluga_server::voices::VoiceCatalog;
    let gv = cfg.genvoice.clone().map(GenVoice::new);
    let tts: Box<dyn Tts> = match (&gv, &cfg.eleven) {
        (Some(_), _) => Box::new(GenVoice::new(cfg.genvoice.clone().unwrap())),
        (None, Some(e)) => Box::new(ElevenLabs::new(e.clone())),
        _ => {
            eprintln!("Cần GENVOICE_API_KEY (hoặc ELEVENLABS_API_KEY + ELEVENLABS_VOICE_ID) trong server/.env.");
            return 2;
        }
    };
    let cat = VoiceCatalog::load(&cfg.voices_path, &cfg.voice_clips_dir, &cfg.data_dir, "");
    let missing: Vec<_> = cat.entries().iter().filter(|e| !cat.has_clip(&e.id)).map(|e| e.name.clone()).collect();
    println!("Danh mục: {} ({} giọng), thư mục clip: {}", cfg.voices_path.display(), cat.entries().len(), cfg.voice_clips_dir.display());
    if missing.is_empty() {
        println!("Mọi giọng đã có clip, không tạo gì.");
        return 0;
    }
    println!("Sẽ tạo {} clip: {}", missing.len(), missing.join(", "));
    println!("Câu mẫu ({} ký tự): {}", cat.sample().chars().count(), cat.sample());
    if let Some(g) = &gv {
        match g.credit_balance().await {
            Ok(c) => println!("Credit trước: {c}"),
            Err(e) => println!("Không đọc được credit: {}", e.message),
        }
    }
    let (made, failed) = cat.generate_missing(tts.as_ref(), "vi").await;
    for id in &made {
        let size = std::fs::metadata(cfg.voice_clips_dir.join(format!("{id}.mp3"))).map(|m| m.len()).unwrap_or(0);
        println!("  ok   {id} ({size} byte)");
    }
    for (id, why) in &failed {
        println!("  lỗi  {id}: {why}");
    }
    if let Some(g) = &gv {
        if let Ok(c) = g.credit_balance().await {
            println!("Credit sau: {c}");
        }
    }
    if failed.is_empty() { 0 } else { 1 }
}

/// `beluga-server gen-voice-fillers`: create `server/voices/<voiceId>/<fillerId>.mp3` for every
/// voice x wait/pick-up line of `server/voices.json` that has no clip yet (same TTS as the previews;
/// existing clips are kept, delete a file to regenerate it). Spends voice credits.
async fn gen_voice_fillers(cfg: &Config) -> i32 {
    use beluga_server::voice::{ElevenLabs, GenVoice, Tts};
    use beluga_server::voices::VoiceCatalog;
    let gv = cfg.genvoice.clone().map(GenVoice::new);
    let tts: Box<dyn Tts> = match (&gv, &cfg.eleven) {
        (Some(_), _) => Box::new(GenVoice::new(cfg.genvoice.clone().unwrap())),
        (None, Some(e)) => Box::new(ElevenLabs::new(e.clone())),
        _ => {
            eprintln!("Cần GENVOICE_API_KEY (hoặc ELEVENLABS_API_KEY + ELEVENLABS_VOICE_ID) trong server/.env.");
            return 2;
        }
    };
    let cat = VoiceCatalog::load(&cfg.voices_path, &cfg.voice_clips_dir, &cfg.data_dir, "");
    let missing = cat.missing_fillers();
    println!(
        "Danh mục: {} ({} giọng x {} câu chờ), thư mục clip: {}",
        cfg.voices_path.display(),
        cat.entries().len(),
        cat.fillers().len(),
        cfg.voice_clips_dir.display()
    );
    if missing.is_empty() {
        println!("Mọi câu chờ đã có clip, không tạo gì.");
        return 0;
    }
    let chars: usize = missing.iter().map(|(_, f)| f.text.chars().count()).sum();
    println!("Sẽ tạo {} clip ({chars} ký tự).", missing.len());
    if let Some(g) = &gv {
        match g.credit_balance().await {
            Ok(c) => println!("Credit trước: {c}"),
            Err(e) => println!("Không đọc được credit: {}", e.message),
        }
    }
    let (made, failed) = cat.generate_missing_fillers(tts.as_ref(), "vi").await;
    for key in &made {
        let size = std::fs::metadata(cfg.voice_clips_dir.join(format!("{key}.mp3"))).map(|m| m.len()).unwrap_or(0);
        println!("  ok   {key} ({size} byte)");
    }
    for (key, why) in &failed {
        println!("  lỗi  {key}: {why}");
    }
    if let Some(g) = &gv {
        if let Ok(c) = g.credit_balance().await {
            println!("Credit sau: {c}");
        }
    }
    if failed.is_empty() { 0 } else { 1 }
}

/// Fail early (with a clear message) if the CLI is missing; login problems surface per turn.
async fn check_claude(bin: &str) {
    match tokio::process::Command::new(bin).arg("--version").output().await {
        Ok(o) if o.status.success() => {
            tracing::info!("claude CLI: {}", String::from_utf8_lossy(&o.stdout).trim());
        }
        _ => tracing::error!("Không chạy được '{bin} --version'. Đã cài Claude Code chưa? (lượt học sẽ báo claude_unavailable)"),
    }
}
