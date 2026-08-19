//! GH #271 — an empty `api_key` is an absent `api_key`, and the proof is the
//! header the endpoint records.
//!
//! `LlmParams::parse` rejects a *missing* `api_key` on the `api_key` lane, but
//! it accepts `""`: `Option<String>` turns the empty string into `Some("")`,
//! which is not `None`. Both lanes then handed that value to the wire layer
//! with `unwrap_or_default()`, so a keyless configuration sent
//! `Authorization: Bearer ` with nothing after it.
//!
//! # Why omitting the header, and not refusing the empty value
//!
//! #268 and #270 repaired the same shape in `mcp` and `web_search`, and #270
//! additionally made an empty **required** credential a parse-time refusal.
//! Refusal is wrong here: an OpenAI-compatible server on localhost typically
//! ignores the header entirely, and `templates/builder-hive/intake-llm` points
//! at exactly such an endpoint, so refusing would break a keyless setup the
//! library itself ships. Sending the empty bearer is wrong too, for #270's
//! reason: against a server that would have answered anonymously an empty
//! `Authorization` header can be a flat rejection, and that failure then reads
//! as "the provider is down" rather than as "a key nobody configured".
//!
//! So: **no key ⇒ no header**. That is the same sentence `web_search` and
//! `mcp` now say, which is the point — the rule is "an empty credential is an
//! absent credential", stated once and true everywhere.
//!
//! # What is pinned
//!
//! The recorded request header, both directions, on **both** lanes a static
//! `api_key` can travel (`chat_completions` and `responses`): a key that is
//! set has to arrive verbatim, a key that is empty must produce no header at
//! all. The first direction keeps a future filter from swallowing a real
//! credential; the second is the defect.
//!
//! The third test guards the neighbouring track. `llm` presents a second kind
//! of bearer — the OAuth access token the broker issues — and that one does
//! **not** come from `params`, so the emptiness rule must not reach it. It is
//! kept out by construction: the wire layer presents verbatim whatever bearer
//! it is handed and judges nothing, and the filter sits at the two call sites
//! that read `params.api_key`. `the_wire_presents_the_bearer_it_is_handed`
//! pins that split, so a later move of the filter *into* the wire layer —
//! which would silently change the broker lane too — turns red here.

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
use mock_openai::{MockOpenAI, canned_chat_completion};
use mock_responses::{MockResponses, canned_sse_text};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// An obvious placeholder — never a real credential, and shaped so a failure
/// message cannot be mistaken for a leak.
const PLACEHOLDER_KEY: &str = "gh271-placeholder-key";

/// One inference message; the content is irrelevant, only the request the cell
/// makes is under test.
fn inference_msg() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "hi"}]
        })))
        .build()
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

/// Drive a real `LlmCell` built from `raw` through one message and hand back
/// the emitted body, so every assertion below rests on a positive receipt (the
/// cell answered) rather than on an empty capture list read too early.
async fn run_once(raw: Value) -> Value {
    let params = LlmParams::parse(&raw).expect("the params parse");
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let td = TempDir::new().expect("tempdir");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db opens");
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();
    cell.handle(inference_msg(), &sink, &mut db).await;
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("the llm cell answered within 30s")
        .expect("the sink is still open")
        .content
}

// ───── chat_completions lane ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_key_arrives_as_an_authorization_header() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi back", "stop")]).await;
    let out = run_once(json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": PLACEHOLDER_KEY,
        "base_url": format!("{}/v1", mock.base_url),
    }))
    .await;
    assert!(out.get("meta").is_some(), "the cell emitted a turn: {out}");

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "exactly one provider call");
    assert_eq!(
        reqs[0].headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {PLACEHOLDER_KEY}").as_str()),
        "a configured api_key must reach the wire unchanged"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_api_key_sends_no_authorization_header_at_all() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi back", "stop")]).await;
    let out = run_once(json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "",
        "base_url": format!("{}/v1", mock.base_url),
    }))
    .await;
    assert!(out.get("meta").is_some(), "the cell emitted a turn: {out}");

    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "exactly one provider call");
    assert_eq!(
        reqs[0].headers.get("authorization"),
        None,
        "an empty api_key produced an Authorization header — against an endpoint that \
         would have answered anonymously an empty bearer can be a flat rejection, and that \
         reads as a broken provider rather than as a credential nobody set"
    );
}

// ───── responses lane, same static credential ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_key_arrives_on_the_responses_lane() {
    let api = MockResponses::start(vec![canned_sse_text("hi back", "gpt-5-mock")]).await;
    let out = run_once(json!({
        "provider": "openai",
        "model": "gpt-5",
        "api_key": PLACEHOLDER_KEY,
        "wire_dialect": "responses",
        "base_url": api.base_url.clone(),
    }))
    .await;
    assert!(out.get("meta").is_some(), "the cell emitted a turn: {out}");

    let reqs = api.recorded().await;
    assert_eq!(reqs.len(), 1, "exactly one provider call");
    assert_eq!(
        reqs[0].header("authorization"),
        Some(&format!("Bearer {PLACEHOLDER_KEY}")[..]),
        "a configured api_key must reach the responses wire unchanged"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_api_key_sends_no_authorization_header_on_the_responses_lane() {
    let api = MockResponses::start(vec![canned_sse_text("hi back", "gpt-5-mock")]).await;
    let out = run_once(json!({
        "provider": "openai",
        "model": "gpt-5",
        "api_key": "",
        "wire_dialect": "responses",
        "base_url": api.base_url.clone(),
    }))
    .await;
    assert!(out.get("meta").is_some(), "the cell emitted a turn: {out}");

    let reqs = api.recorded().await;
    assert_eq!(reqs.len(), 1, "exactly one provider call");
    assert_eq!(
        reqs[0].header("authorization"),
        None,
        "an empty api_key produced an Authorization header on the responses lane — the \
         rule is one rule, not one per dialect"
    );
}

// ───── the neighbouring track: the broker's token is not param-shaped ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wire_presents_the_bearer_it_is_handed() {
    let api = MockResponses::start(vec![canned_sse_text("hi", "gpt-5-mock")]).await;
    let url = format!("{}/responses", api.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    // `call_responses` is the shared exit of BOTH bearer tracks: the static
    // `api_key` and the OAuth access token the broker issues. It must stay
    // dumb — it presents what it is handed. `Some("")` is therefore still a
    // header, and the emptiness rule lives one level up, at the two call sites
    // that read `params.api_key`. If this assertion ever has to change, the
    // filter has moved into the wire and the broker lane changed with it.
    meclaw_cells::llm::wire::call_responses(
        &client,
        &url,
        Some(""),
        &[],
        &json!({"model": "gpt-5"}),
        Duration::from_secs(5),
    )
    .await
    .expect("the mock answered");

    let reqs = api.recorded().await;
    assert_eq!(reqs.len(), 1);
    assert!(
        reqs[0].header("authorization").is_some(),
        "the wire layer must not judge the bearer it is handed — that judgement \
         belongs to the params boundary, which the broker's token never crosses"
    );
}
