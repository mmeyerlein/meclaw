//! GH #1241 -- the llm cell drives the native Anthropic Messages lane
//! (`provider: "anthropic"`), pinned against a mock: what the request carries
//! (path, headers, body) and what the cell makes of the answer.
//!
//! * T10-09: the lane itself -- `POST <base_url>/messages` with `x-api-key`
//!   and `anthropic-version`, the answer emitted as an assistant turn; the
//!   app attribution params (`x_title`, `http_referer`) reach the wire as
//!   headers, as on the chat and responses lanes.
//! * T10-04: an image attachment travels as a base64 `image` block in the
//!   youngest `user` turn, never in a `peer` turn.
//! * T10-10: the thinking blocks of a tool turn go back unchanged, signature
//!   included -- only in the open tool round.
//! * The registry's push (`$reset` naming `wire_dialect` among the other
//!   package keys) changes the model of an anthropic cell like that of a
//!   decisions cell.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::{LlmParams, MODEL_PACKAGE_KEYS};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{AttachmentReader, ContractView, DbConn, DiskBlobStore};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::MockOpenAI;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// base64: `iVBORw0KGgpHSDg3`.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nGH87";

fn params(mock: &MockOpenAI, extra: Value) -> LlmParams {
    let mut raw = json!({
        "provider": "anthropic",
        "model": "claude-test-a",
        "api_key": "test-key-1241",
        "base_url": mock.base_url,
        "max_tokens": 64,
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        raw[k] = v;
    }
    LlmParams::parse(&raw).expect("anthropic params")
}

fn sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel(8);
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

fn cell_db(td: &TempDir) -> DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    DbConn::wrap(conn, None)
}

fn message(body: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build()
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<CellEmission> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em);
    }
    out
}

