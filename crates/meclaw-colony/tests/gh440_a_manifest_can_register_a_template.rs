//! GH #440: a template enters a RUNNING colony as a manifest declaration.
//!
//! The tests assert the FILESYSTEM and `colony.db`, not the receipt — a
//! registration that merely stopped being rejected would write nothing and
//! still pass an outcome-only assertion (the gh169 lesson).

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

/// A booted colony plus the channels a test talks to it through. Built here
/// rather than via `meclaw_testing::ColonyHandle` because this file needs a
/// `--templates` root that is NOT `<root>/templates`, and only
/// `ColonyTaskConfig::with_templates_root` offers one.
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
/// the CLI's `--templates` resolves to, which is what `add_templates` must
/// write under.
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

/// One knock at the mutation door. `MutationDoor` rather than `Mutation`
/// because a manifest and a single body have to arrive at the SAME door — that
/// is the whole point of the declaration form.
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

/// A shipped template, written by hand into the library.
fn write_template(templates: &std::path::Path, name: &str, version: &str) {
    write(
        templates,
        &format!("{name}/template.json"),
        &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    );
    write(templates, &format!("{name}/config.json"), CELL_CONFIG);
}

/// The declaration form: no path, no root, no version field on the entry — the
/// target is always `{templates_root}/local/<name>/` and the version lives in
/// the `template.json` that ships with the entry.
fn register_named(name: &str) -> Value {
    json!({"scope": "/", "ctx": {}, "diff": {
        "add_templates": [
            {"name": name,
             "files": {
                 "template.json": format!(r#"{{"name": "{name}", "version": "1.0.0"}}"#),
                 "config.json": CELL_CONFIG,
             }}
        ]
    }})
}

fn register_note_unit() -> Value {
    register_named("note-unit")
}

/// The template staging area a registration builds in before it moves the
/// tree into the library with one `rename(2)`. After a commit AND after a
/// refusal it must hold nothing.
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

/// The declaration writes under the resolved `--templates` root, never under
/// `<root>/templates` by assumption: the two differ whenever the operator
/// passed `--templates`, and a colony with two template roots has two answers
/// to one name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mutation_door_writes_under_the_resolved_templates_root() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let elsewhere = td.path().join("library"); // NOT <root>/templates
    std::fs::create_dir_all(&elsewhere).expect("mkdir");

    let colony = boot_colony_with_templates_root(td.path(), &elsewhere).await;
    let outcome = send_mutation(&colony, register_note_unit()).await;

    assert!(outcome.is_committed(), "outcome: {outcome:?}");
    assert!(
        elsewhere.join("local/note-unit/template.json").is_file(),
        "the template landed outside the root the colony was told to use",
    );
    assert!(
        !td.path().join("templates").exists(),
        "the door invented a second template root",
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_template_is_on_disk_and_in_colony_db() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let outcome = send_mutation(&colony, register_note_unit()).await;
    assert!(outcome.is_committed(), "outcome: {outcome:?}");

    let dir = templates.join("local/note-unit");
    assert!(dir.join("template.json").is_file(), "template.json missing");
    assert!(dir.join("config.json").is_file(), "config.json missing");
    assert!(
        staging_is_empty(td.path()),
        "the staging area survived the commit",
    );

    // colony.db, not the receipt: a registration that merely stopped being
    // rejected would write a directory nobody can resolve.
    let rows = read_templates_table(td.path());
    let row = rows
        .iter()
        .find(|r| r.name == "note-unit")
        .expect("no registry row for note-unit");
    assert_eq!(row.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        std::path::Path::new(&row.filesystem_path),
        dir.as_path(),
        "the row must point at the directory that was written",
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_taken_name_is_refused_and_leaves_nothing_behind() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    write_template(&templates, "note-unit", "1.0.0"); // already in the library
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;
    rescan(&colony, &templates).await.expect("rescan");

    let outcome = send_mutation(&colony, register_note_unit()).await;
    assert_eq!(
        error_code(&outcome),
        Some("template_name_taken"),
        "{outcome:?}"
    );

    // The residue half: a refused registration is invisible on disk. gh361
    // pinned exactly this concern for the old builder's on-disk lease and
    // in-flight markers; the concern outlives the template because the colony
    // does the writing now.
    assert!(
        !templates.join("local").exists(),
        "the refusal left a staging or target directory behind",
    );
    assert!(
        staging_is_empty(td.path()),
        "the refusal left a staging directory behind",
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_library_is_out_of_reach() {
    // There is no field to point elsewhere with, so this is a property of the
    // code rather than of the caller's manners. Asserted anyway, because it is
    // the sentence the README makes.
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    write_template(&templates, "talky", "4.2.2");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;
    rescan(&colony, &templates).await.expect("rescan");

    let before = std::fs::read_to_string(templates.join("talky/template.json")).expect("read");
    let outcome = send_mutation(&colony, register_named("talky")).await;
    assert_eq!(
        error_code(&outcome),
        Some("template_name_taken"),
        "{outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(templates.join("talky/template.json")).expect("read"),
        before,
        "a shipped template was rewritten",
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rescan_after_a_registration_finds_exactly_one_answer() {
    // The registration and the scan must agree, or the row is a lie that the
    // next boot discovers by exiting 1.
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    assert!(
        send_mutation(&colony, register_note_unit())
            .await
            .is_committed()
    );
    rescan(&colony, &templates)
        .await
        .expect("the rescan must not abort on what the registration wrote");

    let rows = read_templates_table(td.path());
    assert_eq!(
        rows.iter().filter(|r| r.name == "note-unit").count(),
        1,
        "the rescan produced a second answer to one name: {rows:?}",
    );

    colony.shutdown().await;
}

// ── order across manifest entries ───────────────────────────────────────────

/// One `add_nodes` entry instantiating `note-unit` at the root.
fn grow_notes() -> Value {
    json!({"scope": "/", "ctx": {}, "diff": {
        "add_nodes": [{"name": "notes", "template": "note-unit@1.0.0"}]
    }})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_declaration_resolves_what_an_earlier_one_registered() {
    // The whole reason this is a declaration and not a side channel: ORDER.
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    let manifest = json!({"manifest": [register_note_unit(), grow_notes()]});
    let outcome = send_manifest(&colony, manifest).await;

    assert!(
        outcome.is_committed(),
        "entry 2 could not see what entry 1 registered: {outcome:?}",
    );
    assert!(
        td.path().join("main/notes/config.json").is_file(),
        "the cell was not grown",
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_taken_name_stops_the_manifest_at_its_own_position() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    std::fs::create_dir_all(&templates).expect("mkdir");
    let colony = boot_colony_with_templates_root(td.path(), &templates).await;

    // Entry 1 registers it, entry 2 registers it AGAIN, entry 3 never runs.
    let manifest = json!({"manifest": [
        register_note_unit(),
        register_note_unit(),
        grow_notes(),
    ]});
    let outcome = send_manifest(&colony, manifest).await;

    let (failed_at, remaining, code) = match &outcome {
        MutationDoorOutcome::Manifest(ManifestOutcome::Rejected {
            failed_at,
            remaining,
            error_code,
            ..
        }) => (*failed_at, *remaining, error_code.as_str()),
        other => panic!("expected a rejected manifest, got {other:?}"),
    };
    assert_eq!(code, "template_name_taken", "{outcome:?}");
    // 1-based: an operator counts entries, not indices (manifest.rs).
    assert_eq!(failed_at, 2, "{outcome:?}");
    assert_eq!(remaining, 1, "{outcome:?}");
    // Entry 1 stays applied — a manifest rolls forward, there is no rollback.
    assert!(
        templates.join("local/note-unit/template.json").is_file(),
        "the refusal undid an entry that had committed",
    );
    assert!(
        !td.path().join("main/notes").exists(),
        "an entry after the refusal was applied",
    );

    colony.shutdown().await;
}
