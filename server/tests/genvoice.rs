//! GenVoice TTS/STT against a local mock HTTP service (202 -> poll -> download), end to end through
//! the real server with the mock brain and a fake phone.

mod common;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use beluga_server::config::{Config, GenvoiceCfg};
use beluga_server::voice::STT_KEYTERMS;
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Mode {
    #[default]
    Ok,
    Http(u16),
    TaskFailed,
    Hang,
}

#[derive(Clone, Debug)]
struct TtsReq {
    text: String,
    lang: String,
    model: String,
    voice: String,
    key: String,
}

struct Job {
    text: String,
    done_at: Instant,
    mode: Mode,
    counted: bool,
}

#[derive(Default)]
struct Mock {
    reqs: Vec<TtsReq>,
    jobs: HashMap<String, Job>,
    latency_ms: Vec<u64>,
    default_latency: u64,
    modes: Vec<Mode>,
    default_mode: Mode,
    inflight: usize,
    max_inflight: usize,
    auth_hits: usize,
    auth_status: u16,
    audio_downloads: usize,
    stt_bodies: Vec<Vec<u8>>,
    stt_polls: HashMap<String, u32>,
}

type Shared = Arc<Mutex<Mock>>;

async fn auth_me(State(m): State<Shared>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    let mut g = m.lock().unwrap();
    g.auth_hits += 1;
    if g.auth_status != 0 && g.auth_status != 200 {
        return (StatusCode::from_u16(g.auth_status).unwrap(), Json(json!({"detail": "no"})));
    }
    assert_eq!(headers.get("xi-api-key").unwrap(), "test-key");
    (StatusCode::OK, Json(json!({"credit_balance": 100000})))
}

async fn tts_submit(
    State(m): State<Shared>,
    Path(voice): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut g = m.lock().unwrap();
    let n = g.reqs.len();
    g.reqs.push(TtsReq {
        text: body["text"].as_str().unwrap_or("").into(),
        lang: body["language_code"].as_str().unwrap_or("").into(),
        model: body["model_id"].as_str().unwrap_or("").into(),
        voice,
        key: headers.get("xi-api-key").and_then(|v| v.to_str().ok()).unwrap_or("").into(),
    });
    let mode = g.modes.get(n).copied().unwrap_or(g.default_mode);
    if let Mode::Http(c) = mode {
        return (StatusCode::from_u16(c).unwrap(), Json(json!({"detail": "denied"})));
    }
    let lat = g.latency_ms.get(n).copied().unwrap_or(g.default_latency);
    let text = body["text"].as_str().unwrap_or("").to_string();
    g.jobs.insert(
        n.to_string(),
        Job { text, done_at: Instant::now() + Duration::from_millis(lat), mode, counted: false },
    );
    g.inflight += 1;
    g.max_inflight = g.max_inflight.max(g.inflight);
    (StatusCode::ACCEPTED, Json(json!({"id": n.to_string(), "status": "pending"})))
}

async fn history(State(m): State<Shared>, Path(id): Path<String>, headers: HeaderMap) -> Json<Value> {
    let mut g = m.lock().unwrap();
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    let (status, newly) = match g.jobs.get_mut(&id) {
        None => ("failed", false),
        Some(job) => {
            if job.mode == Mode::Hang || Instant::now() < job.done_at {
                ("processing", false)
            } else {
                let newly = !job.counted;
                job.counted = true;
                (if job.mode == Mode::TaskFailed { "failed" } else { "completed" }, newly)
            }
        }
    };
    if newly {
        g.inflight -= 1;
    }
    match status {
        "completed" => Json(json!({"id": id, "status": "completed",
            "result": {"audio_url": format!("http://{host}/audio/{id}.mp3")}, "error": null})),
        "failed" => Json(json!({"id": id, "status": "failed", "error": "synthesis exploded"})),
        s => Json(json!({"id": id, "status": s, "progress": 10})),
    }
}

