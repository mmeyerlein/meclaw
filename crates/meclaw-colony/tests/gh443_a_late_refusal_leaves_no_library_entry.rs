//! GH #443: a later refusal in the SAME diff must leave no library entry.
//!
//! `add_templates` runs first in `handle_mutation`, because a later entry of
//! the same diff has to be able to RESOLVE what it declared. Until this issue
//! the declaration also became VISIBLE first — one `rename(2)` into
//! `{templates_root}/local/<name>/` at the very top of the mutation — so every
//! refusal below it left the directory behind while the receipt said
//! `rejected` and `colony.db` held no row. The operator was then told to clear
//! residue by hand, and the retry of the very same manifest hit
//! `template_name_taken` on a name nothing had registered.
//!
//! The fix makes the bad state unrepresentable rather than cleaning it up: the
//! declaration lives in its own staging area for the whole mutation and moves
//! into the library immediately before the commit flush. The tests below
//! assert the FILESYSTEM and `colony.db` on every abort stage — a receipt-only
//! assertion would pass on residue.

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

/// A booted colony plus the channels a test talks to it through. Copied from
/// the gh440 suite rather than shared: a test helper that two suites own is a
/// third thing to keep in step with the door.
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

/// Boot a colony whose template library is `templates_root` — the same value
/// the CLI's `--templates` resolves to, which is what `add_templates` writes
/// under.
async fn boot_colony_with_templates_root(
    root: &std::path::Path,
    templates_root: &std::path::Path,
) -> Colony {
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

async fn send_mutation(c: &Colony, payload: Value) -> MutationDoorOutcome {
    send_door(c, payload).await
}

async fn send_manifest(c: &Colony, payload: Value) -> MutationDoorOutcome {
    send_door(c, payload).await
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

/// The template staging area a registration builds in. After a commit AND
/// after a refusal it must hold nothing — on either path the mutation owns
/// those bytes and nobody else ever will.
fn staging_is_empty(root: &std::path::Path) -> bool {
    let staging = root.join(".staging-templates");
    match std::fs::read_dir(&staging) {
        Err(_) => true,
        Ok(mut d) => d.next().is_none(),
    }
}

/// A row of `colony.db`'s `templates` table.
#[derive(Debug)]
struct TemplateRowView {
    name: String,
    version: Option<String>,
    filesystem_path: String,
}

fn read_templates_table(root: &std::path::Path) -> Vec<TemplateRowView> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT name, version, filesystem_path FROM templates ORDER BY name")
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| {
            Ok(TemplateRowView {
                name: r.get(0)?,
                version: r.get(1)?,
                filesystem_path: r.get(2)?,
            })
        })
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

/// One `add_templates[]` entry in the declaration form.
fn declaration(name: &str) -> Value {
    json!({"name": name,
    "files": {
        "template.json": format!(r#"{{"name": "{name}", "version": "1.0.0"}}"#),
        "config.json": CELL_CONFIG,
    }})
}

/// An `add_edges` entry naming two addresses that exist nowhere — refused in
/// the validation block, which runs long AFTER the `add_templates` step.
fn edge_to_nowhere() -> Value {
    json!({"from": "./nowhere", "to": "./also-nowhere"})
}

/// The whole point of the issue in one payload: ONE mutation that declares a
/// class and then asks for something the validator refuses.
fn declare_then_refuse(name: &str) -> Value {
    json!({"scope": "/", "ctx": {}, "diff": {
        "add_templates": [declaration(name)],
        "add_edges": [edge_to_nowhere()],
    }})
}

/// The same declaration without the refused half — the retry an operator
/// makes after fixing the manifest.
fn declare_only(name: &str) -> Value {
    json!({"scope": "/", "ctx": {}, "diff": {
        "add_templates": [declaration(name)],
    }})
}

/// Nothing of a refused declaration survives: no directory in the library, no
/// staged bytes, no registry row.
fn assert_no_trace(root: &std::path::Path, templates: &std::path::Path, name: &str) {
    assert!(
        !templates.join("local").join(name).exists(),
        "a refused declaration left '{name}' in the library",
    );
    assert!(
        staging_is_empty(root),
        "a refused declaration left staged bytes behind",
    );
    let rows = read_templates_table(root);
    assert!(
        !rows.iter().any(|r| r.name == name),
        "a refused declaration left a registry row: {rows:?}",
    );
}

/// Abort stage 1: the validator refuses a LATER key of the same diff. The
/// declaration was already resolvable at that moment — and must still be
/// invisible afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_validation_refusal_after_the_declaration_leaves_no_library_entry() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let outcome = send_mutation(&colony, declare_then_refuse("note-unit")).await;
    assert!(
        error_code(&outcome).is_some(),
        "the diff must be refused: {outcome:?}",
    );
    assert_no_trace(td.path(), &templates, "note-unit");

    colony.shutdown().await;
}

/// Abort stage 2: the refusal happens after the declared class has already
/// been INSTANTIATED inside the same diff. The instance is staged from the
/// declaration's own staging area, so this is the case where the two halves
/// could disagree about where the template lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_after_the_declared_class_was_instantiated_leaves_no_library_entry() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_templates": [declaration("note-unit")],
            "add_nodes": [{"name": "notes", "template": "note-unit@1.0.0"}],
            "add_edges": [edge_to_nowhere()],
        }}),
    )
    .await;
    assert!(
        error_code(&outcome).is_some(),
        "the diff must be refused: {outcome:?}",
    );
    assert_no_trace(td.path(), &templates, "note-unit");
    assert!(
        !td.path().join("main/notes").exists(),
        "the refused diff grew a cell",
    );

    colony.shutdown().await;
}

