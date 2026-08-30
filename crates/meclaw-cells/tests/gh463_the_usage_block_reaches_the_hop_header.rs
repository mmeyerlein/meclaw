//! GH #463 — the usage block travels whole, from the provider's wire into the
//! hop header the ledger sums.
//!
//! The measurement behind the issue: the `llm` cell parsed a provider's cached
//! token count and its own cost figure off the response and then dropped both,
//! and it put `latency_ms` in the body — which `/colony/ledger` never reads. So
//! a watcher that wanted "cost per model" had the number in the log and no way
//! to sum it.
//!
//! These tests drive the real `LlmCell` against a mock provider and read the
//! **emitted header**, because that is the compartment the colony persists into
//! `message_log.headers` under `$.hop`. Asserting the parser alone would pin a
//! struct field, not the promise.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::MockOpenAI;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// A chat-completions body with a caller-chosen `usage` block.
fn chat_completion_with_usage(usage: meclaw_core::serde_json::Value) -> MockResponse {
    let json = json!({
        "id": "chatcmpl-gh463",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {"role": "assistant", "content": "hi back"},
            "finish_reason": "stop"
        }],
        "usage": usage
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Drive one `LlmCell` turn against a mock answering `response`, and return the
/// emitted UBF content.
async fn one_turn(response: MockResponse) -> meclaw_core::serde_json::Value {
    let mock = MockOpenAI::start(vec![response]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-gh463",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
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
            "messages": [{"origin":"user","type":"text","text":"Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the cell must emit a turn").content
}

/// The OpenAI / OpenRouter spelling: `prompt_tokens_details.cached_tokens`
/// beside the two token counts, plus OpenRouter's own `usage.cost`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_tokens_and_the_providers_cost_reach_the_hop_header() {
    let content = one_turn(chat_completion_with_usage(json!({
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "prompt_tokens_details": {"cached_tokens": 8},
        "cost": 0.000_25
    })))
    .await;

    let header = content["header"]
        .as_object()
        .expect("the emission carries a header object");
    assert_eq!(header["tokens_prompt"], 10);
    assert_eq!(header["tokens_completion"], 5);
    assert_eq!(
        header["tokens_cached"], 8,
        "the cache-read count is the whole point of the issue: {header:?}"
    );
    assert_eq!(
        header["cost"], 0.000_25,
        "the PROVIDER's own cost figure, never one the substrate computed"
    );
    assert!(
        header["latency_ms"].as_u64().is_some(),
        "latency belongs in the header, because that is what the ledger sums"
    );
    // Compatibility: it stays in `meta` for every reader that has had it since
    // Phase 8. One number in two places is cheaper than a migration.
    assert!(content["meta"]["latency_ms"].as_u64().is_some());
}

/// The Anthropic spelling of the same figure. One header key, whatever the
/// provider called it on the wire — a ledger that had to know which dialect
/// wrote a row would not be an aggregate reader.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_anthropic_spelling_of_the_cache_count_lands_in_the_same_key() {
    let content = one_turn(chat_completion_with_usage(json!({
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "cache_read_input_tokens": 7
    })))
    .await;

    assert_eq!(content["header"]["tokens_cached"], 7);
}

/// A provider that reports neither figure leaves neither key behind. An
/// unreported cost and a call that cost nothing are different facts, and a
/// summed zero would make them the same number.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_reports_no_cache_and_no_cost_leaves_no_zeroes() {
    let content = one_turn(chat_completion_with_usage(json!({
        "prompt_tokens": 10,
        "completion_tokens": 5
    })))
    .await;

    let header = content["header"]
        .as_object()
        .expect("the emission carries a header object");
    assert!(
        !header.contains_key("tokens_cached"),
        "absent, not zero: {header:?}"
    );
    assert!(!header.contains_key("cost"), "absent, not zero: {header:?}");
    assert_eq!(
        header["tokens_prompt"], 10,
        "the reported figures are there"
    );
}

/// The error path carries the measurement it does have. A timeout is the hop
/// whose latency an operator most wants summed, and it reports no tokens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_call_still_puts_its_latency_in_the_header() {
    let content = one_turn(MockResponse::ok_json(b"{\"not\": \"a completion\"}")).await;

    let header = content["header"]
        .as_object()
        .expect("the emission carries a header object");
    assert_eq!(header["finish_reason"], "error");
    assert!(
        header["latency_ms"].as_u64().is_some(),
        "a failed call took time too: {header:?}"
    );
    assert!(
        !header.contains_key("tokens_prompt") && !header.contains_key("cost"),
        "a call that failed reports no usage: {header:?}"
    );
}
