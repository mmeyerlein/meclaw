//! GH #957 -- a decision is whole or it is an error.
//!
//! Each way a call can fail -- an HTTP 400, the operation timeout, a body
//! that is no JSON, an answer missing a question, a choice nobody asked
//! for -- is exactly ONE emission on the error path (`finish_reason`
//! `error`, a named `error_code`, `meta.provider` = `decisions`) and never a
//! `decision`. A body without a valid `decide` slot is `decide_invalid`, and
//! no request is sent for it. Network only against the in-process mock.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::{MockOpenAI, canned_error_status};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/decide"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

fn decide_body() -> Value {
    json!({
        "messages": [],
        "decide": {
            "state": "The user said: show me the weather",
            "questions": {
                "topic": {"kind": "choice", "instructions": "Which topic?",
                          "options": {"weather": "Weather", "none": "Nothing"}},
                "wants": {"kind": "yes_no", "instructions": "Show it?"}
            }
        }
    })
}

/// One call of a fresh cell against `responses`; returns what the mock saw
/// and every emission.
async fn one_call(
    responses: Vec<MockResponse>,
    timeout_ms: u64,
    body: Value,
) -> (usize, Vec<CellEmission>) {
    let mock = MockOpenAI::start(responses).await;
    let raw = json!({
        "provider": "decisions", "model": "vendor/decider", "api_key": "k",
        "base_url": format!("{}/api", mock.base_url), "external_timeout_ms": timeout_ms,
    });
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();
    let msg = MessageBuilder::new(Path::new("/decide"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    let mut ems = Vec::new();
    while let Ok(em) = rx.try_recv() {
        ems.push(em);
    }
    (mock.recorded_requests().await.len(), ems)
}

/// Exactly one error, no decision; returns its `error_code`.
fn the_one_error(ems: &[CellEmission]) -> String {
    assert_eq!(ems.len(), 1, "one emission");
    let c = &ems[0].content;
    assert_eq!(c["header"]["finish_reason"], "error", "{c}");
    assert_eq!(c["meta"]["provider"], "decisions", "{c}");
    assert!(c.get("decision").is_none(), "no half decision: {c}");
    meclaw_core::validate_ubf_body(c).expect("a UBF body");
    c["header"]["error_code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn answer(v: Value) -> MockResponse {
    MockResponse::ok_json(&meclaw_core::serde_json::to_vec(&v).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_http_400_is_one_error() {
    let (sent, ems) = one_call(vec![canned_error_status(400)], 30_000, decide_body()).await;
    assert_eq!(sent, 1);
    assert_eq!(the_one_error(&ems), "provider_error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_operation_timeout_is_one_error() {
    // The mock holds its answer far past the cell's own A-timeout, so the
    // timeout is what ends the call -- not the mock.
    let held = answer(json!({"answers": {}})).with_delay(Duration::from_secs(30));
    let (_, ems) = one_call(vec![held], 200, decide_body()).await;
    assert_eq!(the_one_error(&ems), "timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_is_no_json_is_one_error() {
    let (_, ems) = one_call(
        vec![MockResponse::ok_json(b"{not json")],
        30_000,
        decide_body(),
    )
    .await;
    assert_eq!(the_one_error(&ems), "provider_error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_answer_is_no_decision() {
    let missing = answer(json!({"answers": {
        "topic": {"choice": "weather", "probabilities": {"weather": 1, "none": 0}}}}));
    let (_, ems) = one_call(vec![missing], 30_000, decide_body()).await;
    assert_eq!(the_one_error(&ems), "decision_incomplete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_choice_nobody_asked_for_is_no_decision() {
    let unasked = answer(json!({"answers": {
        "topic": {"choice": "sport", "probabilities": {"sport": 1}},
        "wants": {"noul": 0.4}}}));
    let (_, ems) = one_call(vec![unasked], 30_000, decide_body()).await;
    assert_eq!(the_one_error(&ems), "decision_incomplete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_without_a_valid_decide_sends_nothing() {
    for body in [
        json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]}),
        json!({"messages": [], "decide": {"state": "x", "questions": {}}}),
        json!({"messages": [], "decide": {"state": "x", "questions": {
            "q": {"kind": "choice", "instructions": "one option", "options": {"a": "A"}}}}}),
    ] {
        let (sent, ems) = one_call(vec![], 30_000, body.clone()).await;
        assert_eq!(sent, 0, "no request for {body}");
        assert_eq!(the_one_error(&ems), "decide_invalid", "{body}");
    }
}

/// Review I-1 / OR-DP-59: the error path is failover-ready. A failed call
/// hands its request on -- the incoming `decide` slot and the caller's
/// `messages` ride in the error body unchanged, so a failover edge onto a
/// second `decisions` cell has something to decide (it would answer
/// `decide_invalid` otherwise).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_call_hands_its_request_to_a_failover_cell() {
    let mut body = decide_body();
    body["messages"] = json!([{"origin": "user", "type": "text", "text": "the weather"}]);
    let (sent, ems) = one_call(vec![canned_error_status(400)], 30_000, body.clone()).await;
    assert_eq!(sent, 1);
    assert_eq!(the_one_error(&ems), "provider_error");
    let c = &ems[0].content;
    assert_eq!(c["decide"], body["decide"], "decide rides on: {c}");
    assert_eq!(c["messages"], body["messages"], "messages ride on: {c}");

    // A second, healthy cell takes the error body as it is and decides.
    let ok = answer(json!({"answers": {
        "topic": {"choice": "weather", "probabilities": {"weather": 0.9, "none": 0.1}},
        "wants": {"noul": 0.8}}}));
    let (sent, ems) = one_call(
        vec![ok],
        30_000,
        json!({
            "messages": c["messages"].clone(), "decide": c["decide"].clone(),
        }),
    )
    .await;
    assert_eq!(sent, 1);
    assert_eq!(ems.len(), 1);
    assert_eq!(
        ems[0].content["header"]["finish_reason"], "stop",
        "{}",
        ems[0].content
    );
}