async fn audio(State(m): State<Shared>, Path(file): Path<String>, headers: HeaderMap) -> (StatusCode, Vec<u8>) {
    let mut g = m.lock().unwrap();
    if headers.get("xi-api-key").map(|v| v != "test-key").unwrap_or(true) {
        return (StatusCode::UNAUTHORIZED, vec![]);
    }
    g.audio_downloads += 1;
    let id = file.trim_end_matches(".mp3");
    let text = g.jobs.get(id).map(|j| j.text.clone()).unwrap_or_default();
    (StatusCode::OK, format!("MP3:{text}").into_bytes())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

async fn stt_submit(State(m): State<Shared>, body: Bytes) -> (StatusCode, Json<Value>) {
    let mut g = m.lock().unwrap();
    let id = format!("s{}", g.stt_bodies.len());
    g.stt_bodies.push(body.to_vec());
    (StatusCode::ACCEPTED, Json(json!({"task_id": id, "status": "pending"})))
}

async fn stt_get(State(m): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    let mut g = m.lock().unwrap();
    let n = {
        let c = g.stt_polls.entry(id.clone()).or_insert(0);
        *c += 1;
        *c
    };
    let idx: usize = id.trim_start_matches('s').parse().unwrap();
    let body = g.stt_bodies[idx].clone();
    if contains(&body, b"HANGSTT") || n < 2 {
        return Json(json!({"task_id": id, "status": "processing"}));
    }
    if contains(&body, b"BOOM") {
        return Json(json!({"task_id": id, "status": "failed", "detail_error": "bad audio"}));
    }
    let text = if contains(&body, b"SILENCE") { "" } else { "Đúng rồi, ok, next step là gì?" };
    Json(json!({"task_id": id, "status": "completed", "result": {"text": text, "language_code": "vie"}}))
}

async fn start_mock(m: Shared) -> SocketAddr {
    let app = Router::new()
        .route("/v1/auth/me", get(auth_me))
        .route("/v1/text-to-speech/{voice}", post(tts_submit))
        .route("/v1/history/{id}", get(history))
        .route("/audio/{file}", get(audio))
        .route("/v1/speech-to-text", post(stt_submit))
        .route("/v1/speech-to-text/{id}", get(stt_get))
        .with_state(m);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    addr
}

struct Opts {
    latency: u64,
    modes: Vec<Mode>,
    default_mode: Mode,
    latencies: Vec<u64>,
    auth_status: u16,
    brain_delay_ms: u64,
    tts_base_ms: u64,
    tts_per_char_ms: u64,
    stt_base_ms: u64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            latency: 100,
            modes: vec![],
            default_mode: Mode::Ok,
            latencies: vec![],
            auth_status: 200,
            brain_delay_ms: 5,
            tts_base_ms: 8000,
            tts_per_char_ms: 60,
            stt_base_ms: 8000,
        }
    }
}

struct Env {
    addr: SocketAddr,
    mock: Shared,
}

async fn env(o: Opts) -> Env {
    let mock: Shared = Arc::new(Mutex::new(Mock {
        default_latency: o.latency,
        latency_ms: o.latencies,
        modes: o.modes,
        default_mode: o.default_mode,
        auth_status: o.auth_status,
        ..Default::default()
    }));
    let mock_addr = start_mock(mock.clone()).await;
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("board")).unwrap();
    let cfg = Config {
        port: 0,
        mock: true,
        mock_delay_ms: o.brain_delay_ms,
        capture_timeout_ms: 8000,
        data_dir: dir.join("data"),
        board_dir: dir.join("board"),
        tutor_dir: dir.join("tutor"),
        claude_bin: "claude".into(),
        claude_effort: None,
        claude_model_map: vec![],
        eleven: None,
        genvoice: Some(GenvoiceCfg {
            api_key: "test-key".into(),
            voice_id: "5yEPTrA0uAhZsBEhHjfi".into(),
            tts_model: "eleven_flash_v2_5".into(),
            base_url: format!("http://{mock_addr}"),
            poll_ms: 15,
            audio_wait_ms: 20_000,
        }),
        tts_provider: "auto".into(),
        stt_provider: "auto".into(),
        tts_base_ms: o.tts_base_ms,
        tts_per_char_ms: o.tts_per_char_ms,
        stt_base_ms: o.stt_base_ms,
        stt_per_sec_ms: 0,
        fixed_pair_code: Some(CODE.into()),
        models_path: std::path::PathBuf::from("no-models.json"),
        secrets: Default::default(),
        compact_tokens: 120_000,
        util_in_mock: false,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = beluga_server::start(cfg, listener).await;
    });
    Env { addr, mock }
}

