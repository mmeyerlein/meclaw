//! GH #664 — a class may have more than one version, and the library is keyed
//! by both.
//!
//! Until this wave the scanner refused two `template.json` declaring one
//! `name`, whatever their versions said (GH #277, ruling Q7). The resolver
//! never needed that: it has always answered `name@version` exactly and a bare
//! name with the highest version. The scan was stricter than the rule it was
//! protecting, and the price was paid in a live colony — a new version of a
//! local class could only enter under a NEW NAME, so the catalogue named the
//! old version while the colony ran the new bytes.
//!
//! What is unique now is `(name, version)`. What stays refused is the same
//! `name@version` twice, and an unversioned entry beside versioned ones.

use meclaw_colony::templates::{ScannerError, scan_templates_dir};

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

/// One template directory, named the way a versioned registration builds it.
fn write_class(templates: &std::path::Path, dir: &str, name: &str, version: &str) {
    write(
        templates,
        &format!("{dir}/template.json"),
        &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    );
}

/// Two versions of one class are two entries; the same version twice is the
/// ambiguity Q7 really meant, and it still aborts the whole scan.
#[test]
fn the_scan_carries_two_versions_and_aborts_on_two_identical_ones() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    write_class(&templates, "local/a@1.0.0", "a", "1.0.0");
    write_class(&templates, "local/a@1.0.1", "a", "1.0.1");

    let found = scan_templates_dir(&templates).expect("two versions of one class are two entries");
    assert_eq!(found.len(), 2, "the scan lost a version: {found:?}");
    let mut versions: Vec<&str> = found
        .iter()
        .map(|t| t.version.as_deref().unwrap_or("<none>"))
        .collect();
    versions.sort_unstable();
    assert_eq!(versions, vec!["1.0.0", "1.0.1"]);

    // The third directory repeats a version that is already there. A pinned
    // reference would have two answers, so the scan aborts — and it names both
    // directories, because the collision is a place on disk, not a word.
    write_class(&templates, "vendored/a-again", "a", "1.0.1");
    let err = scan_templates_dir(&templates).expect_err("the same version twice must abort");
    let (name, version) = match &err {
        ScannerError::DuplicateVersion { name, version, .. } => (name.clone(), version.clone()),
        other => panic!("expected DuplicateVersion, got {other:?}"),
    };
    assert_eq!(name, "a");
    assert_eq!(version.as_deref(), Some("1.0.1"));
    let rendered = err.to_string();
    assert!(
        rendered.contains("a@1.0.1"),
        "the message must name the class AND the version: {rendered}"
    );
    assert!(
        rendered.contains(&templates.join("local/a@1.0.1").display().to_string())
            && rendered.contains(&templates.join("vendored/a-again").display().to_string()),
        "the message must name both directories: {rendered}"
    );
}

// ── the registration side ────────────────────────────────────────────────────

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

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    })
}

/// A booted colony plus the channels a test talks to it through. Built here
/// rather than via `meclaw_testing::ColonyHandle` because this file needs a
/// `--templates` root of its own, and only `ColonyTaskConfig::with_templates_root`
/// offers one.
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

