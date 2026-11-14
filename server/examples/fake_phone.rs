//! A fake iPhone for one real end-to-end lesson: it connects to a RUNNING server over the
//! WebSocket, asks for help with a photographed problem, answers the tutor's question WRONG on
//! purpose, and records what happened (timings, events, mp3 files, transcript).
//!
//! Spends Claude quota and voice credit, so it is an example you run by hand:
//!
//! ```text
//! cargo run --example fake_phone -- --addr 127.0.0.1:8799 --code 123456 --jpeg problem.jpg --out out-dir
//! ```
//!
//! Stops at once on a `rate_limited` error. It never runs more than two student turns.

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

const FIRST: &str = "Giảng tớ câu a";
/// Wrong on purpose and wrong at the first step of any plan: treats a quadratic as linear.
const WRONG: &str = "Tớ nghĩ là chia hai vế cho x, ra x lớn hơn năm trừ sáu, tức là x lớn hơn âm một.";

fn arg(name: &str, default: &str) -> String {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned()).unwrap_or_else(|| default.to_string())
}

/// (kbps of the first frame, seconds) of an MP3.
fn mp3_info(b: &[u8]) -> Option<(u32, f64)> {
    let mut i = 0;
    if b.starts_with(b"ID3") && b.len() > 10 {
        i = 10 + (((b[6] & 0x7f) as usize) << 21 | ((b[7] & 0x7f) as usize) << 14 | ((b[8] & 0x7f) as usize) << 7 | (b[9] & 0x7f) as usize);
    }
    const BR: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const SR: [u32; 3] = [44100, 48000, 32000];
    let (mut secs, mut first) = (0.0, None);
    while i + 4 <= b.len() {
        if b[i] != 0xFF || b[i + 1] & 0xFE != 0xFA {
            i += 1;
            continue; // only MPEG-1 layer III is expected from the voice service
        }
        let (bri, sri, pad) = ((b[i + 2] >> 4) as usize, ((b[i + 2] >> 2) & 3) as usize, ((b[i + 2] >> 1) & 1) as u32);
        if BR[bri] == 0 || sri > 2 {
            i += 1;
            continue;
        }
        let len = (144 * BR[bri] * 1000 / SR[sri] + pad) as usize;
        first.get_or_insert(BR[bri]);
        secs += 1152.0 / SR[sri] as f64;
        i += len.max(4);
    }
    first.map(|br| (br, secs))
}

#[derive(Default, Debug)]
struct TurnStats {
    sent_ms: u128,
    first_state_ms: Option<u128>,
    capture_request_ms: Option<u128>,
    first_say_ms: Option<u128>,
    first_audio_ms: Option<u128>,
    turn_end_ms: Option<u128>,
    events: BTreeMap<String, u32>,
    audio_bytes: usize,
}

