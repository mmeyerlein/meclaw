//! GH #1350, second strand -- the review findings of the first one.
//!
//! - I1: after a model change by the registry the operation timeout of a
//!   cell that opts in (`external_timeout_from_chain`) is the one of the
//!   SERVED model, derived by the timeout chain from the pushed row's rate;
//!   before, the push reset it to the value derived from the row the cell was
//!   born on (1 241 s for a fast model born on a slow row). Review I1: a cell
//!   without the opt-in -- a deadline, a value of its own -- keeps its
//!   timeout and writes no line.
//! - M1: a server that streams every call at `index` 0 tells them apart by a
//!   new `id`; each is a call of its own and counts in the limit.
//! - M3: a stream cut at the limit has no usage chunk (it comes last); the
//!   cell says it has only an estimate and counts what it received -- never
//!   a figure passed off as the provider's, never a 0.
//! - M4: a continuation after a `length` cut is one more call of the same
//!   round and is held to the same limit, with the same error; review M2: the
//!   figures of the calls before the cut are booked with it.
//!
//! The provider is a raw socket that answers each request from a queue and
//! can hold a stream OPEN: a cell that waits for the whole answer never
//! answers, and the hang mark fails the test instead of a verdict on time.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::{LlmParams, largest_cleared_external_ms};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::{deadline, log_recorder};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// One SSE frame of a chat-completions stream.
fn frame(chunk: &Value) -> String {
    format!("data: {chunk}\n\n")
}

/// A chunk with one delta.
fn chunk(id: &str, delta: Value) -> String {
    frame(
        &json!({"id": id, "object": "chat.completion.chunk", "model": "fast-model",
                  "choices": [{"index": 0, "delta": delta}]}),
    )
}

/// `n` calls, each at provider index `index_of(i)`: the opening delta with
/// its id and name, then the arguments without an id.
fn calls(id: &str, n: usize, index_of: fn(usize) -> usize) -> Vec<String> {
    let mut out = vec![chunk(id, json!({"role": "assistant", "content": "On it."}))];
    for i in 0..n {
        out.push(chunk(
            id,
            json!({"tool_calls": [{"index": index_of(i), "id": format!("call_{i}"),
                   "type": "function", "function": {"name": "fetch", "arguments": ""}}]}),
        ));
        out.push(chunk(
            id,
            json!({"tool_calls": [{"index": index_of(i),
                   "function": {"arguments": format!("{{\"n\": {i}}}")}}]}),
        ));
    }
    out
}

/// The end of a stream: the finish reason, the usage chunk and `[DONE]`.
fn end(id: &str, finish: &str) -> Vec<String> {
    vec![
        frame(
            &json!({"id": id, "object": "chat.completion.chunk", "model": "fast-model",
                      "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}),
        ),
        frame(
            &json!({"id": id, "object": "chat.completion.chunk", "model": "fast-model",
                      "choices": [], "usage": {"prompt_tokens": 50, "completion_tokens": 60,
                                               "cost": 0.0011}}),
        ),
        "data: [DONE]\n\n".to_string(),
    ]
}

/// One answer of the provider.
enum Reply {
    /// An event stream; `hold` keeps it open after the last frame.
    Stream { frames: Vec<String>, hold: bool },
}

/// A provider on a raw socket: each connection reads one request and gets the
/// next reply of `replies`. Hands back the base url.
async fn provider(replies: Vec<Reply>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, len) = loop {
                    let Ok(n) = sock.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
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
                    let Ok(n) = sock.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let Reply::Stream { frames, hold } = reply;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                            transfer-encoding: chunked\r\n\r\n";
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for f in frames {
                    let c = format!("{:x}\r\n{f}\r\n", f.len());
                    if sock.write_all(c.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                }
                if hold {
                    // The rest never comes: wait until the cell
                    // closes its end.
                    let _ = sock.read(&mut tmp).await;
                } else {
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                }
            });
        }
    });
    format!("http://{addr}/v1")
}

fn cell_db(td: &TempDir) -> DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    DbConn::wrap(conn, None)
}

fn sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

