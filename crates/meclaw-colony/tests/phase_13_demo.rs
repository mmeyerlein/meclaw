//! Phase-13 Phase-Demo (Task 13-O-1).
//!
//! Skalierungs-Beweis als POSITIVE Receipts: 50 stateful Cells, davon nur 5
//! tatsächlich gewollt awake — der Rest bleibt dormant, weil Cells dem Idle-
//! Modell folgen (`cell.timeout = 0` → Idle-Despawn, cell.db Resume bei Wake).
//!
//! Vier Receipts:
//!   1. Boot → 50 Cells `lifecycle_status == "NotYetSpawned"`.
//!   2. 5 angesprochen → 5 Awake + 45 NotYetSpawned, `counter == 1` pro Cell.
//!   3. Idle ausgewartet → 5 Asleep (Self-Despawn).
//!   4. Re-send an dieselben 5 → `counter == 2` (cell.db Resume) + 5 Awake.
//!
//! Topologie (Vorlage: `phase_13_lifecycle_demo`):
//!   td.path()/main/config.json              (hive, root)
//!   td.path()/main/cell-000/config.json     (persist_mock, idle_timeout_ms=120,
//!                                            echo_to=/sink)
//!   ... cell-001 .. cell-049 ...
//!   /sink                                   (CaptureCell, direkt via h.spawn
//!                                            VOR bootstrap registriert für
//!                                            Anti-Cascade).

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

