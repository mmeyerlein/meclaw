//! GH #1260 (OR-WG-6) -- a `tool_scope` with `require` asks for the tool call.
//!
//! WHY this file exists: a fast model given a turn that needs a lookup wrote
//! "let me check" and stopped, or made the answer up -- in the colony 0 of 5
//! simple turns called their tool, in a probe of the same window 7 of 10, and
//! 10 of 10 only with `tool_choice: required`. The curator asks for the call
//! per request (`tool_scope.require`), and the `llm` cell turns it into the
//! wire's field -- for this request only, over a static
//! `provider_extra.tool_choice`, and never on a request that keeps no tool (a
//! provider refuses a required call without tools).
//!
//! Measured at the seam: the request bodies the mock provider received.
//!
//! Fix round 1 (review M-3/M-4): two requests must not carry the requirement.
//! A request that keeps no tool after the scope (a provider refuses a required
//! call without tools with a 400), and an Anthropic Messages request with
//! thinking on (the provider refuses `tool_choice: {type: "any"}` beside
//! extended thinking with a 400 as well). Both go out without the field, so
//! the turn still gets an answer, and both say so in one `WARN` line --
//! silent, the missing call would look like the model's choice. Measured
//! there in the tracing events of the call as well.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::mpsc;

async fn cell(mock: &MockOpenAI, extra: Value) -> (LlmCell, DbConn, TempDir) {
    let td = TempDir::new().unwrap();
    let mut raw = json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    });
    if !extra.is_null() {
        raw["provider_extra"] = extra;
    }
    let params = LlmParams::parse(&raw).expect("params");
    let cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (cell, DbConn::wrap(conn, None), td)
}

async fn send(cell: &mut LlmCell, db: &mut DbConn, body: Value) {
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
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    let em = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("the cell must emit")
        .expect("the sink stays open");
    assert_eq!(
        em.content["header"]["finish_reason"], "stop",
        "{}",
        em.content
    );
}

fn menu() -> Value {
    let mut tools = serde_json::Map::new();
    for name in ["web", "weather"] {
        let tool = json!({"type": "function", "function": {"name": name, "parameters": {}}});
        tools.insert(name.into(), json!({"text": tool.to_string()}));
    }
    Value::Object(tools)
}

fn turn(text: &str) -> Value {
    json!([{"origin": "user", "type": "text", "text": text}])
}

