//! GH #194 — the post-state node view a diff's edges are checked against is ONE
//! view, and every entry that takes a path out of it reaches both spellings.
//!
//! `validate_edges_and_cycle` answers "will this endpoint be there?" from two
//! sets: `nodes`, the scope's short names, and `deep_endpoint_paths`, the
//! absolute paths of everything that exists at any depth. GH #193 taught the
//! `remove_nodes` loop to canonicalise through `scoped_name`, so a node on its
//! way out leaves `nodes` however its `match.name` was spelled. It still left
//! the other half untouched: `deep_endpoint_paths` arrives as the PRE-state and
//! nothing was ever subtracted from it. For a node that already existed at
//! depth, that is exactly the set the endpoint check consults — so
//!
//! ```json
//! {"remove_nodes": [{"match": {"name": "unit/q"}}],
//!  "add_edges":    [{"from": "./anchor", "to": "./unit/q"}]}
//! ```
//!
//! committed, and `/anchor -> /unit/q` sat in `colony.db` pointing at a node the
//! same mutation had disconnected. Newly reachable since #179 made a deep
//! `match.name` hit at all; before that the removal simply did nothing.
//!
//! And the same question asked of the two operations written while the deep set
//! was assumed immutable answers the same way, because both also change what a
//! path means:
//!
//! - a `swap_nodes[].match.name` names the node the swap is REPLACING. Its edges
//!   are swung onto the target and it is left disconnected — but it stayed in
//!   the view in both spellings, so an `add_edges` in the same diff wired a lane
//!   onto it that the swing (which runs over the pre-diff edges) never touched.
//! - a `move_nodes[].match.name` names an address the mutation VACATES. The
//!   directory is gone by `rename(2)` and the registry row re-addressed, and an
//!   `add_edges` naming the old address committed a lane onto a path with
//!   nothing at it.
//!
//! So this file does not pin a fix per operation. It pins the rule the whole
//! family shares — a node on its way out is no endpoint, whichever operation is
//! taking it out and however deep it lives — and it pins the leniency next to
//! it: each of the three still commits when no edge contradicts it, and a deep
//! node nothing touches is still perfectly addressable.
//!
//! The assertions read `colony.db`. A check that merely stopped rejecting would
//! commit a dead lane and pass an outcome-only assertion.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

/// The two ways of writing one deep name. Every deep case runs over both,
/// because "these two spellings decide the same way" is half the claim.
const DEEP_SPELLINGS: [&str; 2] = ["unit/q", "./unit/q"];
/// And the level-one pair, because the swap and move halves of the defect were
/// never deep-only — the view was not subtracted in EITHER spelling.
const SHORT_SPELLINGS: [&str; 2] = ["fetch", "./fetch"];

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
/// which is what survives a restart and therefore the only proof that a lane is
/// real rather than merely un-rejected.
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

/// A bootstrapped colony that has both depths: root cell `main` (logical `/`),
/// a plain cell `/anchor` to wire from, a level-one cell `/fetch`, a hive
/// `/unit` and a live cell `/unit/q` inside it. `main` being the single root
/// cell directory is what makes the layout realistic — logical `/unit/q` maps
/// to `{root}/main/unit/q` (spec Z.331), the mapping every deep name survives
/// or does not.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
    write(td.path(), "main/fetch/config.json", CELL_CONFIG);
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

/// The same colony with `/anchor -> {to}` already in the graph, which is what
/// gives a disconnect, a swap and a move something to do.
async fn colony_wired_to(to: &str) -> (tempfile::TempDir, ColonyHandle) {
    let (td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({"scope": "/", "diff": {"add_edges": [{"from": "./anchor", "to": to}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "precondition: /anchor -> {to} is wired; got {outcome:?}"
    );
    (td, h)
}

fn assert_edge_schema(outcome: &MutationOutcome, what: &str) {
    assert!(
        matches!(
            outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "edge_schema"
        ),
        "{what} must reject as edge_schema; got {outcome:?}"
    );
}

// ── a node on its way out is no endpoint, whoever is taking it out ──────────

/// `remove_nodes` at depth — the defect as reported. The endpoint check exists
/// to stop an edge from naming a node that will not be there; for a node that
/// already existed at depth it was answered from a set the removal never
/// reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_name_the_diff_removes_is_no_endpoint_in_either_spelling() {
    for spelling in DEEP_SPELLINGS {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "remove_nodes": [{"match": {"name": spelling}}],
                    "add_edges": [{"from": "./anchor", "to": spelling}]
                }
            }),
        )
        .await;
        assert_edge_schema(
            &outcome,
            &format!("wiring a deep node the same diff disconnects ('{spelling}')"),
        );
        h.shutdown().await;
        assert!(
            !edge_persisted(td.path(), "/anchor", "/unit/q"),
            "and the refused lane must not be in the persisted graph for '{spelling}'"
        );
    }
}

/// The rule is about the NAME, not about the diff writing one spelling
/// throughout — the mixture is the everyday case, since a `match.name` usually
/// carries no prefix and the canonical endpoint form does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_deep_spellings_meet_across_the_diff() {
    for (removed, endpoint) in [("unit/q", "./unit/q"), ("./unit/q", "unit/q")] {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "remove_nodes": [{"match": {"name": removed}}],
                    "add_edges": [{"from": "./anchor", "to": endpoint}]
                }
            }),
        )
        .await;
        assert_edge_schema(
            &outcome,
            &format!("remove '{removed}' + edge to '{endpoint}' names one node and"),
        );
        h.shutdown().await;
        assert!(!edge_persisted(td.path(), "/anchor", "/unit/q"));
    }
}