fn says(f: &[Value]) -> Vec<&Value> {
    f.iter().filter(|x| x["t"] == "say").collect()
}

fn of_type<'a>(f: &'a [Value], t: &str) -> Vec<&'a Value> {
    f.iter().filter(|x| x["t"] == t).collect()
}

fn notices<'a>(f: &'a [Value], code: &str) -> Vec<&'a Value> {
    f.iter().filter(|x| x["t"] == "notice" && x["code"] == code).collect()
}

fn decode(f: &Value) -> String {
    let b = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap()).unwrap();
    String::from_utf8(b).unwrap()
}

// ------------------------------------------------------------------ TTS

#[tokio::test]
async fn tts_flow_and_request_shape() {
    let e = env(Opts::default()).await;
    let mut p = Phone::connect(e.addr).await;
    let ready = p.hello(json!({})).await;
    assert_eq!(ready["tts"], "server");
    assert_eq!(ready["stt"], "server");
    assert_eq!(ready["audioWaitMs"], 20000);
    {
        let g = e.mock.lock().unwrap();
        assert_eq!(g.auth_hits, 1, "probe uses auth/me");
        assert_eq!(g.reqs.len(), 0, "probe must not synthesize (it costs credits)");
    }
    p.send(json!({"t":"user_text","text":"hi"})).await;
    let mut frames = p.until(is_turn_end).await;
    frames.extend(p.drain_quiet(400).await);
    let s = says(&frames);
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|x| x["audio"] == true));
    for say in &s {
        let a = frames.iter().find(|f| f["t"] == "audio" && f["seq"] == say["seq"]).expect("audio for each say");
        assert_eq!(a["mime"], "audio/mpeg");
        assert_eq!(decode(a), format!("MP3:{}", say["text"].as_str().unwrap()));
    }
    {
        let g = e.mock.lock().unwrap();
        assert_eq!(g.reqs.len(), 2);
        for say in &s {
            assert!(g.reqs.iter().any(|r| r.text == say["text"].as_str().unwrap()), "each sentence is sent verbatim");
        }
        for r in &g.reqs {
            assert_eq!(r.lang, "vi");
            assert_eq!(r.model, "eleven_flash_v2_5");
            assert_eq!(r.voice, "5yEPTrA0uAhZsBEhHjfi");
            assert_eq!(r.key, "test-key");
        }
    }
    // language follows settings
    p.send(json!({"t":"settings","lang":"en","model":"sonnet","rigor":"medium"})).await;
    p.send(json!({"t":"user_text","text":"xong"})).await;
    p.until(is_turn_end).await;
    p.drain_quiet(300).await;
    assert_eq!(e.mock.lock().unwrap().reqs[2].lang, "en");
}

