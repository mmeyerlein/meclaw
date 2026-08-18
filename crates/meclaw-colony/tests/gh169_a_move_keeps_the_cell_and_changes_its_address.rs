//! GH #169 — a cell has to be able to change its address without losing what
//! it is.
//!
//! A path is a cell's identity, so until now there was no operation that
//! changed one. Tidying a capability into the hive it belongs to meant
//! `add_nodes` at the new path, `add_edges` for every edge the old node had,
//! `remove_nodes` on the old one, and an operator wipe outside the mutation
//! flow — in practice two mutations, because an edge could not address a node
//! the same diff creates (that half is #166).
//!
//! What that costs is not convenience. The new node gets a new `cell_id`, a new
//! `instantiated_at` and a fresh empty `cell.db`, while the old one sits
//! orphaned beside it: for anything with state that is not a move, it is a
//! replacement with amnesia. Every condition and modifier is re-typed by hand
//! at the new address, which is exactly where a typo silently changes routing.
//! And between the two mutations the lane is either wired twice (the call fans
//! out and runs twice) or not at all (it dead-letters).
//!
//! `move_nodes` is one committed mutation with none of that:
//!
//! ```json
//! {"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}
//! ```
//!
//! It is deliberately NOT `swap_nodes`. A swap swings the edges of one
//! implementation onto a DIFFERENT one, with its own identity and its own
//! `cell.db`. A move is the opposite: the same cell, a different address.
//!
//! The tests assert `colony.db` and the filesystem, not the receipt. A move
//! that merely stopped being rejected would leave the cell where it was and
//! still pass an outcome-only assertion.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    BootState, CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome,
    bootstrap_from_filesystem, probe_boot_state,
};
use meclaw_core::{Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

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
    ack_rx.await.unwrap();
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

fn open_colony_db(root: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

/// The persisted registry row for `path`, as `(cell_id, created_at,
/// instantiated_at)` — the three columns a relocation must carry across
/// untouched. `colony.db` rather than the in-RAM registry, because the row is
/// what a reboot rebuilds the colony from.
fn registry_row(root: &std::path::Path, path: &str) -> Option<(String, i64, Option<i64>)> {
    open_colony_db(root)
        .query_row(
            "SELECT cell_id, created_at, instantiated_at FROM registry WHERE path = ?1",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
}

/// Every persisted edge naming `path` at either end, as
/// `(from, to, condition, modifier)`.
fn edges_touching(
    root: &std::path::Path,
    path: &str,
) -> Vec<(String, String, Option<String>, Option<String>)> {
    let conn = open_colony_db(root);
    let mut stmt = conn
        .prepare(
            "SELECT from_path, to_path, condition, modifier FROM edges \
             WHERE from_path = ?1 OR to_path = ?1 ORDER BY from_path, to_path",
        )
        .unwrap();
    let rows = stmt
        .query_map([path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// Seed a persisted counter into a cell's own `cell.db`, standing in for any
/// state a live cell has accumulated. A move that re-instantiated instead of
/// relocating would arrive without it.
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

/// A colony shaped like the report: a root hive `main` (logical `/`), a cell
/// `/anchor` that wires into things, and an empty hive `/talky` that the tool
/// cell is supposed to move into. `main` being the single root cell directory
/// is what makes the layout realistic — logical `/talky/fetch` maps to
/// `{root}/main/talky/fetch` (spec Z.331), the mapping a relocation has to
/// survive on both sides.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
    write(td.path(), "main/talky/config.json", HIVE_CONFIG);
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
    (td, h)
}

/// Instantiate `/fetch` from a template and wire `/anchor -> /fetch` with a
/// condition and a modifier, so the move has an identity, a provenance stamp,
/// state and a non-trivial edge to carry.
async fn colony_with_a_wired_tool_cell() -> (tempfile::TempDir, ColonyHandle) {
    let (td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "fetch", "template": "persist_mock"}],
                "add_edges": [{
                    "from": "./anchor",
                    "to": "./fetch",
                    "condition": "hop.tier == 'gold'",
                    "modifier": {"set_hop": {"lane": "'tools'"}}
                }]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "precondition: the tool cell is instantiated and wired; got {outcome:?}"
    );
    seed_counter(&td.path().join("main/fetch"), 7);
    (td, h)
}

// ── what a move carries ─────────────────────────────────────────────────────

/// The whole point, in one mutation: same `cell_id`, same `instantiated_at`,
/// the `cell.db` at the new address with its contents, the incident edge
/// re-pointed with its condition and modifier intact, and nothing left at the
/// old path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_keeps_the_identity_the_state_and_the_edges() {
    let (td, h) = colony_with_a_wired_tool_cell().await;
    let before = registry_entry(&h, "/fetch")
        .await
        .expect("precondition: /fetch is registered");
    let (row_cell_id, created_at, instantiated_at) =
        registry_row(td.path(), "/fetch").expect("precondition: /fetch has a persisted row");
    assert!(
        instantiated_at.is_some(),
        "precondition: an instantiated node carries its provenance stamp"
    );

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a move into an existing hive must commit; got {outcome:?}"
    );

    // Identity: the row moved, it was not re-minted next door.
    let after = registry_entry(&h, "/talky/fetch")
        .await
        .expect("the cell is registered at its new address");
    assert_eq!(
        after.cell_id, before.cell_id,
        "a move changes the address, not the identity — cell_id must survive"
    );
    assert!(
        registry_entry(&h, "/fetch").await.is_none(),
        "and the old address must be gone from the registry, not doubled"
    );
    let moved =
        registry_row(td.path(), "/talky/fetch").expect("the persisted row is at the new path");
    assert_eq!(
        moved,
        (row_cell_id, created_at, instantiated_at),
        "cell_id, created_at and instantiated_at ride along — the row moved, \
         a fresh instantiation would have re-stamped all three"
    );
    assert!(
        registry_row(td.path(), "/fetch").is_none(),
        "one cell, one row: the old path must not survive as a second row"
    );

    // State: the directory was renamed, so `cell.db` came with it.
    assert_eq!(
        counter(&td.path().join("main/talky/fetch")),
        Some(7),
        "the cell.db arrived at the new path with its contents"
    );
    assert!(
        !td.path().join("main/fetch").exists(),
        "and nothing is left behind at the old path — a relocation, not a copy"
    );

    // Edges: re-pointed, condition and modifier untouched.
    assert!(
        edges_touching(td.path(), "/fetch").is_empty(),
        "no edge may still name the address the cell left"
    );
    let incident = edges_touching(td.path(), "/talky/fetch");
    assert_eq!(
        incident.len(),
        1,
        "exactly the one edge the cell had, at its new address; got {incident:?}"
    );
    let (from, to, condition, modifier) = &incident[0];
    assert_eq!((from.as_str(), to.as_str()), ("/anchor", "/talky/fetch"));
    assert_eq!(
        condition.as_deref(),
        Some("hop.tier == 'gold'"),
        "the condition is carried, not re-typed"
    );
    assert!(
        modifier.as_deref().unwrap_or_default().contains("tools"),
        "and so is the modifier; got {modifier:?}"
    );

    h.shutdown().await;
}

/// The move has to survive the colony that made it. A reboot rebuilds from the
/// persisted registry and edge table (#168), so a relocation that moved only
/// some of what keys on a path — the directory but not the row, the row but not
/// the edges — shows up here as a boot that does not come back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_colony_boots_again_from_what_the_move_persisted() {
    let (td, h) = colony_with_a_wired_tool_cell().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    h.shutdown().await;

    assert_eq!(
        probe_boot_state(&td.path().join("colony.db")).unwrap(),
        BootState::Reboot,
        "boot 2 must classify as a Reboot for this test to mean anything"
    );
    let h2 = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory())],
    );
    bootstrap_from_filesystem(td.path(), &persist_registry(), &h2.runtime())
        .await
        .expect("the colony must boot from the state its own move committed");
    assert!(
        registry_entry(&h2, "/talky/fetch").await.is_some(),
        "and it comes back with the cell at its new address"
    );
    h2.shutdown().await;
}

