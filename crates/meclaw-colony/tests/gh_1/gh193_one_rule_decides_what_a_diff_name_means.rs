//! GH #193 — `foo` and `./foo` are one node, in every position a diff can put
//! a name in.
//!
//! `validate_edges_and_cycle` builds the post-state node set the endpoint check
//! is answered from. Three call sites write into that set or read from it, and
//! each one was fixed on its own: the deep insert (#166), the short insert
//! (#189), the endpoint lookup (always). The fourth — `nodes.remove(name)` for
//! `remove_nodes[].match.name` — kept comparing the spelling as written, so
//!
//! ```json
//! {"remove_nodes": [{"match": {"name": "./fetch"}}],
//!  "add_edges":    [{"from": "./anchor", "to": "./fetch"}]}
//! ```
//!
//! left `fetch` in the set and committed an edge onto a node the same diff was
//! disconnecting, while the identical diff spelling the name `fetch` was
//! refused. That is the LENIENT direction, which is why it outlived its
//! neighbours: it accepts what it should refuse rather than refusing what it
//! should accept, and nothing complains at the time — the edge is committed and
//! the node it names is inactive.
//!
//! So this file does not test a fourth fix. It pins the RULE the four fixes
//! agree on, in one place: `scoped_name` decides what a diff name means, and
//! every position asks it. A future reader who finds a fifth call site should
//! find this test first.
//!
//! The assertions read `colony.db`, not the receipt. An endpoint check that
//! merely stopped accepting would still have to be shown not to have written
//! the edge, and one that merely stopped rejecting would commit a dead lane and
//! pass an outcome-only assertion.

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

/// The two ways of writing one scope-local name. Every test below runs over
/// both, because "these two spellings decide the same way" is the whole claim.
const SPELLINGS: [&str; 2] = ["fetch", "./fetch"];

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

/// A root hive `main` (logical `/`) with one cell `/anchor` to wire from, and a
/// single-cell template to hang beside it. `main` being the single root cell
/// directory is what makes the layout realistic — logical `/fetch` maps to
/// `{root}/main/fetch` (spec Z.331).
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
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

/// The same colony with `/fetch` instantiated and NOT yet wired — the
/// pre-state a `remove_nodes` acts on. Deliberately unwired: the lane
/// `/anchor -> /fetch` is what the rejected diffs below try to add, so it must
/// not already be in the graph, or "the refused edge was not written" would be
/// unprovable.
async fn colony_with_a_tool_cell() -> (tempfile::TempDir, ColonyHandle) {
    let (td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "fetch", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "precondition: /fetch is instantiated; got {outcome:?}"
    );
    (td, h)
}

/// And once more with the lane in place, for the disconnect that is supposed to
/// succeed — a `remove_nodes` only has something to do if an edge names the
/// node.
async fn colony_with_a_wired_tool_cell() -> (tempfile::TempDir, ColonyHandle) {
    let (td, h) = colony_with_a_tool_cell().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_edges": [{"from": "./anchor", "to": "./fetch"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "precondition: /anchor -> /fetch is wired; got {outcome:?}"
    );
    (td, h)
}

// ── the three positions, both spellings ─────────────────────────────────────

/// Position 1 — INSERT (#189). A name the diff creates is reachable as an
/// endpoint in the same diff, whichever way it is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_the_diff_creates_is_an_endpoint_in_both_spellings() {
    for spelling in SPELLINGS {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "add_nodes": [{"name": spelling, "template": "persist_mock"}],
                    "add_edges": [{"from": "./anchor", "to": spelling}]
                }
            }),
        )
        .await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "instantiate-and-wire must commit for name '{spelling}'; got {outcome:?}"
        );
        assert!(
            edge_persisted(td.path(), "/anchor", "/fetch"),
            "and the lane must be in the persisted graph for name '{spelling}'"
        );
        h.shutdown().await;
    }
}

/// Position 2 — ENDPOINT. A node that is merely there is addressable under
/// both spellings. This half has always held; it is here because the family is
/// only pinned if all three positions are stated in one place.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_already_in_the_registry_is_an_endpoint_in_both_spellings() {
    for spelling in SPELLINGS {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "fetch", "template": "persist_mock"}]}
            }),
        )
        .await;
        assert!(matches!(outcome, MutationOutcome::Committed { .. }));

        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_edges": [{"from": "./anchor", "to": spelling}]}
            }),
        )
        .await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "wiring an existing node must commit for endpoint '{spelling}'; got {outcome:?}"
        );
        assert!(edge_persisted(td.path(), "/anchor", "/fetch"));
        h.shutdown().await;
    }
}

/// Position 3 — REMOVE (#193, the defect). A node the diff is disconnecting is
/// NOT an endpoint, whichever way its `match.name` is written. The endpoint
/// check exists to stop an edge from naming a node that will not be there; a
/// spelling must not be able to walk around it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_the_diff_removes_is_no_endpoint_in_either_spelling() {
    for spelling in SPELLINGS {
        let (td, h) = colony_with_a_tool_cell().await;
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
        assert!(
            matches!(
                &outcome,
                MutationOutcome::Rejected { error_code, .. } if error_code == "edge_schema"
            ),
            "wiring a node the same diff disconnects must reject as edge_schema \
             for name '{spelling}'; got {outcome:?}"
        );
        h.shutdown().await;
        assert!(
            !edge_persisted(td.path(), "/anchor", "/fetch"),
            "and the refused lane must not be in the persisted graph for name '{spelling}'"
        );
    }
}

/// The rule is about the NAME, not about the diff writing one spelling
/// throughout. A `remove_nodes` and an `add_edges` that spell the same node
/// differently still name one node — the mixture is the everyday case, since
/// the canonical endpoint form carries the prefix and a `match.name` usually
/// does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_spellings_meet_across_the_diff() {
    for (removed, endpoint) in [("./fetch", "fetch"), ("fetch", "./fetch")] {
        let (td, h) = colony_with_a_tool_cell().await;
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
        assert!(
            matches!(
                &outcome,
                MutationOutcome::Rejected { error_code, .. } if error_code == "edge_schema"
            ),
            "remove '{removed}' + edge to '{endpoint}' names one node and must \
             reject as edge_schema; got {outcome:?}"
        );
        h.shutdown().await;
        assert!(!edge_persisted(td.path(), "/anchor", "/fetch"));
    }
}

/// The canonicalisation must not turn removal itself into a refusal. A
/// `remove_nodes` written with the prefix and NOT contradicted by an edge is a
/// perfectly good disconnect and still commits — registry row intact, per
/// No-Delete, and the incident edge gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prefixed_remove_still_disconnects() {
    let (td, h) = colony_with_a_wired_tool_cell().await;
    let before = registry_entry(&h, "/fetch")
        .await
        .expect("precondition: /fetch is registered");

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"remove_nodes": [{"match": {"name": "./fetch"}}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a disconnect spelled with the canonical prefix must commit; got {outcome:?}"
    );

    let after = registry_entry(&h, "/fetch")
        .await
        .expect("remove_nodes disconnects, it does not delete — the row stays");
    assert_eq!(
        after.cell_id, before.cell_id,
        "and the disconnected node keeps the identity registered at its path"
    );
    assert!(!after.active, "the disconnected node is inactive");

    h.shutdown().await;
    assert!(
        !edge_persisted(td.path(), "/anchor", "/fetch"),
        "its incident edge is gone from the persisted graph"
    );
}
