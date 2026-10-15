//! GH #1076 — the mutation door refuses a body without a `diff`.
//!
//! WHAT WAS WRONG
//! ==============
//! `handle_mutation` reads its work from `payload["diff"]` and read an ABSENT
//! `diff` as the empty one. Measured while seeding a store: the caller put its
//! operation at the top level — `{"scope": …, "seed_rows": [ … ]}` instead of
//! `{"scope": …, "diff": {"seed_rows": [ … ]}}` — and the door answered
//! `committed` eleven times in a row while the store held 0 of the 2 500 rows
//! sent. The vocabulary check (`refuse_unknown_diff_keys`) looks INSIDE the
//! diff and never sees a top-level key; the GH #581 check discriminates on
//! `manifest` alone. Same class as GH #581: "committed" for "did nothing".
//!
//! WHAT IS PINNED
//! ==============
//! - a body without `diff` is `schema`, spurless (no id, no mutation-log row);
//! - a diff operation at the top level is `schema` even BESIDE a `diff` — it
//!   belongs inside `diff`, and the door would have ignored it;
//! - the controls: the same operation under `diff` commits, an explicit empty
//!   `diff: {}` stays the no-op GH #422 pins, and an unknown top-level key that
//!   is no operation (`comment`) stays ignored;
//! - a manifest inherits the check entry by entry;
//! - a body that is no object at all (an array, `null`, a string, a number) is
//!   `schema`, spurless — it reads as no `diff` just the same. The likely one
//!   is the manifest entries sent without their `{"manifest": …}` wrapper.

use meclaw_colony::mutation::{ManifestOutcome, MutationDoorOutcome, MutationOutcome};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    ColonyTaskConfig, colony_task,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::{mpsc, oneshot};

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    })
}

struct Colony {
    inbox_tx: mpsc::Sender<ColonyMsg>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    colony_config: ColonyConfig,
    join: tokio::task::JoinHandle<()>,
}

impl Colony {
    fn runtime(&self) -> ColonyRuntime {
        ColonyRuntime {
            inbox_tx: self.inbox_tx.clone(),
            outputs_tx: self.outputs_tx.clone(),
            colony_config: self.colony_config.clone(),
            blob_store: None,
        }
    }

    async fn shutdown(self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.inbox_tx
            .send(ColonyMsg::Shutdown { ack: ack_tx })
            .await
            .expect("send shutdown");
        ack_rx.await.expect("shutdown ack");
        self.join.await.expect("colony task");
    }
}

/// A shipped template, written by hand into the library.
fn write_template(templates: &std::path::Path, name: &str, version: &str) {
    write(
        templates,
        &format!("{name}/template.json"),
        &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    );
    write(templates, &format!("{name}/config.json"), CELL_CONFIG);
}

async fn boot_colony(root: &std::path::Path, templates_root: &std::path::Path) -> Colony {
    write(root, "main/config.json", HIVE_CONFIG);
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let mut factories = CellFactoryRegistry::new();
    factories.insert("persist_mock".into(), persist_factory());
    let colony_config = ColonyConfig::default();
    let cfg = ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        factories,
        root.to_path_buf(),
        colony_config.clone(),
        None,
        None,
    )
    .with_templates_root(templates_root.to_path_buf());
    let join = tokio::spawn(colony_task(cfg));
    let colony = Colony {
        inbox_tx,
        outputs_tx,
        colony_config,
        join,
    };
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), persist_factory());
    meclaw_colony::bootstrap_from_filesystem(root, &reg, &colony.runtime())
        .await
        .expect("bootstrap");
    colony
}

async fn send_door(c: &Colony, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    c.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

/// The refusal token of a door verdict, whichever form knocked.
fn error_code(outcome: &MutationDoorOutcome) -> Option<&str> {
    match outcome {
        MutationDoorOutcome::Single(MutationOutcome::Rejected { error_code, .. })
        | MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { error_code, .. }) => {
            Some(error_code.as_str())
        }
        _ => None,
    }
}

/// The human-readable half of a refusal — what the operator actually reads.
fn details(outcome: &MutationDoorOutcome) -> Option<&str> {
    match outcome {
        MutationDoorOutcome::Single(MutationOutcome::Rejected { details, .. })
        | MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { details, .. }) => {
            Some(details.as_str())
        }
        _ => None,
    }
}

async fn rescan(c: &Colony, templates: &std::path::Path) -> Result<(), String> {
    let (ack_tx, ack_rx) = oneshot::channel();
    c.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: templates.to_path_buf(),
            ack: ack_tx,
        })
        .await
        .expect("send rescan");
    ack_rx.await.expect("rescan ack")
}

/// A tree whose library answers to `note-unit@1.0.0`.
async fn colony_with_note_unit(td: &tempfile::TempDir) -> (Colony, std::path::PathBuf) {
    let templates = td.path().join("templates");
    write_template(&templates, "note-unit", "1.0.0");
    let colony = boot_colony(td.path(), &templates).await;
    rescan(&colony, &templates).await.expect("rescan");
    (colony, templates)
}

/// One perfectly ordinary declaration: grow `name` from the shipped template.
fn grow(name: &str) -> Value {
    json!({"scope": "/", "ctx": {}, "diff": {
        "add_nodes": [{"name": name, "template": "note-unit@1.0.0"}]
    }})
}

