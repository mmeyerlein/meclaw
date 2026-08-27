//! GH #424 — the first boot grows the `ref` markers its root tree declares.
//!
//! WHAT THIS FILE IS
//! =================
//! Ruling R4 of the 2026-08-26 wave asks for the half of GH #277 that was
//! specified and never built: a root tree that references a composite template
//! and grows itself on the first `meclaw --root` start, through the very
//! resolution and staging a mutation takes.
//!
//! The form is the `cell.type: "ref"` marker, not a `params.graph.nodes` block
//! on a hive (orchestrator ruling O1 of the lane plan). Two reasons carry that
//! choice and both are measured here:
//!
//! * the marker IS the input `mutation/subtree.rs::expand_ref` already
//!   resolves — registry lookup, version pinning, `override_params` layering,
//!   cycle guard, and the same three refusals;
//! * the marker CONSUMES ITSELF. After the growth the referenced template's
//!   content stands in its place and the declaration is gone, so a second boot
//!   finds nothing to grow and a node a mutation removed cannot be re-declared
//!   into existence.
//!
//! `params.graph.nodes` therefore stays a hard boot error — and stops being a
//! backlog item. The first test in this file is that boundary, pinned.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RegistryOverlay,
    bootstrap_from_filesystem, plan_bootstrap,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;
const CELL: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

/// Write a `config.json` under `{root}/{rel}`, creating the directories.
fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// A one-cell template under `templates/<name>/`.
fn write_leaf_template(root: &std::path::Path, name: &str, version: &str) {
    write_leaf_template_at(root, name, name, version);
}

/// The same, with the directory name and the declared template name apart —
/// two directories can declare the same template at different versions.
fn write_leaf_template_at(root: &std::path::Path, dir: &str, name: &str, version: &str) {
    write(
        root,
        &format!("templates/{dir}/template.json"),
        &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    );
    write(root, &format!("templates/{dir}/config.json"), CELL);
}

/// A `ref` marker `config.json`.
fn ref_marker(reference: &str) -> String {
    format!(r#"{{"cell":{{"type":"ref","template":"{reference}"}}}}"#)
}

fn factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    })
}

/// Boot the tree, with the templates scanned FIRST — the order production keeps
/// (`meclaw-cli` runs `boot_load_or_scan` before the bootstrap), and the reason
/// a growth has a registry to resolve against at all.
async fn booted(td: &tempfile::TempDir) -> ColonyHandle {
    match try_boot(td).await {
        Ok(h) => h,
        Err((h, e)) => {
            h.shutdown().await;
            panic!("the tree must boot: {e}")
        }
    }
}

/// The boot's refusal, rendered — for the cases that must NOT boot.
async fn boot_error(td: &tempfile::TempDir) -> String {
    match try_boot(td).await {
        Ok(h) => {
            h.shutdown().await;
            panic!("this tree must not boot")
        }
        Err((h, e)) => {
            h.shutdown().await;
            e
        }
    }
}

#[allow(clippy::result_large_err)]
async fn try_boot(td: &tempfile::TempDir) -> Result<ColonyHandle, (ColonyHandle, String)> {
    let f = factory();
    let h = ColonyHandle::new_with_factories_at(td, vec![("persist_mock".to_string(), f.clone())]);
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");

    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), f);
    match bootstrap_from_filesystem(td.path(), &reg, &h.runtime()).await {
        Ok(_) => Ok(h),
        Err(e) => Err((h, format!("{e:?}"))),
    }
}

/// One registry row as the live colony reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    path: String,
    cell_type: String,
    active: bool,
}

async fn registry_rows(h: &ColonyHandle) -> Vec<Row> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 500,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut rows: Vec<Row> = ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| Row {
            path: e.path,
            cell_type: e.cell_type,
            active: e.active,
        })
        .collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows
}

