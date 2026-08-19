//! GH #264 — a `system.*` subtree can be overwritten, but until now never revoked.
//!
//! `system.*` is durable state of the `llm` cell: one row per slot path, written
//! by an UPSERT. A path that is not sent is a path that is not touched. For a
//! writer with **fixed** paths that is enough — GH #259 repaired two of them by
//! sending the slot unconditionally with an empty rendering, and the upsert
//! overwrites the stale value.
//!
//! It is not enough for a writer whose sub-keys are **data**. The `json` form of
//! the recall bundle (`memory_form: json|both`) is keyed per bundle by the
//! memory hive; the writer does not know which paths the previous turn wrote and
//! therefore cannot name them empty. A key written last turn and absent this
//! turn stayed in the prompt until something happened to write that exact path
//! again — which, for a key derived from what was recalled, may be never.
//!
//! The repair is a **replace marker** in the body: `"$replace": true` inside a
//! node of the incoming `system` subtree means "below this node, exactly what
//! this message carries holds". One message, atomic with the write it belongs
//! to — a revoke-then-write pair would leave the prompt standing without the
//! subtree between the two messages, and the recipient can fire in that window.
//!
//! Two things this file pins, and the second is the load-bearing one:
//!
//! 1. **A marked write revokes.** A key from the previous turn is gone from the
//!    **composed system prompt**, not merely from the store — the prompt is
//!    where the damage showed.
//! 2. **An unmarked write does not.** `system.*` is deliberately accumulated by
//!    several independent writers under different paths. A plain write that
//!    silently dropped everything else under a shared root would turn each of
//!    them into the last one standing. The marker is opt-in, always.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

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

/// A cell on a fresh `cell.db`, `extra` merged over the minimal params.
fn cell_with(td: &TempDir, base_url: &str, extra: Value) -> (LlmCell, DbConn) {
    let mut raw = json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"),
    });
    for (k, v) in extra.as_object().expect("extra must be an object") {
        raw[k] = v.clone();
    }
    let params = LlmParams::parse(&raw).expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// Send one body into the cell. `None` when the cell stayed silent (the
/// system-only path).
async fn send(cell: &mut LlmCell, db: &mut DbConn, body: Value) -> Option<Value> {
    let (sink, mut rx) = mk_sink();
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    rx.recv().await.map(|e| e.content)
}

/// One ordinary inference turn, so the composed system prompt reaches the mock.
fn a_turn() -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": "Hi"}]})
}

/// The system string the provider was sent on call `nth`.
///
/// A tree that renders to nothing produces no system message at all (the
/// house rule `build_openai_request` follows), so the empty string is the
/// honest answer for "the prompt carries nothing".
async fn system_prompt(mock: &MockOpenAI, nth: usize) -> String {
    let snaps = mock.recorded_requests().await;
    let msgs = snaps
        .get(nth)
        .unwrap_or_else(|| panic!("expected at least {} provider call(s)", nth + 1))
        .messages()
        .expect("request must carry messages[]");
    if msgs[0]["role"] != "system" {
        return String::new();
    }
    msgs[0]["content"]
        .as_str()
        .expect("system content is a string")
        .to_string()
}

async fn slot_count(db: &mut DbConn) -> i64 {
    db.call(|conn| conn.query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0)))
        .await
        .unwrap()
}

/// Write a row straight into `cell.db`, the way `seed/system.jsonl` does —
/// past the message gate, because a seed is configuration.
async fn seed_slot(db: &mut DbConn, slot_path: &str, text: &str) {
    let (p, t) = (slot_path.to_string(), text.to_string());
    db.call(move |conn| {
        conn.execute(
            "INSERT INTO system (slot_path, value, updated_at) VALUES (?, ?, 1)",
            rusqlite::params![p, format!(r#"{{"text":"{t}"}}"#)],
        )
    })
    .await
    .unwrap();
}

// ───────────────────────── 1. a marked write revokes ────────────────────────

/// The pin from the issue. Two turns with keys the writer does not control:
/// turn one writes `a` and `b`, turn two writes only `a`. Afterwards `b` must be
/// gone from the composed system prompt.
///
/// Before the marker existed this failed on the second half: `b` was never
/// named again, so its row stood untouched and its text kept riding into every
/// later prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_the_next_turn_does_not_name_is_gone_from_the_prompt() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    // Turn one: the bundle names two keys of its own choosing.
    send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"recall": {
            "$replace": true,
            "a": {"text": "ALPHA-FACT"},
            "b": {"text": "BETA-FACT"},
        }}}}),
    )
    .await;

    // Turn two: the same root, one key only — plus the turn that renders it.
    let mut body = a_turn();
    body["system"] = json!({"memory": {"recall": {
        "$replace": true,
        "a": {"text": "ALPHA-FACT-2"},
    }}});
    send(&mut cell, &mut db, body).await;

    let prompt = system_prompt(&mock, 0).await;
    assert!(
        prompt.contains("ALPHA-FACT-2"),
        "the key this turn named must be in the prompt: {prompt:?}"
    );
    assert!(
        !prompt.contains("BETA-FACT"),
        "a key the marked write did not name must be revoked, not left standing: {prompt:?}"
    );
}

