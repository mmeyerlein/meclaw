//! GH #422 — the colony rolls a manifest off itself, and stops at the first
//! refusal.
//!
//! WHAT A MANIFEST IS
//! ==================
//! One body carrying an ORDERED LIST of ordinary mutation bodies. Ruling R5:
//! the colony walks the list in order, hands every entry to the very
//! `handle_mutation` a single body takes, stops at the FIRST refusal, and
//! answers with ONE receipt — "k applied, entry k+1 refused with `error_code`,
//! the rest untouched".
//!
//! THE THREE THINGS THIS FILE MEASURES
//! ===================================
//! * **No rollback.** What committed before the refusal stays committed. That
//!   is not a shortcoming of the form, it is the form: a rollback would need a
//!   second, inverse mutation language, and the receipt already says exactly
//!   where to resume.
//! * **The audit knows where it stopped.** One `mutation_log` row per applied
//!   entry, plus the refused one's own `rejected` row. "Resumable" is a
//!   property of that record, not a promise in prose.
//! * **The answer lands in its own slot.** `manifest`, beside `mutation` /
//!   `graph` / `ledger` — a different question gets a different slot.
//!
//! HOW THE DOOR IS REACHED
//! =======================
//! Through the EDA dispatch path, like its sibling file
//! `gh422_the_single_mutation_body_does_not_move.rs`: a probe cell emits the
//! body at `/colony/mutations` and captures the reply. Only that path builds
//! the reply slot this file reads.

use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::{ColonyHandle, EmitOnceMockCellFactory};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

/// Generous: the colony boots a tree and wakes a dormant probe (30s convention).
const REPLY_WAIT: Duration = Duration::from_secs(30);

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// One knock at the door with `payload`, and everything it left behind.
struct Knock {
    root: tempfile::TempDir,
    _probe_dir: tempfile::TempDir,
    handle: ColonyHandle,
    reply: Value,
}

/// Boot a colony holding one `persist_mock` template, let a probe emit
/// `payload` at `/colony/mutations`, and return the reply.
///
/// The payload gains an empty `messages[]`: a CELL emission is UBF-validated in
/// the outputs arm, and a body without one of the UBF alternatives never
/// reaches a door. That is a property of the emit path — a `curl` at the HTTP
/// door needs no such key — and `messages` is not a key the manifest detection
/// looks at.
async fn knock(payload: Value) -> Knock {
    let mut payload = payload;
    if let Some(obj) = payload.as_object_mut() {
        obj.entry("messages").or_insert_with(|| json!([]));
    }
    let root = tempfile::TempDir::new().unwrap();
    write(root.path(), "main/config.json", HIVE_CONFIG);
    write(
        root.path(),
        "templates/persist_mock/template.json",
        r#"{"name":"persist_mock"}"#,
    );
    write(
        root.path(),
        "templates/persist_mock/config.json",
        CELL_CONFIG,
    );
    let probe_dir = tempfile::TempDir::new().unwrap();

    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let handle = ColonyHandle::new_with_factories_at(
        &root,
        vec![("persist_mock".to_string(), factory.clone())],
    );

    let (capture_tx, mut capture_rx) = mpsc::channel(8);
    let probe = Arc::new(EmitOnceMockCellFactory::new(
        Path::new("/colony/mutations"),
        payload,
        capture_tx,
    ));
    let spawned = probe
        .spawn_cell(
            Path::new("/probe"),
            json!({}),
            handle.outputs_sender(),
            probe_dir.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            handle.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            64,
        )
        .expect("probe spawn");
    handle.register_spawned(Path::new("/probe"), spawned).await;

    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), factory);
    bootstrap_from_filesystem(root.path(), &reg, &handle.runtime())
        .await
        .expect("bootstrap");

    let (ack_tx, ack_rx) = oneshot::channel();
    handle
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();

    handle
        .send(
            MessageBuilder::new(Path::new("/probe"))
                .body(Body::Inline(json!({"messages": []})))
                .build(),
        )
        .await;

    let mut seen: Vec<Value> = Vec::new();
    let reply = loop {
        let got = match tokio::time::timeout(REPLY_WAIT, capture_rx.recv()).await {
            Ok(Some(m)) => m,
            Ok(None) => panic!("/probe capture channel closed before the door answered"),
            Err(_) => {
                let dlq = handle.drain_dead_letters().await;
                panic!(
                    "the door did not answer within {REPLY_WAIT:?}; seen: {seen:?}; DLQ: {dlq:?}"
                )
            }
        };
        let body = match got.body {
            Body::Inline(v) => v,
            Body::Blob(id) => panic!("the door replied with a blob body ({id})"),
        };
        if body.get("manifest").is_some() || body.get("mutation").is_some() {
            break body;
        }
        seen.push(body);
    };
    Knock {
        root,
        _probe_dir: probe_dir,
        handle,
        reply,
    }
}

/// One `add_nodes` entry putting `name` at the root, from the shipped fixture
/// template.
fn entry(name: &str) -> Value {
    json!({"scope": "/", "diff": {"add_nodes": [{"name": name, "template": "persist_mock"}]}})
}

/// The registry paths the run persisted, minus the fixture's own probe.
fn grown_paths(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT path FROM registry ORDER BY path")
        .unwrap();
    let out: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .filter(|p| p != "/probe")
        .collect();
    drop(stmt);
    out
}

