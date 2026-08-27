//! GH #166 — an `add_edges` endpoint must reach a node the SAME diff creates
//! one level deeper than the scope.
//!
//! `add_nodes[].name` may be multi-segment: the containment guard resolves it
//! against the mutation scope and only refuses `..` segments and absolute
//! names. So `{"name": "talky/fetch"}` under scope `/` is the sanctioned way to
//! hang a new cell inside an existing hive without owning that hive's scope.
//! Wiring it in the same breath — `{"to": "./talky/fetch"}` — was rejected with
//! `edge_schema: to='./talky/fetch' unknown`.
//!
//! The validator kept the diff's new nodes in one namespace and looked them up
//! in another: `add_nodes[].name` went into the post-state set as written
//! (`talky/fetch`), while a multi-segment endpoint resolves against the scope
//! first and asks for `/talky/fetch`. The two never met, so an edge could
//! address a deep node that already existed and never one the diff was
//! creating. Single-segment names took the other branch and were fine, which is
//! why this survived: every everyday mutation is single-segment.
//!
//! Why one mutation instead of two: rewiring a tool lane from an old cell to a
//! new one has to choose, across two mutations, between a window where both are
//! wired (the call fans out and runs twice) and one where neither is (the call
//! dead-letters). One mutation has neither window, and that is the entire point
//! of the atomicity model.
//!
//! The tests assert the resulting graph, not the receipt: an endpoint check
//! that merely stopped rejecting would commit a dead edge and still pass an
//! outcome-only assertion.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use tokio::sync::oneshot;

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
            limit: 100,
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
/// which is what survives a restart and therefore the only proof that the edge
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

/// A colony shaped like the one in the report: a hive `/talky` that already
/// holds a cell, reached from outside by `/anchor`, plus a single-cell template
/// to hang next to it. The hive directory has to exist on disk as well as in the
/// scope set — the staged node is renamed INTO it.
///
/// The lane from `/anchor` is not decoration: without an edge crossing the
/// hive's boundary, `/talky` is disconnected and its whole subtree computes as
/// inactive, so the mutation would trip the stop-wiring guard on an unrelated
/// deactivation instead of exercising the endpoint check.
async fn colony_with_a_populated_hive() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "talky/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let tpl = td.path().join("templates").join("web-fetch-tool");
    write(&tpl, "template.json", r#"{"name":"web-fetch-tool"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/talky/split"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let factory: std::sync::Arc<dyn meclaw_colony::CellFactory> =
        std::sync::Arc::new(meclaw_testing::factories::EchoCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("echo".to_string(), factory)]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.add_hive_scope(Path::new("/")).await;
    h.add_hive_scope(Path::new("/talky")).await;
    h.spawn(Path::new("/talky/split"), || {
        EchoMockCell::new(Path::new("/talky/split"))
    })
    .await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor")).emitted_target(Path::new("/talky/split"))
    })
    .await;
    h.add_edge(
        Uuid::now_v7(),
        Path::new("/anchor"),
        Path::new("/talky/split"),
    )
    .await;
    (td, h)
}

/// The report's mutation, verbatim in shape: instantiate `talky/fetch` and wire
/// `./talky/split -> ./talky/fetch` in one diff.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_mutation_instantiates_a_deep_node_and_wires_it() {
    let (td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "talky/fetch", "template": "web-fetch-tool"}],
                "add_edges": [{
                    "from": "./talky/split", "to": "./talky/fetch",
                    "condition": "has(hop.tool_name) && hop.tool_name == 'web_fetch'"
                }]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "instantiating at depth and wiring in the same diff must commit; got {outcome:?}"
    );

    let fetch = registry_entry(&h, "/talky/fetch")
        .await
        .expect("the new cell is registered at its resolved path");
    assert!(
        fetch.active,
        "and it is wired, not an island — an edge arrived with it"
    );
    assert!(
        edge_persisted(td.path(), "/talky/split", "/talky/fetch"),
        "the lane itself must be in the persisted graph, not merely un-rejected"
    );

    h.shutdown().await;
}

/// The endpoint check must stay a check. A deep endpoint that names nothing —
/// neither pre-state nor anything this diff creates — is still `edge_schema`; a
/// fix that simply waved multi-segment endpoints through would pass the test
/// above and fail this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_endpoint_that_names_nothing_is_still_rejected() {
    let (_td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "talky/fetch", "template": "web-fetch-tool"}],
                "add_edges": [{"from": "./talky/split", "to": "./talky/typo"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(
            &outcome,
            MutationOutcome::Rejected { error_code, .. } if error_code == "edge_schema"
        ),
        "a deep endpoint nobody creates must still reject as edge_schema; got {outcome:?}"
    );

    h.shutdown().await;
}

/// The same reach, but the diff creates BOTH ends. Neither endpoint exists in
/// pre-state, so this fails on `from` as well as `to` — it pins that the fix
/// applies to both sides of the edge and not just the target.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_endpoints_may_be_born_in_the_same_diff() {
    let (td, h) = colony_with_a_populated_hive().await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [
                    {"name": "talky/fetch", "template": "web-fetch-tool"},
                    {"name": "talky/render", "template": "web-fetch-tool"}
                ],
                "add_edges": [{"from": "./talky/fetch", "to": "./talky/render"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an edge between two nodes the diff creates at depth must commit; got {outcome:?}"
    );
    assert!(edge_persisted(td.path(), "/talky/fetch", "/talky/render"));

    h.shutdown().await;
}
