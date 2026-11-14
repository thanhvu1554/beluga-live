#![allow(dead_code)]
//! Shared helpers for integration tests: a fake phone and temp dirs.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub const CODE: &str = "424242";

pub fn temp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("beluga-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub struct Phone {
    pub ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Phone {
    pub async fn connect(addr: std::net::SocketAddr) -> Phone {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
        Phone { ws }
    }

    pub async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    pub async fn send_binary(&mut self, b: Vec<u8>) {
        self.ws.send(Message::Binary(b.into())).await.unwrap();
    }

    /// Next JSON frame, or None on close / timeout (`ms`).
    pub async fn next_within(&mut self, ms: u64) -> Option<Value> {
        loop {
            match tokio::time::timeout(Duration::from_millis(ms), self.ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).ok(),
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(_))) => continue,
                Err(_) => return None,
            }
        }
    }

    pub async fn next(&mut self) -> Option<Value> {
        self.next_within(5000).await
    }

    pub async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(f) = self.next().await {
            let done = pred(&f);
            out.push(f);
            if done {
                return out;
            }
        }
        panic!("stream ended before condition; got {out:#?}");
    }

    /// Everything that arrives within `ms` of silence.
    pub async fn drain_quiet(&mut self, ms: u64) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(f) = self.next_within(ms).await {
            out.push(f);
        }
        out
    }

    pub async fn hello(&mut self, extra: Value) -> Value {
        let mut h = json!({"t":"hello","v":1,"device":"test-phone","pairCode":CODE,
            "settings":{"lang":"vi","model":"sonnet","rigor":"medium"}});
        for (k, v) in extra.as_object().unwrap() {
            h[k] = v.clone();
        }
        self.send(h).await;
        self.next().await.expect("ready")
    }
}

pub fn is_turn_end(f: &Value) -> bool {
    f["t"] == "turn_end"
}

/// (bitrate kbps of the first frame, duration in seconds) of an MP3, by walking its frames.
pub fn mp3_info(b: &[u8]) -> Option<(u32, f64)> {
    let mut i = 0;
    if b.starts_with(b"ID3") && b.len() > 10 {
        i = 10 + (((b[6] & 0x7f) as usize) << 21 | ((b[7] & 0x7f) as usize) << 14 | ((b[8] & 0x7f) as usize) << 7 | (b[9] & 0x7f) as usize);
    }
    const BR_V1_L3: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const BR_V2_L3: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    const SR_V1: [u32; 3] = [44100, 48000, 32000];
    let (mut secs, mut first_br) = (0.0, None);
    while i + 4 <= b.len() {
        if b[i] != 0xFF || b[i + 1] & 0xE0 != 0xE0 {
            i += 1;
            continue;
        }
        let v1 = (b[i + 1] >> 3) & 3 == 3;
        let (bri, sri, pad) = ((b[i + 2] >> 4) as usize, ((b[i + 2] >> 2) & 3) as usize, ((b[i + 2] >> 1) & 1) as u32);
        let br = if v1 { BR_V1_L3[bri] } else { BR_V2_L3[bri] };
        if br == 0 || sri > 2 {
            i += 1;
            continue;
        }
        let sr = if v1 { SR_V1[sri] } else { SR_V1[sri] / if (b[i + 1] >> 3) & 3 == 2 { 2 } else { 4 } };
        let (samples, coef) = if v1 { (1152.0, 144) } else { (576.0, 72) };
        let len = (coef * br * 1000 / sr + pad) as usize;
        first_br.get_or_insert(br);
        secs += samples / sr as f64;
        i += len.max(4);
    }
    first_br.map(|br| (br, secs))
}

pub fn simple_hash(b: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}
