//! GH #993 -- the `llm` cell sends a sampling param only when the model takes
//! it, and says what it left out.
//!
//! A model that does not know a sampling field may refuse the whole request
//! for it (measured: one hosted model answers a request that carries
//! `temperature` with an error, and the cell sent `temperature` on every call).
//! So a model package may name the sampling params its model takes
//! (`supported_params`), the cell sends only those, and the answer names every
//! field it left out in `hop.dropped`. Measured where it lands:
//!
//! 1. a model whose list leaves out `temperature` gets a request without it,
//!    and the answer that reaches the next cell says `dropped: ["temperature"]`;
//! 2. a model whose list names `temperature` gets it, and nothing is dropped;
//! 3. a key in `provider_extra` is sent even when the list leaves it out, and
//!    is not called dropped -- the pass-through stays the last word;
//! 4. without a list the request is the request of before (the frozen wire
//!    fixture, byte for byte) and the hop carries no `dropped`.
//!
//! Free of a paid call by construction: every provider here is the in-process
//! mock, and every model name is generic.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The legacy wire, frozen by `llm_chat_completions_wire_regression.rs`.
const OFF_FIXTURE: &str = "tests/fixtures/expected_chat_completions_body.json";

/// Failure marker, generous per the 30 s convention -- not a timing claim.
const MARKER: Duration = Duration::from_secs(30);

fn llm_config(base_url: &str, extra: &Value) -> String {
    let mut params = json!({
        "provider": "openai", "model": "model-a", "api_key": "test-key",
        "base_url": base_url, "temperature": 0.3, "reasoning_effort": "low"
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    meclaw_core::serde_json::to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": params,
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .unwrap()
}

/// One turn through a colony `/brain -> /sink`: the request the provider got
/// and the hop of the answer AS THE SINK RECEIVED IT -- the receiver, not the
/// emitter, is where `dropped` has to arrive.
async fn one_turn(extra: Value) -> (Value, Map<String, Value>) {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main/brain")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        meclaw_core::serde_json::to_string_pretty(&json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [{"from": "./brain", "to": "/sink"}]}}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/brain/config.json"),
        llm_config(&format!("{}/v1", mock.base_url), &extra),
    )
    .unwrap();

    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), llm_f.clone())]);
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(4);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap");

    h.send(
        MessageBuilder::new(Path::new("/brain"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": "hi"}]
            })))
            .build(),
    )
    .await;
    let answer = tokio::time::timeout(MARKER, sink_rx.recv())
        .await
        .ok()
        .flatten()
        .expect("the answer reaches the sink");
    let requests = mock.recorded_requests().await;
    assert_eq!(requests.len(), 1, "exactly one provider call");
    h.shutdown().await;
    (requests[0].body.clone(), answer.headers.hop.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh993_a_model_without_temperature_gets_none() {
    let (request, hop) = one_turn(json!({"supported_params": ["reasoning"]})).await;
    assert!(request.get("temperature").is_none(), "{request}");
    assert_eq!(
        request["reasoning"],
        json!({"effort": "low"}),
        "a named field still travels: {request}"
    );
    assert_eq!(
        request["max_tokens"], 32_768,
        "not a sampling field: {request}"
    );
    assert_eq!(hop["finish_reason"], "stop", "{hop:?}");
    assert_eq!(hop["dropped"], json!(["temperature"]), "{hop:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh993_a_model_with_temperature_gets_it() {
    let (request, hop) = one_turn(json!({
        "supported_params": ["temperature", "top_p", "reasoning"]
    }))
    .await;
    assert_eq!(request["temperature"], 0.3, "{request}");
    assert_eq!(request["reasoning"], json!({"effort": "low"}), "{request}");
    assert!(!hop.contains_key("dropped"), "nothing dropped: {hop:?}");

    // The top-level reasoning wire is governed by ITS field names.
    let (request, hop) = one_turn(json!({
        "reasoning_wire": "top_level", "thinking_budget": 512,
        "supported_params": ["temperature", "reasoning_effort"]
    }))
    .await;
    assert_eq!(request["temperature"], 0.3, "{request}");
    assert_eq!(request["reasoning_effort"], "low", "{request}");
    assert!(request.get("thinking_token_budget").is_none(), "{request}");
    assert_eq!(hop["dropped"], json!(["thinking_token_budget"]), "{hop:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh993_provider_extra_still_wins() {
    let (request, hop) = one_turn(json!({
        "supported_params": [],
        "provider_extra": {"temperature": 0.9, "top_p": 0.5}
    }))
    .await;
    assert_eq!(
        request["temperature"], 0.9,
        "the overlay is sent: {request}"
    );
    assert_eq!(request["top_p"], 0.5, "{request}");
    assert!(request.get("reasoning").is_none(), "{request}");
    assert_eq!(
        hop["dropped"],
        json!(["reasoning"]),
        "an overlay key is never called dropped: {hop:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh993_no_list_keeps_todays_request() {
    let (request, hop) = one_turn(json!({})).await;
    assert_eq!(request["temperature"], 0.3, "{request}");
    assert_eq!(request["reasoning"], json!({"effort": "low"}), "{request}");
    assert!(!hop.contains_key("dropped"), "{hop:?}");

    // And byte for byte: the broad request of the wire regression is the
    // frozen fixture, without a list.
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-a0",
        "base_url": format!("{}/v1", mock.base_url),
        "temperature": 0.3,
        "max_tokens": 512,
        "system_order": ["identity", "instructions"],
        "provider_extra": {"seed": 42},
        "http_referer": "https://example.test",
        "x_title": "Example App",
    });
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let raw_db =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(raw_db, None);
    let (tx, _rx) = mpsc::channel::<CellEmission>(4);
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
            "system": {
                "identity": {"text": "You are a fixture."},
                "instructions": {"text": "Be terse."},
                "tools": {
                    "get_weather": {"text": "{\"name\":\"get_weather\",\"description\":\"w\",\"parameters\":{\"type\":\"object\",\"properties\":{}}}"}
                }
            },
            "messages": [
                {"origin": "user", "type": "text", "text": "first"},
                {"origin": "assistant", "type": "tool_call", "id": "call_1",
                 "text": "{\"name\":\"get_weather\",\"arguments\":\"{}\"}"},
                {"origin": "tool", "type": "tool_result", "id": "call_1", "text": "sunny"},
                {"origin": "assistant", "type": "text", "text": "it is sunny"},
                {"origin": "user", "type": "text", "text": "thanks"}
            ]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    let fixture: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(OFF_FIXTURE).expect("the frozen fixture"),
    )
    .unwrap();
    assert_eq!(
        meclaw_core::serde_json::to_string(&snaps[0].body).unwrap(),
        meclaw_core::serde_json::to_string(&fixture).unwrap()
    );
}