fn answer(content: Value, stop_reason: &str) -> MockResponse {
    let body = json!({
        "id": "msg_1241", "type": "message", "role": "assistant", "model": "claude-test-a",
        "content": content, "stop_reason": stop_reason,
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

fn user(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

fn tool_call(id: &str) -> Value {
    let text = json!({"name": "get", "arguments": "{}"}).to_string();
    json!({"origin": "assistant", "type": "tool_call", "id": id, "text": text})
}

fn tool_result(id: &str) -> Value {
    json!({"origin": "tool", "type": "tool_result", "id": id, "text": "r"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cell_drives_the_messages_lane_and_names_the_app() {
    let mock = MockOpenAI::start(vec![answer(
        json!([{"type": "text", "text": "Hello."}]),
        "end_turn",
    )])
    .await;
    let mut cell = LlmCell::new(
        params(
            &mock,
            json!({"x_title": "Example App", "http_referer": "https://example.com"}),
        ),
        reqwest::Client::new(),
    );
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (out, mut rx) = sink();
    cell.handle(message(json!({"messages": [user("hi")]})), &out, &mut db)
        .await;

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "the request must reach the provider");
    let req = &reqs[0];
    assert_eq!(req.path, "/messages");
    let header = |k: &str| req.headers.get(k).map(String::as_str);
    assert_eq!(header("x-api-key"), Some("test-key-1241"));
    assert_eq!(header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(header("authorization"), None, "the key travels once");
    assert_eq!(header("x-title"), Some("Example App"));
    assert_eq!(header("http-referer"), Some("https://example.com"));
    assert_eq!(req.body["model"], "claude-test-a");
    assert_eq!(req.body["max_tokens"], 64);
    assert_eq!(
        req.body["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}])
    );

    let ems = drain(&mut rx);
    assert_eq!(ems.len(), 1);
    let content = &ems[0].content;
    assert_eq!(content["header"]["finish_reason"], "stop", "{content}");
    assert_eq!(content["messages"][0]["text"], "Hello.", "{content}");
    // GH #1282: the meta names the provider the call went to, not `openai`.
    assert_eq!(content["meta"]["provider"], "anthropic", "{content}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_attachment_joins_the_youngest_user_turn_as_base64() {
    let mock = MockOpenAI::start(vec![answer(
        json!([{"type": "text", "text": "A fox."}]),
        "end_turn",
    )])
    .await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob = store
        .write_streaming(PNG_BYTES, "image/png", Some("fox.png"))
        .await
        .unwrap();
    let block: meclaw_core::ConsumesBlock = meclaw_core::serde_json::from_value(json!({
        "body": {"messages": {"type": "array"}, "attachments": {"type": "array"}}
    }))
    .unwrap();
    let contract = ContractView {
        consumes: Some(Arc::new(meclaw_core::CompiledConsumes::compile(&block))),
        ..ContractView::default()
    };
    let reader = AttachmentReader::for_contract(&contract, Some(store));
    let mut cell = LlmCell::new(params(&mock, json!({})), reqwest::Client::new())
        .with_attachment_reader(reader);
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (out, mut rx) = sink();
    cell.handle(
        message(json!({
            "messages": [
                user("what is this?"),
                {"origin": "peer", "type": "text", "text": "p", "speaker": "Ann"}
            ],
            "attachments": [{
                "blob_id": blob.blob_id.to_string(),
                "mime_type": "image/png",
                "filename": "fox.png",
                "size_bytes": blob.size_bytes,
            }]
        })),
        &out,
        &mut db,
    )
    .await;

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "{:?}", drain(&mut rx));
    assert_eq!(
        reqs[0].body["messages"],
        json!([{"role": "user", "content": [
            {"type": "text", "text": "what is this?"},
            {"type": "image", "source": {
                "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgpHSDg3"}},
            {"type": "text", "text": "[peer \u{b7} Ann]\np"}
        ]}])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_thinking_of_a_tool_turn_goes_back_unchanged_in_the_open_round_only() {
    let thinking_a = json!({"type": "thinking", "thinking": "plan a", "signature": "sig-a"});
    let thinking_b = json!({"type": "redacted_thinking", "data": "opaque-b"});
    let mock = MockOpenAI::start(vec![
        answer(
            json!([thinking_a, {"type": "tool_use", "id": "t1", "name": "get", "input": {}}]),
            "tool_use",
        ),
        answer(
            json!([thinking_b, {"type": "tool_use", "id": "t2", "name": "get", "input": {}}]),
            "tool_use",
        ),
        answer(json!([{"type": "text", "text": "done"}]), "end_turn"),
    ])
    .await;
    let mut cell = LlmCell::new(params(&mock, json!({})), reqwest::Client::new());
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (out, mut rx) = sink();
    let rounds = [
        json!([user("q")]),
        json!([user("q"), tool_call("t1"), tool_result("t1")]),
        json!([
            user("q"),
            tool_call("t1"),
            tool_result("t1"),
            tool_call("t2"),
            tool_result("t2")
        ]),
    ];
    for history in rounds {
        cell.handle(message(json!({"messages": history})), &out, &mut db)
            .await;
    }
    assert_eq!(drain(&mut rx).len(), 3);

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 3);
    let tool_use = |id: &str| json!({"type": "tool_use", "id": id, "name": "get", "input": {}});
    assert_eq!(
        reqs[1].body["messages"][1],
        json!({"role": "assistant", "content": [thinking_a, tool_use("t1")]}),
        "the open round carries its thinking first, signature and all"
    );
    assert_eq!(
        reqs[2].body["messages"][1],
        json!({"role": "assistant", "content": [tool_use("t1")]}),
        "a closed round carries none"
    );
    assert_eq!(
        reqs[2].body["messages"][3],
        json!({"role": "assistant", "content": [thinking_b, tool_use("t2")]})
    );
    // The cell keeps the open round only: the round change dropped t1's row,
    // the final answer closed the round and dropped t2's.
    assert_eq!(kept_rows(&mut db).await, Vec::<String>::new());
}

/// The `tool_use` ids kept in the cell's `llm_thinking` table, sorted.
async fn kept_rows(db: &mut DbConn) -> Vec<String> {
    db.call(|conn| {
        let mut stmt = conn
            .prepare("SELECT tool_use_id FROM llm_thinking ORDER BY tool_use_id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(Result::unwrap).collect()
    })
    .await
}

/// Re-check M1: a round change drops the rows of its own conversation only
/// -- another conversation's open round keeps its thinking and sends it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_round_change_in_one_conversation_keeps_the_open_round_of_another() {
    let thinking = |s: &str| json!({"type": "thinking", "thinking": s, "signature": s});
    let tool_use = |id: &str| json!({"type": "tool_use", "id": id, "name": "get", "input": {}});
    let mock = MockOpenAI::start(vec![
        answer(json!([thinking("a1"), tool_use("a1")]), "tool_use"),
        answer(json!([thinking("b1"), tool_use("b1")]), "tool_use"),
        answer(json!([thinking("a2"), tool_use("a2")]), "tool_use"),
        answer(json!([{"type": "text", "text": "done"}]), "end_turn"),
    ])
    .await;
    let mut cell = LlmCell::new(params(&mock, json!({})), reqwest::Client::new());
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (out, mut rx) = sink();
    let turns = [
        json!([user("a")]),
        json!([user("b")]),
        json!([user("a"), tool_call("a1"), tool_result("a1")]),
        json!([user("b"), tool_call("b1"), tool_result("b1")]),
    ];
    for (i, history) in turns.into_iter().enumerate() {
        cell.handle(message(json!({"messages": history})), &out, &mut db)
            .await;
        if i == 2 {
            assert_eq!(kept_rows(&mut db).await, ["a2", "b1"], "a's round change");
        }
    }
    assert_eq!(drain(&mut rx).len(), 4);
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 4);
    assert_eq!(
        reqs[3].body["messages"][1],
        json!({"role": "assistant", "content": [thinking("b1"), tool_use("b1")]}),
        "b's open round still carries its thinking"
    );
    assert_eq!(
        kept_rows(&mut db).await,
        ["a2"],
        "b's answer closed b's round"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registry_push_resetting_wire_dialect_changes_the_model() {
    let mock = MockOpenAI::start(vec![answer(
        json!([{"type": "text", "text": "ok"}]),
        "end_turn",
    )])
    .await;
    let mut cell = LlmCell::new(params(&mock, json!({})), reqwest::Client::new());
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (out, mut rx) = sink();

    // The registry's push: the package, every other package key reset --
    // `wire_dialect` among them (OR-programm-tm.K-KATALOG.4).
    let reset: Vec<&str> = MODEL_PACKAGE_KEYS
        .iter()
        .copied()
        .filter(|k| *k != "model")
        .collect();
    assert!(reset.contains(&"wire_dialect"));
    cell.handle(
        message(json!({"system": {}, "params": {"model": "claude-test-b", "$reset": reset}})),
        &out,
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "a push is answered with silence");

    cell.handle(message(json!({"messages": [user("hi")]})), &out, &mut db)
        .await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "{:?}", drain(&mut rx));
    assert_eq!(reqs[0].path, "/messages");
    assert_eq!(reqs[0].body["model"], "claude-test-b");
}

/// GH #1282 (R025): an escalated tool round reaches the cell with the quick
/// voice's bridge sentence as the last turn; the request that leaves the
/// cell still ends on a user message, so no model is sent a prefill.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_window_ending_on_a_bridge_sentence_is_sent_without_a_prefill() {
    let mock = MockOpenAI::start(vec![answer(
        json!([{"type": "text", "text": "Sunny."}]),
        "end_turn",
    )])
    .await;
    let mut cell = LlmCell::new(params(&mock, json!({})), reqwest::Client::new());
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (out, mut rx) = sink();
    let bridge =
        json!({"origin": "assistant", "type": "text", "text": "One moment, I am looking that up."});
    cell.handle(
        message(json!({"messages": [user("weather?"), bridge]})),
        &out,
        &mut db,
    )
    .await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "{:?}", drain(&mut rx));
    let messages = reqs[0].body["messages"].as_array().unwrap().clone();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["role"], "user");
    let said = messages[0]["content"][1]["text"].as_str().unwrap_or("");
    assert!(
        said.ends_with("One moment, I am looking that up."),
        "{said}"
    );
}
