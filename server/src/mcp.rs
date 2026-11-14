//! Minimal MCP server (JSON-RPC over HTTP POST). The tutor's per-session route has two tools:
//! `capture_photo` and `memory_note`; the general route has the phone tools.
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

fn memory_tool_def() -> Value {
    json!({
        "name": "memory_note",
        "description": "Remember something about the student for next time. Call it only AFTER you have already spoken your first lines of this turn (the student is waiting to hear you; never make this the first thing you do in a turn), at most twice per turn, then keep talking and never mention it. kind=mistake: the student got a key step wrong (topic = short name of the kind of error, reuse the exact old name if [memory] already has it; note = how it went wrong). kind=mastered: the student did a previously weak topic right on their own, e.g. the review question. kind=style: a way of teaching that works for this student. kind=progress: ONLY when a whole lesson is finished and you are about to send @done (topic = what was covered); never at the start of a lesson.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "subject": {"type": "string", "description": "toan, ly, hoa, van, anh-ssat, duc, tin-cpp or chung (any other subject)"},
                "kind": {"type": "string", "enum": ["mistake", "mastered", "style", "progress"]},
                "topic": {"type": "string"},
                "note": {"type": "string"}
            },
            "required": ["subject", "kind", "topic"],
            "additionalProperties": false
        }
    })
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Per-session route (`/mcp/<sessionId>`, used by the tutor): `capture_photo` and `memory_note`.
pub async fn handle_message(session: Option<UnboundedSender<Cmd>>, msg: &Value) -> Option<Value> {
    dispatch(session, msg, false).await
}

/// General route (`/mcp`, any Claude Code session on this PC): all phone tools.
pub async fn handle_general_message(session: Option<UnboundedSender<Cmd>>, msg: &Value) -> Option<Value> {
    dispatch(session, msg, true).await
}

/// Handle one JSON-RPC message. `None` for notifications (no response body).
async fn dispatch(session: Option<UnboundedSender<Cmd>>, msg: &Value, general: bool) -> Option<Value> {
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
        "tools/list" => {
            let tools = if general { general_tools() } else { vec![tool_def(), memory_tool_def()] };
            ok(&id, json!({"tools": tools}))
        }
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            if general && GENERAL_TOOL_NAMES.contains(&name) {
                return Some(ok(&id, call_general(name, &msg["params"]["arguments"], session).await));
            }
            match name {
                "capture_photo" => ok(&id, capture(session).await),
                "memory_note" if !general => ok(&id, memory_note(session, &msg["params"]["arguments"]).await),
                _ => err(&id, -32602, "unknown tool"),
            }
        }
        _ => err(&id, -32601, "method not found"),
    })
}

const GENERAL_TOOL_NAMES: &[&str] = &["phone_status", "capture_photo", "speak", "present", "listen", "interrupt"];
const NO_PHONE: &str = "Không có điện thoại nào đang kết nối. Mở app Beluga Live trên iPhone và để nó nối tới server (Tailscale).";

fn general_tools() -> Vec<Value> {
    let obj = |props: Value, required: Value| json!({"type": "object", "properties": props, "required": required});
    vec![
        json!({"name": "phone_status",
            "description": "Is the Beluga Live iPhone connected? Returns connected, sessionId, title, state (idle/busy/listening), and which voice providers (tts/stt) are in use.",
            "inputSchema": obj(json!({}), json!([]))}),
        json!({"name": "capture_photo",
            "description": "Take a sharp photo with the phone camera and return it as an image. The student must point the phone at the subject first.",
            "inputSchema": obj(json!({}), json!([]))}),
        json!({"name": "speak",
            "description": "Say text out loud on the phone (one or more sentences, plain speech, no markup). Fails if the tutor is mid-turn; call interrupt first to cut in.",
            "inputSchema": obj(json!({"text": {"type": "string"}}), json!(["text"]))}),
        json!({"name": "present",
            "description": "Send text in the lesson line format: plain lines are spoken; lines like `@step {\"title\":\"...\"}`, `@show {\"type\":\"math\",\"tex\":\"...\"}`, `@ask {...}`, `@done {}` drive the board. Returns how many events were sent and which lines were dropped as invalid.",
            "inputSchema": obj(json!({"lines": {"type": "string", "description": "newline-separated lesson lines"}}), json!(["lines"]))}),
        json!({"name": "listen",
            "description": "Wait for the student's next spoken utterance and return its text. While waiting, the utterance is NOT given to the tutor.",
            "inputSchema": obj(json!({"timeout_s": {"type": "number", "description": "default 30, max 120"}}), json!([]))}),
        json!({"name": "interrupt",
            "description": "Stop the tutor's running turn, or cut off speech queued by speak/present.",
            "inputSchema": obj(json!({}), json!([]))}),
    ]
}

