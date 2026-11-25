//! GH #1283 "Done when": the request shapes the providers refused (#1282 and
//! the rows of the same class) replayed through the `llm` cell against the
//! strict mock, which plays the recorded refusals of the conformance
//! recordings (GH #1609). Each case drives the cell with a model that has a
//! recording, so the model rules judge it; with the cell's fix the provider
//! answers 200, with the fix taken back the mock answers the recorded refusal
//! (the mutants are in the strand report, not in the tree).
//!
//! * #1282 on the chat wire: a window ending on an assistant text, a Claude
//!   model behind the router (R2) -- the cell folds the text into a note.
//! * #1282 on the Messages wire (R1): the same window, folded the same way.
//! * `temperature` to a model without it (R4): the catalogue row's
//!   `supported_params` drops it (GH #993).
//! * budget `thinking` to a model that refuses it (R6): dropped by the same list.
//! * `reasoning {enabled: false}` to a model that must reason (R3): no cell
//!   fix exists on the chat wire; the off form is refused with the recorded
//!   text, the low effort is taken.
//!
//! Every recorded model carries its catalogue row's `supported_params`, as
//! the registry hands it to the cell: without the list the cell sends
//! `temperature` on a guess, and the strict mock refuses that as recorded (R4).
//!
//! `thinking` disabled (R7) and `tool_choice: required` (R9) are not
//! replayed: no recording refuses them (strand report, part B).

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::{MockOpenAI, canned_chat_completion, model_refusals};
use tempfile::TempDir;
use tokio::sync::mpsc;

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

fn messages_answer() -> MockResponse {
    let body = json!({
        "id": "msg_1283", "type": "message", "role": "assistant", "model": "m",
        "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

fn user(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

fn bridge() -> Value {
    json!({"origin": "assistant", "type": "text", "text": "One moment, I am looking that up."})
}

/// One turn of a cell with `params` (the mock's base URL laid in) over
/// `messages`; the provider's status for the request and the emission.
async fn turn(mock: &MockOpenAI, mut params: Value, messages: Vec<Value>) -> Value {
    params["base_url"] = json!(mock.base_url);
    params["api_key"] = json!("test-key-1283");
    params["x_title"] = json!("meclaw-lab gh1283");
    params["http_referer"] = json!("https://meclaw.ai");
    let mut cell = LlmCell::new(LlmParams::parse(&params).unwrap(), reqwest::Client::new());
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (out, mut rx) = sink();
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": messages})))
        .build();
    cell.handle(msg, &out, &mut db).await;
    assert_eq!(mock.recorded_requests().await.len(), 1, "one request");
    rx.try_recv().expect("the cell answers").content
}

/// The provider took the request: the cell answers with the mock's text.
fn assert_answered(content: &Value) {
    assert_eq!(content["header"]["finish_reason"], "stop", "{content}");
    assert_eq!(content["messages"][0]["text"], "ok", "{content}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1282_a_bridge_sentence_on_the_chat_wire_is_taken() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let params = json!({"provider": "openai", "model": "anthropic/claude-haiku-5.5",
        "supported_params": ["reasoning"]});
    let content = turn(&mock, params, vec![user("weather?"), bridge()]).await;
    assert_answered(&content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1282_a_bridge_sentence_on_the_messages_wire_is_taken() {
    let mock = MockOpenAI::start(vec![messages_answer()]).await;
    let params = json!({"provider": "anthropic", "model": "claude-haiku-5-5", "max_tokens": 64});
    let content = turn(&mock, params, vec![user("weather?"), bridge()]).await;
    assert_answered(&content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn temperature_to_a_model_without_it_is_taken_once_the_row_drops_it() {
    let model = "anthropic/claude-haiku-5.5";
    assert!(
        model_refusals(model, "chat")
            .iter()
            .any(|r| r.param == "temperature")
    );
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let params = json!({"provider": "openai", "model": model, "temperature": 0.3,
        "supported_params": ["reasoning"]});
    let content = turn(&mock, params, vec![user("hi")]).await;
    assert_answered(&content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn budget_thinking_to_a_model_that_refuses_it_is_taken_once_the_row_drops_it() {
    let model = "claude-haiku-5-5";
    assert!(
        model_refusals(model, "messages")
            .iter()
            .any(|r| r.param == "thinking_enabled")
    );
    let mock = MockOpenAI::start(vec![messages_answer()]).await;
    let params = json!({"provider": "anthropic", "model": model, "max_tokens": 2048,
        "thinking_budget": 1024, "supported_params": ["reasoning_effort"]});
    let content = turn(&mock, params, vec![user("hi")]).await;
    assert_answered(&content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reasoning_off_to_a_model_that_must_reason_is_refused_with_the_recorded_text() {
    let model = "anthropic/claude-sonnet-5.5";
    let rec = model_refusals(model, "chat")
        .into_iter()
        .find(|r| r.param == "reasoning_off")
        .expect("R3 recorded");
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let off = json!({"provider": "openai", "model": model, "reasoning": {"enabled": false},
        "supported_params": ["reasoning"]});
    let content = turn(&mock, off, vec![user("hi")]).await;
    assert!(
        content.to_string().contains(&rec.message()),
        "the refusal reaches the answer: {content}"
    );
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let low = json!({"provider": "openai", "model": model, "reasoning_effort": "low",
        "supported_params": ["reasoning"]});
    assert_answered(&turn(&mock, low, vec![user("hi")]).await);
}
