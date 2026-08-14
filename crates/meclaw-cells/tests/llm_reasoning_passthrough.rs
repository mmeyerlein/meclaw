//! GH #124 lever 1 — the reasoning passthrough on the chat-completions lane.
//!
//! The `llm` cell could not hand a thinking-class model a deliberation budget
//! on this wire: only the Responses lane knows reasoning items. A core brain
//! therefore ran at whatever the provider defaults to, with no control over how
//! long it deliberates. `reasoning_effort` / `reasoning` close that gap as
//! ORDINARY params (audit ruling A4 — no special mechanics), and this file
//! pins the wire consequence a unit test cannot show: what the provider
//! actually receives.
//!
//! The unset case is the load-bearing one. A provider that does not know the
//! field must never see it.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tempfile::TempDir;

#[path = "mock_openai.rs"]
mod mock_openai;
use mock_openai::{MockOpenAI, canned_chat_completion};

/// Drive one inference through a cell built from `extra_params` and return the
/// request body the mock provider received.
async fn captured_request(extra_params: Value) -> Value {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let mut raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let raw_obj = raw.as_object_mut().expect("params object");
    for (k, v) in extra_params.as_object().expect("extra params object") {
        raw_obj.insert(k.clone(), v.clone());
    }
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
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
        .body(Body::Inline(
            json!({"messages": [{"origin":"user","type":"text","text":"Hi"}]}),
        ))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the cell must emit an answer");

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    snaps[0].body.clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unset_reasoning_param_sends_no_reasoning_field_at_all() {
    let body = captured_request(json!({})).await;
    assert!(
        body.get("reasoning").is_none(),
        "unset ⇒ the provider must not see the field: {body}"
    );
    // …and nothing else moved: the pre-#124 body is intact.
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["max_tokens"], 4096);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_effort_shorthand_reaches_the_provider_as_a_reasoning_block() {
    let body = captured_request(json!({"reasoning_effort": "low"})).await;
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reasoning_object_reaches_the_provider_verbatim() {
    let block = json!({"effort": "high", "exclude": true, "max_tokens": 2048});
    let body = captured_request(json!({"reasoning": block.clone()})).await;
    assert_eq!(
        body["reasoning"], block,
        "the object is passed through, not re-shaped"
    );
}

/// The documented precedence, proven on the wire: with both forms set the
/// provider sees the object and never a merge of the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_object_wins_over_the_shorthand_on_the_wire() {
    let body = captured_request(json!({
        "reasoning_effort": "low",
        "reasoning": {"effort": "high"},
    }))
    .await;
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
}

/// A deliberation budget is a knob, not an identity — it must be changeable by
/// message at runtime, and the SAME call must already use the new value
/// (params-slot ordering, cell-types § llm).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_message_can_retune_the_budget_for_the_very_same_call() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
        "reasoning_effort": "high",
    });
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
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
            "params": {"reasoning_effort": "low"},
            "messages": [{"origin":"user","type":"text","text":"Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the cell must emit an answer");

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps[0].body["reasoning"], json!({"effort": "low"}));
    assert_eq!(cell.params.reasoning_effort.as_deref(), Some("low"));
    // …and it survived into the cell.db overlay, so a wake replays it.
    let stored: String = conn
        .call(|c| {
            c.query_row(
                "SELECT value FROM params WHERE key='reasoning_effort'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await;
    assert_eq!(stored, r#""low""#);
}
