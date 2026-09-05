//! Phase-13.5-A6 factory for `EmitOnceMockCell`. Stateful spawn pattern
//! analogous to `PersistCellFactory` and `MultiUpdateMockCellFactory`, but
//! without a `cell.db` (in-memory only; the rusqlite connection is opened out of
//! substrate convenience but ignored by the cell).
//!
//! The config is set at construction time (`initial_target`, `initial_content`,
//! `capture_tx`) — `validate_params` and `spawn_cell` ignore the `raw` JSON.
//! The factory is `pub` for the A6 demo tests.

use crate::mocks::EmitOnceMockCell;
use meclaw_colony::{
    CellFactory, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
};
use meclaw_core::serde_json::Value;
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Factory that instantiates `EmitOnceMockCell` with the configured initial
/// emit + capture channel. Stateful (returns `Dormant` per Phase-13-K-2).
pub struct EmitOnceMockCellFactory {
    initial_target: Path,
    initial_content: Value,
    capture_tx: mpsc::Sender<Message>,
}

impl EmitOnceMockCellFactory {
    /// Construct a factory with the EmitOnceMockCell config.
    pub fn new(
        initial_target: Path,
        initial_content: Value,
        capture_tx: mpsc::Sender<Message>,
    ) -> Self {
        Self {
            initial_target,
            initial_content,
            capture_tx,
        }
    }
}

impl CellFactory for EmitOnceMockCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    fn validate_params(&self, _raw: &JsonValue) -> Result<(), String> {
        // The A6 mock ignores params — the config lives in the factory field.
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _raw: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Phase-13-K-2: NO initial spawn — the mailbox pair goes to
        // RegisterDormant; WakeFn/RespawnFn build cell + task on demand.
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // Shared build-from-receiver closure: open cell.db (substrate convenience),
        // construct cell + DbConn::wrap + build_stateful_task_with_peace.
        let path_for_build = path.clone();
        let initial_target = self.initial_target.clone();
        let initial_content = self.initial_content.clone();
        let capture_tx = self.capture_tx.clone();
        let outputs_tx_for_build = outputs_tx.clone();
        let cell_dir_for_build = cell_dir.clone();
        let inbox_tx_for_build = colony_inbox_tx.clone();
        // Phase-13.5 A8: blob_store captured by the build closure (moved in,
        // cloned per build) → no .await in the await-free respawn corridor.
        let blob_store_for_build = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views, captured for
        // every build (initial wake + respawn converge by construction).
        let consumes_for_build = contract.consumes.clone();
        let bounds_for_build = contract.transfer_bounds();
        type BuildFromRecv = Arc<
            dyn Fn(
                    mpsc::Receiver<Message>,
                ) -> (
                    JoinHandle<()>,
                    tokio::sync::oneshot::Receiver<()>,
                    tokio::sync::oneshot::Sender<()>,
                    tokio::sync::oneshot::Receiver<()>,
                    tokio::sync::oneshot::Receiver<()>,
                ) + Send
                + Sync,
        >;
        let build_from_recv: BuildFromRecv = Arc::new(move |recv: mpsc::Receiver<Message>| {
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&cell_dir_for_build.join("cell.db"))
                    .expect("cell.db open_or_create failed");
            let db = meclaw_colony::DbConn::wrap(conn, None);
            let cell = EmitOnceMockCell::new(
                initial_target.clone(),
                initial_content.clone(),
                capture_tx.clone(),
            );
            build_stateful_task_with_peace(
                path_for_build.clone(),
                recv,
                outputs_tx_for_build.clone(),
                inbox_tx_for_build.clone(),
                idle_timeout,
                message_timeout,
                cell_timeout,
                cell,
                db,
                blob_store_for_build.clone(),
                consumes_for_build.clone(),
                bounds_for_build.clone(),
            )
        });

        // RespawnFn: frischer Channel + build_from_recv.
        let respawn_arc = build_from_recv.clone();
        let respawn_path = path.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (s, r) = mpsc::channel::<Message>(respawn_mailbox_capacity);
                let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = (respawn_arc)(r);
                // Phase-13.5 Slice 4 T6: re-notify the colony of the fresh stop
                // pair (the frozen RespawnFn 3-tuple cannot return it). try_send,
                // never await — this closure runs in the await-free respawn
                // corridor (see `renotify_stop_wiring`).
                meclaw_colony::renotify_stop_wiring(
                    &respawn_inbox_tx,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (s, join, peace_rx, backstop_rx)
            },
        );

        // WakeFn: caller-supplied receiver + build_from_recv + spawn_watcher.
        let wake_arc = build_from_recv;
        let wake_path = path.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = (wake_arc)(recv);
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            // Phase-13.5 Lifecycle-3b Task 7.5: return live peace-stop wiring.
            (stop_tx, death_ack_rx)
        });

        // Phase-13.5 Lifecycle-3b Task 3: placeholder peace-stop ends for the
        // Dormant (lazy-wake) variant. The task is spawned later in WakeFn, so
        // these ends are inert until Task 4 wires the lazy-wake peace-stop into
        // the registry; the colony drops them in Task 3.
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}