fn text_ok(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

fn text_err(t: &str) -> Value {
    json!({"content": [{"type": "text", "text": t}], "isError": true})
}

async fn ask<T>(tx: &UnboundedSender<Cmd>, make: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> Option<T> {
    let (reply, rx) = oneshot::channel();
    tx.send(make(reply)).ok()?;
    rx.await.ok()
}

async fn call_general(name: &str, args: &Value, session: Option<UnboundedSender<Cmd>>) -> Value {
    let Some(tx) = session else {
        return if name == "phone_status" {
            text_ok(&json!({"connected": false, "hint": NO_PHONE}))
        } else {
            text_err(NO_PHONE)
        };
    };
    let closed = || text_err("Phiên đã đóng.");
    match name {
        "phone_status" => match ask(&tx, |reply| Cmd::Status { reply }).await {
            Some(v) => text_ok(&v),
            None => closed(),
        },
        "capture_photo" => capture(Some(tx)).await,
        "speak" => {
            let Some(text) = args["text"].as_str().filter(|t| !t.trim().is_empty()) else {
                return text_err("Thiếu tham số text.");
            };
            let text = text.to_string();
            match ask(&tx, |reply| Cmd::Speak { text, reply }).await {
                Some(Ok(v)) => text_ok(&v),
                Some(Err(e)) => text_err(&e),
                None => closed(),
            }
        }
        "present" => {
            let lines = match &args["lines"] {
                Value::String(s) => s.clone(),
                Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n"),
                _ => return text_err("Thiếu tham số lines."),
            };
            match ask(&tx, |reply| Cmd::Present { lines, reply }).await {
                Some(Ok(v)) => text_ok(&v),
                Some(Err(e)) => text_err(&e),
                None => closed(),
            }
        }
        "listen" => {
            let secs = args["timeout_s"].as_f64().unwrap_or(30.0).clamp(0.1, 120.0);
            let rx = match ask(&tx, |reply| Cmd::Listen { reply }).await {
                Some(Ok(rx)) => rx,
                Some(Err(e)) => return text_err(&e),
                None => return closed(),
            };
            match tokio::time::timeout(std::time::Duration::from_secs_f64(secs), rx).await {
                Ok(Ok(text)) => text_ok(&json!({"heard": text})),
                Ok(Err(_)) => text_err("Lệnh listen này bị thay bằng một lệnh listen khác."),
                Err(_) => text_ok(&json!({"heard": null, "text": format!("Không có ai nói trong {secs} giây.")})),
            }
        }
        "interrupt" => match ask(&tx, |reply| Cmd::McpInterrupt { reply }).await {
            Some(v) => text_ok(&v),
            None => closed(),
        },
        _ => text_err("unknown tool"),
    }
}

async fn memory_note(session: Option<UnboundedSender<Cmd>>, args: &Value) -> Value {
    let text_err = |t: &str| json!({"content": [{"type": "text", "text": t}], "isError": true});
    let Some(tx) = session else {
        return text_err("Không có phiên nào đang mở.");
    };
    let (reply, rx) = oneshot::channel();
    if tx.send(Cmd::MemoryNote { args: args.clone(), reply }).is_err() {
        return text_err("Phiên đã đóng.");
    }
    match rx.await {
        Ok(Ok(v)) => json!({"content": [{"type": "text", "text": v.to_string()}]}),
        Ok(Err(msg)) => text_err(&msg),
        Err(_) => text_err("Phiên đã đóng."),
    }
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

/// Handle a POST body (single message or batch) on the per-session route.
pub async fn handle_body(session: Option<UnboundedSender<Cmd>>, body: Value) -> Option<Value> {
    body_with(session, body, false).await
}

/// Same for the general route.
pub async fn handle_general_body(session: Option<UnboundedSender<Cmd>>, body: Value) -> Option<Value> {
    body_with(session, body, true).await
}

async fn body_with(session: Option<UnboundedSender<Cmd>>, body: Value, general: bool) -> Option<Value> {
    match body {
        Value::Array(items) => {
            let mut out = Vec::new();
            for m in &items {
                if let Some(r) = dispatch(session.clone(), m, general).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(Value::Array(out))
            }
        }
        m => dispatch(session, &m, general).await,
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
        assert_eq!(r["result"]["tools"][1]["name"], "memory_note");
        assert_eq!(r["result"]["tools"][1]["inputSchema"]["required"], json!(["subject", "kind", "topic"]));
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
    async fn memory_note_reaches_the_session_and_is_unknown_on_the_general_route() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            if let Some(Cmd::MemoryNote { args, reply }) = rx.recv().await {
                let _ = reply.send(if args["kind"] == "mistake" { Ok(json!({"saved": true})) } else { Err("bad kind".into()) });
            }
        });
        let call = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memory_note","arguments":{"subject":"toan","kind":"mistake","topic":"x"}}});
        let r = handle_message(Some(tx.clone()), &call).await.unwrap();
        assert!(r["result"]["isError"].is_null(), "{r}");
        assert_eq!(r["result"]["content"][0]["text"], "{\"saved\":true}");
        let r = handle_message(None, &call).await.unwrap();
        assert_eq!(r["result"]["isError"], true);
        let r = handle_general_message(Some(tx), &call).await.unwrap();
        assert!(r.get("error").is_some(), "memory_note is not a phone tool: {r}");
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
