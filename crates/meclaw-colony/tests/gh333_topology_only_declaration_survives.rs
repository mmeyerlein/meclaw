//! GH #333 — a topology-only `consumes` declaration is still a declaration.
//!
//! `consumes.topology.inbound_edges` (GH #160) is not a message compartment: it
//! gates a **spawn-time** capability, the read-only `NeighbourhoodView` a cell
//! uses to learn its own doorway from the authority instead of reading
//! `colony.db`. It therefore contributes no required key to any of the three
//! message compartments — and `CompiledConsumes::is_vacuous` counted only those.
//!
//! A cell whose whole contract is that one declaration was consequently
//! **vacuous**, and a vacuous view is dropped at spawn (`ContractView.consumes
//! = None`, `bootstrap.rs`). The gate behind it then answered "not declared" and
//! withheld the handle — no error, no dead letter, no diagnostic: exactly the
//! silent shape GH #323 had one field over. This is no #160 conflict: the
//! `NeighbourhoodView` IS the sanctioned path, the bug only withheld it.
//!
//! This file drives the real boot seam. A tree is written to disk, a real colony
//! boots it, and the factory that spawns each cell records the handle the
//! substrate hands it — the same `NeighbourhoodView::for_contract` call the
//! `vault` factory makes. Both halves are pinned:
//!
//! * the declaring cell gets a handle, and the handle **answers** — a positive
//!   receipt, the capability actually works through the live colony;
//! * a cell next to it that declares nothing gets no handle, so the gate is
//!   still a gate.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ContractView, NeighbourhoodView, RespawnFn, SpawnedCellKind,
    bootstrap_from_filesystem,
};
use meclaw_core::{Path, serde_json::Value as JsonValue};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::mpsc;

const BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// What the substrate handed one spawned cell: its path and the topology handle
/// it did (or did not) receive.
type SpawnRecord = (String, Option<NeighbourhoodView>);

/// An `echo` factory that reports the topology handle the substrate grants at
/// spawn. It performs the identical `for_contract` call the real `vault`
/// factory makes, so this test observes the production gate and not a copy of
/// its logic.
struct RecordingFactory {
    seen: mpsc::UnboundedSender<SpawnRecord>,
}

impl CellFactory for RecordingFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        EchoCellFactory.validate_params(params)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let view =
            NeighbourhoodView::for_contract(&contract, path.clone(), colony_inbox_tx.clone());
        let _ = self.seen.send((path.as_str().to_string(), view));
        Arc::new(EchoCellFactory).spawn_cell(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        Arc::new(EchoCellFactory).build_boot_inactive_respawn(
            path,
            params,
            outputs_tx,
            cell_dir,
            contract,
            colony_inbox_tx,
            idle_timeout,
            cell_timeout,
            message_timeout,
            blob_store,
            mailbox_capacity,
        )
    }
}

/// `/watcher` declares ONLY `consumes.topology.inbound_edges` — the vault's
/// declaration minus its required body keys — and `/worker`
/// declares nothing at all. One edge points at the watcher, so its
/// neighbourhood has a non-empty answer to give.
fn tree(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("main/watcher")).unwrap();
    std::fs::create_dir_all(root.join("main/worker")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./worker","to":"./watcher"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("main/watcher/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/worker"},
            "contract":{"version":"0.1.0","settings":{},
              "consumes":{"topology":{"inbound_edges":{"type":"array","required":true}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("main/worker/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/watcher"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

fn registry(factory: &Arc<dyn CellFactory>) -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert("echo".into(), factory.clone());
    r
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cell_that_declares_only_topology_is_handed_its_neighbourhood() {
    let td = tempfile::TempDir::new().unwrap();
    tree(td.path());
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel::<SpawnRecord>();
    let factory: Arc<dyn CellFactory> = Arc::new(RecordingFactory { seen: seen_tx });

    let h = ColonyHandle::new_with_factories_at(&td, vec![("echo".to_string(), factory.clone())]);
    bootstrap_from_filesystem(td.path(), &registry(&factory), &h.runtime())
        .await
        .expect("the tree boots");

    // The boot has returned, so every eager spawn it performed has already
    // reported; draining without blocking keeps a missing record a failed
    // assertion rather than a hung test.
    let mut watcher: Option<Option<NeighbourhoodView>> = None;
    let mut worker: Option<Option<NeighbourhoodView>> = None;
    while let Ok((path, view)) = seen_rx.try_recv() {
        match path.as_str() {
            "/watcher" => watcher = Some(view),
            "/worker" => worker = Some(view),
            other => panic!("unexpected cell spawned: {other}"),
        }
    }

    let view = watcher
        .expect("the declaring cell was spawned")
        .expect("a topology-only declaration must still reach the spawn gate (GH #333)");
    assert_eq!(
        view.inbound(BUDGET).await.expect("the colony answers"),
        vec![Path::new("/worker")],
        "and the handle is live: it names the edge that points at this cell"
    );

    assert!(
        worker.expect("the second cell was spawned").is_none(),
        "a cell that declares nothing still gets no handle — the gate stays a gate"
    );
    h.shutdown().await;
}
