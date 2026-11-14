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

/// Fail early (with a clear message) if the CLI is missing; login problems surface per turn.
async fn check_claude(bin: &str) {
    match tokio::process::Command::new(bin).arg("--version").output().await {
        Ok(o) if o.status.success() => {
            tracing::info!("claude CLI: {}", String::from_utf8_lossy(&o.stdout).trim());
        }
        _ => tracing::error!("Không chạy được '{bin} --version'. Đã cài Claude Code chưa? (lượt học sẽ báo claude_unavailable)"),
    }
}
