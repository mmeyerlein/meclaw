//! GH #198 — the post-state node view a diff's edges are checked against is ONE
//! view, and every entry that puts a node AT a path reaches both spellings.
//!
//! The opposite half of #194. That issue fixed what a diff takes OUT of the
//! view; this is what it failed to put IN. `swap_nodes[].with.name` (the
//! instantiate form) and `move_nodes[].to` are addresses the diff CREATES, and
//! neither was ever added to the view, so
//!
//! ```json
//! {"move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}],
//!  "add_edges":  [{"from": "./anchor", "to": "./unit/fetch"}]}
//! ```
//!
//! was refused with `edge_schema: to='./unit/fetch' unknown` — the endpoint
//! naming a node the same diff was putting there.
//!
//! This is #166 on the two operations #166 did not cover, and it undercut the
//! argument both were built on. #166 exists so an `add_edges` may point at a
//! node arriving in the same diff, because splitting into two mutations means
//! choosing between a window where a lane is wired twice and one where it is not
//! wired at all. #169 shipped `move_nodes` to be the operation with no such
//! window — and then a caller who wanted to give the relocated cell one extra
//! lane in the same breath could not.
//!
//! So this file does not pin a fix per operation either. It pins the rule the
//! whole family shares — a node this diff puts at a path IS an endpoint, in both
//! spellings, whichever operation puts it there — beside the two boundaries that
//! must survive it: the vacated address is still no endpoint (#194, the same
//! diff, the other direction), and the existing-node form of `swap_nodes[].with`
//! is still a reference and not a claim (#195).
//!
//! The assertions read `colony.db`. A check that merely stopped rejecting would
//! prove nothing about the lane actually being there.

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

/// The two ways of writing one endpoint. Every case runs over both, because
/// "these two spellings decide the same way" is half the claim — that is what
/// #189 and #193 cost, one call site at a time.
const SPELLINGS: [&str; 2] = ["", "./"];

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

/// A bootstrapped colony that has both depths: root cell `main` (logical `/`),
/// a plain cell `/anchor` to wire from, a level-one cell `/fetch`, a hive
/// `/unit` and a live cell `/unit/q` inside it. `main` being the single root
/// cell directory is what makes the layout realistic — logical `/unit/q` maps
/// to `{root}/main/unit/q` (spec Z.331).
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

fn assert_committed(outcome: &MutationOutcome, what: &str) {
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{what} must commit; got {outcome:?}"
    );
}

// ── a node this diff puts at a path IS an endpoint ──────────────────────────

/// The reported case: a relocation and one extra lane onto the relocated cell,
/// in one committed mutation. This is the entire argument `move_nodes` was
/// built on — one mutation, no window where the graph is half-wired — and it
/// was the one thing the operation could not do.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_target_is_an_endpoint_for_the_same_diff() {
    for to in SPELLINGS.map(|p| format!("{p}unit/fetch")) {
        for endpoint in SPELLINGS.map(|p| format!("{p}unit/fetch")) {
            let (td, h) = bootstrapped_colony().await;
            let outcome = send_mutation(
                &h,
                json!({
                    "scope": "/",
                    "diff": {
                        "move_nodes": [{"match": {"name": "fetch"}, "to": to}],
                        "add_edges": [{"from": "./anchor", "to": endpoint}]
                    }
                }),
            )
            .await;
            assert_committed(
                &outcome,
                &format!("move to '{to}' + a lane onto '{endpoint}'"),
            );
            assert!(
                registry_entry(&h, "/unit/fetch").await.is_some(),
                "the cell is registered at its new address"
            );
            h.shutdown().await;
            assert!(
                edge_persisted(td.path(), "/anchor", "/unit/fetch"),
                "and the lane onto the new address is in the persisted graph \
                 (to='{to}', endpoint='{endpoint}')"
            );
        }
    }
}

/// The move target need not be deep — a relocation out of a hive up to level one
/// lands in the SHORT namespace, which is the other half of the view and the
/// half #194's `vacate` had to reach separately.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_move_target_is_an_endpoint_for_the_same_diff() {
    for endpoint in SPELLINGS.map(|p| format!("{p}spare")) {
        let (td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "move_nodes": [{"match": {"name": "unit/q"}, "to": "spare"}],
                    "add_edges": [{"from": "./anchor", "to": endpoint}]
                }
            }),
        )
        .await;
        assert_committed(
            &outcome,
            &format!("move up to '/spare' + a lane onto '{endpoint}'"),
        );
        h.shutdown().await;
        assert!(
            edge_persisted(td.path(), "/anchor", "/spare"),
            "the lane onto the new short address is persisted for '{endpoint}'"
        );
    }
}

/// The rule is about the endpoint, not about the lane's direction — the view is
/// consulted identically for `from` and `to`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_target_is_an_endpoint_on_the_from_side_too() {
    let (td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}],
                "add_edges": [{"from": "./unit/fetch", "to": "./anchor"}]
            }
        }),
    )
    .await;
    assert_committed(&outcome, "a lane OUT of the relocated cell");
    h.shutdown().await;
    assert!(edge_persisted(td.path(), "/unit/fetch", "/anchor"));
}

