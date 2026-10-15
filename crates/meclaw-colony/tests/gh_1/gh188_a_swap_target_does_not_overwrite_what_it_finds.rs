//! GH #188 — a `swap_nodes[].with` instantiation must not write over a
//! directory that is already at its target path.
//!
//! `add_nodes` looks at its `final_path` before staging: if the directory is
//! there, the entry is a Reconnect/Resume and nothing is copied over it. The
//! with-side never looked. It staged a fresh template tree and let
//! `atomic_rename_or_overwrite_all` take the `final_path exists` branch, which
//! replaces `config.json` in place — a second `cell.id` for a path that already
//! had one, a different `cell.type` reading the `cell.db` the old one wrote, and
//! no diagnostic anywhere.
//!
//! #179 closed the half where the target carries a registry row: that is a
//! `naming_collision` before anything is staged. What was left is a directory
//! **nothing in the registry claims** — a hand-placed tree, the residue of an
//! aborted migration, a node whose row was cleared outside the mutation flow.
//! The registry check cannot see it, because the registry is exactly what it is
//! missing from.
//!
//! Two answers were defensible: refuse, or let the with-side declare that it
//! means to take the directory over. The ruling for this pass is **refuse**,
//! consistent with #179 — an occupied path is occupied — and the objection to
//! refusing has lost its force twice over:
//!
//! * Re-taking such a path does not need a manual wipe at all. An `add_nodes` at
//!   an existing directory is a Resume whether or not a registry row names it
//!   (`colony.rs` step 1a decides on FS existence), and an `add_nodes[].adopt`
//!   entry takes an unregistered tree over deliberately, with the on-disk
//!   `cell.type` checked against what the diff expected. The explicit-takeover
//!   knob the issue weighed already exists; it is just not on the with-side.
//! * And a manual wipe no longer breaks the next boot. That was #168, where the
//!   planner still read the hive's `params.graph`; since the edge table is the
//!   boot topology on a Reboot, a wiped directory costs nothing
//!   (`gh168_the_edge_table_is_the_boot_topology`).
//!
//! The assertions are on the filesystem and `colony.db`. One is on the message,
//! deliberately: "no diagnostic" is half of what the issue reports, so the
//! refusal naming the path it found is part of the fix and not a nicety.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::{Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

/// The config of the directory nobody registered. Carries its own `cell.id`,
/// which is the thing a with-side overwrite silently re-mints.
const LEFTOVER_CONFIG: &str = r#"{"cell":{"type":"persist_mock","id":"01890000-0000-7000-8000-00000000beef","idle_timeout_ms":60000},"params":{"terminal":true,"marker":"left behind"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 200,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Whether the persisted graph carries `from -> to` — `colony.db`, because that
/// is what a reboot rebuilds the topology from.
fn edge_persisted(root: &std::path::Path, from: &str, to: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
            [from, to],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// Seed a persisted counter into a cell directory's own `cell.db`, standing in
/// for whatever state the leftover accumulated while it was alive.
fn seed_counter(cell_dir: &std::path::Path, value: i64) {
    let conn =
        meclaw_colony::persist::cell_db::open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO system (slot_path, value, updated_at) VALUES ('counter', ?1, 0)",
        [value.to_string()],
    )
    .unwrap();
}

fn counter(cell_dir: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        cell_dir.join("cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    conn.query_row(
        "SELECT value FROM system WHERE slot_path='counter'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse::<i64>().ok())
}

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    })
}

fn persist_registry() -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), persist_factory());
    reg
}

/// A root hive `main` (logical `/`), an `/anchor` that wires into things, and
/// `/old` — the cell a swap is going to replace. `main` being the single root
/// cell directory is what makes the layout realistic: logical `/leftover` maps
/// to `{root}/main/leftover` (spec Z.331), which is the path the with-side
/// resolves and the one the test plants a directory at.
async fn wired_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
    write(td.path(), "main/old/config.json", CELL_CONFIG);
    write(
        td.path(),
        "templates/persist_mock/template.json",
        r#"{"name":"persist_mock"}"#,
    );
    write(td.path(), "templates/persist_mock/config.json", CELL_CONFIG);

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory())],
    );
    bootstrap_from_filesystem(td.path(), &persist_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_edges": [{"from": "./anchor", "to": "./old"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "precondition: /anchor -> /old is wired; got {outcome:?}"
    );
    (td, h)
}

