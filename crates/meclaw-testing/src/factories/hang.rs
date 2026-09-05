//! Factory for `HangMockCell` — Paket-3 message_timeout B-backstop demos.
//!
//! Structurally identical to `PersistCellFactory` (Phase-7.5 Dormant stateful
//! pattern): the cell boots `Dormant`, opens `cell.db` lazily in the `WakeFn`,
//! and restarts via the `RespawnFn` closure. Both paths route through
//! `build_stateful_task_with_peace`, which carries the `message_timeout`
//! B-backstop into `cell_task_stateful`.
//!
//! `spawn_count` is the restart-observable signal: it increments on every
//! successful build (initial Wake + every Respawn). A backstop firing ends the
//! task → `CellDied{death_kind: Backstop}` → `handle_cell_died` → RespawnFn → a
//! fresh build → `spawn_count` bumps. Tests poll `wait_for_spawn_count(2)` as a
//! positive "the cell restarted" receipt.

use crate::mocks::HangMockCell;
use meclaw_colony::{
    CellFactory, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Factory that instantiates `HangMockCell` with an eager-opened cell.db.
pub struct HangCellFactory {
    /// Test-only counter: incremented per successful build (initial Wake +
    /// every Respawn). Tests poll it via `wait_for_spawn_count`.
    pub spawn_count: Arc<std::sync::atomic::AtomicU32>,
}

impl HangCellFactory {
    /// Open `cell.db` (M1 Resume; the cell ignores it) + construct the cell.
    /// Returns the cell and the owned Connection; the caller hands the
    /// Connection to `cell_task_stateful`. Increments `spawn_count` after a
    /// successful build.
    pub fn build_cell_with_open_db(
        &self,
        cell_dir: &std::path::Path,
        params: &JsonValue,
    ) -> Result<(HangMockCell, rusqlite::Connection), String> {
        let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db"))
            .map_err(|e| e.to_string())?;
        let cell = HangMockCell::from_params(params)?;
        self.spawn_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok((cell, conn))
    }
}

impl CellFactory for HangCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
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
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // RespawnFn — crash/backstop-restart path (captures clones outside the
        // closure → the `handle_cell_died` corridor stays await-free).
        let factory = self.clone();
        let respawn_path = path.clone();
        let respawn_params = params.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_cell_dir = cell_dir.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        let respawn_blob_store = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn_bounds = contract.transfer_bounds();
        let respawn_mailbox_capacity = mailbox_capacity;
        let respawn: RespawnFn = Box::new(
            move || -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                let (cell, conn) = factory
                    .build_cell_with_open_db(&respawn_cell_dir, &respawn_params)
                    .expect("respawn: open cell.db");
                let db = meclaw_colony::DbConn::wrap(conn, None);
                let (s, r) = mpsc::channel::<Message>(respawn_mailbox_capacity);
                let (j, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateful_task_with_peace(
                    respawn_path.clone(),
                    r,
                    respawn_outputs.clone(),
                    respawn_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    respawn_blob_store.clone(),
                    respawn_consumes.clone(),
                    respawn_bounds.clone(),
                );
                meclaw_colony::renotify_stop_wiring(
                    &respawn_inbox_tx,
                    respawn_path.clone(),
                    stop_tx,
                    death_ack_rx,
                );
                (s, j, peace_rx, backstop_rx)
            },
        );

        // WakeFn — NotYetSpawned/Asleep → Awake. Opens cell.db fresh + spawns
        // cell_task_stateful via the shared helper + registers the watcher.
        let wake_factory = self.clone();
        let wake_path = path.clone();
        let wake_params = params.clone();
        let wake_outputs = outputs_tx.clone();
        let wake_cell_dir = cell_dir.clone();
        let wake_inbox_tx = colony_inbox_tx.clone();
        let wake_watcher_inbox = colony_inbox_tx.clone();
        let wake_blob_store = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let wake_consumes = contract.consumes.clone();
        let wake_bounds = contract.transfer_bounds();
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let (cell, conn) = wake_factory
                .build_cell_with_open_db(&wake_cell_dir, &wake_params)
                .expect("wake: open cell.db");
            let db = meclaw_colony::DbConn::wrap(conn, None);
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) =
                build_stateful_task_with_peace(
                    wake_path.clone(),
                    recv,
                    wake_outputs.clone(),
                    wake_inbox_tx.clone(),
                    idle_timeout,
                    message_timeout,
                    cell_timeout,
                    cell,
                    db,
                    wake_blob_store.clone(),
                    wake_consumes.clone(),
                    wake_bounds.clone(),
                );
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            (stop_tx, death_ack_rx)
        });

        // Inert placeholder peace-stop ends for the Dormant variant (the live
        // task is spawned later in WakeFn; the colony drops these).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn build_cell_increments_spawn_count_and_opens_db() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory = Arc::new(HangCellFactory {
            spawn_count: Arc::new(AtomicU32::new(0)),
        });
        let (_cell, _conn) = factory
            .build_cell_with_open_db(
                &cell_dir,
                &meclaw_core::serde_json::json!({"sleep_ms": 5, "emitted_target": "/sink"}),
            )
            .unwrap();
        assert_eq!(factory.spawn_count.load(Ordering::Relaxed), 1);
        assert!(cell_dir.join("cell.db").exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_cell_returns_dormant() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory = Arc::new(HangCellFactory {
            spawn_count: Arc::new(AtomicU32::new(0)),
        });
        let (out_tx, _out_rx) = mpsc::channel(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/a"),
                meclaw_core::serde_json::json!({"hang_forever": true}),
                out_tx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                Some(std::time::Duration::from_millis(200)),
                None,
                1000,
            )
            .unwrap();
        assert!(matches!(spawned, SpawnedCellKind::Dormant { .. }));
        // Dormant pre-Wake: cell.db not yet opened.
        assert!(!cell_dir.join("cell.db").exists());
    }
}
