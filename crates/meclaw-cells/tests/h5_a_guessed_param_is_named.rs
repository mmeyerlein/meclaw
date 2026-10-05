//! GH #999 (R-H5-3 = c) -- a cell without `supported_params` still sends its
//! sampling fields, byte for byte as before, and every answer names them in
//! `hop.unverified`: sent on a guess, unchecked against the model.
//!
//! Measured (Loop 13): three production brains sent `temperature` to a model
//! that takes none, and the router ignored it SILENTLY -- the same defect as a
//! refusal, only invisible. Dropping every unlisted field would break local
//! cells that rely on a top-level reasoning budget, so the request stays and
//! the guess becomes visible instead. A key of `provider_extra` counts when it
//! is a sampling field (`SAMPLING_PARAMS`): the overlay is sent unchecked too.
//! With a list nothing is unverified and no key is written. Success and error
//! answers alike; measured at the receiver, and the request against the frozen
//! wire fixture.

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "h5_support/mod.rs"]
mod h5;

use h5::{one_turn, status_json};
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The legacy wire, frozen by `llm_chat_completions_wire_regression.rs`.
const OFF_FIXTURE: &str = "tests/fixtures/expected_chat_completions_body.json";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_without_a_list_the_request_is_unchanged_and_unverified() {
    // At the receiver: success and error.
    let t = one_turn(canned_chat_completion("ok", "stop"), json!({})).await;
    assert_eq!(t.requests[0]["temperature"], 0.3, "{}", t.requests[0]);
    assert_eq!(t.hop["finish_reason"], "stop", "{:?}", t.hop);
    assert_eq!(t.hop["unverified"], json!(["temperature"]), "{:?}", t.hop);
    assert!(!t.hop.contains_key("dropped"), "{:?}", t.hop);

    let t = one_turn(
        status_json(404, json!({"error": {"message": "no such model"}})),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "error", "{:?}", t.hop);
    assert_eq!(t.hop["unverified"], json!(["temperature"]), "{:?}", t.hop);

    // Byte for byte: the broad request of the wire regression (the #993
    // Lock 4 cell) is the frozen fixture, and its answer says what it guessed.
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
    let (tx, mut rx) = mpsc::channel::<CellEmission>(4);
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
    let emission = rx.try_recv().expect("the answer was emitted");
    assert_eq!(
        emission.content["header"]["unverified"],
        json!(["temperature"]),
        "`seed` is no sampling field: {}",
        emission.content["header"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_with_a_list_nothing_is_unverified() {
    let t = one_turn(
        canned_chat_completion("ok", "stop"),
        json!({"supported_params": ["temperature", "reasoning"], "reasoning_effort": "low"}),
    )
    .await;
    assert_eq!(t.requests[0]["temperature"], 0.3, "{}", t.requests[0]);
    assert!(!t.hop.contains_key("unverified"), "{:?}", t.hop);
    assert!(!t.hop.contains_key("dropped"), "{:?}", t.hop);

    let t = one_turn(
        status_json(404, json!({"error": {"message": "no such model"}})),
        json!({"supported_params": ["temperature"]}),
    )
    .await;
    assert!(!t.hop.contains_key("unverified"), "{:?}", t.hop);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_provider_extra_is_unverified_without_a_list() {
    let t = one_turn(
        canned_chat_completion("ok", "stop"),
        json!({"reasoning_effort": "low",
               "provider_extra": {"top_p": 0.5, "seed": 7}}),
    )
    .await;
    assert_eq!(t.requests[0]["top_p"], 0.5, "{}", t.requests[0]);
    assert_eq!(
        t.hop["unverified"],
        json!(["temperature", "top_p", "reasoning"]),
        "in SAMPLING_PARAMS order, `seed` is no sampling field: {:?}",
        t.hop
    );
}