async fn boot(root: &std::path::Path, templates_root: &std::path::Path) -> Colony {
    write(root, "main/config.json", HIVE_CONFIG);
    std::fs::create_dir_all(templates_root).expect("mkdir templates");
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

async fn send(c: &Colony, payload: Value) -> MutationDoorOutcome {
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

fn error_code(outcome: &MutationDoorOutcome) -> Option<&str> {
    match outcome {
        MutationDoorOutcome::Single(MutationOutcome::Rejected { error_code, .. })
        | MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { error_code, .. }) => {
            Some(error_code.as_str())
        }
        _ => None,
    }
}

fn details(outcome: &MutationDoorOutcome) -> String {
    match outcome {
        MutationDoorOutcome::Single(MutationOutcome::Rejected { details, .. })
        | MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { details, .. }) => {
            details.clone()
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The declaration form: no version field on the ENTRY — the version lives in
/// the `template.json` the entry ships, which is the same bytes the scan reads.
fn register(name: &str, version: Option<&str>) -> Value {
    register_all(&[(name, version)])
}

/// The same form with several entries in ONE `add_templates` array — the diff
/// has to refuse there what it refuses against the registry, or the refusal
/// holds only for diffs with one entry.
fn register_all(entries: &[(&str, Option<&str>)]) -> Value {
    let arr: Vec<Value> = entries
        .iter()
        .map(|(name, version)| {
            let template_json = match version {
                Some(v) => format!(r#"{{"name": "{name}", "version": "{v}"}}"#),
                None => format!(r#"{{"name": "{name}"}}"#),
            };
            json!({"name": name,
                   "files": {"template.json": template_json, "config.json": CELL_CONFIG}})
        })
        .collect();
    json!({"scope": "/", "ctx": {}, "diff": {"add_templates": arr}})
}

/// The rows of `colony.db`'s `templates` table for one name — the catalogue,
/// not the receipt: a registration that merely stopped being refused would
/// write a directory nobody can resolve.
fn rows_named(root: &std::path::Path, name: &str) -> Vec<(String, Option<String>, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT template_id, version, filesystem_path FROM templates WHERE name = ?1 ORDER BY version")
        .expect("prepare");
    let rows = stmt
        .query_map([name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

/// The measured case from a live colony: `screen@1.0.0` was registered, and
/// `screen@1.0.1` could not follow it. The way around it was a new name per
/// version, which turns one class into a family of names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_version_registers_beside_the_old_one() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;

    assert!(
        send(&colony, register("demo", Some("1.0.0")))
            .await
            .is_committed()
    );
    let second = send(&colony, register("demo", Some("1.0.1"))).await;
    assert!(
        second.is_committed(),
        "a new version of a registered class was refused: {second:?}"
    );

    let rows = rows_named(td.path(), "demo");
    assert_eq!(rows.len(), 2, "the catalogue lost a version: {rows:?}");
    let versions: Vec<Option<String>> = rows.iter().map(|(_, v, _)| v.clone()).collect();
    assert_eq!(
        versions,
        vec![Some("1.0.0".into()), Some("1.0.1".into())],
        "both versions must stand in the catalogue",
    );

    colony.shutdown().await;
}

/// The same `name@version` twice would be two answers to one pinned reference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_identical_version_is_still_taken() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;

    assert!(
        send(&colony, register("demo", Some("1.0.0")))
            .await
            .is_committed()
    );
    assert!(
        send(&colony, register("demo", Some("1.0.1")))
            .await
            .is_committed()
    );
    let again = send(&colony, register("demo", Some("1.0.1"))).await;
    assert_eq!(error_code(&again), Some("template_name_taken"), "{again:?}");
    assert!(
        details(&again).contains("demo@1.0.1"),
        "the refusal must name the version, not just the class: {}",
        details(&again)
    );

    colony.shutdown().await;
}

/// A catalogue in which one class has a nameless version is the ambiguity Q7
/// removed — `resolve` would order it deterministically, but nobody could pin
/// it. Both directions are refused, under the same `error_code`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unversioned_entry_beside_a_versioned_one_is_refused_by_name() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;

    assert!(
        send(&colony, register("demo", Some("1.0.0")))
            .await
            .is_committed()
    );
    let bare = send(&colony, register("demo", None)).await;
    assert_eq!(error_code(&bare), Some("template_name_taken"), "{bare:?}");
    assert!(
        details(&bare).contains("1.0.0"),
        "the refusal must name the versions that ARE registered: {}",
        details(&bare)
    );

    // The mirror: an unversioned class is registered, a versioned entry of the
    // same name follows.
    assert!(send(&colony, register("plain", None)).await.is_committed());
    let versioned = send(&colony, register("plain", Some("2.0.0"))).await;
    assert_eq!(
        error_code(&versioned),
        Some("template_name_taken"),
        "{versioned:?}"
    );

    // The same two entries in ONE diff. `refuse_if_taken` runs against the
    // registry as it stood BEFORE the diff, so without a self-check of the diff
    // both would commit and the catalogue would hold exactly the state this
    // refusal exists to prevent. The path check that catches two identical
    // entries cannot see it: since the target carries the version, the two
    // build different directories.
    let in_one_diff = send(
        &colony,
        register_all(&[("twins", Some("1.0.0")), ("twins", None)]),
    )
    .await;
    assert_eq!(
        error_code(&in_one_diff),
        Some("template_name_taken"),
        "an unpinnable entry beside a versioned one of the same name must be \
         refused inside one diff too: {in_one_diff:?}",
    );
    assert!(
        !templates.join("local/twins@1.0.0").exists() && !templates.join("local/twins").exists(),
        "the refused diff left a directory behind",
    );

    colony.shutdown().await;
}

/// Orchestrator ruling of fix round 1: a `version` the resolver cannot read is
/// refused at the door rather than written. `parse_simple_version` is strict
/// (`major.minor.patch`), and an entry declaring `1.0` used to register into
/// `local/<name>/` with `"1.0"` in its registry row — after which NO reference
/// reached it: `@1.0` is not a version a reference can carry, and the bare name
/// finds no parsable candidate and no unversioned one either. An unreachable
/// registration is worse than a refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_version_the_resolver_cannot_read_is_refused_at_the_door() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;

    for bad in ["1.0", "1.2.x", "2.0.0-rc1", ""] {
        let outcome = send(&colony, register("demo", Some(bad))).await;
        assert_eq!(
            error_code(&outcome),
            Some("schema"),
            "'{bad}' is not a version the resolver reads: {outcome:?}",
        );
        assert!(
            details(&outcome).contains(bad) || bad.is_empty(),
            "the refusal must quote what was declared: {}",
            details(&outcome),
        );
    }
    assert!(
        !templates.join("local").exists(),
        "a refused version left a directory behind",
    );

    // The neighbouring case stays as it was: no `version` at all is legal.
    assert!(send(&colony, register("plain", None)).await.is_committed());

    colony.shutdown().await;
}

/// Siblings on disk, not a child directory per version: the scan does not
/// descend into a directory that carries a `template.json`, so a `local/<name>/`
/// that is already live would make every version beneath it invisible. The
/// sibling form needs no migration — what lies there stays there and stays
/// resolvable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_versions_lie_side_by_side_on_disk() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    // Pre-existing stock from before this build: a class registered under the
    // unversioned directory form, with its registry row.
    write_class(&templates, "local/legacy", "legacy", "0.9.0");
    write(&templates, "local/legacy/config.json", CELL_CONFIG);
    let colony = boot(td.path(), &templates).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    colony
        .inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: templates.clone(),
            ack: ack_tx,
        })
        .await
        .expect("send rescan");
    ack_rx.await.expect("ack").expect("rescan");

    assert!(
        send(&colony, register("demo", Some("1.0.0")))
            .await
            .is_committed()
    );
    assert!(
        send(&colony, register("demo", Some("1.0.1")))
            .await
            .is_committed()
    );

    assert!(
        templates.join("local/demo@1.0.0/template.json").is_file(),
        "the first registration did not build a versioned directory",
    );
    assert!(
        templates.join("local/demo@1.0.1/template.json").is_file(),
        "the second version did not land beside the first",
    );
    assert!(
        !templates.join("local/demo").exists(),
        "one rule, not two: a versioned entry never builds the bare directory",
    );
    // The old form is untouched and still answers.
    assert!(
        templates.join("local/legacy/template.json").is_file(),
        "the pre-existing directory form was moved or removed (No-Delete)",
    );
    let legacy = rows_named(td.path(), "legacy");
    assert_eq!(legacy.len(), 1, "the old form lost its row: {legacy:?}");
    assert_eq!(
        std::path::Path::new(&legacy[0].2),
        templates.join("local/legacy"),
        "the old row must still point at the directory it always did",
    );

    colony.shutdown().await;
}

// ── resolving, rescanning, and moving between two versions ───────────────────

/// The version the persisted node config names — the leaf stamp of the
/// provenance, which is what "which version is this node running" means.
fn node_version(root: &std::path::Path, name: &str) -> Option<String> {
    let raw = std::fs::read_to_string(root.join("main").join(name).join("config.json"))
        .unwrap_or_else(|e| panic!("read config.json of '{name}': {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config.json is JSON");
    v["cell"]["provenance"]["template_version"]
        .as_str()
        .map(str::to_string)
}

fn edge_count(root: &std::path::Path, from: &str, to: &str) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row(
        "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
        [from, to],
        |r| r.get(0),
    )
    .expect("count edges")
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

/// Both versions registered, then instantiated twice. R3 has always said this;
/// until now it never had more than one candidate to say it about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bare_name_resolves_to_the_newer_and_a_pin_resolves_exactly() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;
    assert!(
        send(&colony, register("demo", Some("1.0.0")))
            .await
            .is_committed()
    );
    assert!(
        send(&colony, register("demo", Some("1.0.1")))
            .await
            .is_committed()
    );

    let grown = send(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {"add_nodes": [
            {"name": "newest", "template": "demo"},
            {"name": "pinned", "template": "demo@1.0.0"},
        ]}}),
    )
    .await;
    assert!(grown.is_committed(), "{grown:?}");

    assert_eq!(
        node_version(td.path(), "newest").as_deref(),
        Some("1.0.1"),
        "a bare name must resolve to the highest version",
    );
    assert_eq!(
        node_version(td.path(), "pinned").as_deref(),
        Some("1.0.0"),
        "a pinned reference must resolve exactly",
    );

    colony.shutdown().await;
}

