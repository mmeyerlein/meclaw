//! GH #1410 -- a strict streaming provider for the `llm` cell's three wires.
//!
//! A raw socket on `127.0.0.1:0` that takes one request per scripted answer,
//! checks it the way the real provider would refuse it, and answers with an
//! SSE event stream. A script may hold its stream open at a gate until the
//! test releases it: a partial the test receives while the gate is shut was
//! provably emitted before the stream ended -- an event, never a clock.
//!
//! Strict (GH #1283): a request a real provider would answer differently is
//! refused with `400` and a body naming the rule, so a cell that forgets to
//! ask for a stream gets an error, not a stream it did not ask for:
//!
//! * every dialect -- the method is `POST`;
//! * [`Dialect::Chat`] -- `POST …/chat/completions`, `authorization: Bearer`,
//!   `stream: true` and `stream_options.include_usage: true`, `model` and
//!   `messages`;
//! * [`Dialect::Responses`] -- `POST …/responses`, `authorization: Bearer`,
//!   `stream: true`, `model` and `input`;
//! * [`Dialect::Messages`] -- `POST …/messages`, `x-api-key` and
//!   `anthropic-version` headers, `stream: true`, `model`, `max_tokens` and
//!   `messages`, and no top-level key outside [`MESSAGES_KEYS`] (Anthropic
//!   answers `400` "Extra inputs are not permitted").
//!
//! The fixture builders ([`chat`], [`responses`], [`messages`]) write the
//! `data:` frames of each wire in the shape the providers document.

use meclaw_core::serde_json::{self, Value, json};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

/// Which wire the provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// OpenAI-compatible chat completions.
    Chat,
    /// OpenAI Responses.
    Responses,
    /// Anthropic Messages.
    Messages,
}

/// One scripted answer: `head` goes out at once, `tail` after the test
/// releases the gate (no gate when `tail` is empty and `hold` is false).
/// With `hold` the stream stays open after the tail until the provider is
/// dropped -- an answer that never ends.
#[derive(Debug, Clone, Default)]
pub struct Script {
    /// Frames sent before the gate.
    pub head: Vec<String>,
    /// Frames sent after [`SseProvider::release`].
    pub tail: Vec<String>,
    /// Keep the stream open after the tail.
    pub hold: bool,
}

impl Script {
    /// A stream sent whole, without a gate.
    pub fn whole(frames: Vec<String>) -> Self {
        Self {
            head: frames,
            ..Self::default()
        }
    }

    /// A stream that waits at the gate after `head`.
    pub fn gated(head: Vec<String>, tail: Vec<String>) -> Self {
        Self {
            head,
            tail,
            hold: false,
        }
    }
}

/// The top-level keys a Messages request may carry (review M3: Anthropic
/// refuses every other one with `400`).
pub const MESSAGES_KEYS: &[&str] = &[
    "model",
    "max_tokens",
    "messages",
    "system",
    "tools",
    "tool_choice",
    "thinking",
    "temperature",
    "stream",
    "metadata",
    "stop_sequences",
    "top_p",
    "top_k",
    "output_config",
];

/// A request the provider read.
#[derive(Debug, Clone)]
pub struct SeenRequest {
    /// Request method.
    pub method: String,
    /// Request path.
    pub path: String,
    /// Lower-cased header names to values.
    pub headers: HashMap<String, String>,
    /// The JSON body.
    pub body: Value,
}

/// The running provider.
pub struct SseProvider {
    /// `http://127.0.0.1:<port>/v1`.
    pub base_url: String,
    gates: Vec<Option<oneshot::Sender<()>>>,
    seen: mpsc::UnboundedReceiver<SeenRequest>,
    _stop: oneshot::Sender<()>,
}

impl SseProvider {
    /// Open the gate of script `i`.
    pub fn release(&mut self, i: usize) {
        if let Some(tx) = self.gates.get_mut(i).and_then(Option::take) {
            let _ = tx.send(());
        }
    }

    /// The next request the provider read (in order); `None` once the
    /// provider is gone without reading another (library code returns, the
    /// test decides -- the unwrap budget of this crate).
    pub async fn request(&mut self) -> Option<SeenRequest> {
        self.seen.recv().await
    }
}

