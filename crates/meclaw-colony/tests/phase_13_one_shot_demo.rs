//! Phase-13 one-shot demo (task 13-M-1).
//!
//! POSITIVE RECEIPT for `cell.timeout > 0` (one-shot model):
//!   1. NotYetSpawned directly after boot.
//!   2. 1st wake → counter=1 → cell despawned AFTER `handle()` returns → Asleep.
//!   3. 2nd wake → counter=2 via cell.db resume → despawned again → Asleep.
//!
//! Proof line: `cell_task_stateful` must NOT return into the mailbox loop when
//! `cell.timeout > 0` — instead it fires peace and sends `ColonyMsg::Sleep`
//! directly from the recv arm (analogous to the idle arm).

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
async fn one_shot_cell_despawns_after_each_handle_to_asleep_counter_increments() {
    let td = tempfile::TempDir::new().unwrap();

    // FS tree: root hive + stateful persist_mock with `cell.timeout = 1000`
    // (one-shot model — the value does not matter as long as it is > 0). The
    // bootstrap apply maps that to `idle_timeout = None` (see 13-K-2).
    // Output → /sink.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist_mock","timeout":1000},"params":{"emitted_target":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // Factory-Setup.
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // /sink VOR bootstrap registrieren (Anti-Cascade).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    registry.insert("persist_mock".to_string(), persist_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");

    // A1: /persist's echo to /sink needs an explicit catch-all out-edge (the
    // implicit identity-fallback is gone). add_edge does not touch connectivity,
    // so /persist stays NotYetSpawned for RECEIPT 1.
    h.add_edge(Uuid::now_v7(), Path::new("/persist"), Path::new("/sink"))
        .await;

    // RECEIPT 1: NotYetSpawned directly after boot.
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "NotYetSpawned",
        "RECEIPT 1: stateful cell must boot Dormant (NotYetSpawned)"
    );

    // RECEIPT 2: Wake 1 → counter=1 → One-Shot-Despawn → Asleep.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    let m1 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("RECEIPT 2: sink recv timeout (1. Wake)")
        .expect("sink channel closed");
    assert_eq!(
        m1.headers.hop["counter"].as_i64().unwrap(),
        1,
        "RECEIPT 2: first wake, counter=1"
    );
    // Despawn-Wait: peace + Sleep-Send + colony-Verarbeitung.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Asleep",
        "RECEIPT 2: status=Asleep after the one-shot despawn"
    );

    // RECEIPT 3: Wake 2 → counter=2 (cell.db Resumed) → One-Shot-Despawn → Asleep.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    let m2 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("RECEIPT 3: sink recv timeout (2. Wake)")
        .expect("sink channel closed");
    assert_eq!(
        m2.headers.hop["counter"].as_i64().unwrap(),
        2,
        "RECEIPT 3: second wake, counter=2 via cell.db Resume"
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Asleep",
        "RECEIPT 3: status=Asleep after the second one-shot despawn"
    );

    h.shutdown().await;
}