/// The provenance the PERSISTED registry holds — `template` and
/// `template_version` are index columns of `colony.db`, not part of the read
/// DTO, so they are read after a clean shutdown has flushed the write buffer.
///
/// NOTE on the direct SQL: test-side reading of `colony.db`, not cell code.
fn provenance_of(root: &std::path::Path, path: &str) -> (Option<String>, Option<String>) {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    conn.query_row(
        "SELECT template, template_version FROM registry WHERE path = ?1",
        [path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or_else(|e| panic!("no registry row at {path}: {e}"))
}

/// Every `(path, cell_id)` the persisted registry holds.
fn cell_ids(root: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT path, cell_id FROM registry ORDER BY path")
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
// the boundary that stays (O1)
// ──────────────────────────────────────────────────────────────────────────────

/// A hive declaring `params.graph.nodes` breaks the boot, and says which key
/// and which file.
///
/// This is the STATED BOUNDARY, not a gap. `GraphHints` carries `edges` and
/// nothing else, under `deny_unknown_fields` (`config.rs`), so a `nodes` block
/// is a named refusal at plan time. Growing a node at boot happens through the
/// `ref` marker — one declaration form, one resolution path — and a `nodes`
/// block would be a second instantiation language with its own name→path
/// lookup and its own override addressing.
///
/// Pins `SNB-graph-nodes-note-{de,en}` and `SNB-graph-nodes-{de,en}` in
/// `plans/spec-claims/claims.tsv`.
#[test]
fn a_hive_declaring_params_graph_nodes_refuses_the_boot() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},
            "params":{"graph":{"nodes":{"a":{"template":"x"}},"edges":[]}}}"#,
    );

    let errs = plan_bootstrap(
        td.path(),
        &CellFactoryRegistry::new(),
        &RegistryOverlay::new(),
    )
    .expect_err("a `nodes` block must not boot");

    let err = format!("{errs:?}");
    assert!(
        err.contains("nodes"),
        "the refusal must name the key: {err}"
    );
    assert!(
        err.contains("config.json"),
        "the refusal must name the file: {err}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// the growth
// ──────────────────────────────────────────────────────────────────────────────

/// A root tree with a `ref` marker grows on its FIRST boot.
///
/// Before GH #424 this was a refusal — `bootstrap.rs` said "the key `template`
/// (and `type: \"ref\"`) is template-time only … must not stand in an
/// instantiated tree". That was right for as long as the boot could not
/// instantiate. This task removes exactly that premise: the marker is now a
/// declaration the first boot fulfils, through the same `stage_subtree` chain a
/// mutation takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_root_tree_with_a_ref_marker_grows_on_the_first_boot() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/child/config.json",
        &ref_marker("leaf@1.0.0"),
    );

    let h = booted(&td).await;
    let rows = registry_rows(&h).await;
    let child = rows
        .iter()
        .find(|r| r.path == "/child")
        .unwrap_or_else(|| panic!("no /child in the registry: {rows:?}"));
    assert_eq!(
        child.cell_type, "persist_mock",
        "the marker became the cell"
    );

    let cfg = read_json(&td.path().join("main/child/config.json"));
    assert_ne!(
        cfg["cell"]["type"], "ref",
        "the marker consumed itself: {cfg}"
    );
    assert!(
        cfg["cell"]["template"].is_null(),
        "and its declaration is gone: {cfg}"
    );
    h.shutdown().await;
    assert_eq!(
        provenance_of(td.path(), "/child"),
        (Some("leaf".to_string()), Some("1.0.0".to_string())),
        "the registry records the origin the growth came from"
    );
}

/// The boot instantiates once, and a reboot keeps every `cell_id`.
///
/// **This is the pin for `SNB-instantiation-{de,en}`** in
/// `plans/spec-claims/claims.tsv` — the claim that a cell's identity is minted
/// exactly once and survives a restart. It could not be pinned before, because
/// the boot did not instantiate at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_boot_grows_the_ref_and_a_reboot_keeps_every_cell_id() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/child/config.json",
        &ref_marker("leaf@1.0.0"),
    );

    let h = booted(&td).await;
    h.shutdown().await;
    let first = cell_ids(td.path());
    assert!(!first.is_empty(), "the first boot registered something");

    let h = booted(&td).await;
    h.shutdown().await;
    let second = cell_ids(td.path());
    assert_eq!(first, second, "a reboot mints no new identity");
}