// ── what a move refuses ─────────────────────────────────────────────────────

/// The target must be free. Landing a move on an occupied path would do to the
/// occupant exactly what #179 stopped a `swap_nodes[].with` from doing: take
/// over a live cell's directory and the identity that addressed its `cell.db`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_onto_an_occupied_path_is_refused() {
    let (td, h) = colony_with_a_wired_tool_cell().await;
    let occupant = registry_entry(&h, "/anchor")
        .await
        .expect("precondition: /anchor is a live cell");

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "fetch"}, "to": "anchor"}]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "naming_collision"
        ),
        "a move onto an occupied path must reject as naming_collision; got {outcome:?}"
    );
    assert_eq!(
        registry_entry(&h, "/anchor").await.map(|e| e.cell_id),
        Some(occupant.cell_id),
        "the occupant keeps its identity — the reject is pre-destructive"
    );
    assert!(
        td.path().join("main/fetch").exists(),
        "and the cell that would have moved has not moved"
    );

    h.shutdown().await;
}

/// A move obeys the same containment rule as every other diff path: the target
/// is a name relative to the mutation scope, and a scope cannot write outside
/// itself. Otherwise a mutation scoped to a hive could relocate a cell into a
/// hive it has no authority over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_out_of_the_mutation_scope_is_refused() {
    let (td, h) = colony_with_a_wired_tool_cell().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/talky",
            "diff": {"move_nodes": [{"match": {"name": "../fetch"}, "to": "../escaped"}]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "scope_out_of_bounds"
        ),
        "a target outside the mutation scope must reject as scope_out_of_bounds; got {outcome:?}"
    );
    assert!(
        td.path().join("main/fetch").exists(),
        "nothing moved — containment is checked before anything is touched"
    );

    h.shutdown().await;
}

/// `match.name` has to name a node that exists, like `remove_nodes` and
/// `swap_nodes` do. A move of nothing is a typo, and a typo that commits is a
/// mutation that reports success for work it did not do.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_of_a_node_that_does_not_exist_is_refused() {
    let (_td, h) = colony_with_a_wired_tool_cell().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "ghost"}, "to": "talky/ghost"}]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "match_no_hit"
        ),
        "a move whose source names nothing must reject as match_no_hit; got {outcome:?}"
    );

    h.shutdown().await;
}

/// Moving a hive means moving everything under it — every child's registry row,
/// every subtree-internal edge, the hive scope itself. This first version does
/// not do that, and says so instead of doing half of it: a half-moved hive
/// leaves its children addressed under a path that no longer exists, which is
/// the boot failure #168 is about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_of_a_hive_is_refused_rather_than_done_by_halves() {
    let (td, h) = colony_with_a_wired_tool_cell().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "talky"}, "to": "moved_talky"}]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, details, .. }
                if error_code == "schema" && details.contains("hive")
        ),
        "moving a hive must be refused explicitly, naming the reason; got {outcome:?}"
    );
    assert!(
        td.path().join("main/talky").exists(),
        "and the hive stays exactly where it was"
    );

    h.shutdown().await;
}
