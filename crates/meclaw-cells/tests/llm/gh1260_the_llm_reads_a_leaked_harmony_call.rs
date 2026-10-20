//! GH #1260 (OR-WG-7) -- the wiring of the harmony recovery in the `llm`
//! cell, one pin per call site.
//!
//! The rules live in `llm::sanitize` and are unit-tested there; these pins go
//! red when a call site drops the recovery: the chat lane, the Responses
//! lane, and the continuation of an answer cut on `length`. Measured at the
//! emission of a real `LlmCell` against the in-process mock providers, free
//! of a paid call.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
// harness_fold: #[path = "mock_responses.rs"]
#[allow(unused_imports)]
use crate::mock_responses; // harness_fold: mod mock_responses;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::deadline;
use meclaw_testing::mock_http::MockResponse;
use mock_openai::{MockOpenAI, canned_chat_completion};
use mock_responses::MockResponses;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The measured leak (anonymised): a `handoff` call as harmony tokens in
/// `content`, no `tool_calls`.
const LEAKED_HANDOFF: &str = "<|start|>assistant<|channel|>commentary to=functions.handoff <|constrain|>json{\n  \"path\": \"complex\"\n}<|call|>";

/// A `system.tools` subtree carrying one function per name.
fn menu(names: &[&str]) -> Value {
    let mut tools = meclaw_core::serde_json::Map::new();
    for name in names {
        let tool = json!({"type": "function", "function": {"name": name, "parameters": {}}});
        tools.insert((*name).into(), json!({"text": tool.to_string()}));
    }
    Value::Object(tools)
}

/// One turn of a fresh cell with `raw_params`, the menu `names`; the emitted
/// body.
async fn drive(raw_params: Value, names: &[&str]) -> Value {
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
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"tools": menu(names)},
            "messages": [{"origin": "user", "type": "text", "text": "route this"}]
        })))
        .build();
    deadline::hang("handle() returns", cell.handle(msg, &sink, &mut db)).await;
    deadline::hang("the cell answers", rx.recv())
        .await
        .expect("the cell answers")
        .content
}

/// A non-streamed Responses answer with one text output.
fn responses_text(text: &str) -> MockResponse {
    let body = json!({
        "id": "resp-1", "object": "response", "model": "gpt-oss", "status": "completed",
        "error": null,
        "output": [{"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]}],
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_chat_lane_turns_a_leaked_call_into_a_tool_call() {
    let mock = MockOpenAI::start(vec![canned_chat_completion(LEAKED_HANDOFF, "stop")]).await;
    let out = drive(
        json!({"provider": "openai", "model": "gpt-oss", "api_key": "k",
               "base_url": format!("{}/v1", mock.base_url)}),
        &["handoff"],
    )
    .await;
    assert_eq!(out["header"]["finish_reason"], "tool_calls", "{out}");
    let turns = out["messages"].as_array().expect("messages");
    assert_eq!(turns.len(), 1, "{out}");
    assert_eq!(turns[0]["type"], "tool_call", "{out}");
    let function: Value =
        meclaw_core::serde_json::from_str(turns[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(function["name"], "handoff");
    assert!(
        !out.to_string().contains("<|"),
        "no token reaches a person: {out}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_responses_lane_refuses_a_leaked_call_outside_the_menu() {
    let api = MockResponses::start(vec![responses_text(LEAKED_HANDOFF)]).await;
    let out = drive(
        json!({"provider": "openai", "model": "gpt-oss", "api_key": "k",
               "wire_dialect": "responses", "base_url": api.base_url, "max_tokens": 32}),
        &["web_search"],
    )
    .await;
    assert_eq!(out["header"]["finish_reason"], "error", "{out}");
    assert_eq!(out["header"]["error_code"], "provider_error", "{out}");
    assert_eq!(out["meta"]["error"]["kind"], "malformed_tool_call", "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_continuation_after_length_is_read_too() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("part one ", "length"),
        canned_chat_completion(
            "<|channel|>analysis<|message|>secret-trace<|end|><|start|>assistant<|channel|>final<|message|>part two",
            "stop",
        ),
    ])
    .await;
    let out = drive(
        json!({"provider": "openai", "model": "gpt-oss", "api_key": "k",
               "base_url": format!("{}/v1", mock.base_url), "length_continuations": 1}),
        &["handoff"],
    )
    .await;
    assert_eq!(
        out["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "part one part two"}]),
        "{out}"
    );
    assert_eq!(out["header"]["finish_reason"], "stop", "{out}");
}

/// GH #1260 (delta review H): a length cut inside an `analysis` body leaves the rest of
/// the trace at the head of the continuation, without its frame; the cell
/// reopens the frame, so the rest falls and only the final answer is said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rest_of_a_cut_analysis_never_becomes_text() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion(
            "<|channel|>final<|message|>part one<|end|><|start|>assistant<|channel|>analysis<|message|>secret-",
            "length",
        ),
        canned_chat_completion(
            "trace<|end|><|start|>assistant<|channel|>final<|message|>part two",
            "stop",
        ),
    ])
    .await;
    let out = drive(
        json!({"provider": "openai", "model": "gpt-oss", "api_key": "k",
               "base_url": format!("{}/v1", mock.base_url), "length_continuations": 1}),
        &["handoff"],
    )
    .await;
    let turns = out["messages"].as_array().expect("messages");
    assert_eq!(turns.len(), 1, "{out}");
    let text = turns[0]["text"].as_str().expect("a text turn");
    assert!(
        text.starts_with("part one") && text.ends_with("part two"),
        "{out}"
    );
    for leaked in ["secret", "trace", "<|"] {
        assert!(!text.contains(leaked), "`{leaked}` reaches a person: {out}");
    }
    assert_eq!(out["header"]["finish_reason"], "stop", "{out}");
}