/// The rule a request breaks, if any.
fn refusal(dialect: Dialect, req: &SeenRequest) -> Option<String> {
    let b = &req.body;
    let need = |key: &str| b.get(key).is_none_or(Value::is_null);
    let (path, keys): (&str, &[&str]) = match dialect {
        Dialect::Chat => ("/chat/completions", &["model", "messages"]),
        Dialect::Responses => ("/responses", &["model", "input"]),
        Dialect::Messages => ("/messages", &["model", "max_tokens", "messages"]),
    };
    if req.method != "POST" {
        return Some(format!("method {} is not POST", req.method));
    }
    if !req.path.ends_with(path) {
        return Some(format!("path {} is not {path}", req.path));
    }
    if let Some(k) = keys.iter().find(|k| need(k)) {
        return Some(format!("{k} is required"));
    }
    if b.get("stream") != Some(&Value::Bool(true)) {
        return Some("this provider answers streamed requests only".into());
    }
    if dialect == Dialect::Chat
        && b.pointer("/stream_options/include_usage") != Some(&Value::Bool(true))
    {
        return Some("stream_options.include_usage must be true".into());
    }
    if dialect == Dialect::Messages {
        for h in ["x-api-key", "anthropic-version"] {
            if !req.headers.contains_key(h) {
                return Some(format!("header {h} is required"));
            }
        }
        if let Some(k) = b
            .as_object()
            .and_then(|m| m.keys().find(|k| !MESSAGES_KEYS.contains(&k.as_str())))
        {
            return Some(format!("{k}: Extra inputs are not permitted"));
        }
    } else if !req
        .headers
        .get("authorization")
        .is_some_and(|v| v.starts_with("Bearer ") && v.len() > "Bearer ".len())
    {
        return Some("header authorization: Bearer <key> is required".into());
    }
    None
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<SeenRequest> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let (head_end, len) = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (pos + 4, len);
        }
    };
    while buf.len() < head_end + len {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end - 4]).to_string();
    let mut lines = head.lines();
    let request_line: Vec<&str> = lines.next().unwrap_or("").split_whitespace().collect();
    let method = request_line.first().copied().unwrap_or("").to_string();
    let path = request_line.get(1).copied().unwrap_or("").to_string();
    let headers = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    let body = serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
    Some(SeenRequest {
        method,
        path,
        headers,
        body,
    })
}

/// Start a provider of `dialect` answering `scripts` in order, one request
/// each. A loopback bind that fails is the caller's error to name.
pub async fn start(dialect: Dialect, scripts: Vec<Script>) -> std::io::Result<SseProvider> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (seen_tx, seen) = mpsc::unbounded_channel();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let mut gates = Vec::new();
    let mut gate_rxs = Vec::new();
    for _ in &scripts {
        let (tx, rx) = oneshot::channel::<()>();
        gates.push(Some(tx));
        gate_rxs.push(rx);
    }
    tokio::spawn(async move {
        for (script, gate) in scripts.into_iter().zip(gate_rxs) {
            let accepted = tokio::select! {
                a = listener.accept() => a,
                _ = &mut stop_rx => return,
            };
            let Ok((mut sock, _)) = accepted else {
                return;
            };
            let Some(req) = read_request(&mut sock).await else {
                return;
            };
            let refused = refusal(dialect, &req);
            let _ = seen_tx.send(req);
            if let Some(why) = refused {
                let body =
                    json!({"error": {"type": "invalid_request_error", "message": why}}).to_string();
                let head = format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                continue;
            }
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                        connection: close\r\n\r\n";
            if sock.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            for f in &script.head {
                if sock.write_all(f.as_bytes()).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
            }
            if !script.tail.is_empty() {
                tokio::select! {
                    _ = gate => {}
                    _ = &mut stop_rx => return,
                }
                for f in &script.tail {
                    if sock.write_all(f.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                }
            }
            if script.hold {
                let _ = (&mut stop_rx).await;
                return;
            }
            let _ = sock.shutdown().await;
        }
    });
    Ok(SseProvider {
        base_url: format!("http://{addr}/v1"),
        gates,
        seen,
        _stop: stop_tx,
    })
}

/// One `data:` frame.
pub fn frame(event: &Value) -> String {
    format!("data: {event}\n\n")
}

/// Chat-completions frames.
pub mod chat {
    use super::frame;
    use meclaw_core::serde_json::{Value, json};

    fn chunk(delta: Value, finish: Option<&str>) -> String {
        frame(&json!({
            "id": "chatcmpl-sse", "object": "chat.completion.chunk", "model": "sse-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
        }))
    }

    /// A visible text delta.
    pub fn text(t: &str) -> String {
        chunk(json!({"content": t}), None)
    }

    /// A thinking delta (`reasoning`).
    pub fn reasoning(t: &str) -> String {
        chunk(json!({"reasoning": t}), None)
    }

    /// The opening delta of tool call `index`.
    pub fn call_start(index: u64, id: &str, name: &str) -> String {
        chunk(
            json!({"tool_calls": [{"index": index, "id": id, "type": "function",
                                   "function": {"name": name, "arguments": ""}}]}),
            None,
        )
    }

    /// An argument delta of tool call `index`.
    pub fn call_args(index: u64, part: &str) -> String {
        chunk(
            json!({"tool_calls": [{"index": index, "function": {"arguments": part}}]}),
            None,
        )
    }

    /// A mid-stream failure chunk (a top-level `error`, GH #75).
    pub fn error(message: &str) -> String {
        frame(&json!({"error": {"message": message, "type": "server_error", "code": 502}}))
    }