/// Local fixture generator (per Slice-Spec: lokal, kein Hoist in meclaw-testing).
fn write_phase_13_demo_fixture(root: &std::path::Path, n: usize, idle_ms: u64) {
    // Root-Hive (assert_single_root_dir verlangt genau ein Top-Level-Verzeichnis).
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(
        main.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    for i in 0..n {
        let dir = main.join(format!("cell-{i:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            format!(
                r#"{{"cell":{{"type":"persist_mock","idle_timeout_ms":{idle_ms}}},"params":{{"echo_to":"/sink"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
            ),
        )
        .unwrap();
    }
}

/// Snapshot der ColonyRegistry via `ReadRegistry`-Inbox-Call.
async fn read_registry(
    h: &ColonyHandle,
    limit: usize,
) -> Vec<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().entries
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_demo_50_cells_lifecycle_with_db_resume() {
    let td = tempfile::TempDir::new().unwrap();
    // idle_timeout_ms = 3000ms (Test-Hygiene 2026-06-04). Das Idle-Fenster muss
    // großzügig über der Wake-Burst-Messung von RECEIPT 2/4 liegen: 5 sequenzielle
    // send+recv-Roundtrips + ReadRegistry-Inbox-Hop. Unter Workspace-Last lag der
    // alte 500ms-Wert zu knapp — die zuerst geweckte Cell despawnte vor dem
    // Snapshot (`awake==4/5` repro). Self-Despawn ist wall-clock-getrieben → eine
    // poll-up-Sync kann eine bereits despawnte Cell nicht zurückholen; das robuste
    // Knopf ist das Idle-Fenster (genuine timing behavior, dokumentiert). RECEIPT 3
    // wartet das Despawn nicht mehr per fixed-sleep aus, sondern poll-based.
    write_phase_13_demo_fixture(td.path(), 50, 3000);

    // Factory-Setup (Arc-shared: ColonyHandle + bootstrap-Registry).
    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    // /sink VOR bootstrap registrieren (Anti-Cascade: jede /cell-NNN echo_to-Emission
    // muss resolved sein, sonst landet sie im DLQ statt im Sink-Channel).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(128);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap registriert alle 50 /cell-NNN Dormant (PersistCellFactory liefert
    // SpawnedCellKind::Dormant seit 13-K-2 → handle_register_dormant landet im
    // colony_task → registry.lifecycle_status = "NotYetSpawned").
    let mut registry = CellFactoryRegistry::new();
    registry.insert("persist_mock".to_string(), persist_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");

    // A1: every /cell-NNN echoes to /sink, but the implicit identity-fallback is
    // gone — each needs an explicit catch-all out-edge to /sink, else the emission
    // dead-letters as no_route. Mirrors the fixture topology (all cells -> /sink).
    for i in 0..50 {
        h.add_edge(
            Uuid::now_v7(),
            Path::new(&format!("/cell-{i:03}")),
            Path::new("/sink"),
        )
        .await;
    }

    // ─── RECEIPT 1: 50 NotYetSpawned direkt nach Boot ────────────────────────
    // Hilfs-Closure: nur die 50 demo-Cells, /sink + /main-Hive ausblenden.
    fn is_demo_cell(path: &str) -> bool {
        path.starts_with("/cell-")
    }
    let snap1 = read_registry(&h, 100).await;
    let nys1 = snap1
        .iter()
        .filter(|e| is_demo_cell(&e.path))
        .filter(|e| e.lifecycle_status == "NotYetSpawned")
        .count();
    assert_eq!(
        nys1, 50,
        "RECEIPT 1: 50 stateful cells must boot Dormant (NotYetSpawned)"
    );

    // ─── RECEIPT 2: 5 angesprochen → 5 Awake + 45 NYS, counter==1 ────────────
    for i in 0..5 {
        let target = Path::new(&format!("/cell-{i:03}"));
        h.send(MessageBuilder::new(target).build()).await;
        let m = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("RECEIPT 2: sink recv timeout for cell-{i:03}"))
            .expect("sink channel closed");
        assert_eq!(
            m.headers.hop["counter"].as_i64().unwrap(),
            1,
            "RECEIPT 2: cell-{i:03} first wake → counter=1"
        );
    }
    let snap2 = read_registry(&h, 100).await;
    let awake2 = snap2
        .iter()
        .filter(|e| is_demo_cell(&e.path))
        .filter(|e| e.lifecycle_status == "Awake")
        .count();
    let nys2 = snap2
        .iter()
        .filter(|e| is_demo_cell(&e.path))
        .filter(|e| e.lifecycle_status == "NotYetSpawned")
        .count();
    assert_eq!(awake2, 5, "RECEIPT 2: 5 cells Awake after wake-pre-send");
    assert_eq!(nys2, 45, "RECEIPT 2: 45 cells stay NotYetSpawned");

    // ─── RECEIPT 3: Idle ausgewartet → 5 Asleep ──────────────────────────────
    // Poll-based statt fixed-sleep (Test-Hygiene 2026-06-04, Präzedenz e17803a).
    // Self-Despawn ist wall-clock-getrieben (idle_timeout_ms=3000) + WriteOp-Flush;
    // der alte fixed-sleep(700ms) war last-fragil (asleep==3/5 repro). `Asleep` ist
    // terminal bis zum nächsten Send → der Count ist monoton steigend, poll-up ist
    // race-frei. Poll bis asleep==5 ODER 8s-Backstop (Failure-Marker).
    let asleep3 = {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            let snap3 = read_registry(&h, 100).await;
            let c = snap3
                .iter()
                .filter(|e| is_demo_cell(&e.path))
                .filter(|e| e.lifecycle_status == "Asleep")
                .count();
            if c == 5 || std::time::Instant::now() >= deadline {
                break c;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    assert_eq!(asleep3, 5, "RECEIPT 3: 5 cells self-despawned to Asleep");

    // ─── RECEIPT 4: re-send → counter==2 (cell.db Resume) + 5 Awake ──────────
    for i in 0..5 {
        let target = Path::new(&format!("/cell-{i:03}"));
        h.send(MessageBuilder::new(target).build()).await;
        let m = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("RECEIPT 4: sink recv timeout for cell-{i:03}"))
            .expect("sink channel closed");
        assert_eq!(
            m.headers.hop["counter"].as_i64().unwrap(),
            2,
            "RECEIPT 4: cell-{i:03} second wake → counter=2 via cell.db Resume"
        );
    }
    let snap4 = read_registry(&h, 100).await;
    let awake4 = snap4
        .iter()
        .filter(|e| is_demo_cell(&e.path))
        .filter(|e| e.lifecycle_status == "Awake")
        .count();
    assert_eq!(awake4, 5, "RECEIPT 4: 5 cells Awake nach Resume");

    h.shutdown().await;
}
