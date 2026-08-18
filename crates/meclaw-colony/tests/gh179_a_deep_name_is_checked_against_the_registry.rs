//! GH #179 — the node-identity checks must test a multi-segment diff name
//! against the paths that already exist, not against the scope's short names.
//!
//! This is the destructive half of the asymmetry GH #166 fixed on the endpoint
//! check. `validate_naming_and_match` compares every `add_nodes[].name` and
//! every `match.name` against `registry_names`, which `colony.rs` builds as
//! SHORT names whose parent path is exactly the guard scope. A name such as
//! `unit/q` matches nothing in that set — not because nothing is there, but
//! because a path was compared against a set of names. The check produced
//! silence and silence reads as "clear".
//!
//! Two consequences, both reachable from an ordinary diff (multi-segment names
//! are sanctioned: the containment guard resolves them against the scope and
//! only refuses `..` segments and absolute names):
//!
//! - **`naming_collision` never fires.** It is the check that stops an
//!   instantiation from landing on something that already exists. Skipping it
//!   lets a `swap_nodes[].with` instantiation stage a fresh template tree onto a
//!   LIVE cell's directory — the staging apply overwrites its `config.json`,
//!   which re-mints the `cell.id` that is supposed to be assigned exactly once
//!   per path, and replaces the cell's type, params and contract. The
//!   No-Delete-Policy saves the `cell.db` bytes; it does not save the identity
//!   that addressed them. The same reach spelled as a short name is refused.
//! - **`match_no_hit` always fires.** `remove_nodes[].match.name` and
//!   `swap_nodes[].match.name` can never name a node deeper than the scope, so
//!   a hive's contents cannot be disconnected or swapped from outside the hive
//!   at all. That half only refuses to act, but it has the same single cause.
//!
//! The everyday case survived because it is single-segment: a short name takes
//! the other branch and is compared correctly.
//!
//! Note what does NOT change: an `add_nodes` at a deep path whose DIRECTORY
//! exists is a Reconnect/Resume (overview Z.170-180) and must keep committing.
//! The filesystem resume-detect already resolves deep names correctly — but it
//! is a different check with a different purpose, and it is blind precisely
//! where the registry and the filesystem disagree. `naming_collision` is the
//! registry's own gate and has to hold on its own.
//!
//! The tests assert the resulting registry / graph, not the receipt.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::mocks::EchoMockCell;
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

/// Whether the persisted graph carries `from -> to` — read from `colony.db`,
/// which is what survives a restart and therefore the only proof that the lane
/// is real rather than merely un-rejected.
fn edge_persisted(db_dir: &std::path::Path, from: &str, to: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
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

/// Seed a persisted counter into a cell's own `cell.db`, standing in for any
/// state a live cell has accumulated.
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

/// A bootstrapped colony one level deeper than the mutation scope: root cell
/// `main` (logical `/`), a plain cell `/anchor`, a hive `/unit`, and a live cell
/// `/unit/q` inside it. `main` being the single root cell directory is what
/// makes the layout realistic — logical `/unit/q` maps to `{root}/main/unit/q`
/// (spec Z.331), the mapping every deep name has to survive.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
    write(td.path(), "main/unit/config.json", HIVE_CONFIG);
    write(td.path(), "main/unit/q/config.json", CELL_CONFIG);
    write(
        td.path(),
        "templates/persist_mock/template.json",
        r#"{"name":"persist_mock"}"#,
    );
    write(td.path(), "templates/persist_mock/config.json", CELL_CONFIG);

    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), factory.clone())],
    );
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), factory);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;
    (td, h)
}

// ── the destructive half ────────────────────────────────────────────────────

/// A `swap_nodes[].with` instantiation whose name reaches a LIVE deep cell must
/// be refused. The with-side is where the hazard is sharpest: unlike
/// `add_nodes`, staging has no existence-skip there, so the apply reaches the
/// live directory and overwrites its `config.json` — a fresh `cell.id` for a
/// path that already had one, and a different cell type reading the same
/// `cell.db`. `naming_collision` is the only thing that stands in front of that,
/// and for a multi-segment name it never fired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_instantiation_reaching_a_live_deep_cell_is_refused() {
    let (td, h) = bootstrapped_colony().await;
    let cell_dir = td.path().join("main/unit/q");
    seed_counter(&cell_dir, 7);
    let before = registry_entry(&h, "/unit/q")
        .await
        .expect("precondition: /unit/q is a live registered cell");
    let config_before = std::fs::read_to_string(cell_dir.join("config.json")).unwrap();

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "anchor"},
                "with": {"name": "unit/q", "template": "persist_mock"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "naming_collision"
        ),
        "instantiating onto an existing deep path must reject as naming_collision; got {outcome:?}"
    );

    let after = registry_entry(&h, "/unit/q")
        .await
        .expect("the live cell must still be registered after the reject");
    assert_eq!(
        after.cell_id, before.cell_id,
        "cell_id is assigned exactly once per path — a refused mutation must not re-mint it"
    );
    assert_eq!(
        std::fs::read_to_string(cell_dir.join("config.json")).unwrap(),
        config_before,
        "the live cell's config.json must be byte-unchanged — the reject is pre-destructive"
    );
    assert_eq!(
        counter(&cell_dir),
        Some(7),
        "and its cell.db keeps the state that config.json addressed"
    );

    h.shutdown().await;
}