#[tokio::test]
async fn audio_is_created_in_parallel_and_may_arrive_before_its_turn() {
    let e = env(Opts { latencies: vec![700, 80], ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    let t0 = Instant::now();
    p.send(json!({"t":"user_text","text":"tìm nghiệm"})).await;
    let mut audio_seqs = Vec::new();
    let mut say_seqs = Vec::new();
    while audio_seqs.len() < 2 {
        let f = p.next().await.expect("frame");
        match f["t"].as_str().unwrap() {
            "say" => say_seqs.push(f["seq"].as_u64().unwrap()),
            "audio" => audio_seqs.push(f["seq"].as_u64().unwrap()),
            _ => {}
        }
    }
    assert_eq!(say_seqs.len(), 2);
    assert_eq!(audio_seqs[0], say_seqs[1], "the quick second sentence's audio arrives first");
    assert_eq!(audio_seqs[1], say_seqs[0]);
    assert!(t0.elapsed() < Duration::from_millis(1400), "parallel, not sequential: {:?}", t0.elapsed());
    assert_eq!(e.mock.lock().unwrap().max_inflight, 2);
}

/// Guards against a sentence's audio being attached to another sentence's `seq`: every sentence
/// gets a different body from the service, and the tasks finish in a scrambled order (a long
/// early sentence finishing last, a late one first). Each `audio` frame must carry the body that
/// the service made for exactly the text of its own `say`.
#[tokio::test]
async fn each_sentence_gets_its_own_audio_when_tasks_finish_out_of_order() {
    let lat: Vec<u64> = vec![900, 60, 700, 20, 500, 300, 800, 10, 400, 650, 90, 250];
    let e = env(Opts { latencies: lat, latency: 40, brain_delay_ms: 3, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"bài dài"})).await; // 40 numbered sentences, then @done
    let mut says: HashMap<u64, String> = HashMap::new();
    let mut audio_order: Vec<u64> = Vec::new();
    let mut bodies: HashMap<u64, String> = HashMap::new();
    let mut ended = false;
    while !(ended && bodies.len() == says.len()) {
        let Some(f) = p.next_within(8000).await else { panic!("timeout: {} says, {} audio", says.len(), bodies.len()) };
        match f["t"].as_str().unwrap() {
            "say" => {
                says.insert(f["seq"].as_u64().unwrap(), f["text"].as_str().unwrap().to_string());
            }
            "audio" => {
                let seq = f["seq"].as_u64().unwrap();
                assert!(bodies.insert(seq, decode(&f)).is_none(), "two audio frames for seq {seq}");
                audio_order.push(seq);
            }
            "audio_skip" => panic!("unexpected audio_skip: {f}"),
            "turn_end" => ended = true,
            _ => {}
        }
    }
    assert_eq!(says.len(), 40);
    for (seq, text) in &says {
        assert_eq!(bodies[seq], format!("MP3:{text}"), "audio of seq {seq} belongs to another sentence");
    }
    let mut sorted = audio_order.clone();
    sorted.sort();
    assert_ne!(audio_order, sorted, "the test is only meaningful if audio really arrived out of order");
    // The service saw each text exactly once.
    let g = e.mock.lock().unwrap();
    let mut sent: Vec<&str> = g.reqs.iter().map(|r| r.text.as_str()).collect();
    sent.sort();
    sent.dedup();
    assert_eq!(sent.len(), 40);
}

#[tokio::test]
async fn at_most_four_sentences_are_synthesized_at_once() {
    let e = env(Opts { latency: 300, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"giảng bài dài"})).await;
    p.until(is_turn_end).await;
    p.drain_quiet(600).await;
    let g = e.mock.lock().unwrap();
    assert_eq!(g.max_inflight, 4);
    assert_eq!(g.reqs.len(), 40);
}

#[tokio::test]
async fn interrupt_cancels_pending_tts_tasks() {
    let e = env(Opts { latency: 600, brain_delay_ms: 30, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"giảng bài dài"})).await;
    let mut said = 0;
    while said < 6 {
        if p.next().await.unwrap()["t"] == "say" {
            said += 1;
        }
    }
    p.send(json!({"t":"interrupt"})).await;
    let f = p.until(|f| f["t"] == "turn_end" && f["interrupted"] == true).await;
    assert!(f.last().is_some());
    let c1 = e.mock.lock().unwrap().reqs.len();
    assert!(c1 <= 4, "only the 4 in-flight syntheses were ever started, got {c1}");
    let later = p.drain_quiet(1000).await;
    assert!(of_type(&later, "audio").is_empty(), "cancelled sentences never deliver audio: {later:?}");
    assert_eq!(e.mock.lock().unwrap().reqs.len(), c1, "queued syntheses were cancelled, not started");
}

