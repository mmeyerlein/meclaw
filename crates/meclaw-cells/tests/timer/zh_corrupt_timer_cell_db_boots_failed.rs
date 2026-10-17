//! GH #1231 (T6-03): an eager cell whose own `cell.db` is unusable boots as a
//! `failed` cell — the boot itself goes on.
//!
//! The planner no longer opens any cell's database, so the check moved to the
//! cell's own open. For the eager types that open happened inside the build
//! closure behind `.expect("open cell.db")`: a corrupt file panicked the boot.
//! Now `spawn_cell` opens it on the error path `llm` already used and returns
//! `Err`, and the boot registers the cell `failed`.

use meclaw_cells::timer::factory::TimerCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg};
use meclaw_core::Path;
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timer_with_a_corrupt_cell_db_boots_failed() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/t/config.json",
        br#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(td.path(), "main/t/cell.db", b"not a sqlite file");

    let factory: Arc<dyn CellFactory> = Arc::new(TimerCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("timer".to_string(), factory.clone())]);
    let mut factories = meclaw_colony::CellFactoryRegistry::new();
    factories.insert("timer".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("a corrupt cell.db must not fail the boot");

    let (ack, rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: Some(Path::new("/t")),
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 10,
            ack,
        })
        .await
        .expect("colony inbox");
    let reply = rx.await.expect("registry read");
    assert_eq!(reply.entries.len(), 1, "/t is registered");
    assert!(
        reply.entries[0].failed,
        "a cell whose own cell.db does not open is failed"
    );

    h.shutdown().await;
}
