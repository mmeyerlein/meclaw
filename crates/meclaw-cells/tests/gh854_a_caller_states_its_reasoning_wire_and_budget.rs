//! GH #854: a caller states its reasoning effort in the form its server reads,
//! and its thinking budget without hand-written `provider_extra`.
//!
//! `reasoning_effort` used to leave only nested (`"reasoning": {"effort": …}`),
//! which an OpenAI-compatible local server drops, so a local reasoning model
//! thought at its server default (measured: `length` with empty content in 2
//! of 8 calls at the high default, 38 of 38 clean at medium). Read at the mock:
//! the request body is what the server gets.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

async fn one_call(extra: Value) -> Value {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let mut raw = json!({
        "provider": "openai", "model": "m", "api_key": "k",
        "base_url": format!("{}/v1", mock.base_url),
    });
    for (k, v) in extra.as_object().unwrap() {
        raw[k] = v.clone();
    }
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut db = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
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
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    rx.recv().await.expect("an answer");
    mock.recorded_requests().await.remove(0).body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn top_level_puts_effort_and_budget_at_the_root() {
    let body = one_call(json!({
        "reasoning_effort": "medium",
        "reasoning_wire": "top_level",
        "thinking_budget": 4096,
    }))
    .await;
    assert_eq!(body["reasoning_effort"], "medium");
    assert_eq!(body["thinking_token_budget"], 4096);
    assert!(body.get("reasoning").is_none(), "nothing nested: {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_is_the_default_and_carries_the_budget_as_max_tokens() {
    let body = one_call(json!({"reasoning_effort": "low", "thinking_budget": 2048})).await;
    assert_eq!(
        body["reasoning"],
        json!({"effort": "low", "max_tokens": 2048})
    );
    assert!(body.get("reasoning_effort").is_none());
    assert!(body.get("thinking_token_budget").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_extra_wins_over_the_budget_key() {
    let body = one_call(json!({
        "reasoning_wire": "top_level",
        "thinking_budget": 4096,
        "provider_extra": {"thinking_token_budget": 512},
    }))
    .await;
    assert_eq!(body["thinking_token_budget"], 512);
}

#[test]
fn reasoning_wire_and_budget_are_run_time_knobs() {
    let p = LlmParams::parse(&json!({"provider": "openai", "model": "m", "api_key": "k"})).unwrap();
    let update = json!({"reasoning_wire": "top_level", "thinking_budget": 1024})
        .as_object()
        .unwrap()
        .clone();
    let (merged, _) = p.apply_update(&update).expect("mutable");
    assert_eq!(merged.thinking_budget, Some(1024));
    let bad = json!({"reasoning_wire": "sideways"})
        .as_object()
        .unwrap()
        .clone();
    assert!(p.apply_update(&bad).is_err(), "only nested | top_level");
}
