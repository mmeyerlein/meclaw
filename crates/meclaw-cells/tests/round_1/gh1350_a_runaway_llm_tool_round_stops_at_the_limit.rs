//! GH #1350 -- a runaway tool round stops at the first call over the limit.
//!
//! Measured: a fast model answered one turn with 686 tool calls and 32768
//! output tokens over 129 s. The dispatcher's budget (16 calls in one answer)
//! applied only once the whole answer had arrived, and the cell is serial, so
//! the next turn waited behind it. With `tool_call_limit` set, the chat wire
//! streams the answer and the cell stops reading at the first call over the
//! limit -- an error the caller sees at once, not after the full answer.
//!
//! The provider here is a raw socket that streams synthetic tool-call deltas
//! and then holds the stream OPEN: a cell that waits for the full answer never
//! answers, and the hang mark fails the test instead of a verdict on time.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::deadline;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

/// One SSE frame of a chat-completions stream.
fn frame(chunk: &Value) -> String {
    format!("data: {chunk}\n\n")
}

/// The deltas of `n` tool calls, each split over two chunks (the name first,
/// then the arguments), the way a hosted router streams them.
fn tool_call_frames(n: usize) -> Vec<String> {
    let mut out = vec![frame(&json!({
        "id": "chatcmpl-s", "object": "chat.completion.chunk", "model": "fast-model",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Trying again."}}]
    }))];
    for i in 0..n {
        out.push(frame(&json!({
            "id": "chatcmpl-s", "object": "chat.completion.chunk", "model": "fast-model",
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": i, "id": format!("call_{i}"), "type": "function",
                "function": {"name": "fetch", "arguments": ""}
            }]}}]
        })));
        out.push(frame(&json!({
            "id": "chatcmpl-s", "object": "chat.completion.chunk", "model": "fast-model",
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": i, "function": {"arguments": format!("{{\"url\": \"https://example.com/{i}\"}}")}
            }]}}]
        })));
    }
    out
}

/// A provider on a raw socket: reads one request, answers `200` with an
/// event stream of `frames`, and then either ends the stream (`hold` false)
/// or keeps it open until the test ends (`hold` true). Hands back the base
/// url and the request body it read.
async fn stream_provider(
    frames: Vec<String>,
    hold: bool,
) -> (String, oneshot::Receiver<Value>, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (body_tx, body_rx) = oneshot::channel::<Value>();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let (head_end, len) = loop {
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0, "the request ended before its head");
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                break (pos + 4, len);
            }
        };
        while buf.len() < head_end + len {
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0, "the request ended before its body");
            buf.extend_from_slice(&tmp[..n]);
        }
        let body: Value =
            meclaw_core::serde_json::from_slice(&buf[head_end..head_end + len]).unwrap();
        let _ = body_tx.send(body);
        let head = if hold {
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n"
        } else {
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
        };
        sock.write_all(head.as_bytes()).await.unwrap();
        for f in frames {
            if hold {
                let chunk = format!("{:x}\r\n{f}\r\n", f.len());
                if sock.write_all(chunk.as_bytes()).await.is_err() {
                    return;
                }
            } else if sock.write_all(f.as_bytes()).await.is_err() {
                return;
            }
            let _ = sock.flush().await;
        }
        if hold {
            // The rest of the answer never comes: the stream stays open.
            let _ = stop_rx.await;
        }
    });
    (format!("http://{addr}/v1"), body_rx, stop_tx)
}

/// One turn of a fresh cell with `raw_params` and a one-tool menu; the
/// emitted body.
async fn drive(raw_params: Value) -> Value {
    let td = TempDir::new().unwrap();
    let params = LlmParams::parse(&raw_params).expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::new());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let tool = json!({"type": "function", "function": {"name": "fetch", "parameters": {}}});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"tools": {"fetch": {"text": tool.to_string()}}},
            "messages": [{"origin": "user", "type": "text", "text": "look it up"}]
        })))
        .build();
    deadline::hang("handle() returns", cell.handle(msg, &sink, &mut db)).await;
    deadline::hang("the cell answers", rx.recv())
        .await
        .expect("the cell answers")
        .content
}

