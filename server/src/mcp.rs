//! Minimal MCP server (JSON-RPC over HTTP POST) exposing the single `capture_photo` tool.
//!
//! Claude Code is pointed at `http://127.0.0.1:<port>/mcp/<sessionId>` via `--mcp-config`,
//! so the tool call lands in the right phone session without a second process.

use crate::session::Cmd;
use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

pub const PROTOCOL_VERSION: &str = "2025-03-26";

fn tool_def() -> Value {
    json!({
        "name": "capture_photo",
        "description": "Take a sharp photo with the student's phone camera (their problem sheet or notebook) and return it as an image. Use it when you have not seen the problem yet or need to re-check. If the image is blurry or cut off, tell the student briefly what to adjust and call it again.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
    })
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Handle one JSON-RPC message. `None` for notifications (no response body).
pub async fn handle_message(session: Option<UnboundedSender<Cmd>>, msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();
    let Some(id) = id else {
        return None; // notification (e.g. notifications/initialized)
    };
    Some(match method {
        "initialize" => {
            let requested = msg["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION);
            ok(
                &id,
                json!({
                    "protocolVersion": requested,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "beluga", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(&id, json!({"tools": [tool_def()]})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            if name != "capture_photo" {
                return Some(err(&id, -32602, "unknown tool"));
            }
            ok(&id, capture(session).await)
        }
        _ => err(&id, -32601, "method not found"),
    })
}

async fn capture(session: Option<UnboundedSender<Cmd>>) -> Value {
    let text_err = |t: &str| json!({"content": [{"type": "text", "text": t}], "isError": true});
    let Some(tx) = session else {
        return text_err("Không có phiên nào đang mở trên điện thoại.");
    };
    let (reply, rx) = oneshot::channel();
    if tx.send(Cmd::Capture { reply }).is_err() {
        return text_err("Phiên đã đóng.");
    }
    match rx.await {
        Ok(Ok(jpeg)) => json!({"content": [
            {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(jpeg), "mimeType": "image/jpeg"}
        ]}),
        Ok(Err(msg)) => text_err(&msg),
        Err(_) => text_err("Phiên đã đóng."),
    }
}

/// Handle a POST body (single message or batch).
pub async fn handle_body(session: Option<UnboundedSender<Cmd>>, body: Value) -> Option<Value> {
    match body {
        Value::Array(items) => {
            let mut out = Vec::new();
            for m in &items {
                if let Some(r) = handle_message(session.clone(), m).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(Value::Array(out))
            }
        }
        m => handle_message(session, &m).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_and_list() {
        let r = handle_message(None, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}))
            .await
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = handle_message(None, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await.unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], "capture_photo");
        assert!(handle_message(None, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await.is_none());
    }

    #[tokio::test]
    async fn capture_without_session_is_text_error() {
        let r = handle_message(None, &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"capture_photo","arguments":{}}}))
            .await
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["type"], "text");
    }

    #[tokio::test]
    async fn capture_returns_image_content() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            if let Some(Cmd::Capture { reply }) = rx.recv().await {
                let _ = reply.send(Ok(vec![0xFF, 0xD8, 0xFF]));
            }
        });
        let r = handle_message(Some(tx), &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"capture_photo"}}))
            .await
            .unwrap();
        assert_eq!(r["result"]["content"][0]["type"], "image");
        assert_eq!(r["result"]["content"][0]["mimeType"], "image/jpeg");
        assert_eq!(r["result"]["content"][0]["data"], "/9j/");
    }
}
