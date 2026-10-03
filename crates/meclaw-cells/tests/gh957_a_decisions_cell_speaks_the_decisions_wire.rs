//! GH #957 -- a `llm` cell born with `provider: "decisions"` asks typed
//! questions about one state and answers with ONE `decision` emission.
//!
//! Pinned here, against the in-process HTTP mock (no network, no cost):
//!
//! 1. One `decide` slot with one question of each kind (`choice`, `yes_no`,
//!    `scale`) is exactly ONE `POST <base_url>/alpha/decisions` -- no `/v1`,
//!    no stream -- whose body equals `fixtures/decisions_request.json`, with
//!    the bearer set.
//! 2. The answer (`fixtures/decisions_response.json`, the shape of the one
//!    live answer in `fixtures/decisions_response_live.json`) becomes exactly
//!    one emission: `{decision: {answers, model, ms}}` with the answers
//!    normalized per kind, `meta.provider` = `decisions`, `finish_reason`
//!    `stop`. The caller's `context` key rides on through the substrate's
//!    header rule while the input hop dies -- so a caller correlates through
//!    `context` and the cell never learns its key (OR-DP-11).
//! 3. A cell born on an empty model is `decisions_unconfigured` and sends
//!    nothing.
//! 4. The live answer of the hosted service parses (the parser case the
//!    wire proof of the strand left behind).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::MockOpenAI;
use tempfile::TempDir;
use tokio::sync::mpsc;

const REQUEST_FIXTURE: &str = "tests/fixtures/decisions_request.json";
const RESPONSE_FIXTURE: &str = "tests/fixtures/decisions_response.json";
const LIVE_FIXTURE: &str = "tests/fixtures/decisions_response_live.json";

fn fixture(rel: &str) -> Value {
    let text = std::fs::read_to_string(rel).unwrap_or_else(|e| panic!("{rel}: {e}"));
    meclaw_core::serde_json::from_str(&text).expect("fixture is JSON")
}

/// The caller's headers: a correlation key in `context`, a route in `hop`.
fn caller_headers() -> Headers {
    let mut context = Map::new();
    context.insert("show_id".into(), json!("s-1"));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("decide"));
    Headers::from_parts(context, hop)
}

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/decide"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        caller_headers(),
        None,
    );
    (sink, rx)
}

fn decisions_cell(base_url: &str, model: &str) -> (LlmCell, TempDir, DbConn) {
    let raw = json!({
        "provider": "decisions",
        "model": model,
        "api_key": "test-key-decide",
        "base_url": base_url,
    });
    let params = LlmParams::parse(&raw).expect("a decisions cell parses");
    let cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (cell, td, DbConn::wrap(conn, None))
}

/// The cell's own vocabulary for the three questions of the request fixture.
/// The body carries an empty `messages` slot: a UBF body needs one of its
/// three slots, and the cell does not read it in this mode.
fn decide_body() -> Value {
    json!({
        "messages": [],
        "decide": {
            "state": "The user said: Will it rain in the city tomorrow afternoon?",
            "questions": {
                "topic": {
                    "kind": "choice",
                    "instructions": "Which topic does the request belong to?",
                    "options": {
                        "weather": "Weather, forecast, temperature, rain",
                        "calendar": "Appointments, dates, schedule",
                        "none": "Fits none of the topics"
                    }
                },
                "wants_screen": {
                    "kind": "yes_no",
                    "instructions": "Would the person benefit from seeing the answer on a screen?"
                },
                "urgency": {
                    "kind": "scale",
                    "instructions": "How urgent is the request?",
                    "levels": ["low", "medium", "high"]
                }
            }
        }
    })
}

fn decide_msg(body: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/decide"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(body))
        .build()
}