/// Rows in `mutation_log` — the audit a refusal before the id must not touch.
fn mutation_log_rows(root: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row("SELECT COUNT(*) FROM mutation_log", [], |r| r.get(0))
        .expect("count mutation_log")
}

/// The measured shape: the operation at the top level, no `diff` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operation_at_the_top_level_without_a_diff_is_refused() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;
    let before = mutation_log_rows(td.path());

    let outcome = send_door(
        &colony,
        json!({"scope": "/", "seed_rows": [{"target": "store", "table": "t", "rows": [{"a": 1}]}]}),
    )
    .await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");
    let msg = details(&outcome).expect("a refusal carries details");
    assert!(
        msg.contains("`diff`"),
        "the refusal must name `diff`: {msg}"
    );
    assert!(
        msg.contains("seed_rows"),
        "the refusal must name the misplaced key: {msg}"
    );
    assert!(
        matches!(
            outcome,
            MutationDoorOutcome::Single(MutationOutcome::Rejected { id: None, .. })
        ),
        "refused before the id is minted: {outcome:?}",
    );
    assert_eq!(
        mutation_log_rows(td.path()),
        before,
        "the refusal left a log row"
    );

    colony.shutdown().await;
}

/// A body that is no object is refused too, spurless: the door reads it as no
/// `diff` and would answer `committed` with `changes: []`. The array is the
/// likely mistake — the manifest entries without their `{"manifest": …}` wrapper.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_is_no_object_is_refused() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;
    let before = mutation_log_rows(td.path());

    for body in [
        json!([grow("from-an-array")]),
        Value::Null,
        json!("{\"scope\": \"/\", \"diff\": {}}"),
        json!(42),
    ] {
        let outcome = send_door(&colony, body.clone()).await;
        assert_eq!(error_code(&outcome), Some("schema"), "{body}: {outcome:?}");
        let msg = details(&outcome).expect("a refusal carries details");
        assert!(
            msg.contains("object"),
            "the refusal must say what a body is: {msg}"
        );
        assert!(
            matches!(
                outcome,
                MutationDoorOutcome::Single(MutationOutcome::Rejected { id: None, .. })
            ),
            "{body}: refused before the id is minted: {outcome:?}",
        );
    }
    assert_eq!(
        mutation_log_rows(td.path()),
        before,
        "a refusal left a log row"
    );

    colony.shutdown().await;
}

/// A body with no `diff` and no operation anywhere is refused too: there is
/// nothing to apply, and `committed` would claim otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_without_a_diff_is_refused() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(&colony, json!({"scope": "/", "ctx": {}})).await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");

    colony.shutdown().await;
}

/// An operation BESIDE a `diff` is not executed by the door either; applying the
/// diff and dropping the top-level operation without a word is the same lie.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operation_beside_the_diff_is_refused_and_nothing_applies() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(
        &colony,
        json!({"scope": "/", "ctx": {},
               "diff": {"add_nodes": [{"name": "inside", "template": "note-unit@1.0.0"}]},
               "add_nodes": [{"name": "outside", "template": "note-unit@1.0.0"}]}),
    )
    .await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");
    let msg = details(&outcome).expect("a refusal carries details");
    assert!(
        msg.contains("add_nodes"),
        "the refusal must name the misplaced key: {msg}"
    );
    assert!(
        !td.path().join("main/inside").exists(),
        "the diff half applied"
    );
    assert!(
        !td.path().join("main/outside").exists(),
        "the top-level half applied"
    );

    colony.shutdown().await;
}

/// Control: the same operation under `diff` commits and grows the cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_operation_under_diff_commits() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(&colony, grow("notes")).await;

    assert!(outcome.is_committed(), "{outcome:?}");
    assert!(
        td.path().join("main/notes/config.json").is_file(),
        "the cell was not grown"
    );

    colony.shutdown().await;
}

/// Control: an explicit empty `diff` stays the no-op GH #422 pins, and a
/// top-level key that is no operation stays ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_empty_diff_and_a_comment_still_commit() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {}, "comment": "x"}),
    )
    .await;

    assert!(outcome.is_committed(), "{outcome:?}");

    colony.shutdown().await;
}

/// A manifest inherits the check: entry 1 commits, entry 2 carries no `diff`,
/// entry 3 is never read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_stops_at_the_entry_without_a_diff() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let bad = json!({"scope": "/", "ctx": {},
                     "add_nodes": [{"name": "second", "template": "note-unit@1.0.0"}]});
    let outcome = send_door(
        &colony,
        json!({"manifest": [grow("first"), bad, grow("third")]}),
    )
    .await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");
    let MutationDoorOutcome::Manifest(ManifestOutcome::Rejected {
        failed_at,
        remaining,
        ..
    }) = &outcome
    else {
        panic!("expected a manifest refusal: {outcome:?}");
    };
    assert_eq!(*failed_at, 2, "{outcome:?}");
    assert_eq!(*remaining, 1, "{outcome:?}");
    assert!(
        td.path().join("main/first/config.json").is_file(),
        "entry 1 rolled back"
    );
    assert!(
        !td.path().join("main/second").exists(),
        "the refused entry applied"
    );
    assert!(
        !td.path().join("main/third").exists(),
        "the manifest kept going"
    );

    colony.shutdown().await;
}
