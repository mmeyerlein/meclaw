//! Phase-13 Persistent-Demo (Task 13-N-1).
//!
//! POSITIVES RECEIPT für `cell.timeout = -1` (Persistent-Modell):
//!   1. NotYetSpawned direkt nach Boot.
//!   2. 1. Wake → counter=1 → Status=Awake (kein One-Shot-Despawn).
//!   3. Lange Wartezeit (500ms) → Status BLEIBT Awake (kein Idle-Despawn).
//!   4. 2. Msg → counter=2 ohne Re-Wake (gleiche dauerhafte Cell-Task-Instanz).
//!
//! Beweis-Linie: Bootstrap-Apply (13-K-2) mapped `cell.timeout = -1` auf
//! `idle_timeout = None` (`match c.cell_timeout { 0 => Some(...), _ => None }`).
//! `cell_task_stateful` (13-M-1) hat `if cell_timeout > 0` als One-Shot-Gate
//! — negative Werte fallen NICHT in den `> 0`-Branch. Damit ist sowohl
//! Idle- als auch One-Shot-Despawn baulich unterbunden; der Test verifiziert
//! das emergente Persistent-Verhalten.
//!
//! Topologie (Vorlage: `phase_13_lifecycle_demo`):
//!   td.path()/main/config.json            (hive, root)
//!   td.path()/main/persist/config.json    (persist_mock, cell.timeout = -1,
//!                                          echo_to=/sink)
//!   /sink                                 (CaptureCell, vor bootstrap
//!                                          registriert für Anti-Cascade)

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
async fn persistent_stateful_cell_never_sleeps_after_wake() {
    let td = tempfile::TempDir::new().unwrap();

    // FS-Tree: root-hive + stateful persist_mock mit `cell.timeout = -1`
    // (Persistent-Modell). Bootstrap-Apply mapped das auf
    // `idle_timeout = None` (siehe 13-K-2). Output → /sink.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist_mock","timeout":-1},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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

    // RECEIPT 1: NotYetSpawned direkt nach Boot.
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "NotYetSpawned",
        "RECEIPT 1: persistent stateful cell must boot Dormant (NotYetSpawned)"
    );

    // RECEIPT 2: 1. Wake → counter=1 → Status=Awake (kein One-Shot-Despawn,
    // weil cell_timeout = -1 NICHT in den `> 0`-Branch fällt).
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
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Awake",
        "RECEIPT 2: status=Awake nach erstem Wake (kein One-Shot-Despawn)"
    );

    // RECEIPT 3: Lange Wartezeit — persistent MUSS Awake bleiben (kein Idle-
    // Despawn, weil idle_timeout = None → idle_fut = pending()). 500ms ist
    // großzügig oberhalb jedes plausiblen Idle-Timers.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Awake",
        "RECEIPT 3: persistent cell must NOT sleep after long idle"
    );

    // RECEIPT 4: 2. Msg → counter=2 auf der gleichen, dauerhaft laufenden
    // cell_task_stateful-Instanz (kein Re-Wake, kein cell.db-Resume-Pfad).
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    let m2 = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("RECEIPT 4: sink recv timeout (2. Msg)")
        .expect("sink channel closed");
    assert_eq!(
        m2.headers.hop["counter"].as_i64().unwrap(),
        2,
        "RECEIPT 4: second msg, counter=2 on same long-running task"
    );
    assert_eq!(
        read_registry_status(&h, "/persist").await,
        "Awake",
        "RECEIPT 4: status=Awake bleibt nach zweiter Msg"
    );

    h.shutdown().await;
}