// ------------------------------------------------------------------ TTS errors

#[tokio::test]
async fn http_401_switches_to_device_voice_with_one_notice() {
    let e = env(Opts { default_mode: Mode::Http(401), ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"hi"})).await;
    let mut f = p.until(is_turn_end).await;
    f.extend(p.drain_quiet(400).await);
    assert_eq!(notices(&f, "tts_fallback").len(), 1);
    // each sentence that was promised audio is released
    for say in says(&f).iter().filter(|s| s["audio"] == true) {
        assert!(f.iter().any(|x| x["t"] == "audio_skip" && x["seq"] == say["seq"]), "{say}");
    }
    p.send(json!({"t":"user_text","text":"xong"})).await;
    let mut f2 = p.until(is_turn_end).await;
    f2.extend(p.drain_quiet(200).await);
    assert!(says(&f2).iter().all(|s| s["audio"] == false));
    assert!(notices(&f2, "tts_fallback").is_empty(), "announced only once");
    let mut p2 = Phone::connect(e.addr).await;
    let ready = p2.hello(json!({})).await;
    assert_eq!(ready["tts"], "device");
    assert!(ready.get("audioWaitMs").is_none());
}

#[tokio::test]
async fn three_failed_sentences_in_a_row_switch_tts_off() {
    let e = env(Opts { default_mode: Mode::TaskFailed, brain_delay_ms: 20, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"giảng bài dài"})).await;
    let mut f = p.until(is_turn_end).await;
    f.extend(p.drain_quiet(300).await);
    assert_eq!(notices(&f, "tts_fallback").len(), 1);
    assert!(of_type(&f, "audio_skip").len() >= 3);
    let notice_at = f.iter().position(|x| x["t"] == "notice" && x["code"] == "tts_fallback").unwrap();
    let after: Vec<&Value> = f[notice_at..].iter().filter(|x| x["t"] == "say").collect();
    assert!(!after.is_empty(), "turn continued after the fallback");
    assert!(after.iter().all(|s| s["audio"] == false));
    assert!(e.mock.lock().unwrap().reqs.len() < 20, "no more syntheses after the switch");
}

#[tokio::test]
async fn one_failed_sentence_is_skipped_but_tts_stays_on() {
    let e = env(Opts { modes: vec![Mode::TaskFailed], ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"tìm nghiệm"})).await;
    let mut f = p.until(is_turn_end).await;
    f.extend(p.drain_quiet(400).await);
    let s = says(&f);
    assert_eq!(s.len(), 2);
    assert_eq!(of_type(&f, "audio_skip").len(), 1);
    assert_eq!(of_type(&f, "audio").len(), 1);
    assert!(notices(&f, "tts_fallback").is_empty());
    let mut p2 = Phone::connect(e.addr).await;
    assert_eq!(p2.hello(json!({})).await["tts"], "server");
}