/// Every emission the cell made for one message -- `handle` has returned, so
/// whatever is not in the channel now was never sent.
fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<CellEmission> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_decide_is_one_post_and_one_whole_decision() {
    let answer = meclaw_core::serde_json::to_vec(&fixture(RESPONSE_FIXTURE)).unwrap();
    let mock = MockOpenAI::start(vec![MockResponse::ok_json(&answer)]).await;
    let (mut cell, _td, mut db) =
        decisions_cell(&format!("{}/api", mock.base_url), "vendor/decider");
    let (sink, mut rx) = mk_sink();

    cell.handle(decide_msg(decide_body()), &sink, &mut db).await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one call");
    assert_eq!(snaps[0].method, "POST");
    assert_eq!(snaps[0].path, "/api/alpha/decisions", "no /v1 on this wire");
    assert_eq!(
        snaps[0].body,
        fixture(REQUEST_FIXTURE),
        "the request drifted from the fixture:\n{}",
        meclaw_core::serde_json::to_string_pretty(&snaps[0].body).unwrap()
    );
    assert_eq!(
        snaps[0].headers.get("authorization").map(String::as_str),
        Some("Bearer test-key-decide")
    );

    let ems = drain(&mut rx);
    assert_eq!(ems.len(), 1, "one emission per call");
    let em = &ems[0];
    assert_eq!(em.target, Path::new("/caller"));
    assert_eq!(
        em.content["decision"]["answers"],
        json!({
            "topic": {"choice": "weather",
                      "p": {"weather": 0.95, "calendar": 0.04, "none": 0.01}},
            "wants_screen": {"yes": 0.81},
            "urgency": {"value": 0.15, "p": {"low": 0.86, "medium": 0.14, "high": 0.0}}
        })
    );
    assert_eq!(em.content["decision"]["model"], "vendor/decider-1");
    assert!(em.content["decision"]["ms"].is_u64());
    assert_eq!(em.content["meta"]["provider"], "decisions");
    assert_eq!(em.content["header"]["finish_reason"], "stop");
    assert_eq!(em.content["header"]["tokens_prompt"], 405);
    meclaw_core::validate_ubf_body(&em.content).expect("a UBF body");

    // The substrate's header rule over this emission: context rides on
    // unchanged, the caller's hop dies. The cell wrote no caller key.
    let header = em.content["header"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let next = em.input_headers.carry_context_with_hop(header);
    assert_eq!(next.context.get("show_id"), Some(&json!("s-1")));
    assert!(next.hop.get("route").is_none(), "the input hop died");
    assert!(next.hop.get("show_id").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_born_without_a_model_is_unconfigured_and_sends_nothing() {
    let mock = MockOpenAI::start(vec![]).await;
    let (mut cell, _td, mut db) = decisions_cell(&format!("{}/api", mock.base_url), "");
    let (sink, mut rx) = mk_sink();

    cell.handle(decide_msg(decide_body()), &sink, &mut db).await;

    assert!(mock.recorded_requests().await.is_empty(), "no request");
    let ems = drain(&mut rx);
    assert_eq!(ems.len(), 1);
    assert_eq!(ems[0].content["header"]["finish_reason"], "error");
    assert_eq!(
        ems[0].content["header"]["error_code"],
        "decisions_unconfigured"
    );
    assert!(ems[0].content.get("decision").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_live_answer_of_the_service_parses() {
    let live = meclaw_core::serde_json::to_vec(&fixture(LIVE_FIXTURE)).unwrap();
    let mock = MockOpenAI::start(vec![MockResponse::ok_json(&live)]).await;
    let (mut cell, _td, mut db) =
        decisions_cell(&format!("{}/api", mock.base_url), "vendor/decider");
    let (sink, mut rx) = mk_sink();

    cell.handle(decide_msg(decide_body()), &sink, &mut db).await;

    let ems = drain(&mut rx);
    assert_eq!(ems.len(), 1);
    let answers = &ems[0].content["decision"]["answers"];
    assert_eq!(answers["topic"]["choice"], "weather");
    assert!(answers["wants_screen"]["yes"].is_f64());
    assert!(answers["urgency"]["value"].is_number());
    assert_eq!(
        answers["urgency"]["p"].as_object().map(|p| p.len()),
        Some(3),
        "every level by name: {answers}"
    );
}
