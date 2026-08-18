//! Phase-13 step 13-K-2 bootstrap switch:
//! stateful factories now return `SpawnedCellKind::Dormant`, the bootstrap apply
//! sends `ColonyMsg::RegisterDormant` → `lifecycle_status == "NotYetSpawned"`.
//! Stateless factories stay `SpawnedCellKind::Active` → `"Awake"`.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::{EchoCellFactory, PersistCellFactory};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn factories_with_persist_and_echo() -> CellFactoryRegistry {
    let mut r: CellFactoryRegistry = CellFactoryRegistry::new();
    let echo: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    let persist: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    r.insert("echo".into(), echo);
    r.insert("persist".into(), persist);
    r
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_stateful_starts_not_yet_spawned() {
    let td = tempfile::TempDir::new().unwrap();
    // `<td>/main` is the single top-level dir (assert_single_root_dir),
    // mapped to `/` by plan_bootstrap. `<td>/main/persist/` → `/persist`.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Stateful PersistMockCell with high idle_timeout — must NOT auto-sleep
    // during the test window. `terminal: true` keeps it from emitting outputs.
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let h = ColonyHandle::new();
    let report =
        bootstrap_from_filesystem(td.path(), &factories_with_persist_and_echo(), &h.runtime())
            .await
            .expect("bootstrap_from_filesystem must succeed");
    assert_eq!(report.cell_count, 1);

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
    let reply = ack_rx.await.unwrap();
    let e = reply
        .entries
        .into_iter()
        .find(|e| e.path == "/persist")
        .expect("/persist must be registered");
    assert_eq!(
        e.lifecycle_status, "NotYetSpawned",
        "stateful cell must boot Dormant (NotYetSpawned)"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_stateless_stays_awake() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Stateless EchoCellFactory under /echo. Echo target stays unresolved
    // (no /sink registered) — that's fine; the cell never receives a message
    // during the assertion window, so no DLQ side-effects matter.
    write(
        td.path(),
        "main/echo/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let h = ColonyHandle::new();
    let report =
        bootstrap_from_filesystem(td.path(), &factories_with_persist_and_echo(), &h.runtime())
            .await
            .expect("bootstrap_from_filesystem must succeed");
    assert_eq!(report.cell_count, 1);

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
    let reply = ack_rx.await.unwrap();
    let e = reply
        .entries
        .into_iter()
        .find(|e| e.path == "/echo")
        .expect("/echo must be registered");
    assert_eq!(
        e.lifecycle_status, "Awake",
        "stateless cell must stay Awake after bootstrap"
    );
    h.shutdown().await;
}