/// The second boot grows nothing, because the marker is gone.
///
/// Idempotence STRUCTURALLY (orchestrator ruling O1): there is no ledger of
/// what was grown, because after the growth there is nothing left that asks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_boot_grows_nothing_because_the_marker_is_gone() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/child/config.json",
        &ref_marker("leaf@1.0.0"),
    );

    let h = booted(&td).await;
    let before = registry_rows(&h).await;
    h.shutdown().await;

    // The plan the second boot would make: no growth left to plan.
    let plan = plan_bootstrap(
        td.path(),
        &CellFactoryRegistry::new(),
        &RegistryOverlay::new(),
    );
    // The factory registry is empty here on purpose — the question is only
    // whether a MARKER is still declared, and a marker needs no factory.
    if let Ok(p) = plan {
        assert!(p.growths.is_empty(), "nothing left to grow");
    }

    let h = booted(&td).await;
    let after = registry_rows(&h).await;
    h.shutdown().await;
    assert_eq!(before, after, "the second boot changed nothing");
}

/// A growth whose template the registry does not hold is `template_missing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_growth_whose_template_the_registry_does_not_hold_is_template_missing() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/child/config.json",
        &ref_marker("leaf@9.9.9"),
    );

    let err = boot_error(&td).await;
    assert!(
        err.contains("template_missing"),
        "the refusal carries the mutation path's own code: {err}"
    );
    assert!(err.contains("leaf@9.9.9"), "and names the reference: {err}");
}

/// An unversioned reference resolves, and the instance records the version it
/// actually got.
///
/// The plan for this lane asked for "two versions of one name, the highest
/// wins" — a premise that no longer holds at HEAD: the scanner refuses a
/// duplicate template NAME outright ("a template name must be unique so a
/// bare-name reference has one answer"), so the ambiguity the old rule resolved
/// cannot arise any more. What remains true, and is what matters here, is that
/// a bare name is a legal reference at boot and the grown node names the
/// resolved version rather than the reference string.
///
/// Measured at the GROWN TREE, not by calling `TemplatesRegistry::resolve` —
/// otherwise the test would pin the registry rather than the boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_growth_resolves_an_unversioned_reference_to_the_pinned_one() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.2.0");
    write(td.path(), "main/config.json", HIVE);
    write(td.path(), "main/child/config.json", &ref_marker("leaf"));

    let h = booted(&td).await;
    h.shutdown().await;
    assert_eq!(
        provenance_of(td.path(), "/child"),
        (Some("leaf".to_string()), Some("1.2.0".to_string())),
        "the grown node names the RESOLVED version, not the bare reference"
    );
}

/// A ring of `ref`s in the root tree is `template_ref_cycle`, with the ring
/// rendered — the same guard and the same wording as at the mutation door.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ref_ring_in_the_root_tree_is_template_ref_cycle() {
    let td = tempfile::TempDir::new().unwrap();
    // `a` contains a ref to `b`, `b` contains a ref to `a`.
    write(
        td.path(),
        "templates/a/template.json",
        r#"{"name":"a","version":"1.0.0"}"#,
    );
    write(td.path(), "templates/a/config.json", HIVE);
    write(
        td.path(),
        "templates/a/inner/config.json",
        r#"{"cell":{"type":"ref","template":"b@1.0.0"}}"#,
    );
    write(
        td.path(),
        "templates/b/template.json",
        r#"{"name":"b","version":"1.0.0"}"#,
    );
    write(td.path(), "templates/b/config.json", HIVE);
    write(
        td.path(),
        "templates/b/inner/config.json",
        r#"{"cell":{"type":"ref","template":"a@1.0.0"}}"#,
    );
    write(td.path(), "main/config.json", HIVE);
    write(td.path(), "main/child/config.json", &ref_marker("a@1.0.0"));

    let err = boot_error(&td).await;
    assert!(
        err.contains("template_ref_cycle"),
        "the ring is named as a ring: {err}"
    );
}