    /// The finish chunk, the usage chunk and `[DONE]`.
    pub fn end(finish: &str, prompt: u64, completion: u64) -> Vec<String> {
        vec![
            chunk(json!({}), Some(finish)),
            frame(&json!({
                "id": "chatcmpl-sse", "object": "chat.completion.chunk", "model": "sse-model",
                "choices": [], "usage": {"prompt_tokens": prompt, "completion_tokens": completion}
            })),
            "data: [DONE]\n\n".to_string(),
        ]
    }
}

/// Responses frames.
pub mod responses {
    use super::frame;
    use meclaw_core::serde_json::json;

    /// `response.created`.
    pub fn created() -> String {
        frame(&json!({"type": "response.created",
                      "response": {"id": "resp_sse", "model": "sse-model"}}))
    }

    /// A visible text delta.
    pub fn text(t: &str) -> String {
        frame(&json!({"type": "response.output_text.delta", "delta": t}))
    }

    /// A reasoning-summary delta (thinking, never visible text).
    pub fn reasoning(t: &str) -> String {
        frame(&json!({"type": "response.reasoning_summary_text.delta", "delta": t}))
    }

    /// The `function_call` item opening.
    pub fn call_added(call_id: &str, name: &str) -> String {
        frame(&json!({"type": "response.output_item.added", "item": {
            "type": "function_call", "call_id": call_id, "name": name, "arguments": ""}}))
    }

    /// An argument delta of the open `function_call`.
    pub fn call_args(part: &str) -> String {
        frame(&json!({"type": "response.function_call_arguments.delta", "delta": part}))
    }

    /// The done `function_call` item carrying the whole arguments.
    pub fn call_done(call_id: &str, name: &str, arguments: &str) -> String {
        frame(&json!({"type": "response.output_item.done", "item": {
            "type": "function_call", "call_id": call_id, "name": name,
            "arguments": arguments}}))
    }

    /// The done message item carrying the whole text.
    pub fn message_done(t: &str) -> String {
        frame(&json!({"type": "response.output_item.done", "item": {
            "type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": t}]}}))
    }

    /// `response.completed` with the usage.
    pub fn completed(input: u64, output: u64) -> String {
        frame(&json!({"type": "response.completed", "response": {
            "id": "resp_sse", "model": "sse-model",
            "usage": {"input_tokens": input, "output_tokens": output}}}))
    }
}

/// Anthropic Messages frames (`event:` line plus `data:` line).
pub mod messages {
    use meclaw_core::serde_json::{Value, json};

    fn event(kind: &str, data: Value) -> String {
        format!("event: {kind}\ndata: {data}\n\n")
    }

    /// `message_start` with the input usage.
    pub fn start(input_tokens: u64) -> String {
        event(
            "message_start",
            json!({"type": "message_start", "message": {
                "id": "msg_sse", "type": "message", "role": "assistant", "model": "sse-model",
                "content": [], "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": input_tokens, "output_tokens": 1}}}),
        )
    }

    /// `content_block_start` of a text block.
    pub fn text_start(index: u64) -> String {
        event(
            "content_block_start",
            json!({"type": "content_block_start", "index": index,
                   "content_block": {"type": "text", "text": ""}}),
        )
    }

    /// `content_block_start` of a thinking block.
    pub fn thinking_start(index: u64) -> String {
        event(
            "content_block_start",
            json!({"type": "content_block_start", "index": index,
                   "content_block": {"type": "thinking", "thinking": ""}}),
        )
    }

    /// A `thinking_delta`.
    pub fn thinking(index: u64, t: &str) -> String {
        event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": index,
                   "delta": {"type": "thinking_delta", "thinking": t}}),
        )
    }

    /// An `error` event in the middle of the stream.
    pub fn error(kind: &str, message: &str) -> String {
        event(
            "error",
            json!({"type": "error", "error": {"type": kind, "message": message}}),
        )
    }

    /// A `text_delta`.
    pub fn text(index: u64, t: &str) -> String {
        event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": index,
                   "delta": {"type": "text_delta", "text": t}}),
        )
    }

    /// `content_block_start` of a `tool_use` block.
    pub fn tool_start(index: u64, id: &str, name: &str) -> String {
        event(
            "content_block_start",
            json!({"type": "content_block_start", "index": index,
                   "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}}),
        )
    }

    /// An `input_json_delta`.
    pub fn tool_args(index: u64, part: &str) -> String {
        event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": index,
                   "delta": {"type": "input_json_delta", "partial_json": part}}),
        )
    }

    /// `content_block_stop`.
    pub fn stop(index: u64) -> String {
        event(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": index}),
        )
    }

    /// `message_delta` with the stop reason and the output usage, then
    /// `message_stop`.
    pub fn end(stop_reason: &str, output_tokens: u64) -> Vec<String> {
        vec![
            event(
                "message_delta",
                json!({"type": "message_delta",
                       "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                       "usage": {"output_tokens": output_tokens}}),
            ),
            event("message_stop", json!({"type": "message_stop"})),
        ]
    }

    /// A `ping`.
    pub fn ping() -> String {
        event("ping", json!({"type": "ping"}))
    }
}
