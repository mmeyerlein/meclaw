//! Phase-13.5 Slice-6 (A7) — `colony.json` `idle_timeout_default_ms` reaches the
//! bootstrap-spawn path (Reviewer-Auflage A3, way 1 of 2).
//!
//! A stateful `persist_mock` cell with `cell.timeout: 0` and **no** per-cell
//! `idle_timeout_ms` override inherits the colony-wide default. With a
//! `colony.json` setting `idle_timeout_default_ms: 120`, the cell despawns
//! (Awake → Asleep) ~120ms after its last message. WITHOUT the A7 wiring it would
//! inherit the 60_000ms `DEFAULT_IDLE_TIMEOUT_MS` constant and stay Awake — so the
//! Asleep observation after a short sleep is the positive proof that the
//! colony.json value is applied.
//!
//! The Mutation-spawn way (`add_nodes`) is covered in
//! `phase_13_5_a7_idle_default_mutation.rs`.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg};
use meclaw_core::{MessageBuilder, Path, Uuid};
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
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{path} must be registered"))
        .lifecycle_status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_cell_inherits_colony_json_idle_default() {
    let td = tempfile::TempDir::new().unwrap();

    // colony.json sets a small idle default; absent the A7 wiring the cell would
    // use the 60_000ms constant instead.
    write(
        td.path(),
        "colony.json",
        r#"{"idle_timeout_default_ms": 120}"#,
    );
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // persist_mock: cell.timeout defaults to 0 (idle model), NO idle_timeout_ms
    // override → inherits colony.json idle_timeout_default_ms.
    write(
        td.path(),
        "main/persist/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory.clone())],
    );

    let (sink_tx, mut sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
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
    // implicit identity-fallback is gone), else the wake emission dead-letters
    // and sink_rx never receives.
    h.add_edge(Uuid::now_v7(), Path::new("/persist"), Path::new("/sink"))
        .await;

    // Wake the cell. The /sink echo is the positive wake-receipt (CaptureCell
    // receives ⟺ the cell was live/Awake) — no racy registry read against the
    // short idle window. The old `assert == "Awake"` lost a race under load when
    // the wake→read latency exceeded the 120ms idle; the old `sleep(280); assert
    // Asleep` lost the converse race when despawn ran slow. Both windows removed.
    h.send(MessageBuilder::new(Path::new("/persist")).build())
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("wake: sink recv timeout")
        .expect("sink channel closed");

    // Positive discriminator, robust under load: poll until Asleep with a
    // deadline well under the 60_000ms constant. A cell on the colony.json
    // default (120ms) sleeps within ~ms; a cell that wrongly inherited the
    // 60_000ms DEFAULT_IDLE_TIMEOUT_MS constant would NOT sleep within the
    // deadline → the poll fails loudly. No tight observation window either side.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if read_registry_status(&h, "/persist").await == "Asleep" {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "cell did not go Asleep within 30s — colony.json \
                 idle_timeout_default_ms (120) not applied (would be the 60_000ms constant)"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    h.shutdown().await;
}
