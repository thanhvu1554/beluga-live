//! Manual check against the REAL GenVoice service using the mock brain (no Claude quota).
//! Spends a little GenVoice credit (~100 for two short turns), so it is `#[ignore]`.
//!
//! Run: `cargo test --test real_voice -- --ignored --nocapture`
//! Needs GENVOICE_API_KEY in server/.env (never printed).

mod common;

use base64::Engine;
use beluga_server::config::Config;
use common::*;
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use tokio::net::TcpListener;

#[tokio::test]
#[ignore]
async fn real_genvoice_audio_latency_with_mock_brain() {
    std::env::set_var("BELUGA_VOICE_IN_MOCK", "1");
    let mut cfg = Config::from_args(&["--mock".to_string()]);
    assert!(cfg.genvoice.is_some(), "GENVOICE_API_KEY missing in server/.env");
    let dir = temp_dir();
    cfg.data_dir = dir.join("data");
    cfg.tutor_dir = dir.join("tutor");
    cfg.fixed_pair_code = Some(CODE.into());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let t_start = Instant::now();
    tokio::spawn(async move {
        let _ = beluga_server::start(cfg, listener).await;
    });
    let mut p = Phone::connect(addr).await;
    let ready = p.hello(json!({})).await;
    println!("ready after {} ms: tts={} stt={} audioWaitMs={}", t_start.elapsed().as_millis(), ready["tts"], ready["stt"], ready["audioWaitMs"]);
    assert_eq!(ready["tts"], "server", "probe failed (see log)");

    let mut say_at: HashMap<u64, (Instant, String)> = HashMap::new();
    let mut n_audio = 0;
    for text in ["giảng bài này đi", "tìm nghiệm xong rồi"] {
        p.send(json!({"t":"user_text","text":text})).await;
        let mut pending = 0;
        let mut ended = false;
        while !(ended && pending == 0) {
            let Some(f) = p.next_within(15000).await else { panic!("timeout waiting for audio") };
            match f["t"].as_str().unwrap_or("") {
                "say" => {
                    pending += 1;
                    say_at.insert(f["seq"].as_u64().unwrap(), (Instant::now(), f["text"].as_str().unwrap().to_string()));
                }
                "audio" => {
                    pending -= 1;
                    n_audio += 1;
                    let seq = f["seq"].as_u64().unwrap();
                    let (t, txt) = &say_at[&seq];
                    let bytes = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap()).unwrap();
                    let mp3 = bytes.starts_with(b"ID3") || (bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0);
                    println!(
                        "seq {seq:>2}: say -> audio {:>5} ms, {:>6} bytes, mp3={mp3}, {} chars",
                        t.elapsed().as_millis(),
                        bytes.len(),
                        txt.chars().count()
                    );
                    assert!(mp3, "not an mp3");
                }
                "audio_skip" => panic!("audio_skip: {f}"),
                "notice" if f["code"] == "tts_fallback" => panic!("fell back to device voice"),
                "turn_end" => ended = true,
                _ => {}
            }
        }
    }
    assert_eq!(n_audio, say_at.len());
}