/// A node a mutation removed does not come back on the next boot.
///
/// **The second open design question of the roadmap line to GH #352**, answered
/// rather than deferred: nothing stands in the tree any more that could
/// re-declare the removed node, because the declaration consumed itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_removed_by_a_mutation_does_not_come_back_on_the_next_boot() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/child/config.json",
        &ref_marker("leaf@1.0.0"),
    );

    let h = booted(&td).await;
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {"remove_nodes": [{"match": {"name": "child"}}]}}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_nodes must commit: {outcome:?}"
    );
    h.shutdown().await;

    let h = booted(&td).await;
    let rows = registry_rows(&h).await;
    h.shutdown().await;
    let child = rows
        .iter()
        .find(|r| r.path == "/child")
        .expect("/child row");
    assert!(
        !child.active,
        "a removed node stays removed across a reboot: {rows:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// nesting (task 17)
// ──────────────────────────────────────────────────────────────────────────────

/// A marker the operator wrote INSIDE another marker's directory grows too, in
/// the second pass.
///
/// Without this a seed that declares two levels boots half. The loop is bounded
/// by the number of markers the first pass found: every pass consumes at least
/// one, so a pass that consumes none while markers remain is a defect, and it
/// is named rather than spun on.
///
/// Note what does NOT need changing for this: `subtree.rs`'s
/// `reject_stray_ref_entries` refuses a `ref` directory holding more than a
/// `config.json` — correctly, IN A TEMPLATE, where a file beside the marker
/// gives one address two sources. It never sees a root-tree marker: the growth
/// hands `parse_subtree` the TEMPLATE directory, and the root-tree marker is
/// read by the boot planner. In the root tree a directory below the marker
/// gives a DIFFERENT address its first source, which is the opposite case.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marker_written_inside_another_marker_grows_in_the_second_pass() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    // A composite that brings an empty `orgs` hive with it.
    write(
        td.path(),
        "templates/composite/template.json",
        r#"{"name":"composite","version":"1.0.0"}"#,
    );
    write(td.path(), "templates/composite/config.json", HIVE);
    write(td.path(), "templates/composite/orgs/config.json", HIVE);

    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/os/config.json",
        &ref_marker("composite@1.0.0"),
    );
    // The operator prescribes a deeper node inside a position that does not
    // exist yet — that is the whole point of a declaration.
    write(
        td.path(),
        "main/os/orgs/acme/config.json",
        &ref_marker("leaf@1.0.0"),
    );

    let h = booted(&td).await;
    let rows = registry_rows(&h).await;
    h.shutdown().await;
    assert!(
        rows.iter().any(|r| r.path == "/os/orgs/acme"),
        "both levels stand after ONE boot: {rows:?}"
    );
    assert_eq!(
        provenance_of(td.path(), "/os/orgs/acme"),
        (Some("leaf".to_string()), Some("1.0.0".to_string()))
    );
}

/// A marker child the template ALSO brings is a named refusal, before any
/// rename.
///
/// The same class `swap_nodes` already knows: a directory that is already there
/// is named back, never overwritten. Pre-destructive on purpose — nothing may
/// be half-replaced when the two sources are discovered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marker_child_that_the_template_also_brings_is_a_named_refusal() {
    let td = tempfile::TempDir::new().unwrap();
    write_leaf_template(td.path(), "leaf", "1.0.0");
    write(
        td.path(),
        "templates/composite/template.json",
        r#"{"name":"composite","version":"1.0.0"}"#,
    );
    write(td.path(), "templates/composite/config.json", HIVE);
    write(td.path(), "templates/composite/access/config.json", CELL);

    write(td.path(), "main/config.json", HIVE);
    write(
        td.path(),
        "main/os/config.json",
        &ref_marker("composite@1.0.0"),
    );
    // The operator prescribes `access` — and so does the template.
    write(
        td.path(),
        "main/os/access/config.json",
        &ref_marker("leaf@1.0.0"),
    );

    let err = boot_error(&td).await;
    assert!(
        err.contains("access"),
        "the refusal names the colliding address: {err}"
    );
    assert!(
        err.contains("two sources"),
        "…and says what the collision is: {err}"
    );
    // Pre-destructive: the operator's own declaration is still there.
    let cfg = read_json(&td.path().join("main/os/access/config.json"));
    assert_eq!(
        cfg["cell"]["type"], "ref",
        "nothing was overwritten before the refusal: {cfg}"
    );
}
