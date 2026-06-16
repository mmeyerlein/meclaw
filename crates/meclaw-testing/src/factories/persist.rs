//! Factory for `PersistMockCell` — Phase-5 persist test topologies.
//!
//! Phase-6.5: `cell.db` Connection-Ownership lebt im `cell_task_stateful`-
//! Stack-Frame. Die Factory öffnet die Connection per Spawn (initial + jeder
//! Restart) via `open_or_create_cell_db` (M1 Resume-mit-State) und übergibt
//! sie owned an `cell_task_stateful`. Die Cell hat KEIN `conn`-Field mehr.
//!
//! Single-open path `build_cell_with_open_db`: called both by `spawn_cell`
//! (init) and the `RespawnFn` closure (post-panic). Guarantees that the cell
//! runs `overlay_from_db` on every (re-)start. T30 verifies that the
//! `RespawnFn` calls `build_cell_with_open_db` fresh each time (not once-captured).

use crate::mocks::PersistMockCell;
use meclaw_colony::{
    CellFactory, RespawnFn, SpawnedCellKind, WakeFn, build_stateful_task_with_peace,
};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Factory that instantiates `PersistMockCell` with an eager-opened cell.db.
///
/// Phase-7.5 T2: the per-cell `cell_dir` is no longer a factory field
/// (Phase-6.5 Test-Hack). The Colony passes `cell_dir` to `spawn_cell` as
/// the new substrate-level param, which then flows to
/// `build_cell_with_open_db` (init + every respawn via the RespawnFn-closure).
pub struct PersistCellFactory {
    /// Test-only counter: incremented per successful `build_cell_with_open_db` call.
    /// Both initial spawn and every respawn count. Tests poll via `wait_for_spawn_count`.
    pub spawn_count: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl PersistCellFactory {
    /// Single-open path: open cell.db via `open_or_create_cell_db` (M1 Resume) +
    /// construct `PersistMockCell` + overlay_from_db. Returns the Cell and the
    /// owned Connection separately — the caller (`spawn_cell` or the respawn
    /// closure) hands the Connection to `cell_task_stateful`.
    ///
    /// `cell_dir` is now a per-call param (Phase-7.5 T2), no longer a factory field.
    pub fn build_cell_with_open_db(
        &self,
        cell_dir: &std::path::Path,
        params: &JsonValue,
    ) -> Result<(PersistMockCell, rusqlite::Connection), String> {
        let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db"))
            .map_err(|e| e.to_string())?;
        let mut cell = PersistMockCell::from_params(params)?;
        cell.overlay_from_db(&conn).map_err(|e| e.to_string())?;
        // Increment AFTER successful build — semantics: "number of successful builds".
        self.spawn_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok((cell, conn))
    }
}

impl CellFactory for PersistCellFactory {
    /// Lazy stateful kind (Dormant) — F1-KH2 kind discriminator: registration
    /// paths may call `spawn_cell` task-free and must install the real WakeFn.
    fn is_lazy(&self) -> bool {
        true
    }

    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

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
        // Phase-13-K-2: KEIN initialer Spawn mehr — Mailbox-Paar wird an
        // RegisterDormant zurückgereicht; Status startet als NotYetSpawned.
        // Wake-Pre-Send öffnet cell.db + spawned cell_task_stateful via WakeFn.
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);