/// One turn of a fresh cell with `raw_params` and a one-tool menu; the
/// emitted body.
async fn drive(raw_params: Value) -> Value {
    let td = TempDir::new().unwrap();
    let params = LlmParams::parse(&raw_params).expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::new());
    let mut db = cell_db(&td);
    let (sink, mut rx) = sink();
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
           "x_title": "meclaw-lab gh1350", "http_referer": "https://example.com",
           // Far beyond the hang mark: an answer inside it is the cut.
           "external_timeout_ms": 600_000})
}

fn tool_calls_of(out: &Value) -> Vec<Value> {
    out["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|t| t["type"] == "tool_call")
        .cloned()
        .collect()
}

// ── M1 ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_streamed_at_index_zero_with_new_ids_are_calls_of_their_own() {
    let mut frames = calls("chatcmpl-z", 3, |_| 0);
    frames.extend(end("chatcmpl-z", "tool_calls"));
    let base = provider(vec![Reply::Stream {
        frames,
        hold: false,
    }])
    .await;
    let out = drive(params(&base, 16)).await;
    assert_eq!(out["header"]["finish_reason"], "tool_calls", "{out}");
    let calls = tool_calls_of(&out);
    assert_eq!(
        calls.len(),
        3,
        "three ids at index 0 are three calls: {out}"
    );
    for (i, call) in calls.iter().enumerate() {
        assert_eq!(call["id"], format!("call_{i}"), "{out}");
        let function: Value =
            meclaw_core::serde_json::from_str(call["text"].as_str().unwrap()).unwrap();
        assert_eq!(function["arguments"], format!("{{\"n\": {i}}}"), "{out}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_streamed_at_index_zero_count_in_the_limit() {
    let base = provider(vec![Reply::Stream {
        frames: calls("chatcmpl-z", 8, |_| 0),
        hold: true,
    }])
    .await;
    let out = drive(params(&base, 2)).await;
    assert_eq!(out["meta"]["error"]["kind"], "tool_call_limit", "{out}");
    assert_eq!(out["meta"]["error"]["calls"], 3, "{out}");
}

// ── M3 ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cut_stream_states_no_cost_and_says_estimate() {
    let base = provider(vec![Reply::Stream {
        frames: calls("chatcmpl-s", 40, |i| i),
        hold: true,
    }])
    .await;
    let out = drive(params(&base, 16)).await;
    let err = &out["meta"]["error"];
    assert_eq!(err["kind"], "tool_call_limit", "{out}");
    assert_eq!(err["usage"], "estimate", "{out}");
    assert!(err["received_chunks"].as_u64().unwrap_or(0) > 0, "{out}");
    assert!(err["received_chars"].as_u64().unwrap_or(0) > 0, "{out}");
    let header = out["header"].as_object().unwrap();
    for key in ["cost", "tokens_prompt", "tokens_completion"] {
        assert!(!header.contains_key(key), "no {key} is invented: {out}");
    }
}

// ── M4 ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_continuation_over_the_limit_is_the_same_error() {
    let mut cut = vec![chunk(
        "chatcmpl-c",
        json!({"role": "assistant", "content": "The first part"}),
    )];
    cut.extend(end("chatcmpl-c", "length"));
    let base = provider(vec![
        Reply::Stream {
            frames: cut,
            hold: false,
        },
        Reply::Stream {
            frames: calls("chatcmpl-d", 40, |i| i),
            hold: true,
        },
    ])
    .await;
    let mut p = params(&base, 16);
    p["length_continuations"] = json!(1);
    let out = drive(p).await;
    assert_eq!(out["header"]["error_code"], "provider_error", "{out}");
    let err = &out["meta"]["error"];
    assert_eq!(err["kind"], "tool_call_limit", "{out}");
    assert_eq!(err["limit"], 16, "{out}");
    assert_eq!(err["calls"], 17, "{out}");
    // Review M2: the first call finished with the provider's figures; they
    // are booked, and the cut call's own cost is still an estimate.
    assert_eq!(err["usage"], "estimate", "{out}");
    assert_eq!(out["header"]["tokens_prompt"], 50, "{out}");
    assert_eq!(out["header"]["tokens_completion"], 60, "{out}");
    assert_eq!(out["header"]["cost"], 0.0011, "{out}");
}

// ── I1 ────────────────────────────────────────────────────────────────────

