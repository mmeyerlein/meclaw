//! GH #853: a model has a start value, a guarded run-time path and a package —
//! visible, reversible, operator only.
//!
//! Measured at the seam: every assertion reads the request body at a mock
//! provider, the `invalid_input` emission of the cell, the overlay rows in the
//! cell's own `cell.db`, or the one visible params line. Nothing asserts
//! "the cell applied it" from the inside.
//!
//! The run-time path is the params overlay that has existed since W4b; what
//! this issue adds around it:
//!
//! - `$reset` returns keys to the start value, and the stored overlay follows,
//!   so a respawn comes back on the start value too;
//! - package keys (`MODEL_PACKAGE_KEYS`) take effect only from a params-only
//!   message — a turn carrying `params.model` changes nothing;
//! - `base_url` moves only inside `params.base_url_allow`, never to `null`;
//! - `external_timeout_ms` may not outgrow the cell's backstop;
//! - `wire_dialect` changes at run time, `provider` and the credential do not;
//! - `model_prompt` is the FIRST block of the system part.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "mock_responses.rs"]
mod mock_responses;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::{LlmParams, MODEL_PACKAGE_KEYS};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use mock_responses::{MockResponses, canned_sse_text};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

fn birth(base_url: &str, allow: &[&str]) -> Value {
    json!({
        "provider": "openai",
        "model": "start-model",
        "api_key": "test-key-853",
        "base_url": base_url,
        "base_url_allow": allow,
    })
}

/// Born the way the factory births it: start value from `config.json`, the
/// overlay from the cell's own `cell.db`.
fn born(birth: &Value, td: &TempDir) -> (LlmCell, DbConn) {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let cell = LlmCell::restored(&conn, birth, reqwest::Client::builder().build().unwrap())
        .expect("restore");
    (cell, DbConn::wrap(conn, None))
}

fn params_only(update: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/operator"))
        .body(Body::Inline(json!({ "params": update })))
        .build()
}

fn turn(extra: Value) -> meclaw_core::Message {
    let mut body = json!({
        "system": {"identity": {"text": "I am the persona."}},
        "messages": [{"origin": "user", "type": "text", "text": "Hi"}],
    });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build()
}