/// The instantiate form of `swap_nodes[].with` stages a fresh template tree at
/// `with.name` — the same act of creation an `add_nodes` performs, which #166
/// made addressable. Wiring an extra lane onto the replacement is the swap's
/// version of the move's problem: the swing carries the lanes that were there
/// BEFORE the diff, so a new one has to be expressible in the same diff or not
/// at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swap_instantiate_target_is_an_endpoint_for_the_same_diff() {
    for with_name in SPELLINGS.map(|p| format!("{p}replacement")) {
        for endpoint in SPELLINGS.map(|p| format!("{p}replacement")) {
            let (td, h) = bootstrapped_colony().await;
            let outcome = send_mutation(
                &h,
                json!({
                    "scope": "/",
                    "diff": {
                        "swap_nodes": [{
                            "match": {"name": "fetch"},
                            "with": {"name": with_name, "template": "persist_mock"}
                        }],
                        "add_edges": [{"from": "./anchor", "to": endpoint}]
                    }
                }),
            )
            .await;
            assert_committed(
                &outcome,
                &format!("swap in '{with_name}' + a lane onto '{endpoint}'"),
            );
            h.shutdown().await;
            assert!(
                edge_persisted(td.path(), "/anchor", "/replacement"),
                "the lane onto the swapped-in node is persisted \
                 (with.name='{with_name}', endpoint='{endpoint}')"
            );
        }
    }
}

/// And at depth, where the endpoint is answered from the absolute-path half of
/// the view rather than the short-name one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_swap_instantiate_target_is_an_endpoint_for_the_same_diff() {
    let (td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "swap_nodes": [{
                    "match": {"name": "unit/q"},
                    "with": {"name": "unit/r", "template": "persist_mock"}
                }],
                "add_edges": [{"from": "./anchor", "to": "./unit/r"}]
            }
        }),
    )
    .await;
    assert_committed(&outcome, "a deep swap-in + a lane onto it");
    h.shutdown().await;
    assert!(edge_persisted(td.path(), "/anchor", "/unit/r"));
}

/// One diff, both directions of the same view: the address the move vacates and
/// the address it creates, asked at once. This is the pair the two issues split
/// between them, and the point of building the view from one enumeration is
/// that it can answer both without either half drifting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_move_vacates_one_address_and_creates_another() {
    let (td, h) = bootstrapped_colony().await;
    let refused = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}],
                "add_edges": [{"from": "./anchor", "to": "./fetch"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(
            refused,
            MutationOutcome::Rejected { ref error_code, .. } if error_code == "edge_schema"
        ),
        "the address the move VACATES is still no endpoint (#194); got {refused:?}"
    );

    let committed = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}],
                "add_edges": [{"from": "./anchor", "to": "./unit/fetch"}]
            }
        }),
    )
    .await;
    assert_committed(&committed, "and the address it CREATES is one");
    h.shutdown().await;
    assert!(!edge_persisted(td.path(), "/anchor", "/fetch"));
    assert!(edge_persisted(td.path(), "/anchor", "/unit/fetch"));
}

// ── and the boundaries that must survive ────────────────────────────────────

/// The view gains what the diff creates and nothing else. An endpoint no entry
/// of the diff puts anywhere is still unknown — widening the view until every
/// name resolves would retire the check rather than complete it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_endpoint_the_diff_creates_nowhere_is_still_unknown() {
    let (td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "move_nodes": [{"match": {"name": "fetch"}, "to": "unit/fetch"}],
                "add_edges": [{"from": "./anchor", "to": "./unit/ghost"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(
            outcome,
            MutationOutcome::Rejected { ref error_code, .. } if error_code == "edge_schema"
        ),
        "an endpoint nothing creates must still reject; got {outcome:?}"
    );
    h.shutdown().await;
    assert!(!edge_persisted(td.path(), "/anchor", "/unit/ghost"));
}

/// The existing-node form of `swap_nodes[].with` REFERENCES a node instead of
/// creating one (#195 pinned it as not a claim). It contributes nothing to the
/// view either — the node it names is already in the pre-state, or an
/// `add_nodes` of the same diff is putting it there. A `with.name` that names
/// neither is still a `match_no_hit`, not a self-satisfying endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_form_with_name_creates_nothing_to_wire() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "swap_nodes": [{"match": {"name": "fetch"}, "with": {"name": "ghost"}}],
                "add_edges": [{"from": "./anchor", "to": "./ghost"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(
            outcome,
            MutationOutcome::Rejected { ref error_code, .. } if error_code == "match_no_hit"
        ),
        "an existing-form with.name naming nothing must stay match_no_hit; got {outcome:?}"
    );
    h.shutdown().await;
}