async fn bodies(mock: &MockOpenAI) -> Vec<Value> {
    mock.recorded_requests()
        .await
        .into_iter()
        .map(|s| s.body)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_required_scope_asks_for_the_call_on_that_request_only() {
    let mock = MockOpenAI::start(
        (0..3)
            .map(|_| canned_chat_completion("ok", "stop"))
            .collect(),
    )
    .await;
    let (mut c, mut db, _td) = cell(&mock, Value::Null).await;
    send(
        &mut c,
        &mut db,
        json!({"system": {"tools": menu()}, "messages": turn("1"),
               "tool_scope": {"allow": ["web", "weather"], "require": true}}),
    )
    .await;
    send(
        &mut c,
        &mut db,
        json!({"messages": turn("2"), "tool_scope": {"allow": ["web"]}}),
    )
    .await;
    // Nothing left on the request: no tools, and no requirement either.
    send(
        &mut c,
        &mut db,
        json!({"messages": turn("3"), "tool_scope": {"allow": [], "require": true}}),
    )
    .await;
    let got = bodies(&mock).await;
    assert_eq!(got[0]["tool_choice"], "required", "{}", got[0]);
    assert!(
        got[1].get("tool_choice").is_none(),
        "the next request is back to the provider's default: {}",
        got[1]
    );
    assert!(
        got[2].get("tools").is_none() && got[2].get("tool_choice").is_none(),
        "no tool stays, so nothing is required: {}",
        got[2]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_required_scope_wins_over_a_static_tool_choice() {
    let mock = MockOpenAI::start(
        (0..2)
            .map(|_| canned_chat_completion("ok", "stop"))
            .collect(),
    )
    .await;
    let (mut c, mut db, _td) = cell(&mock, json!({"tool_choice": "auto"})).await;
    send(
        &mut c,
        &mut db,
        json!({"system": {"tools": menu()}, "messages": turn("1"),
               "tool_scope": {"require": true}}),
    )
    .await;
    send(&mut c, &mut db, json!({"messages": turn("2")})).await;
    let got = bodies(&mock).await;
    assert_eq!(got[0]["tool_choice"], "required", "{}", got[0]);
    assert_eq!(
        got[1]["tool_choice"], "auto",
        "the static overlay stands again on the next call: {}",
        got[1]
    );
}

// ─────────────────────────────────────────────────────────────── log capture

/// The `WARN` messages of the call. Installed per test with `set_default`
/// (thread-local) on a current-thread runtime, so no other test's lines leak
/// in. A std mutex in a TEST collector; the no-lock rule is about cell state.
#[derive(Clone, Default)]
struct Warns(Arc<Mutex<Vec<String>>>);

impl Warns {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Message(String);

impl tracing::field::Visit for Message {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for Warns {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() != tracing::Level::WARN {
            return;
        }
        let mut m = Message(String::new());
        event.record(&mut m);
        self.0.lock().unwrap().push(m.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

// ─────────────────────────────────────────────────────────────── the harness

fn raw_cell(raw: Value) -> (LlmCell, DbConn, TempDir) {
    let td = TempDir::new().unwrap();
    let params = LlmParams::parse(&raw).expect("params");
    let cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (cell, DbConn::wrap(conn, None), td)
}

async fn send_any(cell: &mut LlmCell, db: &mut DbConn, body: Value) -> Value {
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
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    tokio::time::timeout(meclaw_testing::deadline::HANG, rx.recv())
        .await
        .expect("the cell must emit")
        .expect("the sink stays open")
        .content
}

fn anthropic_answer() -> MockResponse {
    let body = json!({
        "id": "msg_1260", "type": "message", "role": "assistant", "model": "claude-test-a",
        "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 2}
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

fn anthropic(mock: &MockOpenAI, extra: Value) -> Value {
    let mut raw = json!({
        "provider": "anthropic", "model": "claude-test-a", "api_key": "test-key-1260",
        "base_url": mock.base_url, "max_tokens": 64,
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        raw[k] = v;
    }
    raw
}

fn required() -> Value {
    json!({"system": {"tools": menu()}, "messages": turn("weather?"),
           "tool_scope": {"allow": ["web", "weather"], "require": true}})
}

// ─────────────────────────────────────────────────────────────── the claims

#[tokio::test]
async fn a_required_call_without_a_tool_left_goes_without_and_says_so() {
    let warns = Warns::default();
    let _guard = tracing::subscriber::set_default(warns.clone());
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let (mut c, mut db, _td) = raw_cell(json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    }));
    let out = send_any(
        &mut c,
        &mut db,
        json!({"system": {"tools": menu()}, "messages": turn("1"),
               "tool_scope": {"allow": [], "require": true}}),
    )
    .await;
    assert_eq!(out["header"]["finish_reason"], "stop", "{out}");
    let body = &mock.recorded_requests().await[0].body;
    assert!(
        body.get("tools").is_none() && body.get("tool_choice").is_none(),
        "{body}"
    );
    let lines = warns.lines();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("requires a tool call but leaves no tool"))
            .count(),
        1,
        "one WARN line says why: {lines:?}"
    );
}

#[tokio::test]
async fn a_required_call_beside_anthropic_thinking_goes_without_and_says_so() {
    let warns = Warns::default();
    let _guard = tracing::subscriber::set_default(warns.clone());
    let mock = MockOpenAI::start(vec![anthropic_answer()]).await;
    let (mut c, mut db, _td) = raw_cell(anthropic(
        &mock,
        json!({"reasoning": {"type": "enabled", "budget_tokens": 1024}}),
    ));
    let out = send_any(&mut c, &mut db, required()).await;
    assert_eq!(out["header"]["finish_reason"], "stop", "{out}");
    let body = &mock.recorded_requests().await[0].body;
    assert_eq!(body["thinking"]["type"], "enabled", "{body}");
    assert!(body.get("tools").is_some(), "the tools still go: {body}");
    assert!(
        body.get("tool_choice").is_none(),
        "the provider refuses a forced call beside thinking: {body}"
    );
    let lines = warns.lines();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("requires a tool call") && l.contains("thinking"))
            .count(),
        1,
        "one WARN line says why: {lines:?}"
    );
}

#[tokio::test]
async fn a_required_call_on_anthropic_without_thinking_asks_for_any_tool() {
    let warns = Warns::default();
    let _guard = tracing::subscriber::set_default(warns.clone());
    let mock = MockOpenAI::start(vec![anthropic_answer(), anthropic_answer()]).await;
    let (mut c, mut db, _td) = raw_cell(anthropic(&mock, Value::Null));
    send_any(&mut c, &mut db, required()).await;
    // Thinking switched off by name is off: the call is asked for.
    let (mut c, mut db, _td2) =
        raw_cell(anthropic(&mock, json!({"reasoning": {"type": "disabled"}})));
    send_any(&mut c, &mut db, required()).await;
    let got = mock.recorded_requests().await;
    for req in &got {
        assert_eq!(
            req.body["tool_choice"],
            json!({"type": "any"}),
            "{}",
            req.body
        );
    }
    let lines = warns.lines();
    assert!(
        lines.iter().all(|l| !l.contains("requires a tool call")),
        "{lines:?}"
    );
}

/// GH #1260 (OR-WG-9, B-L2): a package whose pinned host refuses a forced call
/// says so (`tool_choice_required: false`). Measured: gpt-oss-120b pinned to
/// one host, `tool_choice: "required"` -> HTTP 404 (the router's tool
/// compatibility filter drops the only endpoint), every tool round of a
/// simple turn silent after its bridge; the same window with `auto` called
/// the right tool 20 of 20 times. On such a package the cell sends no
/// `tool_choice` (the provider's `auto`) and says it once per package and
/// waking, not once per call.
///
/// The road the shipped colonies take (review I-1): the cell is born on a
/// `${ctx.model}` row without the key (birth value `true`), the registry
/// pushes the package with `false`, and a later row without the key pushes a
/// `$reset` of it -- back to the birth value, the forced call again.
#[tokio::test]
async fn a_package_that_cannot_force_the_call_degrades_to_auto_and_says_so_once() {
    let warns = Warns::default();
    let _guard = tracing::subscriber::set_default(warns.clone());
    let mock = MockOpenAI::start(
        (0..6)
            .map(|_| canned_chat_completion("ok", "stop"))
            .collect(),
    )
    .await;
    // Born without the key: a host that takes a forced call.
    let (mut c, mut db, _td) = raw_cell(json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    }));
    let degraded = |lines: &[String]| {
        lines
            .iter()
            .filter(|l| l.contains("require_degraded"))
            .count()
    };
    send_any(&mut c, &mut db, required()).await;
    // The registry's push of a package whose host refuses the forced call.
    push(
        &mut c,
        &mut db,
        json!({"model": "openai/gpt-oss-120b", "tool_choice_required": false}),
    )
    .await;
    send_any(&mut c, &mut db, required()).await;
    send_any(&mut c, &mut db, required()).await;
    assert_eq!(
        degraded(&warns.lines()),
        1,
        "one line per package, not per call: {:?}",
        warns.lines()
    );
    // A push of a key outside the package is no package change (review M-1).
    push(
        &mut c,
        &mut db,
        json!({"http_referer": "https://example.org"}),
    )
    .await;
    send_any(&mut c, &mut db, required()).await;
    assert_eq!(
        degraded(&warns.lines()),
        1,
        "no package change, no new line: {:?}",
        warns.lines()
    );
    // The next row carries no key: the registry resets it to the birth value.
    push(
        &mut c,
        &mut db,
        json!({"$reset": ["tool_choice_required"], "model": "gpt-z"}),
    )
    .await;
    send_any(&mut c, &mut db, required()).await;
    let got = mock.recorded_requests().await;
    assert_eq!(got.len(), 5);
    let forced: Vec<Value> = got.iter().map(|r| r.body["tool_choice"].clone()).collect();
    assert_eq!(
        forced,
        vec![
            json!("required"),
            Value::Null,
            Value::Null,
            Value::Null,
            json!("required")
        ],
        "birth forces, the pushed package does not, the reset forces again"
    );
    for req in &got {
        assert!(
            req.body.get("tools").is_some(),
            "the tools still go: {}",
            req.body
        );
    }
    assert_eq!(degraded(&warns.lines()), 1, "{:?}", warns.lines());
    // A package push is a package change: the next degraded call says it again.
    push(
        &mut c,
        &mut db,
        json!({"model": "openai/gpt-oss-120b", "tool_choice_required": false}),
    )
    .await;
    send_any(&mut c, &mut db, required()).await;
    assert_eq!(degraded(&warns.lines()), 2, "{:?}", warns.lines());
}

/// A params-only push from the operator's edge, the registry's shape.
async fn push(cell: &mut LlmCell, db: &mut DbConn, params: Value) {
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
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
        .body(Body::Inline(json!({"system": {}, "params": params})))
        .build();
    cell.handle(msg, &sink, db).await;
}