/// Plant a cell directory the colony has never heard of at `{root}/main/leftover`
/// — created after boot, so no walk ever saw it and no registry row names it.
/// This is the shape the issue names: an aborted migration, a hand-placed tree,
/// a node whose row was cleared outside the mutation flow.
fn plant_unregistered_leftover(td: &tempfile::TempDir) -> std::path::PathBuf {
    write(td.path(), "main/leftover/config.json", LEFTOVER_CONFIG);
    let dir = td.path().join("main/leftover");
    seed_counter(&dir, 42);
    dir
}

// ── the refusal ─────────────────────────────────────────────────────────────

/// The report, end to end: a with-side instantiation aimed at a directory
/// nothing in the registry claims is refused, and the directory it found is
/// byte-for-byte where it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_with_side_instantiation_does_not_overwrite_an_unregistered_directory() {
    let (td, h) = wired_colony().await;
    let leftover = plant_unregistered_leftover(&td);
    let config_before = std::fs::read_to_string(leftover.join("config.json")).unwrap();
    let old_before = registry_entry(&h, "/old")
        .await
        .expect("precondition: /old is registered");

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "old"},
                "with": {"name": "leftover", "template": "persist_mock"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "naming_collision"
        ),
        "an occupied path is occupied whether or not the registry knows it; got {outcome:?}"
    );

    // The directory the mutation found is untouched — this is the whole harm.
    assert_eq!(
        std::fs::read_to_string(leftover.join("config.json")).unwrap(),
        config_before,
        "the leftover's config.json must be byte-unchanged: its cell.id is \
         assigned once per path and its cell.type says how to read its cell.db"
    );
    assert_eq!(
        counter(&leftover),
        Some(42),
        "and the state that config.json addressed is still addressable"
    );

    // And the swap did not half-happen: the source keeps its identity and its lane.
    let old_after = registry_entry(&h, "/old")
        .await
        .expect("the swap source must still be registered after the reject");
    assert_eq!(
        old_after.cell_id, old_before.cell_id,
        "a refused swap is pre-destructive — the source is not replaced"
    );
    assert!(
        registry_entry(&h, "/leftover").await.is_none(),
        "and the refused target never entered the registry"
    );

    h.shutdown().await;
    assert!(
        edge_persisted(td.path(), "/anchor", "/old"),
        "the lane still names the source, not the target that was never built"
    );
    assert!(!edge_persisted(td.path(), "/anchor", "/leftover"));
}

/// "No diagnostic" is half of what the issue reports, so the refusal has to say
/// what it found. An operator reading this must learn that there is a directory
/// in the way and where it is — not that some name collided somewhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refusal_names_the_directory_it_found() {
    let (td, h) = wired_colony().await;
    plant_unregistered_leftover(&td);

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "old"},
                "with": {"name": "leftover", "template": "persist_mock"}
            }]}
        }),
    )
    .await;
    let MutationOutcome::Rejected { details, .. } = &outcome else {
        panic!("expected a reject; got {outcome:?}");
    };
    assert!(
        details.contains("main/leftover"),
        "the refusal must name the directory it found, so the operator can look \
         at it; got {details}"
    );
    assert!(
        details.contains("swap_nodes[].with.name"),
        "and name the diff entry that aimed there; got {details}"
    );

    h.shutdown().await;
}

// ── what must keep working ──────────────────────────────────────────────────

/// The ordinary swap is untouched: a with-side target whose path is free is
/// instantiated and the source's edges swing onto it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_with_side_target_on_a_free_path_still_commits() {
    let (td, h) = wired_colony().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "old"},
                "with": {"name": "fresh", "template": "persist_mock"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a swap onto a free path must still commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/fresh").await.is_some(),
        "the new implementation is registered at its own path"
    );

    h.shutdown().await;
    assert!(
        edge_persisted(td.path(), "/anchor", "/fresh"),
        "and the source's lane swung onto it"
    );
}

/// The guard is on the with-side only. Re-taking the very directory the swap
/// was refused over stays possible without touching the filesystem by hand: an
/// `add_nodes` at an existing path is a Resume — decided on FS existence, so it
/// works for an UNregistered directory too — and it keeps the tree as it found
/// it. This is the route the refusal above points the operator at, so it is
/// pinned here rather than assumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_directory_can_still_be_taken_over_by_add_nodes() {
    let (_td, h) = wired_colony().await;
    let leftover = plant_unregistered_leftover(&_td);
    let config_before = std::fs::read_to_string(leftover.join("config.json")).unwrap();

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "leftover", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an add_nodes at an existing directory is a Resume and must commit; got {outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(leftover.join("config.json")).unwrap(),
        config_before,
        "a Resume keeps the directory as it found it — that is what makes it a \
         resume and not an instantiation"
    );
    assert_eq!(counter(&leftover), Some(42), "and the cell.db resumes");

    h.shutdown().await;
}
