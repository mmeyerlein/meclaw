//! Phase-13 End-to-End-Lifecycle-Demo (Task 13-K-3).
//!
//! POSITIVE RECEIPT across all four status transitions of a stateful cell:
//!   1. NotYetSpawned (directly after boot, before any wake-pre-send).
//!   2. Awake (1st wake → cell.db Created, counter=1).
//!   3. Asleep (idle despawn after `idle_timeout_ms` elapses).
//!   4. Awake (2nd wake → cell.db Resumed via overlay_from_db, counter=2).
//!
//! The counter rises monotonically 0→1→2 — proof of the cell.db resume via the
//! `OpenStatus::Resumed` path (build_cell_with_open_db loads `system.counter`
//! on the second wake from the cell.db written in between).
//!
//! Topology (modelled on `phase_7_5_demo::demo_production_bootstrap_spawn_with_restart`):
//!   td.path()/main/config.json            (hive, root)
//!   td.path()/main/persist/config.json    (persist_mock, idle_timeout_ms=120,
//!                                          echo_to=/sink, without panic_after)
//!   /sink                                 (CaptureCell, registered directly via
//!                                          h.spawn BEFORE bootstrap for
//!                                          anti-cascade)

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg};
use meclaw_core::{Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::PersistCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn read_registry_status(h: &ColonyHandle, path: &str) -> String {
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
    reply
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{path} must be registered"))
        .lifecycle_status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lifecycle_full_cycle_with_db_resume() {
    let td = tempfile::TempDir::new().unwrap();

    // FS tree: root hive + stateful persist_mock with:
    //   - idle_timeout_ms = 120ms (small, keeps the test deterministic)
    //   - cell.timeout = 0 (idle model, default)
    //   - echo_to = /sink (output lands at the sink, no self-loop)
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":120},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // Factory-Setup (Arc-shared: ColonyHandle + bootstrap-Registry).
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // Register /sink BEFORE bootstrap (anti-cascade: /persist's echo_to must be
    // resolvable, otherwise the cell output emission lands in the DLQ).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap registriert /persist Dormant (PersistCellFactory liefert
    // SpawnedCellKind::Dormant seit 13-K-2 → handle_register_dormant landet im
    // colony_task → registry.lifecycle_status = "NotYetSpawned").
    let mut registry = CellFactoryRegistry::new();
    registry.insert("persist_mock".to_string(), persist_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");

    // A1: /persist's echo to /sink needs an explicit catch-all out-edge — the
    // implicit identity-fallback is gone. add_edge only touches the edge table,
    // not connectivity, so /persist stays NotYetSpawned for RECEIPT 1.
    h.add_edge(Uuid::now_v7(), Path::new("/persist"), Path::new("/sink"))
        .await;

    // RECEIPT 1: NotYetSpawned directly after boot.
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "NotYetSpawned",
        "RECEIPT 1: stateful cell must boot Dormant (NotYetSpawned)"
    );

    // RECEIPT 2: 1. Wake → cell.db Created, counter=1, Status=Awake.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    let m1 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("RECEIPT 2: sink recv timeout (1. Wake)")
        .expect("sink channel closed");
    assert_eq!(
        m1.headers.hop["counter"].as_i64().unwrap(),
        1,
        "RECEIPT 2: first wake, fresh cell.db, counter=1"
    );
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Awake",
        "RECEIPT 2: status=Awake after the first wake pre-send"
    );

    // RECEIPT 3: idle elapsed → status=Asleep.
    // 120ms idle_timeout + buffer for the sleep arm + WriteOp flush → 260ms.
    tokio::time::sleep(Duration::from_millis(260)).await;
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Asleep",
        "RECEIPT 3: status=Asleep after the idle despawn"
    );

    // RECEIPT 4: 2nd wake → cell.db Resumed (overlay_from_db loads counter=1),
    // handle increments to 2, status=Awake.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    let m2 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("RECEIPT 4: sink recv timeout (2. Wake)")
        .expect("sink channel closed");
    assert_eq!(
        m2.headers.hop["counter"].as_i64().unwrap(),
        2,
        "RECEIPT 4: second wake, cell.db Resumed → counter persisted across sleep"
    );
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Awake",
        "RECEIPT 4: status=Awake after the second wake pre-send"
    );

    h.shutdown().await;
}
