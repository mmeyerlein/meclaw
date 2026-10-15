//! GH #168 — an edge the operator already removed must not be able to fail a boot.
//!
//! `remove_nodes` disconnects a node: every edge it takes part in leaves the
//! persisted edge table. The `params.graph.edges` block of the owning hive's
//! `config.json` is written once, at instantiation, and never rewritten — so
//! after the mutation the file still declares lanes the colony no longer has.
//!
//! That disagreement is invisible until a directory goes away. Remove the
//! endpoint's directory as well (the operator wipe path — there is no delete
//! op) and the next boot planned the stale `config.json` edge, found nothing
//! to resolve its endpoint against and died on `DanglingEndpoint`. systemd
//! restarted it, it died again: a colony in a crash loop over a lane that was
//! removed on purpose, recoverable only by hand-editing the JSON.
//!
//! The fix is the read side, not a config rewrite: on a **Reboot** the
//! persisted edge table IS the topology. That is not a new rule — it is the
//! one the runtime has always followed (`colony_task` hydrates edges from
//! `colony.db` and logs "params.graph hints ignored"). Only the planner still
//! believed the file. Now both read the same authority, so a rebuild from the
//! tree is the colony that was running, and the removed lanes stay removed.

use meclaw_colony::{
    BootState, CellFactory, CellFactoryRegistry, MutationOutcome, bootstrap_from_filesystem,
    probe_boot_state, read_registry_overlay,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn echo_cell(root: &std::path::Path, rel: &str, emitted_target: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    std::fs::write(
        root.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn hive(root: &std::path::Path, rel: &str, params: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    std::fs::write(
        root.join(rel).join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    )
    .unwrap();
}

/// A cell `/a` wired into a sub-hive `/sub`, declared in the root hive's
/// `params.graph` — the shape an instantiation writes and never revisits.
fn write_topology(root: &std::path::Path) {
    hive(
        root,
        "main",
        r#"{"graph":{"edges":[{"from":"./a","to":"./sub"}]}}"#,
    );
    echo_cell(root, "main/a", "/sub");
    hive(root, "main/sub", r#"{"graph":{"edges":[]}}"#);
    echo_cell(root, "main/sub/inner", "/sub/inner");
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
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

fn edge_rows(db_path: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT from_path, to_path FROM edges ORDER BY from_path, to_path")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Boot once, disconnect `/a` with a real `remove_nodes` mutation, shut down.
/// Leaves a `colony.db` whose edge table no longer names `/a` while the root
/// hive's `config.json` still declares the lane.
async fn boot_once_and_disconnect(td: &tempfile::TempDir) {
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the first boot of a healthy tree must succeed");
    let outcome = send_mutation(
        &h,
        json!({"scope": "/", "diff": {"remove_nodes": [{"match": {"name": "a"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_nodes must commit for this test to mean anything, got {outcome:?}"
    );
    h.shutdown().await;

    assert!(
        edge_rows(&td.path().join("colony.db")).is_empty(),
        "the disconnect must have taken the lane out of the edge table"
    );
    assert!(
        std::fs::read_to_string(td.path().join("main/config.json"))
            .unwrap()
            .contains("./sub"),
        "and the hive's config.json must still declare it — that disagreement IS the defect"
    );
}

/// The crash loop, end to end: disconnect, wipe the endpoint's directory,
/// boot again. The receipt is positive on both sides — the colony comes up
/// AND the lane stays removed, so "it boots" cannot be reached by quietly
/// re-adopting the edge the file still declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_lane_whose_endpoint_was_wiped_does_not_fail_the_next_boot() {
    let td = tempfile::TempDir::new().unwrap();
    boot_once_and_disconnect(&td).await;

    // The operator wipe: the disconnected subtree's directory goes away. There
    // is no delete op, so this is the only way to get rid of it.
    std::fs::remove_dir_all(td.path().join("main/sub")).unwrap();

    assert_eq!(
        probe_boot_state(&td.path().join("colony.db")).unwrap(),
        BootState::Reboot,
        "boot 2 must classify as a Reboot for this test to mean anything"
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let report = bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime()).await;
    let report = report.expect(
        "an edge the operator already removed must not be able to fail the boot \
         (before the fix: BootstrapErrors{DanglingEndpoint})",
    );
    h.shutdown().await;

    assert_eq!(
        report.edge_count, 0,
        "the reboot runs on the persisted edge table, which has no edges left"
    );
    assert!(
        edge_rows(&td.path().join("colony.db")).is_empty(),
        "and the removed lane must not come back from the config.json"
    );
}

/// The plan-level half, asserted where the decision is made: on a Reboot the
/// planned edge set is the persisted one, even though the file on disk still
/// declares a lane. The fix is a change of authority, not a config rewrite —
/// the `config.json` is deliberately left untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_persisted_edge_table_is_what_a_reboot_plans() {
    let td = tempfile::TempDir::new().unwrap();
    boot_once_and_disconnect(&td).await;

    let db_path = td.path().join("colony.db");
    let overlay = read_registry_overlay(&db_path).unwrap();
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &overlay,
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("planning a disconnected-but-intact tree must succeed");

    assert!(
        plan.edges.is_empty(),
        "a Reboot plans the edge table, not the file; got {:?}",
        plan.edges
            .iter()
            .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
            .collect::<Vec<_>>()
    );
    assert!(
        std::fs::read_to_string(td.path().join("main/config.json"))
            .unwrap()
            .contains("./sub"),
        "the seed on disk is untouched — the colony reads it, it does not rewrite it"
    );
}

/// The counter-pin: on a **FirstBoot** the file IS the source. Nothing about
/// the seeding path may move — a fresh tree still gets exactly the lanes it
/// declares, because there is no committed topology to prefer over them.
#[test]
fn a_first_boot_still_plans_the_edges_the_file_declares() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &Default::default(),
        BootState::FirstBoot,
        None,
    )
    .expect("a fresh tree must plan");

    assert_eq!(plan.edges.len(), 1, "the declared lane is the first boot's");
    assert_eq!(plan.edges[0].from.as_str(), "/a");
    assert_eq!(plan.edges[0].to.as_str(), "/sub");
}