        // RespawnFn — Crash-Restart-Pfad (unverändert seit 13-E-1, allokiert
        // frischen Channel + frische Connection via build_cell_with_open_db).
        let factory = self.clone();
        let respawn_path = path.clone();
        let respawn_params = params.clone();
        let respawn_outputs = outputs_tx.clone();
        let respawn_cell_dir = cell_dir.clone();
        let respawn_inbox_tx = colony_inbox_tx.clone();
        // Phase-13.5 A8: blob_store captured per `.clone()` OUTSIDE the closure
        // (like cell_dir) → no .await in the await-free respawn corridor.
        let respawn_blob_store = blob_store.clone();
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
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
                    .expect("respawn: open + overlay");
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
                );
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
                (s, j, peace_rx, backstop_rx)
            },
        );

        // WakeFn — NotYetSpawned/Asleep → Awake (Phase-13-K-2 NEU).
        // Opens cell.db fresh via build_cell_with_open_db (M1 Resume via
        // overlay_from_db) + spawns cell_task_stateful via shared helper +
        // registers the watcher so handle_cell_died sieht das gleiche Pattern
        // wie ein RespawnFn-Lauf.
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
        let wake: WakeFn = Box::new(move |recv: mpsc::Receiver<Message>| {
            let (cell, conn) = wake_factory
                .build_cell_with_open_db(&wake_cell_dir, &wake_params)
                .expect("wake: open + overlay");
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
                );
            meclaw_colony::spawn_watcher(
                &wake_watcher_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            // Phase-13.5 Lifecycle-3b Task 7.5: return the woken task's live
            // peace-stop wiring for the colony to store in the RegistryEntry.
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

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::Path;

    /// Phase-13-K-2: factory returns `Dormant`. cell.db is opened lazily inside
    /// the WakeFn (M1 Resume via overlay_from_db). The test drives wake(receiver)
    /// to verify the lazy-open path lands the file at `cell_dir/cell.db`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn persist_factory_spawns_cell_with_cell_db_open() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory = Arc::new(PersistCellFactory {
            spawn_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });
        let (out_tx, _out_rx) = tokio::sync::mpsc::channel(8);
        let (inbox_tx, _inbox_rx) = tokio::sync::mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/a"),
                meclaw_core::serde_json::json!({}),
                out_tx,
                cell_dir.clone(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                None,
                None,
                1000,
            )
            .unwrap();
        // Dormant pre-Wake: cell.db is NOT yet open.
        assert!(
            !cell_dir.join("cell.db").exists(),
            "cell.db must not exist before Wake (Dormant pre-Wake)"
        );
        let (sender, receiver, wake) = match spawned {
            SpawnedCellKind::Dormant {
                sender,
                receiver,
                wake,
                ..
            } => (sender, receiver, wake),
            SpawnedCellKind::Active { .. } => {
                unreachable!("Phase-13-K-2: Dormant expected")
            }
        };
        wake(receiver);
        assert!(
            cell_dir.join("cell.db").exists(),
            "cell.db was opened by Wake closure"
        );
        drop(sender);
    }

    #[test]
    fn build_cell_with_open_db_returns_cell_and_conn() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory = Arc::new(PersistCellFactory {
            spawn_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });
        let (cell, _conn) = factory
            .build_cell_with_open_db(&cell_dir, &meclaw_core::serde_json::json!({}))
            .unwrap();
        assert_eq!(cell.counter, 0, "Bootstrap counter");
    }

    #[test]
    fn spawn_count_increments_on_build_cell_with_open_db() {
        use meclaw_core::serde_json::json;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory = Arc::new(PersistCellFactory {
            spawn_count: Arc::new(AtomicU32::new(0)),
        });
        let _c1 = factory
            .build_cell_with_open_db(&cell_dir, &json!({}))
            .unwrap();
        assert_eq!(factory.spawn_count.load(Ordering::Relaxed), 1);
        let _c2 = factory
            .build_cell_with_open_db(&cell_dir, &json!({}))
            .unwrap();
        assert_eq!(factory.spawn_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn build_cell_with_open_db_reloads_overlay_on_every_call() {
        use meclaw_core::serde_json::json;
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().join("a");
        std::fs::create_dir(&cell_dir).unwrap();
        let factory = Arc::new(PersistCellFactory {
            spawn_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });

        // 1. Call: cell.db wird angelegt (counter=0 default).
        let (cell1, conn1) = factory
            .build_cell_with_open_db(&cell_dir, &json!({}))
            .unwrap();
        assert_eq!(cell1.counter, 0);
        drop(cell1);
        drop(conn1); // Connection geschlossen → WAL-Lock frei.

        // 2. Extern: counter=99 in cell.db schreiben (simulated Snapshot von vorigem Cell-Run).
        {
            let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO system (slot_path, value, updated_at) VALUES ('counter', '99', 0)",
                [],
            )
            .unwrap();
        }

        // 3. 2. Call: build_cell_with_open_db lädt overlay NEU — counter=99.
        let (cell2, _conn2) = factory
            .build_cell_with_open_db(&cell_dir, &json!({}))
            .unwrap();
        assert_eq!(
            cell2.counter, 99,
            "RespawnFn-Pfad lädt cell.db jedes Mal, nicht captured-once"
        );
    }
}