/// `swap_nodes[].match.name` names the node being REPLACED. Its edges are swung
/// onto the target, which runs over the edges that were there before the diff —
/// so a lane the same diff adds onto the replaced node is not swung with them
/// and is left pointing at a disconnected cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_a_swap_replaces_is_no_endpoint() {
    for (spelling, persisted) in [
        (SHORT_SPELLINGS[0], "/fetch"),
        (SHORT_SPELLINGS[1], "/fetch"),
        (DEEP_SPELLINGS[0], "/unit/q"),
        (DEEP_SPELLINGS[1], "/unit/q"),
    ] {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "swap_nodes": [{
                        "match": {"name": spelling},
                        "with": {"name": "replacement", "template": "persist_mock"}
                    }],
                    "add_edges": [{"from": "./anchor", "to": spelling}]
                }
            }),
        )
        .await;
        assert_edge_schema(
            &outcome,
            &format!("wiring the node a swap in the same diff replaces ('{spelling}')"),
        );
        h.shutdown().await;
        assert!(
            !edge_persisted(td.path(), "/anchor", persisted),
            "and the refused lane must not be in the persisted graph for '{spelling}'"
        );
    }
}

/// `move_nodes[].match.name` names an address the mutation VACATES: the
/// directory leaves by `rename(2)` and the registry row is re-addressed. An
/// edge naming the old address afterwards points at nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_address_a_move_vacates_is_no_endpoint() {
    for (spelling, target, persisted) in [
        (SHORT_SPELLINGS[0], "unit/fetch", "/fetch"),
        (SHORT_SPELLINGS[1], "unit/fetch", "/fetch"),
        (DEEP_SPELLINGS[0], "spare", "/unit/q"),
        (DEEP_SPELLINGS[1], "spare", "/unit/q"),
    ] {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "move_nodes": [{"match": {"name": spelling}, "to": target}],
                    "add_edges": [{"from": "./anchor", "to": spelling}]
                }
            }),
        )
        .await;
        assert_edge_schema(
            &outcome,
            &format!("wiring the address a move in the same diff vacates ('{spelling}')"),
        );
        h.shutdown().await;
        assert!(
            !edge_persisted(td.path(), "/anchor", persisted),
            "and the refused lane must not be in the persisted graph for '{spelling}'"
        );
    }
}

// ── and the leniency that must survive ──────────────────────────────────────

/// The subtraction must not turn a deep node into an unaddressable one. A node
/// nothing in the diff touches is wired exactly as before, in both spellings —
/// this is the half #166/#179 made work and it stays working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_node_nothing_touches_is_still_an_endpoint() {
    for spelling in DEEP_SPELLINGS {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({"scope": "/", "diff": {"add_edges": [{"from": "./anchor", "to": spelling}]}}),
        )
        .await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "wiring an untouched deep node must commit for '{spelling}'; got {outcome:?}"
        );
        h.shutdown().await;
        assert!(edge_persisted(td.path(), "/anchor", "/unit/q"));
    }
}

/// A deep disconnect that no edge contradicts is a perfectly good disconnect:
/// it commits, the incident lane goes, and the registry row stays — No-Delete
/// keeps the identity registered at the path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_remove_still_disconnects() {
    let (td, h) = colony_wired_to("./unit/q").await;
    let before = registry_entry(&h, "/unit/q")
        .await
        .expect("precondition: /unit/q is registered");

    let outcome = send_mutation(
        &h,
        json!({"scope": "/", "diff": {"remove_nodes": [{"match": {"name": "unit/q"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a deep disconnect must commit; got {outcome:?}"
    );

    let after = registry_entry(&h, "/unit/q")
        .await
        .expect("remove_nodes disconnects, it does not delete — the row stays");
    assert_eq!(
        after.cell_id, before.cell_id,
        "and the disconnected node keeps the identity registered at its path"
    );
    assert!(!after.active, "the disconnected node is inactive");

    h.shutdown().await;
    assert!(
        !edge_persisted(td.path(), "/anchor", "/unit/q"),
        "its incident lane is gone from the persisted graph"
    );
}

/// A swap no edge contradicts still swaps, and the swing still moves the
/// pre-existing lane onto the replacement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swap_no_edge_contradicts_still_swings_its_lanes() {
    let (td, h) = colony_wired_to("./fetch").await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "fetch"},
                "with": {"name": "replacement", "template": "persist_mock"}
            }]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a swap must still commit; got {outcome:?}"
    );
    h.shutdown().await;
    assert!(
        edge_persisted(td.path(), "/anchor", "/replacement"),
        "the pre-existing lane is swung onto the replacement"
    );
    assert!(
        !edge_persisted(td.path(), "/anchor", "/fetch"),
        "and no longer names the node that was replaced"
    );
}

/// A move no edge contradicts still moves, and every lane naming the old
/// address names the new one afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_no_edge_contradicts_still_carries_its_lanes() {
    let (td, h) = colony_wired_to("./fetch").await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a move must still commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/unit/fetch").await.is_some(),
        "and the cell is registered at its new address"
    );
    h.shutdown().await;
    assert!(
        edge_persisted(td.path(), "/anchor", "/unit/fetch"),
        "the lane names the new address"
    );
    assert!(
        !edge_persisted(td.path(), "/anchor", "/fetch"),
        "and no longer names the vacated one"
    );
}