/// Abort stage 3: the refusal lies INSIDE `add_templates`, at declaration
/// n > 1. Declaration 1 has already been staged when declaration 2 is refused,
/// and it must not become visible either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_at_the_second_declaration_leaves_the_first_invisible() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    // Residue nobody has a registry row for: `add_templates` refuses it by
    // name rather than overwriting it (No-Delete).
    std::fs::create_dir_all(templates.join("local/taken")).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_templates": [declaration("note-unit"), declaration("taken")],
        }}),
    )
    .await;
    assert_eq!(
        error_code(&outcome),
        Some("template_name_taken"),
        "{outcome:?}",
    );
    assert_no_trace(td.path(), &templates, "note-unit");
    assert!(
        std::fs::read_dir(templates.join("local/taken"))
            .expect("the pre-existing directory was removed")
            .next()
            .is_none(),
        "the refusal wrote into a directory it did not create",
    );

    colony.shutdown().await;
}

/// Two declarations of ONE name in ONE diff: the second is refused at its own
/// position, and the first stays invisible. Neither is registered, so the
/// registry snapshot cannot see the collision — the staged targets can.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_name_declared_twice_in_one_diff_is_refused_and_leaves_nothing() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_templates": [declaration("note-unit"), declaration("note-unit")],
        }}),
    )
    .await;
    assert_eq!(
        error_code(&outcome),
        Some("template_name_taken"),
        "{outcome:?}",
    );
    assert_no_trace(td.path(), &templates, "note-unit");

    colony.shutdown().await;
}

/// The retry: the same declaration, sent again after the refusal, commits
/// without a single act of hand-clearing. That is the operator-visible half of
/// the issue — the old residue turned every retry into `template_name_taken`
/// for a name nothing had registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_declaration_retries_clean_after_a_late_refusal() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    assert!(
        error_code(&send_mutation(&colony, declare_then_refuse("note-unit")).await).is_some(),
        "the first knock must be refused",
    );
    let outcome = send_mutation(&colony, declare_only("note-unit")).await;
    assert!(
        outcome.is_committed(),
        "the retry needed hand-clearing first: {outcome:?}",
    );

    let dir = templates.join("local/note-unit");
    assert!(dir.join("template.json").is_file(), "template.json missing");
    assert!(dir.join("config.json").is_file(), "config.json missing");
    assert!(staging_is_empty(td.path()), "the commit left staged bytes");
    let rows = read_templates_table(td.path());
    let row = rows
        .iter()
        .find(|r| r.name == "note-unit")
        .expect("no registry row after the retry");
    assert_eq!(row.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        std::path::Path::new(&row.filesystem_path),
        dir.as_path(),
        "the row points somewhere other than the library directory",
    );

    colony.shutdown().await;
}

/// A manifest rolls forward: an entry that COMMITTED stays committed when a
/// LATER entry is refused. The deferred rename must not weaken that — each
/// manifest entry is its own mutation, and the guard only ever owns the bytes
/// of the mutation it belongs to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_manifest_entry_survives_a_refusal_in_a_later_entry() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let manifest = json!({"manifest": [
        declare_only("note-unit"),
        declare_then_refuse("second-unit"),
    ]});
    let outcome = send_manifest(&colony, manifest).await;
    assert!(
        error_code(&outcome).is_some(),
        "the manifest must be refused at entry 2: {outcome:?}",
    );

    assert!(
        templates.join("local/note-unit/template.json").is_file(),
        "the refusal in entry 2 undid entry 1",
    );
    assert_no_trace(td.path(), &templates, "second-unit");

    colony.shutdown().await;
}

/// The success path in ONE diff: a class declared and instantiated by the same
/// mutation. The instance is staged from the declaration's staging area and
/// the registry row names the library directory — the two must not disagree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declaration_and_its_instance_commit_together_in_one_diff() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let outcome = send_mutation(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_templates": [declaration("note-unit")],
            "add_nodes": [{"name": "notes", "template": "note-unit@1.0.0"}],
        }}),
    )
    .await;
    assert!(outcome.is_committed(), "outcome: {outcome:?}");

    let dir = templates.join("local/note-unit");
    assert!(
        dir.join("config.json").is_file(),
        "the class is not in the library"
    );
    assert!(
        td.path().join("main/notes/config.json").is_file(),
        "the instance was not grown",
    );
    assert!(staging_is_empty(td.path()), "the commit left staged bytes");
    let rows = read_templates_table(td.path());
    let row = rows
        .iter()
        .find(|r| r.name == "note-unit")
        .expect("no registry row");
    assert_eq!(
        std::path::Path::new(&row.filesystem_path),
        dir.as_path(),
        "the row points at the staging area instead of the library",
    );

    colony.shutdown().await;
}