/// A cell born with `start` (the chain of `talky/quick`: 1 241 s, backstop
/// 1 366 s, unless `start` says otherwise) and `backstop_ms`, pushed the
/// package of another row by a params-only message the way the registry
/// sends it; the params line after the push.
async fn after_push(start: Value, backstop_ms: u64, push: Value) -> String {
    let td = TempDir::new().unwrap();
    let mut raw = json!({
        "provider": "openai", "model": "slow-model", "api_key": "k",
        "base_url": "http://127.0.0.1:1/v1", "external_timeout_ms": 1_241_000
    });
    for (k, v) in start.as_object().expect("start params") {
        raw[k] = v.clone();
    }
    let params = LlmParams::parse(&raw).expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::new())
        .with_message_timeout(Some(std::time::Duration::from_millis(backstop_ms)));
    let mut db = cell_db(&td);
    let (sink, _rx) = sink();
    let msg = MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({"system": {}, "params": push})))
        .build();
    deadline::hang("handle() returns", cell.handle(msg, &sink, &mut db)).await;
    cell.params_line("/llm")
}

/// The opt-in of a cell whose timeout is an output time (`talky/quick`).
fn chain() -> Value {
    json!({"external_timeout_from_chain": true})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_a_model_change_the_timeout_is_the_served_models() {
    let line = after_push(
        chain(),
        1_366_000,
        json!({"model": "fast-model", "max_output": 1000, "output_tps": 100,
               "$reset": ["external_timeout_ms", "max_tokens"]}),
    )
    .await;
    assert!(line.contains("model=fast-model[overlay]"), "{line}");
    // 1 000 tokens at 100 tok/s + 10 s for the first token.
    assert!(
        line.contains("external_timeout_ms=20000[overlay]"),
        "{line}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slower_row_is_held_under_the_backstop_and_the_change_stands() {
    let line = after_push(
        chain(),
        1_366_000,
        json!({"model": "slower-model", "max_output": 131_072, "output_tps": 42,
               "$reset": ["external_timeout_ms", "max_tokens"]}),
    )
    .await;
    assert!(line.contains("model=slower-model[overlay]"), "{line}");
    let cap = largest_cleared_external_ms(1_366_000, 0);
    assert!(
        line.contains(&format!("external_timeout_ms={cap}[overlay]")),
        "{line}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_without_a_rate_keeps_the_timeout_of_birth() {
    let line = after_push(
        chain(),
        1_366_000,
        json!({"model": "fast-model", "max_output": 1000,
               "$reset": ["external_timeout_ms", "output_tps"]}),
    )
    .await;
    assert!(
        line.contains("external_timeout_ms=1241000[start]"),
        "{line}"
    );
}

/// Review I1: a deadline cell (`talky/decide`: 3 s under a 13 s backstop)
/// keeps its deadline after a push with a rate, and no line says it was
/// held -- before, the chain gave it 10 s and more, held back at 3 s by the
/// cap with a WARN on every push.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deadline_cell_keeps_its_deadline_and_says_nothing() {
    let log = log_recorder::global();
    let line = after_push(
        json!({"external_timeout_ms": 3_000}),
        13_000,
        json!({"model": "deadline-model", "max_output": 4096, "output_tps": 42,
               "$reset": ["external_timeout_ms", "max_tokens"]}),
    )
    .await;
    assert!(line.contains("model=deadline-model[overlay]"), "{line}");
    assert!(line.contains("external_timeout_ms=3000[start]"), "{line}");
    let said = log.lines(tracing::Level::WARN, &["deadline-model", "is held at"]);
    assert!(said.is_empty(), "no line about a held timeout: {said:?}");
}

/// Review I1: a timeout the cell names itself (an instance's
/// `override_params`, no opt-in) stays after a push with a rate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timeout_of_its_own_stays_after_a_push_with_a_rate() {
    let line = after_push(
        json!({"external_timeout_ms": 300_000}),
        1_366_000,
        json!({"model": "fast-model", "max_output": 1000, "output_tps": 100,
               "$reset": ["external_timeout_ms", "max_tokens"]}),
    )
    .await;
    assert!(line.contains("external_timeout_ms=300000[start]"), "{line}");
}