fn params(base_url: &str, limit: u32) -> Value {
    json!({"provider": "openai", "model": "fast-model", "api_key": "k",
           "base_url": base_url, "tool_call_limit": limit,
           // The operation timeout is far beyond the hang mark: an answer
           // inside the mark is the cut, never this timeout.
           "external_timeout_ms": 600_000})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_call_over_the_limit_ends_the_round_while_the_stream_is_still_open() {
    let (base, body, _stop) = stream_provider(tool_call_frames(40), true).await;
    let out = drive(params(&base, 16)).await;
    let sent = body.await.expect("the provider read the request");
    assert_eq!(sent["stream"], true, "a capped round is streamed: {sent}");
    assert_eq!(out["header"]["finish_reason"], "error", "{out}");
    assert_eq!(out["header"]["error_code"], "provider_error", "{out}");
    let err = &out["meta"]["error"];
    assert_eq!(err["source"], "wire", "{out}");
    assert_eq!(err["kind"], "tool_call_limit", "{out}");
    assert_eq!(err["limit"], 16, "{out}");
    assert_eq!(err["calls"], 17, "the cut is at the 17th call: {out}");
    // The conversation goes back unchanged, as with every wire failure.
    assert_eq!(out["messages"][0]["text"], "look it up", "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_round_within_the_limit_is_the_answer_it_always_was() {
    let mut frames = tool_call_frames(3);
    frames.push(frame(&json!({
        "id": "chatcmpl-s", "object": "chat.completion.chunk", "model": "fast-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    })));
    frames.push(frame(&json!({
        "id": "chatcmpl-s", "object": "chat.completion.chunk", "model": "fast-model",
        "choices": [], "usage": {"prompt_tokens": 50, "completion_tokens": 60}
    })));
    frames.push("data: [DONE]\n\n".to_string());
    let (base, _body, _stop) = stream_provider(frames, false).await;
    let out = drive(params(&base, 16)).await;
    assert_eq!(out["header"]["finish_reason"], "tool_calls", "{out}");
    let calls: Vec<&Value> = out["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|t| t["type"] == "tool_call")
        .collect();
    assert_eq!(calls.len(), 3, "{out}");
    for (i, call) in calls.iter().enumerate() {
        assert_eq!(call["id"], format!("call_{i}"), "{out}");
        let function: Value =
            meclaw_core::serde_json::from_str(call["text"].as_str().unwrap()).unwrap();
        assert_eq!(function["name"], "fetch", "{out}");
        let args: Value =
            meclaw_core::serde_json::from_str(function["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["url"], format!("https://example.com/{i}"), "{out}");
    }
    assert_eq!(out["header"]["tokens_completion"], 60, "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_limit_on_a_one_body_answer_is_the_same_error() {
    // A server that ignores `stream` and answers one JSON body: the cell
    // counts the calls in it and answers with the same error.
    let calls: Vec<Value> = (0..20)
        .map(|i| {
            json!({"id": format!("call_{i}"), "type": "function",
                        "function": {"name": "fetch", "arguments": "{}"}})
        })
        .collect();
    let body = json!({
        "id": "chatcmpl-1", "object": "chat.completion", "model": "fast-model",
        "choices": [{"index": 0, "finish_reason": "tool_calls",
                     "message": {"role": "assistant", "content": null, "tool_calls": calls}}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 400}
    });
    let mock =
        mock_openai::MockOpenAI::start(vec![meclaw_testing::mock_http::MockResponse::ok_json(
            body.to_string().as_bytes(),
        )])
        .await;
    let out = drive(params(&format!("{}/v1", mock.base_url), 16)).await;
    assert_eq!(out["header"]["error_code"], "provider_error", "{out}");
    assert_eq!(out["meta"]["error"]["kind"], "tool_call_limit", "{out}");
    assert_eq!(out["meta"]["error"]["calls"], 20, "{out}");
}

/// `rel` names the template file from the repository root, so the line that
/// reads a template names it (the export audit checks that it ships).
fn template_config(rel: &str) -> Value {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The quick voice stops a round at the budget of the dispatcher that runs
/// its calls -- one number, read from the dispatcher, never a second one.
#[test]
fn the_quick_voice_stops_at_its_dispatchers_budget() {
    let quick = template_config("templates/talky/quick/config.json");
    let surface = template_config("templates/talky/dispatcher/config.json");
    let budget = surface
        .pointer("/params/max_calls")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            template_config("templates/dispatcher/config.json")
                .pointer("/params/max_calls")
                .and_then(Value::as_u64)
                .expect("the dispatcher states max_calls")
        });
    assert_eq!(
        quick["params"]["tool_call_limit"], budget,
        "{}",
        quick["params"]
    );
    assert_eq!(
        quick.pointer("/contract/settings/tool_call_limit/default"),
        Some(&Value::from(budget))
    );
}
