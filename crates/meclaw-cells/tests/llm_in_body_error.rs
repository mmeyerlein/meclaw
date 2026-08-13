//! GH #75 — an upstream error returned INSIDE a 200 body, on both dialects.
//!
//! An OpenAI-compatible gateway answers `HTTP 200` with a body that carries no
//! `choices` at all, only a top level `error` object. That is how such a
//! gateway surfaces a transient upstream failure. Before this pin the translate
//! stage saw the missing `choices[0]`, reported `provider_error` with
//! `meta.error.source: "parse"`, and replaced the provider's sentence with
//! `missing choices[0]` — so a failover edge conditioned on
//! `error_code == "rate_limit"` could never fire.
//!
//! The three bodies pinned here are the whole discriminator:
//! an in-body 429, an in-body 5xx, and a body that really has no shape.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "mock_responses.rs"]
mod mock_responses;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::MockOpenAI;
use mock_responses::MockResponses;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The verbatim body shape observed live on 2026-08-12 (issue #75): HTTP 200,
/// no `choices`, one `error` object whose `code` is the upstream status.
fn in_body_error_200(message: &str, code: u16) -> MockResponse {
    let body = json!({"error": {"message": message, "code": code}});
    MockResponse::ok_json(body.to_string().as_bytes())
}

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
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

fn inference_msg() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "who are you?"}]
        })))
        .build()
}

/// Drive one inference through a real `LlmCell` against `base_url` and return
/// the emitted body.
async fn drive(raw_params: Value) -> Value {
    let td = TempDir::new().unwrap();
    let params = LlmParams::parse(&raw_params).expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();
    cell.handle(inference_msg(), &sink, &mut db).await;
    rx.recv()
        .await
        .expect("the cell must emit something")
        .content
}

// ───── chat-completions dialect ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_completions_in_body_429_reaches_the_rate_limit_lane() {
    let mock = MockOpenAI::start(vec![in_body_error_200(
        "openai/gpt-x is temporarily rate-limited upstream. Please retry shortly.",
        429,
    )])
    .await;
    let out = drive(json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    }))
    .await;

    // The lane a failover edge routes on.
    assert_eq!(out["header"]["finish_reason"], "error");
    assert_eq!(
        out["header"]["error_code"], "rate_limit",
        "an in-body 429 must land where an HTTP-level 429 lands: {out}"
    );
    // The source is the wire, not the parser.
    assert_eq!(out["meta"]["error"]["source"], "wire");
    assert_eq!(out["meta"]["error"]["kind"], "rate_limited");
    assert_eq!(out["meta"]["error"]["in_body"], true);
    assert_eq!(out["meta"]["error"]["upstream_status"], 429);
    // What the provider said, not what the parser missed.
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("rate-limited upstream"),
        "the upstream sentence must reach the operator: {detail}"
    );
    assert!(
        !detail.contains("missing choices[0]"),
        "a parser complaint must not stand in for the provider's message: {detail}"
    );
    // Gate-1: the conversation is passed through unchanged, so a failover edge
    // can hand it to a backup brain.
    assert_eq!(out["messages"][0]["text"], "who are you?");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_completions_in_body_5xx_is_a_transient_provider_error() {
    let mock = MockOpenAI::start(vec![in_body_error_200("upstream is unavailable", 503)]).await;
    let out = drive(json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    }))
    .await;
    assert_eq!(out["header"]["error_code"], "provider_error");
    assert_eq!(out["meta"]["error"]["source"], "wire");
    assert_eq!(out["meta"]["error"]["kind"], "transient");
    assert_eq!(out["meta"]["error"]["upstream_status"], 503);
}

/// The discriminator: a body with neither `choices` nor `error` is still a
/// genuine shape defect, and still says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shapeless_200_body_is_still_a_parse_failure() {
    let garbage = json!({"id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-x"});
    let mock = MockOpenAI::start(vec![MockResponse::ok_json(garbage.to_string().as_bytes())]).await;
    let out = drive(json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    }))
    .await;
    assert_eq!(out["header"]["error_code"], "provider_error");
    assert_eq!(
        out["meta"]["error"]["source"], "parse",
        "a shapeless body is a parse failure, and must stay one: {out}"
    );
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("missing choices[0]"),
        "`missing choices[0]` stays reserved for exactly this body: {detail}"
    );
}

// ───── responses dialect ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responses_in_body_429_reaches_the_rate_limit_lane() {
    let api = MockResponses::start(vec![in_body_error_200(
        "provider is rate-limited upstream, retry shortly",
        429,
    )])
    .await;
    let out = drive(json!({
        "provider": "openai", "model": "gpt-5", "api_key": "sk-test",
        "wire_dialect": "responses", "base_url": api.base_url, "max_tokens": 32,
    }))
    .await;
    assert_eq!(out["header"]["error_code"], "rate_limit");
    assert_eq!(out["meta"]["error"]["source"], "wire");
    assert_eq!(out["meta"]["error"]["kind"], "rate_limited");
    assert_eq!(out["meta"]["error"]["in_body"], true);
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(detail.contains("rate-limited upstream"), "{detail}");
}

/// A Responses body carries a `null` `error` field on the happy path. That must
/// not be read as a failure — otherwise this fix would break every successful
/// non-streamed response.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responses_null_error_field_on_a_good_body_is_not_an_error() {
    let good = json!({
        "id": "resp-1", "object": "response", "model": "gpt-5", "status": "completed",
        "error": null,
        "output": [{"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": "hello"}]}],
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    let api = MockResponses::start(vec![MockResponse::ok_json(good.to_string().as_bytes())]).await;
    let out = drive(json!({
        "provider": "openai", "model": "gpt-5", "api_key": "sk-test",
        "wire_dialect": "responses", "base_url": api.base_url, "max_tokens": 32,
    }))
    .await;
    assert_eq!(
        out["header"]["finish_reason"], "stop",
        "a completed response with error:null must stay a success: {out}"
    );
    assert_eq!(out["messages"][0]["text"], "hello");
}