/// Every `mutation_log` row, as `(id, status)`, oldest first.
fn mutation_log(root: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT id, status FROM mutation_log ORDER BY created_at, id")
        .unwrap();
    let out: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    drop(stmt);
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// the roll-off
// ──────────────────────────────────────────────────────────────────────────────

/// The message door answers a manifest in its OWN slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_message_door_answers_a_manifest_in_its_own_slot() {
    let k = knock(json!({"manifest": [entry("a")]})).await;
    assert!(
        k.reply.get("manifest").is_some(),
        "a manifest answers in `manifest`: {}",
        k.reply
    );
    assert!(
        k.reply.get("mutation").is_none(),
        "and NOT in `mutation` — a different question, a different slot: {}",
        k.reply
    );
    k.handle.shutdown().await;
}

/// Every entry commits: one id per entry, in order, all distinct.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_that_fully_commits_reports_one_id_per_entry() {
    let k = knock(json!({"manifest": [entry("a"), entry("b"), entry("c")]})).await;
    let m = &k.reply["manifest"];
    assert_eq!(m["outcome"], "committed", "{}", k.reply);
    assert_eq!(m["applied"], 3);
    let ids: Vec<&str> = m["ids"]
        .as_array()
        .expect("ids")
        .iter()
        .map(|v| v.as_str().expect("id string"))
        .collect();
    assert_eq!(ids.len(), 3, "one id per entry");
    let distinct: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        3,
        "each entry got its own mutation id: {ids:?}"
    );

    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert_eq!(grown_paths(&root), vec!["/a", "/b", "/c"]);
}

/// The first refusal stops the run, and nothing behind it is looked at.
///
/// This is the NO-ROLLBACK statement in one measurement: entry 1 committed and
/// is still there after entry 2 was refused, and entry 3 was never attempted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_stops_at_the_first_refusal_and_leaves_the_rest_alone() {
    let k = knock(json!({"manifest": [
        entry("a"),
        {"scope": "/", "diff": {"add_nodes": [{"name": "b", "template": "no-such-template@1.0.0"}]}},
        entry("c"),
    ]}))
    .await;
    let m = &k.reply["manifest"];
    assert_eq!(m["outcome"], "rejected", "{}", k.reply);
    assert_eq!(m["applied"], 1, "one entry committed before the refusal");
    assert_eq!(m["failed_at"], 2, "1-based: an operator counts entries");
    assert_eq!(m["remaining"], 1, "entry 3 was never looked at");
    assert_eq!(m["error_code"], "template_missing");
    assert_eq!(m["ids"].as_array().expect("ids").len(), 1);

    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert_eq!(
        grown_paths(&root),
        vec!["/a"],
        "no rollback: what committed stays committed, and nothing behind the \
         refusal was attempted"
    );
}

/// The audit knows where the run stopped: one row per applied entry, plus the
/// refused one's own `rejected` row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_writes_one_mutation_log_row_per_applied_entry() {
    let k = knock(json!({"manifest": [
        entry("a"),
        entry("b"),
        {"scope": "/", "diff": {"add_nodes": [{"name": "c", "template": "no-such-template@1.0.0"}]}},
    ]}))
    .await;
    let m = &k.reply["manifest"];
    assert_eq!(m["applied"], 2, "{}", k.reply);
    assert_eq!(m["failed_at"], 3);

    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    let rows = mutation_log(&root);
    let committed: Vec<&(String, String)> = rows.iter().filter(|(_, s)| s == "committed").collect();
    let rejected: Vec<&(String, String)> = rows.iter().filter(|(_, s)| s == "rejected").collect();
    assert_eq!(
        committed.len(),
        2,
        "k rows for k applied entries; got {rows:?}"
    );
    assert_eq!(
        rejected.len(),
        1,
        "and one for the entry that refused; got {rows:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// a body that meant to be a manifest and could not be read
// ──────────────────────────────────────────────────────────────────────────────

/// A broken manifest form is `schema`, applies nothing, and names no position.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_manifest_is_schema_and_applies_nothing() {
    let k = knock(json!({"manifest": []})).await;
    let m = &k.reply["manifest"];
    assert_eq!(m["outcome"], "rejected", "{}", k.reply);
    assert_eq!(m["applied"], 0);
    assert_eq!(m["error_code"], "schema", "no new error_code is minted");
    assert!(
        m.get("failed_at").is_none(),
        "there was no position — the manifest was unreadable as a whole: {}",
        k.reply
    );
    assert!(
        m["details"]
            .as_str()
            .expect("details")
            .contains("empty manifest"),
        "the refusal says which shape was wrong: {}",
        k.reply
    );
    k.handle.shutdown().await;
}

/// A body that is both forms at once is refused, not guessed at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_is_both_forms_is_refused() {
    let k = knock(json!({"manifest": [entry("a")], "diff": {}})).await;
    let m = &k.reply["manifest"];
    assert_eq!(m["outcome"], "rejected", "{}", k.reply);
    assert_eq!(m["error_code"], "schema");
    let root = k.root.path().to_path_buf();
    k.handle.shutdown().await;
    assert!(
        grown_paths(&root).is_empty(),
        "and nothing at all was applied"
    );
}