fn system_content(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn next_error_code(rx: &mut mpsc::Receiver<CellEmission>) -> Option<String> {
    let em = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()??;
    em.content["header"]["error_code"]
        .as_str()
        .map(str::to_string)
}

async fn overlay_keys(db: &mut DbConn) -> Vec<String> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT key FROM params ORDER BY key").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>()
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_only_switch_reaches_the_second_endpoint_and_reset_brings_back_the_first() {
    let one = MockOpenAI::start(vec![
        canned_chat_completion("from one", "stop"),
        canned_chat_completion("from one again", "stop"),
    ])
    .await;
    let two = MockOpenAI::start(vec![canned_chat_completion("from two", "stop")]).await;
    let one_url = format!("{}/v1", one.base_url);
    let two_url = format!("{}/v1", two.base_url);
    let td = TempDir::new().unwrap();
    let b = birth(&one_url, &[&one.base_url, &two.base_url]);
    let (mut cell, mut db) = born(&b, &td);
    let (sink, mut rx) = sink();

    // The package: model, endpoint and the model's own prompt block.
    cell.handle(
        params_only(json!({
            "model": "second-model",
            "base_url": two_url,
            "model_prompt": "Answer in short sentences; this model rambles.",
        })),
        &sink,
        &mut db,
    )
    .await;
    assert!(rx.try_recv().is_err(), "a params-only message stays silent");
    cell.handle(turn(json!({})), &sink, &mut db).await;
    let _ = rx.recv().await;

    let at_two = two.recorded_requests().await;
    assert_eq!(at_two.len(), 1, "the next call reaches the second endpoint");
    assert!(one.recorded_requests().await.is_empty());
    assert_eq!(at_two[0].model(), Some("second-model"));
    let system = system_content(&at_two[0].body);
    assert!(
        system.starts_with("Answer in short sentences; this model rambles.\n\nI am the persona."),
        "model_prompt is the FIRST block of the system part, before the persona: {system:?}"
    );

    // The line names every key's source.
    let line = cell.params_line("/brain");
    assert!(line.starts_with("llm: params /brain "), "{line}");
    assert!(line.contains("model=second-model[overlay]"), "{line}");
    assert!(line.contains("temperature=0.7[start]"), "{line}");
    assert!(
        line.contains("(overlay: base_url,model,model_prompt)"),
        "{line}"
    );
    assert!(!line.contains("test-key-853"), "never a secret: {line}");
    assert!(
        !line.contains("rambles"),
        "model_prompt only as length + hash: {line}"
    );
    assert!(line.contains("model_prompt=len:46,sha:"), "{line}");

    // $reset returns all three to the start value.
    cell.handle(
        params_only(json!({"$reset": ["model", "base_url", "model_prompt"]})),
        &sink,
        &mut db,
    )
    .await;
    assert!(rx.try_recv().is_err(), "a reset is silent as well");
    assert!(
        overlay_keys(&mut db).await.is_empty(),
        "the stored overlay follows the reset"
    );
    cell.handle(turn(json!({})), &sink, &mut db).await;
    let _ = rx.recv().await;
    let at_one = one.recorded_requests().await;
    assert_eq!(at_one.len(), 1, "back on the first endpoint");
    assert_eq!(at_one[0].model(), Some("start-model"));
    assert_eq!(
        system_content(&at_one[0].body),
        "I am the persona.",
        "no model_prompt block after the reset"
    );

    // …and a respawn comes back on the start value too.
    drop(cell);
    drop(db);
    let (mut cell, mut db) = born(&b, &td);
    assert_eq!(cell.params.model, "start-model");
    cell.handle(turn(json!({})), &sink, &mut db).await;
    let _ = rx.recv().await;
    let at_one = one.recorded_requests().await;
    assert_eq!(at_one.len(), 2);
    assert_eq!(at_one[1].model(), Some("start-model"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_overlay_survives_a_respawn_until_it_is_reset() {
    let td = TempDir::new().unwrap();
    let b = birth("http://127.0.0.1:1/v1", &[]);
    let (mut cell, mut db) = born(&b, &td);
    let (sink, _rx) = sink();
    cell.handle(
        params_only(json!({"model": "overlay-model"})),
        &sink,
        &mut db,
    )
    .await;
    drop(cell);
    drop(db);
    let (mut cell, mut db) = born(&b, &td);
    assert_eq!(
        cell.params.model, "overlay-model",
        "the overlay is respawn-proof"
    );
    cell.handle(params_only(json!({"$reset": ["model"]})), &sink, &mut db)
        .await;
    drop(cell);
    drop(db);
    let (cell, _db) = born(&b, &td);
    assert_eq!(cell.params.model, "start-model", "so is the reset");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_carrying_params_model_changes_nothing() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("a", "stop"),
        canned_chat_completion("b", "stop"),
    ])
    .await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = born(&birth(&format!("{}/v1", mock.base_url), &[]), &td);
    let (sink, mut rx) = sink();

    cell.handle(
        turn(json!({"params": {"model": "smuggled-model"}})),
        &sink,
        &mut db,
    )
    .await;
    let em = rx.recv().await.expect("the turn still runs");
    assert_eq!(
        em.content["header"]["finish_reason"], "stop",
        "the conversation is not broken off"
    );
    cell.handle(turn(json!({})), &sink, &mut db).await;
    let _ = rx.recv().await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0].model(), Some("start-model"), "not on this turn");
    assert_eq!(snaps[1].model(), Some("start-model"), "not on the next one");
    assert!(
        overlay_keys(&mut db).await.is_empty(),
        "nothing reached the overlay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_guards_refuse_with_invalid_input() {
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = born(&birth("http://127.0.0.1:1/v1", &[]), &td);
    let cell_timeout = Duration::from_millis(180_000);
    cell = cell.with_message_timeout(Some(cell_timeout));
    let (sink, mut rx) = sink();

    for (update, why) in [
        (
            json!({"base_url": "http://127.0.0.1:2/v1"}),
            "no base_url_allow: base_url is fixed at run time",
        ),
        (json!({"base_url": null}), "base_url null at run time"),
        (json!({"provider": "openai"}), "provider is immutable"),
        (json!({"api_key": "other"}), "the credential is immutable"),
        (
            json!({"external_timeout_ms": 175_000}),
            "a call timeout the backstop does not clear",
        ),
        (
            json!({"$reset": ["no_such_param"]}),
            "an unknown reset name",
        ),
        (json!({"$reset": "model"}), "$reset is a list"),
    ] {
        cell.handle(params_only(update.clone()), &sink, &mut db)
            .await;
        assert_eq!(
            next_error_code(&mut rx).await.as_deref(),
            Some("invalid_input"),
            "{why}: {update}"
        );
    }
    assert!(
        overlay_keys(&mut db).await.is_empty(),
        "no refusal wrote anything"
    );
    assert_eq!(cell.params.external_timeout_ms, 110_000);

    // Inside the margin it is a knob like any other.
    cell.handle(
        params_only(json!({"external_timeout_ms": 150_000})),
        &sink,
        &mut db,
    )
    .await;
    assert!(rx.try_recv().is_err());
    assert_eq!(cell.params.external_timeout_ms, 150_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_base_url_outside_the_list_is_refused_and_one_inside_is_taken() {
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = born(
        &birth("http://127.0.0.1:1/v1", &["http://127.0.0.1:2"]),
        &td,
    );
    let (sink, mut rx) = sink();
    cell.handle(
        params_only(json!({"base_url": "http://127.0.0.1:3/v1"})),
        &sink,
        &mut db,
    )
    .await;
    assert_eq!(
        next_error_code(&mut rx).await.as_deref(),
        Some("invalid_input")
    );
    cell.handle(
        params_only(json!({"base_url": "http://user:pw@127.0.0.1:2/v1"})),
        &sink,
        &mut db,
    )
    .await;
    assert_eq!(
        next_error_code(&mut rx).await.as_deref(),
        Some("invalid_input"),
        "userinfo is never a listed origin"
    );
    cell.handle(
        params_only(json!({"base_url": "http://127.0.0.1:2/other/v1"})),
        &sink,
        &mut db,
    )
    .await;
    assert!(rx.try_recv().is_err(), "a listed origin is accepted");
    assert_eq!(
        cell.params.base_url.as_deref(),
        Some("http://127.0.0.1:2/other/v1")
    );
    // The list itself is out of the message path's reach.
    cell.handle(
        params_only(json!({"base_url_allow": ["http://127.0.0.1:3"]})),
        &sink,
        &mut db,
    )
    .await;
    assert_eq!(
        next_error_code(&mut rx).await.as_deref(),
        Some("invalid_input")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_dialect_switches_at_run_time() {
    let chat = MockOpenAI::start(vec![]).await;
    let resp = MockResponses::start(vec![canned_sse_text("hello", "resp-model")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = born(
        &birth(&format!("{}/v1", chat.base_url), &[&resp.base_url]),
        &td,
    );
    let (sink, mut rx) = sink();
    cell.handle(
        params_only(json!({"wire_dialect": "responses", "base_url": resp.base_url})),
        &sink,
        &mut db,
    )
    .await;
    assert!(rx.try_recv().is_err());
    cell.handle(turn(json!({})), &sink, &mut db).await;
    let _ = rx.recv().await;
    let recorded = resp.recorded().await;
    assert_eq!(
        recorded.len(),
        1,
        "the next request travels the Responses wire"
    );
    assert!(
        recorded[0].path.ends_with("/responses"),
        "{}",
        recorded[0].path
    );
    assert_eq!(recorded[0].body["model"], "start-model");
    assert!(chat.recorded_requests().await.is_empty());
}

#[test]
fn the_package_keys_are_the_contract() {
    // L2a pushes against exactly these names (plans: R-SN-7). A change here is
    // a change of that contract, not a refactoring.
    assert_eq!(
        MODEL_PACKAGE_KEYS,
        &[
            "model",
            "base_url",
            "wire_dialect",
            "reasoning_effort",
            "reasoning_wire",
            "reasoning",
            "thinking_budget",
            "max_tokens",
            "temperature",
            "external_timeout_ms",
            "provider_extra",
            "model_prompt",
        ]
    );
    // Every package key is a known, run-time-mutable param.
    let p = LlmParams::parse(&json!({
        "provider": "openai", "model": "m", "api_key": "k"
    }))
    .unwrap();
    assert!(p.model_prompt.is_none());
}

#[test]
fn a_model_prompt_over_8_kib_is_refused_at_birth() {
    let raw = json!({
        "provider": "openai", "model": "m", "api_key": "k",
        "model_prompt": "x".repeat(8 * 1024 + 1),
    });
    let err = LlmParams::parse(&raw).unwrap_err();
    assert!(err.contains("model_prompt"), "{err}");
}

/// B-8: a brain that spends a grant for its bearer and holds none yet must not
/// treat a params-only package push as a turn. The registry sends
/// `{"system": {}, "params": {…}}` (the form `llm-registry`, `argus` and
/// `steward` already use); parking it would ask the vault for a credential no
/// provider call needs, and answer the push with a `credential_request` on the
/// brain's lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_only_push_never_opens_a_credential_round() {
    let td = TempDir::new().unwrap();
    let b = json!({
        "provider": "openai", "model": "start-model", "api_key": "",
        "credential_grant_id": "grant:853",
        "base_url": "http://127.0.0.1:1/v1",
    });
    let (mut cell, mut db) = born(&b, &td);
    let (sink, mut rx) = sink();
    let push = MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/registry"))
        .body(Body::Inline(
            json!({"system": {}, "params": {"model": "pushed-model"}}),
        ))
        .build();
    cell.handle(push, &sink, &mut db).await;
    assert!(
        rx.try_recv().is_err(),
        "no credential_request, no receipt: a push is not a turn"
    );
    assert_eq!(
        cell.params.model, "pushed-model",
        "and it is applied at once"
    );
    assert_eq!(overlay_keys(&mut db).await, vec!["model".to_string()]);
}

/// Review of L2, M-1: an overlay written before 0.46.0 could carry a
/// `base_url` no list allowed (a run-time `base_url` needed none then). The
/// restore runs the same run-time guards an update runs and names what fails,
/// so the owner sees it on the `llm: params` warning -- the value stays in
/// force (refusing a boot over an old overlay would take the brain down), and
/// `$reset` returns it to the start value.
#[test]
fn a_restore_names_an_overlay_the_run_time_guards_would_refuse() {
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES ('base_url', ?, 0)",
        ["\"http://127.0.0.1:2/v1\""],
    )
    .unwrap();
    let b = birth("http://127.0.0.1:1/v1", &[]);
    let cell = LlmCell::restored(&conn, &b, reqwest::Client::builder().build().unwrap())
        .expect("restore")
        .with_message_timeout(Some(Duration::from_secs(240)));
    assert_eq!(
        cell.params.base_url.as_deref(),
        Some("http://127.0.0.1:2/v1"),
        "the overlay stays in force"
    );
    let why = cell
        .restore_check()
        .expect_err("an overlay base_url with no list is named");
    assert!(why.contains("base_url_allow"), "{why}");
    assert!(
        !why.contains("127.0.0.1:2"),
        "the detail names no value: {why}"
    );

    // An overlay the guards take is not named.
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES ('model', '\"m2\"', 0)",
        [],
    )
    .unwrap();
    let cell = LlmCell::restored(&conn, &b, reqwest::Client::builder().build().unwrap())
        .expect("restore")
        .with_message_timeout(Some(Duration::from_secs(240)));
    assert!(cell.restore_check().is_ok());
}
