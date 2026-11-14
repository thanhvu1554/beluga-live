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
