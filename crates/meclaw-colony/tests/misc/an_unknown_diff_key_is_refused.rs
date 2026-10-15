//! The mutation door refuses a `diff` key no operation reads.
//!
//! WHAT WAS WRONG
//! ==============
//! The door read the `diff` object key by key — `diff.get("add_nodes")`,
//! `diff.get("add_edges")`, … — and never asked what ELSE was in it. Anything
//! it did not recognise fell through every arm untouched and the declaration
//! answered `committed`. The shape that made it visible: a colony running an
//! OLDER binary is handed an `add_templates` declaration. The old binary has no
//! arm for that key, so it registers nothing, writes nothing, and replies
//! `committed` — the operator reads "applied", the library is empty, and the
//! next declaration that resolves the template fails somewhere else entirely.
//!
//! The same hole swallows a typo (`add_node`, `add_edge`), a key from a newer
//! schema, and a hand-written declaration whose author guessed the vocabulary.
//! In every case the receipt says the work was done.
//!
//! Reporting success without effect is the defect. A `diff` key the door cannot
//! execute is now refused with `error_code: "schema"`, naming the key it could
//! not read AND the keys it can, pre-destructively: nothing is staged, spawned,
//! wired or registered.
//!
//! WHY THE MANIFEST CASE IS HERE TOO
//! =================================
//! A manifest is rolled off entry by entry through the very `handle_mutation` a
//! single body takes, so it inherits the check by construction — but "by
//! construction" is a claim, and the interesting half is the STOP: the entries
//! before the bad one stay committed, the ones after it are never looked at.
//! `--apply` inherits the same way (it is the single form, read from a file).

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

// ── the single form ─────────────────────────────────────────────────────────

/// The reported shape: an old binary is handed a key it has no arm for. Here
/// the key is invented rather than `add_templates`, so the test keeps measuring
/// the class after every real key has been implemented.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_diff_key_is_refused_instead_of_silently_ignored() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_wormholes": [{"name": "w", "to": "elsewhere"}]
        }}),
    )
    .await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");
    let msg = details(&outcome).expect("a refusal carries details");
    assert!(
        msg.contains("add_wormholes"),
        "the refusal must name the key it could not read: {msg}",
    );
    for legal in [
        "add_nodes",
        "remove_nodes",
        "add_edges",
        "remove_edges",
        "swap_nodes",
        "move_nodes",
        "add_templates",
    ] {
        assert!(
            msg.contains(legal),
            "the refusal must name the legal key `{legal}`: {msg}",
        );
    }

    colony.shutdown().await;
}

/// The half that matters more than the token: the legal operations that
/// travelled in the SAME diff do not happen either. A refusal that applied the
/// keys it understood would be the old bug with a louder receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_of_the_diff_is_applied() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_nodes": [{"name": "notes", "template": "note-unit@1.0.0"}],
            "add_wormholes": [{"name": "w"}]
        }}),
    )
    .await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");
    assert!(
        !td.path().join("main/notes").exists(),
        "the door executed the keys it understood and refused the rest",
    );

    colony.shutdown().await;
}

/// The counter-test. A guard that refused everything would pass both tests
/// above and break every colony in the world.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_diff_of_legal_keys_still_commits() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(&colony, grow("notes")).await;
    assert!(outcome.is_committed(), "{outcome:?}");
    assert!(
        td.path().join("main/notes/config.json").is_file(),
        "the cell was not grown",
    );

    colony.shutdown().await;
}

/// An empty `diff` carries no unknown key and must stay the no-op it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_diff_is_not_an_unknown_key() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let outcome = send_door(&colony, json!({"scope": "/", "ctx": {}, "diff": {}})).await;
    assert!(outcome.is_committed(), "{outcome:?}");

    colony.shutdown().await;
}

// ── the manifest form ───────────────────────────────────────────────────────

/// Entry 1 commits, entry 2 carries the unknown key, entry 3 is never read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_stops_at_the_entry_that_carries_the_unknown_key() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (colony, _templates) = colony_with_note_unit(&td).await;

    let bad = json!({"scope": "/", "ctx": {}, "diff": {
        "add_nodes": [{"name": "second", "template": "note-unit@1.0.0"}],
        "add_wormholes": [{"name": "w"}]
    }});
    let outcome = send_door(
        &colony,
        json!({"manifest": [grow("first"), bad, grow("third")]}),
    )
    .await;

    assert_eq!(error_code(&outcome), Some("schema"), "{outcome:?}");
    let MutationDoorOutcome::Manifest(ManifestOutcome::Rejected {
        failed_at,
        remaining,
        ids,
        ..
    }) = &outcome
    else {
        panic!("expected a manifest refusal: {outcome:?}");
    };
    assert_eq!(*failed_at, 2, "{outcome:?}");
    assert_eq!(*remaining, 1, "{outcome:?}");
    assert_eq!(ids.len(), 1, "entry 1 must stay committed: {outcome:?}");

    assert!(
        td.path().join("main/first/config.json").is_file(),
        "the entry before the refusal was rolled back",
    );
    assert!(
        !td.path().join("main/second").exists(),
        "the refused entry applied its legal half",
    );
    assert!(
        !td.path().join("main/third").exists(),
        "the manifest kept going past the refusal",
    );

    colony.shutdown().await;
}