/// The upsert and the lazy remove behind a rescan are keyed by `(name,
/// version)` and `template_id` is stable per pair — so a rescan over a library
/// holding two versions of one class must change nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rescan_keeps_both_rows_and_both_ids() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;
    assert!(
        send(&colony, register("demo", Some("1.0.0")))
            .await
            .is_committed()
    );
    assert!(
        send(&colony, register("demo", Some("1.0.1")))
            .await
            .is_committed()
    );

    let before = rows_named(td.path(), "demo");
    assert_eq!(before.len(), 2, "{before:?}");
    rescan(&colony, &templates)
        .await
        .expect("the rescan must not abort on two versions of one class");
    let after = rows_named(td.path(), "demo");

    assert_eq!(
        before, after,
        "the rescan moved a row, a path or a template_id",
    );

    colony.shutdown().await;
}

/// Moving a node from one version to the next is a graph swap like any other,
/// and it needs no mechanism of its own: `match.name` addresses the node,
/// `with.template` takes `name@version` and resolves exactly.
///
/// What the swap does NOT do is keep the node's name. The old cell stays where
/// it is — disconnected but whole, which is what makes a swap back possible
/// (No-Delete) — so its address is still occupied, and a with-side that names
/// it again is refused as `naming_collision` before anything is staged. Both
/// halves are asserted here, because the refusal is the reason the other half
/// looks the way it does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_nodes_moves_a_node_from_one_version_to_the_next() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let templates = td.path().join("templates");
    let colony = boot(td.path(), &templates).await;
    assert!(
        send(&colony, register("screen", Some("1.0.0")))
            .await
            .is_committed()
    );
    assert!(
        send(&colony, register("screen", Some("1.0.1")))
            .await
            .is_committed()
    );

    let grown = send(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {
            "add_nodes": [
                {"name": "sender", "template": "screen@1.0.0"},
                {"name": "screen", "template": "screen@1.0.0"},
            ],
            "add_edges": [{"from": "./sender", "to": "./screen"}],
        }}),
    )
    .await;
    assert!(grown.is_committed(), "{grown:?}");
    assert_eq!(edge_count(td.path(), "/sender", "/screen"), 1);

    // The with-side instantiates a FRESH cell with its own identity and its own
    // cell.db, so it needs an address of its own.
    let same_name = send(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {"swap_nodes": [
            {"match": {"name": "screen"},
             "with": {"name": "screen", "template": "screen@1.0.1"}}
        ]}}),
    )
    .await;
    assert_eq!(
        error_code(&same_name),
        Some("naming_collision"),
        "a with-side at the match's own address must be refused, not silently \
         overwrite the node that is being swapped out: {same_name:?}",
    );

    let swapped = send(
        &colony,
        json!({"scope": "/", "ctx": {}, "diff": {"swap_nodes": [
            {"match": {"name": "screen"},
             "with": {"name": "screen-next", "template": "screen@1.0.1"}}
        ]}}),
    )
    .await;
    assert!(swapped.is_committed(), "{swapped:?}");

    assert_eq!(
        node_version(td.path(), "screen-next").as_deref(),
        Some("1.0.1"),
        "the new node does not name the new version",
    );
    assert_eq!(
        edge_count(td.path(), "/sender", "/screen-next"),
        1,
        "the inbound edge did not swing onto the new version",
    );
    assert_eq!(
        edge_count(td.path(), "/sender", "/screen"),
        0,
        "the old version kept an external edge",
    );
    // No-Delete: the old unit stays whole, so the swap is reversible.
    assert_eq!(
        node_version(td.path(), "screen").as_deref(),
        Some("1.0.0"),
        "the node that was swapped out was removed or rewritten",
    );

    colony.shutdown().await;
}
