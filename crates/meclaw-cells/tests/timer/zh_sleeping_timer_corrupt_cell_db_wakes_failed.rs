//! GH #1231 (T6-03, fix round 1, review I2): a cell that boots INACTIVE with an
//! unusable own `cell.db` becomes `failed` when it opens that database — on
//! wake, not at boot.
//!
//! The boot does not open a sleeping cell's database. Before this fix the
//! boot-inactive hook checked it anyway and, on an unusable file, returned
//! `None` without a word: the boot registered the inert fallback, the reconnect
//! flipped the cell active and healthy, and every delivery dead-lettered as
//! `cell_inactive` while nothing named the database. Now the reconnect's
//! respawn checks the file first (`check_own_cell_db`, the helper `spawn_cell`
//! uses for awake cells at boot), logs a WARN line and the colony registers the
//! cell `failed` — the verdict an awake cell gets at boot.

use meclaw_cells::timer::factory::TimerCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg};
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

const TIMER: &[u8] = br#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `(active, failed)` of one registry entry, read from the colony's RAM.
async fn entry(h: &ColonyHandle, path: &str) -> (bool, bool) {
    let (ack, rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: Some(Path::new(path)),
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 10,
            ack,
        })
        .await
        .expect("colony inbox");
    let reply = rx.await.expect("registry read");
    assert_eq!(reply.entries.len(), 1, "{path} is registered");
    (reply.entries[0].active, reply.entries[0].failed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sleeping_timer_with_a_corrupt_cell_db_is_failed_on_wake() {
    let td = TempDir::new().unwrap();
    // The mainland `/a -> /b` is connected; the island `/isle` is wired only
    // inside (`/isle/t -> /isle/u`), has no edge from outside, so `/isle/t`
    // derives inactive and boots asleep (the gh495 island shape).
    write(
        td.path(),
        "main/config.json",
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
    );
    write(td.path(), "main/a/config.json", TIMER);
    write(td.path(), "main/b/config.json", TIMER);
    write(
        td.path(),
        "main/isle/config.json",
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./t","to":"./u"}]}}}"#,
    );
    write(td.path(), "main/isle/t/config.json", TIMER);
    write(td.path(), "main/isle/u/config.json", TIMER);
    write(td.path(), "main/isle/t/cell.db", b"not a sqlite file");

    let factory: Arc<dyn CellFactory> = Arc::new(TimerCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("timer".to_string(), factory.clone())]);
    let mut factories = meclaw_colony::CellFactoryRegistry::new();
    factories.insert("timer".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("a sleeping cell's corrupt cell.db must not fail the boot");

    let (active, failed) = entry(&h, "/isle/t").await;
    assert!(!active, "precondition: /isle/t boots asleep");
    assert!(
        !failed,
        "the boot does not open a sleeping cell's cell.db, so it cannot judge it"
    );

    // The wake: one edge from the mainland into the island.
    let (ack, rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope":"/","ctx":{},"diff":{
                "add_edges":[{"from":"./a","to":"./isle/t"}]
            }}),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack,
        })
        .await
        .expect("colony inbox");
    let outcome = rx.await.expect("mutation ack");
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "the wake mutation commits: {outcome:?}"
    );
    h.quiesce().await;

    let (_, failed) = entry(&h, "/isle/t").await;
    assert!(
        failed,
        "a sleeping cell whose own cell.db does not open on wake is failed"
    );

    h.shutdown().await;
}