/// The marker with nothing under it is the pure revocation: one message, no
/// leaves, the subtree is gone. This is the shape a writer needs when its own
/// leg came back empty — it cannot name paths it no longer knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bare_marker_revokes_the_subtree_without_a_second_message() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"recall": {
            "$replace": true,
            "a": {"text": "ALPHA-FACT"},
        }}}}),
    )
    .await;
    assert_eq!(slot_count(&mut db).await, 1);

    let mut body = a_turn();
    body["system"] = json!({"memory": {"recall": {"$replace": true}}});
    send(&mut cell, &mut db, body).await;

    assert_eq!(
        slot_count(&mut db).await,
        0,
        "a bare marker must leave the subtree empty"
    );
    let prompt = system_prompt(&mock, 0).await;
    assert!(
        !prompt.contains("ALPHA-FACT"),
        "the revoked subtree must not reach the prompt: {prompt:?}"
    );
}

/// The replace root is the node the marker sits in, and it stops at a segment
/// boundary: a replace at `memory.recall` never reaches `memory.recallx`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replace_stops_at_the_segment_boundary_of_its_root() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));
    seed_slot(&mut db, "memory.recallx.k", "NEIGHBOUR-FACT").await;

    send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"recall": {"$replace": true, "a": {"text": "ALPHA"}}}}}),
    )
    .await;
    let mut body = a_turn();
    body["system"] = json!({"memory": {"recall": {"$replace": true}}});
    send(&mut cell, &mut db, body).await;

    let prompt = system_prompt(&mock, 0).await;
    assert!(
        prompt.contains("NEIGHBOUR-FACT"),
        "a sibling slot that merely shares a prefix must survive: {prompt:?}"
    );
    assert!(
        !prompt.contains("ALPHA"),
        "the root itself is gone: {prompt:?}"
    );
}

// ───────────────────────── 2. an unmarked write does not ────────────────────

/// The load-bearing counter-pin. `system.*` is filled by several independent
/// writers under one shared root; a plain write must leave every path it did
/// not name exactly where it was. Green before this feature existed and green
/// after — that is the point: the marker is opt-in, always.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_write_leaves_a_foreign_path_under_the_same_root_alone() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    // Writer A owns `memory.identity`, writer B owns `memory.recall.*`.
    send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"identity": {"text": "IDENT-FACT"}}}}),
    )
    .await;
    send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"recall": {"a": {"text": "ALPHA-FACT"}}}}}),
    )
    .await;

    // Writer B writes again, unmarked, and renders.
    let mut body = a_turn();
    body["system"] = json!({"memory": {"recall": {"a": {"text": "ALPHA-FACT-2"}}}});
    send(&mut cell, &mut db, body).await;

    let prompt = system_prompt(&mock, 0).await;
    assert!(
        prompt.contains("IDENT-FACT"),
        "an unmarked write must not touch another writer's path: {prompt:?}"
    );
    assert!(
        prompt.contains("ALPHA-FACT-2"),
        "own path updated: {prompt:?}"
    );
}

/// A marker at `memory.recall` is scoped to `memory.recall`: the sibling
/// subtree `memory.identity` under the SHARED root is not its business.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replace_reaches_only_below_its_own_node() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"identity": {"text": "IDENT-FACT"}}}}),
    )
    .await;
    let mut body = a_turn();
    body["system"] = json!({"memory": {"recall": {"$replace": true, "a": {"text": "ALPHA"}}}});
    send(&mut cell, &mut db, body).await;

    let prompt = system_prompt(&mock, 0).await;
    assert!(
        prompt.contains("IDENT-FACT"),
        "the sibling under the shared root must survive a scoped replace: {prompt:?}"
    );
}

// ───────────────────────── 3. the gate covers the root ──────────────────────

/// `system_writable` fences the leaves a message may write. It has to fence the
/// replace ROOT too, or a writer pinned to `memory.recall` could set the marker
/// one level up and take `memory.identity` with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replace_root_outside_the_allowlist_is_refused() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(
        &td,
        &mock.base_url,
        json!({"system_writable": ["memory.recall"]}),
    );
    seed_slot(&mut db, "memory.identity", "IDENT-FACT").await;

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"$replace": true, "recall": {"text": "ALPHA"}}}}),
    )
    .await
    .expect("a refused write answers");

    assert_eq!(
        out["header"]["error_code"], "invalid_input",
        "a refused replace root is a loud reject: {out}"
    );
    assert_eq!(
        slot_count(&mut db).await,
        1,
        "nothing was written and nothing was deleted"
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a refused system write never calls the provider"
    );
}

/// The `$` namespace is reserved. A misspelled marker must be a loud reject —
/// silently ignoring it would give the writer a revocation that never happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_misspelled_marker_is_a_loud_reject_not_a_silent_no_op() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"$replace_all": true, "a": {"text": "ALPHA"}}}}),
    )
    .await
    .expect("a refused write answers");

    assert_eq!(out["header"]["error_code"], "invalid_input", "{out}");
    assert_eq!(slot_count(&mut db).await, 0, "all-or-nothing");
}

/// A marker whose value is not a boolean is a shape error, not a truthy read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marker_that_is_not_a_boolean_is_refused() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"$replace": "yes", "a": {"text": "ALPHA"}}}}),
    )
    .await
    .expect("a refused write answers");

    assert_eq!(out["header"]["error_code"], "invalid_input", "{out}");
    assert_eq!(slot_count(&mut db).await, 0, "all-or-nothing");
}