#[tokio::main]
async fn main() {
    let addr = arg("--addr", "127.0.0.1:8787");
    let code = arg("--code", "");
    let jpeg = std::fs::read(arg("--jpeg", "problem.jpg")).expect("--jpeg");
    // Other scenarios: `--wait-offer` lets the server speak first (review offer) and only then
    // sends `--first`; `--second` is the second student line.
    let first_text: &'static str = Box::leak(arg("--first", FIRST).into_boxed_str());
    let second_text: &'static str = Box::leak(arg("--second", WRONG).into_boxed_str());
    let wait_offer = std::env::args().any(|a| a == "--wait-offer");
    let out = std::path::PathBuf::from(arg("--out", "fake-phone-out"));
    std::fs::create_dir_all(&out).unwrap();
    let mut transcript = std::fs::File::create(out.join("transcript.txt")).unwrap();
    let mut events = std::fs::File::create(out.join("events.jsonl")).unwrap();

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("connect");
    let send = |v: Value| Message::Text(v.to_string().into());
    ws.send(send(json!({"t":"hello","v":1,"pairCode":code,"device":"fake-phone",
        "settings":{"lang":"vi","model":"sonnet","rigor":"medium"}})))
        .await
        .unwrap();

    let t0 = Instant::now();
    let mut stats: Vec<TurnStats> = Vec::new();
    let mut says: BTreeMap<u64, String> = BTreeMap::new();
    let mut ready = Value::Null;
    let wrote = |s: String, transcript: &mut std::fs::File| {
        let _ = writeln!(transcript, "{s}");
        println!("{s}");
    };

    // Turn 0 is only the handshake; turns 1 and 2 are the student's.
    let mut turn_idx = 0usize;
    let mut sent_turns = 0usize;
    let mut pending_send: Option<&'static str> = None;
    let mut done = false;
    let mut last_activity = Instant::now();
    let mut ask_seen_in_turn = false;
    while !done {
        if let Some(text) = pending_send.take() {
            let mut st = TurnStats::default();
            st.sent_ms = t0.elapsed().as_millis();
            stats.push(st);
            turn_idx = stats.len() - 1;
            ask_seen_in_turn = false;
            wrote(format!("\n=== student turn {} at {} ms: {text}", sent_turns + 1, t0.elapsed().as_millis()), &mut transcript);
            ws.send(send(json!({"t":"user_text","text":text}))).await.unwrap();
            sent_turns += 1;
        }
        let wait = match (sent_turns, wait_offer && ready != Value::Null) {
            (0, true) => Duration::from_secs(10),
            (0, false) => Duration::from_secs(30),
            _ => Duration::from_secs(180),
        };
        let msg = match tokio::time::timeout(wait, ws.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(_) => {
                wrote("-- connection closed".into(), &mut transcript);
                break;
            }
            Err(_) if sent_turns == 0 && wait_offer && ready != Value::Null => {
                wrote("-- NO OFFER: the server said nothing within 10 s after ready; stopping without calling Claude".into(), &mut transcript);
                break;
            }
            Err(_) => {
                wrote(format!("-- TIMEOUT after {wait:?} without frames"), &mut transcript);
                break;
            }
        };
        let Message::Text(t) = msg else { continue };
        let Ok(f) = serde_json::from_str::<Value>(t.as_str()) else { continue };
        let now = t0.elapsed().as_millis();
        last_activity = Instant::now();
        // keep the journal small: audio payloads are saved as files, not logged
        let mut logged = f.clone();
        if logged["t"] == "audio" {
            logged["data"] = json!(format!("<{} base64 chars>", f["data"].as_str().map(|s| s.len()).unwrap_or(0)));
        }
        if logged["t"] == "snapshot" {
            logged["events"] = json!("<snapshot>");
        }
        let _ = writeln!(events, "{}", json!({"ms": now, "frame": logged}));

        let kind = f["t"].as_str().unwrap_or("").to_string();
        if let Some(s) = stats.get_mut(turn_idx).filter(|_| sent_turns > 0) {
            *s.events.entry(kind.clone()).or_insert(0) += 1;
            if s.first_state_ms.is_none() && kind == "state" {
                s.first_state_ms = Some(now - s.sent_ms);
            }
        }
        match kind.as_str() {
            "ready" => {
                ready = f.clone();
                wrote(
                    format!("ready at {now} ms: tts={} stt={} audioWaitMs={} title={}", f["tts"], f["stt"], f["audioWaitMs"], f["title"]),
                    &mut transcript,
                );
                if !wait_offer {
                    pending_send = Some(first_text);
                }
            }
            "capture_request" => {
                if let Some(s) = stats.get_mut(turn_idx) {
                    s.capture_request_ms = Some(now - s.sent_ms);
                }
                wrote(format!("[{now} ms] capture_request -> sending photo after 700 ms"), &mut transcript);
                tokio::time::sleep(Duration::from_millis(700)).await;
                ws.send(send(json!({"t":"photo","requestId":f["requestId"],"w":1200,"h":800,
                    "jpeg":base64::engine::general_purpose::STANDARD.encode(&jpeg)})))
                    .await
                    .unwrap();
            }
            "say" => {
                let seq = f["seq"].as_u64().unwrap_or(0);
                says.insert(seq, f["text"].as_str().unwrap_or("").to_string());
                if let Some(s) = stats.get_mut(turn_idx) {
                    if s.first_say_ms.is_none() {
                        s.first_say_ms = Some(now - s.sent_ms);
                    }
                }
                wrote(format!("[{now} ms] SAY #{seq} audio={}: {}", f["audio"], f["text"].as_str().unwrap_or("")), &mut transcript);
            }
            "audio" => {
                let seq = f["seq"].as_u64().unwrap_or(0);
                let bytes = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap_or("")).unwrap_or_default();
                if let Some(s) = stats.get_mut(turn_idx) {
                    if s.first_audio_ms.is_none() {
                        s.first_audio_ms = Some(now - s.sent_ms);
                    }
                    s.audio_bytes += bytes.len();
                }
                let file = out.join(format!("turn{}_seq{seq:03}.mp3", sent_turns));
                let _ = std::fs::write(&file, &bytes);
                let (kbps, secs) = mp3_info(&bytes).unwrap_or((0, 0.0));
                let sync = bytes.starts_with(b"ID3") || (bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0);
                wrote(
                    format!(
                        "[{now} ms] AUDIO #{seq}: {} bytes, mp3 header={sync}, {kbps} kbps, {secs:.2} s for {} chars",
                        bytes.len(),
                        says.get(&seq).map(|s| s.chars().count()).unwrap_or(0)
                    ),
                    &mut transcript,
                );
            }
            "step" => wrote(format!("[{now} ms] STEP {}: {}", f["index"], f["title"]), &mut transcript),
            "show" => {
                let b = &f["block"];
                let mut brief = b.to_string();
                if brief.chars().count() > 160 {
                    brief = brief.chars().take(160).collect::<String>() + "...";
                }
                wrote(format!("[{now} ms] SHOW {brief}"), &mut transcript);
            }
            "ask" => {
                ask_seen_in_turn = true;
                wrote(format!("[{now} ms] ASK ({}): {}", f["kind"], f["prompt"]), &mut transcript);
            }
            "done" => wrote(format!("[{now} ms] DONE"), &mut transcript),
            "title" | "notice" | "heard" => wrote(format!("[{now} ms] {kind}: {f}"), &mut transcript),
            "error" => {
                wrote(format!("[{now} ms] ERROR: {f}"), &mut transcript);
                if f["code"] == "rate_limited" {
                    wrote("-- Claude rate limit: stopping, not retrying".into(), &mut transcript);
                    break;
                }
            }
            "turn_end" => {
                if let Some(s) = stats.get_mut(turn_idx) {
                    s.turn_end_ms = Some(now - s.sent_ms);
                }
                wrote(format!("[{now} ms] TURN_END (ask seen: {ask_seen_in_turn})"), &mut transcript);
                // let trailing audio frames arrive, then go on
                let quiet = Duration::from_millis(2500);
                while let Ok(Some(Ok(Message::Text(t)))) = tokio::time::timeout(quiet, ws.next()).await {
                    if let Ok(f) = serde_json::from_str::<Value>(t.as_str()) {
                        let now = t0.elapsed().as_millis();
                        if f["t"] == "audio" {
                            let seq = f["seq"].as_u64().unwrap_or(0);
                            let bytes = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap_or("")).unwrap_or_default();
                            if let Some(s) = stats.get_mut(turn_idx) {
                                if s.first_audio_ms.is_none() {
                                    s.first_audio_ms = Some(now - s.sent_ms);
                                }
                                s.audio_bytes += bytes.len();
                                *s.events.entry("audio".into()).or_insert(0) += 1;
                            }
                            let _ = std::fs::write(out.join(format!("turn{}_seq{seq:03}.mp3", sent_turns)), &bytes);
                            let (kbps, secs) = mp3_info(&bytes).unwrap_or((0, 0.0));
                            wrote(format!("[{now} ms] AUDIO #{seq} (late): {} bytes, {kbps} kbps, {secs:.2} s", bytes.len()), &mut transcript);
                        } else {
                            let k = f["t"].as_str().unwrap_or("").to_string();
                            if let Some(s) = stats.get_mut(turn_idx) {
                                *s.events.entry(k).or_insert(0) += 1;
                            }
                        }
                    }
                }
                if sent_turns == 0 {
                    // the server's own turn (review offer): now the student answers it
                    wrote("-- server-initiated turn finished (review offer); student replies".into(), &mut transcript);
                    pending_send = Some(first_text);
                } else if sent_turns == 1 {
                    pending_send = Some(second_text);
                } else {
                    done = true;
                }
            }
            _ => {}
        }
    }
    let _ = last_activity;

    println!("\n==== SUMMARY ====");
    println!("ready: {}", ready);
    for (i, s) in stats.iter().enumerate() {
        println!(
            "turn {}: first state {:?} ms, capture_request {:?} ms, first say {:?} ms, first audio {:?} ms, turn_end {:?} ms (all relative to sending the student's words); audio {} bytes; events {:?}",
            i + 1,
            s.first_state_ms,
            s.capture_request_ms,
            s.first_say_ms,
            s.first_audio_ms,
            s.turn_end_ms,
            s.audio_bytes,
            s.events
        );
    }
}
