//! GH #199 — an `add_nodes[].name` written `./successor` is the node
//! `successor`, to the forward reference that names it too.
//!
//! #189's exact shape at a call site #189 did not touch. The existing-node form
//! of `swap_nodes[].with` may forward-reference a node an `add_nodes` of the
//! same diff is creating; the set that answers that, `add_names`, was collected
//! AS WRITTEN and then consulted as the SHORT-NAME namespace by `name_is_taken`,
//! which strips the canonical `./` prefix before comparing. So the raw string
//! `./successor` sat in a set that is only ever queried with `successor`:
//!
//! | `add_nodes[].name` | `with.name` | before |
//! |---|---|---|
//! | `successor`   | `successor`   | commits |
//! | `successor`   | `./successor` | commits |
//! | `./successor` | `successor`   | `match_no_hit` |
//! | `./successor` | `./successor` | `match_no_hit` |
//!
//! Two characters between a commit and a reject, and the reject said the target
//! was not found when the node was right there in the same diff.
//!
//! Lenient-opposite — a valid diff is refused, nothing commits wrong — which is
//! why it survived four passes over this family (#166, #179, #189, #193) while
//! its louder neighbours were fixed one at a time.
//!
//! So the test pins the rule rather than the cell of the table that was red: the
//! two spellings of one name meet in every combination, across the diff, and the
//! deep case that has always worked keeps working. Beside it the leniency that
//! must survive: a `with.name` naming nothing at all is still `match_no_hit`,
//! and cross-scope binding is unaffected.
//!
//! The assertions read `colony.db` — the swung lane is the proof the swap
//! actually happened, not merely that validation stopped objecting.

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

/// The two ways of writing one name. Every case runs over both on BOTH sides,
/// because "these two spellings decide the same way" is the whole claim and the
/// defect was exactly the mixture.
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
/// which is what survives a restart and therefore the only proof that the swap
/// swung a real lane.
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

/// Root cell `main` (logical `/`), an `/anchor` to wire from, a `/fetch` to swap
/// away, a `/spare` to swap TO without going through `add_nodes`, and a hive
/// `/unit` with a live `/unit/q` in it for the deep case.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
    write(td.path(), "main/fetch/config.json", CELL_CONFIG);
    write(td.path(), "main/spare/config.json", CELL_CONFIG);
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

/// The colony with `/anchor -> {to}` already in the graph, which is what gives
/// the swap a lane to swing and makes the outcome readable in `colony.db`.
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

// ── one name, four spellings of the same forward reference ──────────────────

/// The reported matrix. All four combinations name one node and must decide one
/// way; two of them did not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_spellings_of_one_added_name_meet_across_the_diff() {
    for added in SPELLINGS.map(|p| format!("{p}successor")) {
        for referenced in SPELLINGS.map(|p| format!("{p}successor")) {
            let (td, h) = colony_wired_to("./fetch").await;
            let outcome = send_mutation(
                &h,
                json!({
                    "scope": "/",
                    "diff": {
                        "add_nodes": [{"name": added, "template": "persist_mock"}],
                        "swap_nodes": [{"match": {"name": "fetch"}, "with": {"name": referenced}}]
                    }
                }),
            )
            .await;
            assert!(
                matches!(outcome, MutationOutcome::Committed { .. }),
                "add_nodes '{added}' + with.name '{referenced}' name one node and \
                 must commit; got {outcome:?}"
            );
            assert!(
                registry_entry(&h, "/successor").await.is_some(),
                "the added node is registered for ('{added}', '{referenced}')"
            );
            h.shutdown().await;
            assert!(
                edge_persisted(td.path(), "/anchor", "/successor"),
                "and the swap swung the lane onto it for ('{added}', '{referenced}')"
            );
            assert!(
                !edge_persisted(td.path(), "/anchor", "/fetch"),
                "leaving the replaced node disconnected for ('{added}', '{referenced}')"
            );
        }
    }
}

/// The same at depth, where the name selects the absolute-path namespace
/// instead. This half has worked since #179 — the resolved twin `add_paths` was
/// always built through `resolve_scoped_path`, which normalises the prefix — and
/// canonicalising the short half must not disturb it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_added_name_still_answers_a_deep_forward_reference() {
    for added in SPELLINGS.map(|p| format!("{p}unit/successor")) {
        for referenced in SPELLINGS.map(|p| format!("{p}unit/successor")) {
            let (td, h) = colony_wired_to("./unit/q").await;
            let outcome = send_mutation(
                &h,
                json!({
                    "scope": "/",
                    "diff": {
                        "add_nodes": [{"name": added, "template": "persist_mock"}],
                        "swap_nodes": [{"match": {"name": "unit/q"}, "with": {"name": referenced}}]
                    }
                }),
            )
            .await;
            assert!(
                matches!(outcome, MutationOutcome::Committed { .. }),
                "deep add_nodes '{added}' + with.name '{referenced}' must commit; \
                 got {outcome:?}"
            );
            h.shutdown().await;
            assert!(
                edge_persisted(td.path(), "/anchor", "/unit/successor"),
                "the lane is swung onto the deep successor for ('{added}', '{referenced}')"
            );
        }
    }
}

// ── and the leniency that must survive ──────────────────────────────────────

/// The forward-reference set must not become a set that answers everything. A
/// `with.name` in neither the pre-state nor this diff's `add_nodes` is still a
/// `match_no_hit`, which is what keeps the with-side scope-bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_with_name_naming_nothing_is_still_match_no_hit() {
    for referenced in SPELLINGS.map(|p| format!("{p}ghost")) {
        let (_td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "add_nodes": [{"name": "./successor", "template": "persist_mock"}],
                    "swap_nodes": [{"match": {"name": "fetch"}, "with": {"name": referenced}}]
                }
            }),
        )
        .await;
        assert!(
            matches!(
                outcome,
                MutationOutcome::Rejected { ref error_code, .. } if error_code == "match_no_hit"
            ),
            "a with.name naming nothing must stay match_no_hit for '{referenced}'; \
             got {outcome:?}"
        );
        h.shutdown().await;
    }
}

/// A `with.name` that names a node already in the PRE-state needs no
/// `add_nodes` at all, in either spelling — the forward-reference set is an
/// addition to the registry check, never a replacement for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pre_state_with_name_still_resolves_in_both_spellings() {
    for referenced in SPELLINGS.map(|p| format!("{p}spare")) {
        let (td, h) = colony_wired_to("./fetch").await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"swap_nodes": [{"match": {"name": "fetch"}, "with": {"name": referenced}}]}
            }),
        )
        .await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "an existing with.name '{referenced}' must commit; got {outcome:?}"
        );
        h.shutdown().await;
        assert!(
            edge_persisted(td.path(), "/anchor", "/spare"),
            "the lane is swung onto the pre-state node for '{referenced}'"
        );
    }
}
