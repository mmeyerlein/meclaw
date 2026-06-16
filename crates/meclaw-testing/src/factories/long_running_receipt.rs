//! Factory for `ReceiptMockLongRunningCell` — Phase-10 RespawnFn-Round-Trip
//! tests. Implements `CellFactory` for `ColonyHandle::register_spawned`
//! compatibility (Phase-5-Q8/Q9-Harness). Shared Recorders via `Arc<...>`
//! survive RespawnFn rebuild (Phase-5-Lesson). Latest `inject_tx` lives
//! in `Arc<Mutex<Option<Sender>>>` so tests can fetch the post-restart
//! sender for fresh-I/O-Task event-injection.
//!
//! **Cell.db: in-memory per rebuild** — bewusst gewählt, da der Mock keine
//! persistente State-Wahrheit prüft. Echte Long-Running-Cells in 10-B/C/D
//! öffnen via `open_or_create_cell_db_with_status` (Phase-9-Pattern) und
//! erleben Resume-mit-State über den Restart.

use crate::mocks::{MockEvent, ReceiptMockLongRunningCell};
use meclaw_colony::{CellFactory, DbConn, RespawnFn, SpawnedCellKind, cell_task_long_running};
use meclaw_core::{CellEmission, JsonValue, Message, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

/// Factory used by `phase_10_a_round_trip.rs` to drive
/// `cell_task_long_running` through the real `handle_cell_died`
/// corridor without touching `colony.rs`.
pub struct LongRunningReceiptFactory {
    /// Test-only counter: incremented per successful spawn (initial + every restart).
    pub spawn_count: Arc<AtomicU32>,
    /// Counts `handle()` invocations across all instances (shared Arc survives rebuild).
    pub handle_calls: Arc<AtomicUsize>,
    /// Counts `handle_event()` invocations across all instances (shared Arc survives rebuild).
    pub event_calls: Arc<AtomicUsize>,
    latest_inject: Arc<Mutex<Option<mpsc::Sender<MockEvent>>>>,
    panic_after_first_handle: bool,
    /// B4 backstop demo: sleep this many ms inside each `handle()` call.
    pub sleep_in_handle_ms: u64,
    /// B4 backstop demo: emit a UBF receipt to this path after the optional
    /// sleep. `None` → no emission (default, preserves existing behaviour).
    pub echo_to: Option<meclaw_core::Path>,
}

impl Default for LongRunningReceiptFactory {
    fn default() -> Self {
        Self {
            spawn_count: Arc::new(AtomicU32::new(0)),
            handle_calls: Arc::new(AtomicUsize::new(0)),
            event_calls: Arc::new(AtomicUsize::new(0)),
            latest_inject: Arc::new(Mutex::new(None)),
            panic_after_first_handle: false,
            sleep_in_handle_ms: 0,
            echo_to: None,
        }
    }
}

impl LongRunningReceiptFactory {
    /// Construct a factory whose **first** cell panics on the first
    /// `handle()` call. Post-restart cell instances are healthy.
    pub fn new_with_panic_after_first_handle() -> Self {
        Self {
            panic_after_first_handle: true,
            ..Default::default()
        }
    }

    /// Construct a factory that sleeps `ms` ms inside each `handle()` call
    /// and emits a UBF receipt to `echo_to` (B4 backstop demo).
    pub fn new_with_sleep(ms: u64, echo_to: meclaw_core::Path) -> Self {
        Self {
            sleep_in_handle_ms: ms,
            echo_to: Some(echo_to),
            ..Default::default()
        }
    }

    /// Snapshot the current event-inject sender. Tests call this AFTER
    /// the restart barrier (`wait_for_spawn_count(2)`) to fetch the
    /// fresh I/O-Task's inject channel — the pre-restart sender points
    /// to a dropped channel.
    pub fn latest_inject_tx(&self) -> Option<mpsc::Sender<MockEvent>> {
        self.latest_inject.try_lock().ok().and_then(|g| g.clone())
    }
}

impl CellFactory for LongRunningReceiptFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        // Phase-13.5 Lifecycle-3b Task 4: the INITIAL spawn wires a LIVE
        // peace-stop pair (`stop_rx` into the task, `stop_tx` returned in
        // `Active`) + a `death_ack` pair (`death_ack_tx` into the task's
        // `TermAckGuard`, `death_ack_rx` returned) + `colony_inbox_tx` so a
        // colony-initiated stop returns the mailbox via `ColonyMsg::Stopped`.
        // The RespawnFn (crash-restart) re-spawns with INERT ends — restart-side
        // stop re-wiring is out of Task-4 scope (the colony's stored stop_tx is
        // single-use at disconnect, which precedes any crash here).
        let (stop_tx, initial_stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (initial_death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();

        // The stop pair is passed DIRECTLY into the initial `build(...)` call
        // (no Mutex/Option::take); the `RespawnFn` closure calls
        // `build(None, None)` → restart is always inert.
        let build = {
            let factory = self.clone();
            let path = path.clone();
            let outputs_tx = outputs_tx.clone();
            let colony_inbox_tx = colony_inbox_tx.clone();
            // Phase-13.5 A8: blob_store captured by the build closure (cloned
            // per build) → no .await in the await-free respawn corridor.
            let blob_store = blob_store.clone();
            // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
            let consumes = contract.consumes.clone();
            move |stop_rx: Option<tokio::sync::oneshot::Receiver<()>>,
                  death_ack: Option<tokio::sync::oneshot::Sender<()>>|
                  -> (
                mpsc::Sender<Message>,
                JoinHandle<()>,
                tokio::sync::oneshot::Receiver<()>,
                tokio::sync::oneshot::Receiver<()>,
            ) {
                // CRITICAL: fetch_add VOR Panic-Arm-Entscheidung. `prior == 0`
                // ist die erste Cell-Instanz (armed); jeder spätere Rebuild
                // (Post-Restart) bekommt `prior >= 1` und bleibt gesund.
                // Ohne diese Reihenfolge würde auch die Post-Restart-Instanz
                // panicken und der positive Receipt im Test wäre hohl
                // (handle_calls inkrementiert VOR Panic).
                let prior = factory
                    .spawn_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let (mut cell, inject_tx) = ReceiptMockLongRunningCell::new();
                // Shared recorders: handle/event counts survive rebuild.
                cell.handle_calls = factory.handle_calls.clone();
                cell.event_calls = factory.event_calls.clone();
                if factory.panic_after_first_handle && prior == 0 {
                    cell.panic_in_handle_after = Some(1);
                }
                // B4 backstop demo: propagate sleep + echo_to into each cell instance.
                cell.sleep_in_handle_ms = factory.sleep_in_handle_ms;
                cell.echo_to = factory.echo_to.clone();

                // Publish fresh inject_tx (try_lock is sync, no .await —
                // RespawnFn corridor stays await-free per Phase-5-tripwire).
                *factory
                    .latest_inject
                    .try_lock()
                    .expect("latest_inject: no concurrent rebuild") = Some(inject_tx);

                // In-memory cell.db pro Rebuild (Mock — siehe Modul-Doc).
                let conn = rusqlite::Connection::open_in_memory().expect("open_in_memory");
                let db = DbConn::wrap(conn, None);

                let (tx, rx) = mpsc::channel::<Message>(mailbox_capacity);
                let (peace_tx, peace_rx) = tokio::sync::oneshot::channel();
                // Backstop pair (P3-B-restart): LR has no backstop → never fired.
                let (_backstop_tx, backstop_rx) = tokio::sync::oneshot::channel();
                let p = path.clone();
                let o = outputs_tx.clone();
                let cit = colony_inbox_tx.clone();
                let bs = blob_store.clone();
                let cons = consumes.clone();
                let join = tokio::spawn(async move {
                    cell_task_long_running(
                        p,
                        rx,
                        o,
                        64,
                        cell,
                        db,
                        Some(peace_tx),
                        Some(cit),
                        stop_rx,
                        death_ack,
                        bs,
                        cons,
                    )
                    .await;
                });
                (tx, join, peace_rx, backstop_rx)
            }
        };

        // Initial spawn → live stop/death_ack ends; restart → inert (None, None).
        let (sender, join, peace_rx, backstop_rx) =
            build(Some(initial_stop_rx), Some(initial_death_ack_tx));
        let respawn: RespawnFn = Box::new(move || build(None, None));
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}