/// The identity check is the REGISTRY's gate and must hold without help from
/// the filesystem. The staging resume-skip looks at the target directory, so it
/// is blind wherever registry and filesystem disagree — and there nothing else
/// stops an `add_nodes` from taking over a live cell's path: the registry entry
/// is replaced under the running cell, which keeps its mailbox and its name but
/// no longer owns either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_add_nodes_over_a_registered_deep_path_is_refused() {
    let (_td, h) = bootstrapped_colony().await;
    h.spawn(Path::new("/unit/live"), || {
        EchoMockCell::new(Path::new("/unit/live"))
    })
    .await;
    let before = registry_entry(&h, "/unit/live")
        .await
        .expect("precondition: /unit/live is registered and awake");

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "unit/live", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "naming_collision"
        ),
        "an add_nodes at a registered deep path must reject as naming_collision; got {outcome:?}"
    );

    let after = registry_entry(&h, "/unit/live")
        .await
        .expect("the live cell must still hold its path after the reject");
    assert_eq!(
        after.cell_id, before.cell_id,
        "the running cell keeps the identity registered at its path"
    );

    h.shutdown().await;
}

/// Two spellings of one path in one diff are one path. The in-diff uniqueness
/// guard (Befund 7) exists because the SECOND rename onto the same target fails
/// mid-apply — `LiveTreeMutated`, which strict-fails the whole colony task, not
/// just the mutation. Comparing the names as written lets `unit/n1` and
/// `./unit/n1` past it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_spellings_of_one_deep_path_in_one_diff_collide() {
    let (_td, h) = bootstrapped_colony().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [
                {"name": "unit/n1", "template": "persist_mock"},
                {"name": "./unit/n1", "template": "persist_mock"}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "naming_collision"
        ),
        "an in-diff duplicate must be caught before staging, whatever its spelling; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/unit/n1").await.is_none(),
        "and nothing of the refused diff reaches the registry"
    );

    h.shutdown().await;
}

// ── the mirror image ────────────────────────────────────────────────────────

/// `remove_nodes` = Disconnect: the node's edges go, its registry entry and
/// files stay. A deep `match.name` could never reach its node, so a hive's
/// contents could not be disconnected from a mutation scoped outside the hive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_remove_nodes_match_disconnects_its_node() {
    let (td, h) = bootstrapped_colony().await;
    let wired = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_edges": [{"from": "./anchor", "to": "./unit/q"}]}
        }),
    )
    .await;
    assert!(
        matches!(wired, MutationOutcome::Committed { .. }),
        "precondition: the lane into the hive is wired; got {wired:?}"
    );
    assert!(edge_persisted(td.path(), "/anchor", "/unit/q"));

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"remove_nodes": [{"match": {"name": "unit/q"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a deep remove_nodes match names a real node and must commit; got {outcome:?}"
    );
    assert!(
        !edge_persisted(td.path(), "/anchor", "/unit/q"),
        "the disconnect must have taken the incident edge out of the persisted graph"
    );
    assert!(
        registry_entry(&h, "/unit/q").await.is_some(),
        "No-Delete: disconnect leaves the registry entry in place"
    );

    h.shutdown().await;
}

/// The same reach through `swap_nodes[].match.name`: the deep node is a legal
/// swap source, and the with-side lands next to it inside the hive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_swap_nodes_match_finds_its_node() {
    let (_td, h) = bootstrapped_colony().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "unit/q"},
                "with": {"name": "unit/q2", "template": "persist_mock"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a deep swap_nodes match names a real node and must commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/unit/q2").await.is_some(),
        "the swap's with-side is registered at its resolved deep path"
    );

    h.shutdown().await;
}

// ── what the tightening must NOT take away ──────────────────────────────────

/// An `add_nodes` at a deep path whose directory exists is a Reconnect/Resume
/// (overview Z.170-180), not an instantiation: same `cell_id`, `cell.db`
/// continuity. Making the identity check see deep paths must not turn every
/// resume into a `naming_collision` — the resume targets are exactly the ones
/// that leave the collision set, at depth as at level one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_at_an_existing_deep_path_still_commits() {
    let (td, h) = bootstrapped_colony().await;
    let cell_dir = td.path().join("main/unit/q");
    seed_counter(&cell_dir, 7);
    let before = registry_entry(&h, "/unit/q").await.expect("precondition");

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "unit/q", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a deep resume must still commit; got {outcome:?}"
    );

    let after = registry_entry(&h, "/unit/q")
        .await
        .expect("still registered after the resume");
    assert_eq!(
        after.cell_id, before.cell_id,
        "a resume keeps the identity — no re-mint"
    );
    assert_eq!(
        counter(&cell_dir),
        Some(7),
        "and the cell.db resumes rather than being reseeded"
    );

    h.shutdown().await;
}

/// GH #166's case, kept green: a deep name that no node holds is an
/// instantiation and must land. The check stays a check in both directions —
/// it now sees deep paths, so it must still see when there is nothing there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_deep_instantiation_still_commits() {
    let (_td, h) = bootstrapped_colony().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "unit/fresh", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a deep name nobody holds must still instantiate; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/unit/fresh").await.is_some(),
        "and the new cell is registered at its resolved deep path"
    );

    h.shutdown().await;
}