#[tokio::test]
async fn sentence_over_its_deadline_is_skipped_and_cancelled() {
    // deadline = 250ms + 0 per char; the first synthesis hangs forever
    let e = env(Opts { modes: vec![Mode::Hang], tts_base_ms: 250, tts_per_char_ms: 0, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    p.send(json!({"t":"user_text","text":"tìm nghiệm"})).await;
    let mut f = p.until(is_turn_end).await;
    f.extend(p.drain_quiet(600).await);
    let first_say = says(&f)[0]["seq"].clone();
    let skip = of_type(&f, "audio_skip");
    assert_eq!(skip.len(), 1, "{f:?}");
    assert_eq!(skip[0]["seq"], first_say);
    assert_eq!(of_type(&f, "audio").len(), 1, "the other sentence still got its audio");
    assert!(notices(&f, "tts_fallback").is_empty());
}

#[tokio::test]
async fn startup_probe_failure_means_device_voice_and_one_notice() {
    let e = env(Opts { auth_status: 401, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    let ready = p.hello(json!({})).await;
    assert_eq!(ready["tts"], "device");
    assert!(ready.get("audioWaitMs").is_none());
    assert_eq!(p.next().await.unwrap()["code"], "tts_fallback");
    p.send(json!({"t":"user_text","text":"hi"})).await;
    let f = p.until(is_turn_end).await;
    assert!(says(&f).iter().all(|s| s["audio"] == false));
    assert_eq!(e.mock.lock().unwrap().reqs.len(), 0);
}

// ------------------------------------------------------------------ STT

async fn send_audio(p: &mut Phone, chunks: &[&[u8]]) {
    p.send(json!({"t":"audio_start","mime":"audio/wav","sampleRate":16000})).await;
    for c in chunks {
        p.send_binary(c.to_vec()).await;
    }
    p.send(json!({"t":"audio_end"})).await;
}

#[tokio::test]
async fn stt_wraps_pcm_in_wav_and_feeds_the_tutor() {
    let e = env(Opts::default()).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({"settings":{"lang":"en"}})).await;
    send_audio(&mut p, &[b"pcm-a", b"pcm-b"]).await;
    let f = p.until(is_turn_end).await;
    let heard = of_type(&f, "heard");
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0]["text"], "Đúng rồi, ok, next step là gì?");
    assert!(f.iter().position(|x| x["t"] == "heard") < f.iter().position(|x| x["t"] == "say"));
    let body = e.mock.lock().unwrap().stt_bodies[0].clone();
    assert!(body.starts_with(b"--"), "multipart");
    assert_eq!(count(&body, b"RIFF"), 1);
    assert!(contains(&body, b"WAVEfmt "));
    assert!(contains(&body, b"pcm-apcm-b"));
    assert!(contains(&body, b"name=\"language_code\"\r\n\r\nen\r\n"));
    assert!(contains(&body, b"name=\"timestamps_granularity\"\r\n\r\nnone\r\n"));
    assert!(contains(&body, b"name=\"tag_audio_events\"\r\n\r\nfalse\r\n"));
    assert_eq!(count(&body, b"name=\"keyterms\""), STT_KEYTERMS.len());
    assert!(contains(&body, b"name=\"file\"; filename=\"speech.wav\""));
}

#[tokio::test]
async fn stt_does_not_double_wrap_a_complete_wav() {
    let e = env(Opts::default()).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;
    send_audio(&mut p, &[b"RIFF\x24\0\0\0WAVEfmt ", b"audio-bytes"]).await;
    p.until(is_turn_end).await;
    let body = e.mock.lock().unwrap().stt_bodies[0].clone();
    assert_eq!(count(&body, b"RIFF"), 1);
    assert!(contains(&body, b"name=\"language_code\"\r\n\r\nvi\r\n"));
}

#[tokio::test]
async fn stt_empty_failed_and_timeout_never_reach_the_tutor() {
    let e = env(Opts { stt_base_ms: 400, ..Default::default() }).await;
    let mut p = Phone::connect(e.addr).await;
    p.hello(json!({})).await;

    send_audio(&mut p, &[b"SILENCE"]).await;
    let f = p.next().await.unwrap();
    assert_eq!((f["t"].as_str(), f["code"].as_str()), (Some("notice"), Some("stt_empty")));

    send_audio(&mut p, &[b"BOOM"]).await;
    let f = p.next().await.unwrap();
    assert_eq!(f["code"], "stt_failed");

    let t0 = Instant::now();
    send_audio(&mut p, &[b"HANGSTT"]).await;
    let f = p.next().await.unwrap();
    assert_eq!(f["code"], "stt_failed");
    assert!(t0.elapsed() >= Duration::from_millis(350), "waited for the deadline: {:?}", t0.elapsed());

    send_audio(&mut p, &[]).await; // nothing recorded
    assert_eq!(p.next().await.unwrap()["code"], "stt_empty");

    let rest = p.drain_quiet(300).await;
    assert!(rest.iter().all(|x| x["t"] != "say" && x["t"] != "state"), "no turn was started: {rest:?}");
}
