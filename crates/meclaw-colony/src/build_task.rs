//! Shared spawn helper for stateful cells (Phase 13).
//!
//! `RespawnFn` and (the upcoming) `WakeFn` closures both call this helper
//! with identical captured params → Restart and Wake converge by construction.
//!
//! Phase-13-F-1 introduces the helper as a pure unit (created + unit-tested);
//! the factories still wire `cell_task_stateful` by hand (Phase-13-E-1 state).
//! The factory migration onto this helper lands in Phase-13-G-2.

use crate::DbConn;
use crate::RespawnFn;
use crate::stateful_cell::StatefulCell;
use meclaw_core::{CellEmission, Message, Path};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Spawns `cell_task_stateful` together with a peace-pair (`peace_tx` lives
/// inside the task, `peace_rx` is returned to the caller) and the Phase-13.5
/// Lifecycle-3b colony-initiated peace-stop wiring: a `stop` pair (`stop_rx`
/// moves into the task, `stop_tx` is returned) and a `death_ack` pair
/// (`death_ack` sender moves into the task's `TermAckGuard`, `death_ack_rx` is
/// returned) and the Paket-3 P3-B-restart `backstop` pair (`backstop_tx` moves
/// into the task, `backstop_rx` is returned). Returns
/// `(JoinHandle, peace_rx, stop_tx, death_ack_rx, backstop_rx)`.
///
/// Callers forward `join`+`peace_rx`+`backstop_rx` to `spawn_watcher`. The
/// `stop_tx` lets the colony trigger a peace-stop; `death_ack_rx` fires after
/// the task's `cell.db` is closed.
///
/// Both `RespawnFn` and `WakeFn` route through this helper with identical
/// captured params, ensuring Restart∥Wake converge by construction. Callers
/// whose return shape is frozen (RespawnFn 4-tuple incl. `backstop_rx`) simply
/// drop `stop_tx` / `death_ack_rx`, which Task 4 re-notifies separately.
///
/// `message_timeout` is the Concept-B substrate backstop (`cell.message_timeout`):
/// when `Some(t)`, `cell_task_stateful` wraps each `handle()` in
/// `tokio::time::timeout(t, …)`; on elapse it fires `backstop_tx`, emits a
/// generic timeout error and ends the task so the watcher sees
/// `CellDied{death_kind: Backstop}` → one_for_one restart (Paket-3 P3-B-restart).
/// `None` disables the backstop (behavior-neutral).
///
/// `consumes`: the cell's own pre-compiled required-`consumes` views
/// (`contract.consumes`), forwarded verbatim to `cell_task_stateful` for the
/// delivery-boundary consumes check (Slice 2, consumed in Task 2.4).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn build_stateful_task_with_peace<C: StatefulCell + Send + 'static>(
    own_path: Path,
    receiver: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox_tx: mpsc::Sender<crate::ColonyMsg>,
    idle_timeout: Option<Duration>,
    message_timeout: Option<Duration>,
    cell_timeout: i64,
    cell: C,
    db: DbConn,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
) -> (
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    let (peace_tx, peace_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let (death_ack_tx, death_ack_rx) = oneshot::channel();
    // Paket-3 P3-B-restart: backstop pair. `backstop_tx` moves into the task
    // (fired before the clean `return` on a `message_timeout` elapse);
    // `backstop_rx` is returned so the watcher can classify `DeathKind::Backstop`.
    let (backstop_tx, backstop_rx) = oneshot::channel();
    let cell_join = tokio::spawn(crate::cell_task::cell_task_stateful(
        own_path,
        receiver,
        outputs_tx,
        cell,
        db,
        idle_timeout,
        message_timeout,
        Some(peace_tx),
        Some(backstop_tx),
        Some(colony_inbox_tx),
        cell_timeout,
        Some(stop_rx),
        Some(death_ack_tx),
        blob_store,
        consumes,
    ));
    (cell_join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
}

/// Phase-13.5 Slice 4 T6: re-notify the colony of a freshly built stop pair.
///
/// A disconnect `take()`s a cell's `RegistryEntry.stop_tx` / `death_ack_rx`,
/// leaving them `None`. When a re-spawn closure (`RespawnFn` — crash-restart and
/// reconnect-eager) builds a new cell-task via `build_stateful_task_with_peace`,
/// it gets a fresh `(stop_tx, death_ack_rx)` pair that the frozen 3-tuple
/// `RespawnFn` signature cannot return. Instead of dropping the pair (which left
/// the cell un-stoppable — the interim `stop_wiring_unavailable` guard), the
/// closure calls this helper, which sends `ColonyMsg::StopWiringRestored` so the
/// colony-task can put the pair back onto the `RegistryEntry`.
///
/// **Non-blocking by design (A3 rationale).** This runs inside the `RespawnFn`
/// closure body, which the colony invokes synchronously from the await-free
/// `handle_cell_died` corridor (and the mutation reconnect-eager arm). It MUST
/// NOT `send().await`: a blocking send would deadlock the colony against its own
/// full inbox (the colony cannot drain its inbox while it is stuck inside a
/// respawn closure). So this is a sync `try_send`. On `Err(Full)` the pair is
/// dropped and we `tracing::error!` (never silent) — the interim guard then
/// remains the backstop for a later disconnect of this cell.
pub fn renotify_stop_wiring(
    colony_inbox_tx: &mpsc::Sender<crate::ColonyMsg>,
    path: Path,
    stop_tx: oneshot::Sender<()>,
    death_ack_rx: oneshot::Receiver<()>,
) {
    // try_send (sync, non-blocking) — NEVER send().await here: this closure runs
    // inside the await-free respawn corridor, and a blocking send against a full
    // colony inbox would deadlock (colony cannot drain its inbox while stuck in
    // the respawn closure).
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
        colony_inbox_tx.try_send(crate::ColonyMsg::StopWiringRestored {
            path: path.clone(),
            stop_tx,
            death_ack_rx,
        })
    {
        tracing::error!(
            path = %path.as_str(),
            "stop-wiring re-notify dropped: colony inbox full"
        );
    }
}

/// Spawns `cell_task_long_running` together with a peace-pair (`peace_tx` lives
/// inside the task, `peace_rx` is returned to the caller) and the Phase-13.5
/// Lifecycle-3b colony-initiated peace-stop wiring: a `stop` pair (`stop_rx`
/// moves into the task, `stop_tx` is returned) and a `death_ack` pair
/// (`death_ack` sender moves into the task's `TermAckGuard`, `death_ack_rx` is
/// returned). Returns `(JoinHandle, peace_rx, stop_tx, death_ack_rx, backstop_rx)`.
///
/// Callers forward `join`+`peace_rx` to `spawn_watcher`. The `stop_tx` lets the
/// colony trigger a peace-stop; `death_ack_rx` fires after the task's `cell.db`
/// is closed.
///
/// Funnel-uniform cross-cutting params: `blob_store`, `mailbox_capacity`.
/// The `default_origin_ttl` is threaded through to `cell_task_long_running`
/// unchanged.
///
/// `consumes`: the cell's own pre-compiled required-`consumes` views
/// (`contract.consumes`), forwarded verbatim to `cell_task_long_running` for
/// the delivery-boundary consumes check (Slice 2, consumed in Task 2.4).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn build_long_running_task<L: crate::long_running_cell::LongRunningCell + 'static>(
    own_path: meclaw_core::Path,
    mailbox: mpsc::Receiver<meclaw_core::Message>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    default_origin_ttl: u32,
    cell: L,
    db: DbConn,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
) -> (
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    let (peace_tx, peace_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let (death_ack_tx, death_ack_rx) = oneshot::channel();
    // Paket-3 P3-B-restart: long-running cells have NO B-backstop (spec —
    // dual-task pattern, deferred per Phase-7.5/9). We mint the pair and DROP the
    // sender so `backstop_rx` is forever `Err`/`Empty` → the watcher classifies
    // an LR death as `Normal`/`Panic`, never `Backstop`. Returned for shape
    // uniformity with the stateful helper.
    let (_backstop_tx, backstop_rx) = oneshot::channel();
    let cell_join = tokio::spawn(crate::cell_task::cell_task_long_running(
        own_path,
        mailbox,
        outputs_tx,
        default_origin_ttl,
        cell,
        db,
        Some(peace_tx),
        colony_inbox_tx,
        Some(stop_rx),
        Some(death_ack_tx),
        blob_store,
        consumes,
    ));
    (cell_join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
}

/// Spawns `stateless_dispatcher` together with a peace-pair and returns
/// `(JoinHandle, peace_rx, stop_tx, death_ack_rx, backstop_rx)` (ordered like
/// `build_long_running_task` / `build_stateful_task_with_peace`).
///
/// Paket-8 peace-parity: `peace_tx` is passed **into** `stateless_dispatcher`
/// (no longer kept alive as `let _peace_keep = peace_tx;`). The dispatcher fires
/// it on its stop arm → the watcher's `peace_rx.await` resolves `Ok` → NO
/// `CellDied` → `handle_cell_died` never runs → the registry entry is preserved
/// (No-Delete). On a genuine mailbox-close the dispatcher leaves `peace_tx`
/// unsent → it drops → the watcher's `peace_rx.await` returns `Err` →
/// `DeathKind::Normal` → `remove`. The returned `peace_rx` is forwarded to
/// `spawn_watcher`.
///
/// The dispatcher's stop arm returns the mailbox via `ColonyMsg::Stopped`;
/// `death_ack` fires immediately on frame drop (no `sqlite3_close` to wait
/// for). Three oneshot pairs are created internally:
///
/// - `(peace_tx, peace_rx)` — `peace_tx` is moved into the dispatcher (fired on
///   the stop arm, dropped unsent on mailbox-close); caller holds `peace_rx`
///   for the watcher.
/// - `(stop_tx, stop_rx)` — caller holds `stop_tx`, task owns `stop_rx`.
/// - `(death_ack_tx, death_ack_rx)` — task owns `death_ack_tx` (via
///   `TermAckGuard`); caller holds `death_ack_rx`.
///
/// Cross-cutting funnel params: `blob_store`, `mailbox_capacity`
/// (documentation / plumbing only — callers pre-build the channel),
/// `max_concurrency` and `message_timeout` (the Concept-B backstop, passed
/// through to `stateless_dispatcher`; `None` → no wrapper, behavior-neutral).
///
/// `consumes`: the cell's own pre-compiled required-`consumes` views
/// (`contract.consumes`), forwarded verbatim to `stateless_dispatcher` for the
/// delivery-boundary consumes check (Slice 2, consumed in Task 2.4).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn build_stateless_task<F: crate::stateless_cell::StatelessCell + 'static>(
    own_path: meclaw_core::Path,
    mailbox: mpsc::Receiver<meclaw_core::Message>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    cell: std::sync::Arc<F>,
    max_concurrency: usize,
    message_timeout: Option<std::time::Duration>,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
) -> (
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    let (peace_tx, peace_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let (death_ack_tx, death_ack_rx) = oneshot::channel();
    // Paket-3 P3-B-restart: the stateless B-backstop is per-WORKER (the
    // dispatcher task itself never ends on a backstop — it keeps draining). So
    // there is no task-level backstop death to signal: mint the pair and DROP
    // the sender → `backstop_rx` is forever `Err`/`Empty` → the watcher
    // classifies a dispatcher death as `Normal`/`Panic`, never `Backstop`.
    let (_backstop_tx, backstop_rx) = oneshot::channel();
    // Paket-8: `peace_tx` is passed INTO the dispatcher (no longer kept alive as
    // `_peace_keep`). The dispatcher fires it on the stop arm (→ watcher Ok → no
    // CellDied → entry preserved) and lets it drop unsent on a genuine
    // mailbox-close (→ watcher Err → DeathKind::Normal → remove).
    let cell_join = tokio::spawn(crate::cell_task::stateless_dispatcher(
        own_path,
        mailbox,
        outputs_tx,
        cell,
        max_concurrency,
        message_timeout,
        Some(peace_tx),
        Some(stop_rx),
        colony_inbox_tx,
        Some(death_ack_tx),
        blob_store,
        consumes,
    ));
    (cell_join, peace_rx, stop_tx, death_ack_rx, backstop_rx)
}

/// **Paket-8 (I1-Trichter):** build ONLY the `RespawnFn` for a boot-inactive
/// **eager stateless** cell — WITHOUT spawning the initial task (boot-gating: no
/// task runs at boot for an inactive cell). Mirrors the LR
/// `build_boot_inactive_respawn` shape (proxy/timer/mcp) for the stateless funnel,
/// so all six stateless factories share ONE respawn-closure construction.
///
/// When the reconnect arm invokes the returned closure, it builds a FRESH live
/// `stateless_dispatcher` (via `build_stateless_task`, peace-parity included) with
/// a new live stop pair and re-notifies the colony (`renotify_stop_wiring`) so a
/// later disconnect can peace-stop the reconnected cell. The closure returns the
/// frozen `RespawnFn` 4-tuple `(sender, join, peace_rx, backstop_rx)`; `stop_tx` /
/// `death_ack_rx` are re-notified separately (they cannot ride the 4-tuple).
/// `consumes`: the cell's own pre-compiled required-`consumes` views
/// (`contract.consumes`), captured into the respawn closure and forwarded to
/// `build_stateless_task` on every respawn (Slice 2, consumed in Task 2.4).
#[allow(clippy::too_many_arguments)]
pub fn build_stateless_boot_inactive_respawn<F: crate::stateless_cell::StatelessCell + 'static>(
    own_path: meclaw_core::Path,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    cell: std::sync::Arc<F>,
    max_concurrency: usize,
    message_timeout: Option<std::time::Duration>,
    colony_inbox_tx: mpsc::Sender<crate::ColonyMsg>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    mailbox_capacity: usize,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
) -> RespawnFn {
    Box::new(move || {
        let (sender, receiver) = mpsc::channel::<Message>(mailbox_capacity);
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateless_task(
            own_path.clone(),
            receiver,
            outputs_tx.clone(),
            cell.clone(),
            max_concurrency,
            message_timeout,
            Some(colony_inbox_tx.clone()),
            blob_store.clone(),
            consumes.clone(),
        );
        // Re-notify the fresh stop pair (sync try_send — runs inside the
        // await-free respawn corridor). Mirror proxy/timer/mcp.
        crate::renotify_stop_wiring(&colony_inbox_tx, own_path.clone(), stop_tx, death_ack_rx);
        (sender, join, peace_rx, backstop_rx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stateful_cell::StatefulCell;
    use meclaw_core::OutputSink;

    /// Minimal no-op stateful cell — never panics, never emits, only used to
    /// keep `cell_task_stateful` alive while we smoke-check the helper's
    /// return shape.
    struct NoopStateful;
    impl StatefulCell for NoopStateful {
        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a mut self,
            _msg: Message,
            _sink: &'a OutputSink,
            _db: &'a mut DbConn,
        ) -> impl std::future::Future<Output = ()> + Send + 'a {
            async move {}
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn helper_spawns_task_and_returns_pair() {
        let (_mb_tx, mb_rx) = mpsc::channel::<Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = DbConn::wrap(conn, None);

        let (cell_join, mut peace_rx, _stop_tx, _death_ack_rx, _backstop_rx) =
            build_stateful_task_with_peace(
                Path::new("/probe"),
                mb_rx,
                out_tx,
                inbox_tx,
                None, // idle_timeout
                None, // message_timeout
                0,    // cell_timeout
                NoopStateful,
                db,
                None,
                None,
            );

        // Yield once so the spawned task gets a chance to be scheduled.
        tokio::task::yield_now().await;

        assert!(
            !cell_join.is_finished(),
            "cell task must still be running right after spawn (mailbox still open)"
        );
        assert!(
            matches!(
                peace_rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "peace_rx must be empty — peace_tx is not signalled in Phase-13-F-1"
        );

        // Clean shutdown: drop mailbox sender (already dropped above by leaving
        // scope — explicit drop here for clarity), then drop the cell join
        // handle. `cell_task_stateful` exits cleanly when `mailbox.recv()`
        // returns `None`.
        drop(_mb_tx);
        cell_join.await.expect("cell task should finish cleanly");
    }

    /// Phase-13.5 Lifecycle-3b step-3.7: `build_stateful_task_with_peace` plumbs
    /// the colony-initiated peace-stop. Firing `stop_tx` makes the task finish,
    /// fire `peace_tx`, return its mailbox via `ColonyMsg::Stopped`, close
    /// `cell.db`, then fire `death_ack` (via `TermAckGuard` after sqlite3_close).
    /// `death_ack_rx` must therefore receive after the task frame is torn down.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stop_tx_fires_peace_then_death_ack_after_cell_db_close() {
        let (mb_tx, mb_rx) = mpsc::channel::<Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = DbConn::wrap(conn, None);

        let (cell_join, mut peace_rx, stop_tx, death_ack_rx, _backstop_rx) =
            build_stateful_task_with_peace(
                Path::new("/probe"),
                mb_rx,
                out_tx,
                inbox_tx,
                None, // idle_timeout
                None, // message_timeout
                0,    // cell_timeout
                NoopStateful,
                db,
                None,
                None,
            );

        // Trigger the colony-initiated peace-stop.
        stop_tx.send(()).unwrap();

        // peace fires.
        let peace = tokio::time::timeout(std::time::Duration::from_secs(5), &mut peace_rx).await;
        assert!(peace.is_ok() && peace.unwrap().is_ok(), "peace must fire");

        // death_ack fires after cell.db close (TermAckGuard drops after DbConn).
        let ack = tokio::time::timeout(std::time::Duration::from_secs(5), death_ack_rx).await;
        assert!(
            ack.is_ok() && ack.unwrap().is_ok(),
            "death_ack must fire after cell.db close"
        );

        // Stopped lands in the colony inbox with the mailbox receiver.
        let inbox_msg = tokio::time::timeout(std::time::Duration::from_secs(5), inbox_rx.recv())
            .await
            .expect("inbox recv within 5s")
            .expect("inbox open");
        assert!(matches!(inbox_msg, crate::ColonyMsg::Stopped { .. }));

        cell_join.await.expect("cell task should finish cleanly");
        drop(mb_tx);
    }

    // ── build_long_running_task tests ─────────────────────────────────────────

    /// Minimal no-op long-running cell — never panics, never emits. Only used
    /// to keep `cell_task_long_running` alive while we smoke-check the helper's
    /// return shape. Mirrors `TrivialMock` in `long_running_cell.rs`.
    struct NoopLongRunning;
    struct NoopIo;

    impl crate::long_running_cell::LongRunningCell for NoopLongRunning {
        type Event = ();
        type Reconfig = ();
        type Io = NoopIo;

        fn split_io(&mut self) -> Self::Io {
            NoopIo
        }

        #[allow(clippy::manual_async_fn)]
        fn run_io(
            _io: Self::Io,
            _events_tx: tokio::sync::mpsc::Sender<Self::Event>,
            _reconfig_rx: tokio::sync::mpsc::Receiver<Self::Reconfig>,
        ) -> impl std::future::Future<Output = ()> + Send {
            // Pend for the cell's entire lifetime — the LongRunningCell contract
            // (A1′, see `long_running_cell.rs`): a clean, voluntary `run_io`
            // return while the cell is still live lets the outer `select!` win on
            // I/O-completion and fire the peace-stop. A real LR cell (proxy/timer/
            // mcp) always pends or loops; the earlier `async {}` returned
            // immediately, so under workspace-CPU-saturation the I/O sub-task
            // ended (→ peace) before the `peace_rx must be empty right after
            // spawn` assertion ran — an intermittent timing flake. Pending makes
            // peace fire ONLY on a real stop (mailbox close), so the assertion is
            // deterministic regardless of scheduler load. Shutdown is unchanged:
            // dropping the mailbox sender ends the handler, which aborts this
            // pending I/O task.
            async {
                std::future::pending::<()>().await;
            }
        }

        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a mut self,
            _msg: meclaw_core::Message,
            _sink: &'a meclaw_core::OutputSink,
            _db: &'a mut DbConn,
            _reconfig_tx: &'a tokio::sync::mpsc::Sender<Self::Reconfig>,
        ) -> impl std::future::Future<Output = ()> + Send + 'a {
            async {}
        }

        #[allow(clippy::manual_async_fn)]
        fn handle_event<'a>(
            &'a mut self,
            _event: Self::Event,
            _sink: &'a meclaw_core::OriginSink,
            _db: &'a mut DbConn,
        ) -> impl std::future::Future<Output = ()> + Send + 'a {
            async {}
        }
    }

    /// Smoke-check: `build_long_running_task` spawns the cell task and returns
    /// the expected `(JoinHandle, peace_rx, stop_tx, death_ack_rx, backstop_rx)`
    /// shape, fires no early peace, and shuts down cleanly when the mailbox
    /// closes.
    ///
    /// GH #156 — what this test does NOT assert any more: that the task is
    /// alive at one particular instant. It used to `yield_now()` once and then
    /// check `!cell_join.is_finished()`, which failed on a saturated CI runner.
    /// A single yield is not a synchronisation primitive, and the obvious
    /// repair (assert the running state repeatedly over ~100 ms) failed LOCALLY
    /// every time — the assertion was a race in both directions.
    ///
    /// The first repair was worse than the fault and is recorded here so it is
    /// not attempted again: it fed the task a probe message and waited for the
    /// mailbox capacity to come back, calling that a liveness receipt. But a
    /// probe is an input, and an input can END the cell — the scheduled CI run
    /// of 2026-08-17 caught exactly that, tripping on `peace_rx must be empty`
    /// instead. **A liveness check that perturbs the thing it measures is not a
    /// liveness check**, and swapping one flake for a subtler one is a loss.
    ///
    /// So the assertion is gone rather than replaced, because the test already
    /// proved liveness at its other end and nobody noticed: a task that never
    /// ran cannot answer a mailbox close by finishing cleanly. `cell_join`
    /// returning `Ok` IS the receipt, it is an event rather than an instant,
    /// and it was in the test the whole time. What remains: the helper returns
    /// the documented shape, it fires no early peace, and it shuts down when
    /// the mailbox closes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn lr_helper_spawns_task_and_returns_pair() {
        let (_mb_tx, mb_rx) = mpsc::channel::<Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = DbConn::wrap(conn, None);

        let (cell_join, mut peace_rx, _stop_tx, _death_ack_rx, _backstop_rx) =
            build_long_running_task(
                Path::new("/lr-probe"),
                mb_rx,
                out_tx,
                64,
                NoopLongRunning,
                db,
                None, // colony_inbox_tx — not needed for this smoke test
                None, // blob_store
                None,
            );

        // No early peace. Deterministic without a clock: nothing has been sent
        // and `run_io` pends for the cell's lifetime, so the only writer to
        // this channel is a real stop — which has not been asked for yet.
        assert!(
            matches!(
                peace_rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "peace_rx must be empty right after spawn"
        );

        // The liveness receipt, and the whole of it: close the mailbox and the
        // task finishes cleanly. A task that never ran cannot do that. The
        // timeout is a failure marker, not a semantic bound — 30 s of a path
        // that takes microseconds.
        drop(_mb_tx);
        tokio::time::timeout(std::time::Duration::from_secs(30), cell_join)
            .await
            .expect("the lr cell task must answer a mailbox close within 30s")
            .expect("lr cell task should finish cleanly");
    }

    // ── build_stateless_task tests ────────────────────────────────────────────

    /// Minimal no-op stateless cell — `handle()` is a no-op.  Only used to
    /// keep `stateless_dispatcher` alive while we smoke-check the helper's
    /// return shape.  Mirrors the local-mock convention (`NoopStateful`,
    /// `NoopLongRunning`) established above.
    struct NoopStateless;
    impl crate::stateless_cell::StatelessCell for NoopStateless {
        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a self,
            _msg: Message,
            _sink: &'a meclaw_core::OutputSink,
        ) -> impl std::future::Future<Output = ()> + Send + 'a {
            async move {}
        }
    }

    /// Smoke-check: `build_stateless_task` spawns the dispatcher and returns
    /// the expected `(JoinHandle, peace_rx, stop_tx, death_ack_rx)` shape.
    /// After spawn the mailbox is still open (task is running) and `peace_rx`
    /// is empty. Peace *is* fired on the stop arm; this test exercises the
    /// mailbox-close path instead, where `peace_tx` is moved unsent into the
    /// dispatcher and dropped at task-end → `peace_rx` resolves to `Err`.
    /// Dropping the mailbox sender causes the dispatcher to exit cleanly; the
    /// unsent `peace_tx` is dropped on that path, so `peace_rx` resolves to `Err`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_helper_spawns_task_and_returns_pair() {
        use std::sync::Arc;

        let (_mb_tx, mb_rx) = mpsc::channel::<Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let cell = Arc::new(NoopStateless);

        let (cell_join, peace_rx, _stop_tx, _death_ack_rx, _backstop_rx) = build_stateless_task(
            Path::new("/sl-probe"),
            mb_rx,
            out_tx,
            cell,
            4,    // max_concurrency
            None, // message_timeout
            None, // colony_inbox_tx
            None, // blob_store
            None,
        );

        tokio::task::yield_now().await;

        assert!(
            !cell_join.is_finished(),
            "stateless dispatcher must still be running right after spawn (mailbox still open)"
        );

        // Clean shutdown: drop mailbox sender → dispatcher exits.
        drop(_mb_tx);
        cell_join
            .await
            .expect("stateless dispatcher should finish cleanly");

        // `peace_tx` is moved into the dispatcher; on the mailbox-close path it
        // is dropped unsent at task-end → `peace_rx` resolves to `Err` (genuine
        // normal exit, F5).
        assert!(
            peace_rx.await.is_err(),
            "peace_rx must resolve to Err — peace_tx is dropped unsent on task-end (mailbox-close path)"
        );
    }

    /// Firing `stop_tx` fires peace on the dispatcher's stop arm (parity with
    /// stateful / long-running), then `death_ack_rx` fires (stateless cells
    /// have no `cell.db`, so `death_ack` fires on task frame drop), and
    /// `ColonyMsg::Stopped` is returned with the mailbox receiver. Peace on the
    /// stop arm means the watcher sees `Ok` → NO `CellDied` → `handle_cell_died`
    /// never runs → the registry entry is preserved (No-Delete).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_stop_tx_fires_peace_then_death_ack_and_returns_stopped() {
        use std::sync::Arc;

        let (_mb_tx, mb_rx) = mpsc::channel::<Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let cell = Arc::new(NoopStateless);

        let (cell_join, mut peace_rx, stop_tx, death_ack_rx, _backstop_rx) = build_stateless_task(
            Path::new("/sl-stop"),
            mb_rx,
            out_tx,
            cell,
            4,
            None, // message_timeout
            Some(inbox_tx),
            None,
            None,
        );

        // Trigger the colony-initiated peace-stop.
        stop_tx.send(()).unwrap();

        // Peace fires on the stop arm → watcher would see Ok → NO CellDied →
        // handle_cell_died never runs → entry preserved (No-Delete).
        let peace = tokio::time::timeout(std::time::Duration::from_secs(5), &mut peace_rx).await;
        assert!(
            peace.is_ok() && peace.unwrap().is_ok(),
            "peace MUST fire on the stateless stop arm (parity with stateful/LR)"
        );

        // death_ack fires when the dispatcher frame drops (no db to wait for).
        let ack = tokio::time::timeout(std::time::Duration::from_secs(5), death_ack_rx).await;
        assert!(
            ack.is_ok() && ack.unwrap().is_ok(),
            "death_ack must fire after stop"
        );

        // Stopped lands in the colony inbox with the mailbox receiver.
        let inbox_msg = tokio::time::timeout(std::time::Duration::from_secs(5), inbox_rx.recv())
            .await
            .expect("inbox recv within 5s")
            .expect("inbox open");
        assert!(matches!(inbox_msg, crate::ColonyMsg::Stopped { .. }));

        cell_join
            .await
            .expect("stateless dispatcher should finish cleanly");
        drop(_mb_tx);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boot_inactive_respawn_spawns_live_task_and_renotifies() {
        use std::sync::Arc;

        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let cell = Arc::new(NoopStateless);

        let respawn = build_stateless_boot_inactive_respawn(
            Path::new("/sl-boot"),
            out_tx,
            cell,
            4,    // max_concurrency
            None, // message_timeout
            inbox_tx,
            None, // blob_store
            64,   // mailbox_capacity
            None,
        );

        // Invoking the respawn spawns a LIVE task (boot-gating: nothing ran before
        // this call) and returns the RespawnFn 4-tuple.
        let (sender, join, _peace_rx, _backstop_rx) = respawn();
        tokio::task::yield_now().await;
        assert!(!join.is_finished(), "respawned dispatcher must be running");

        // It re-notifies the colony with a fresh stop pair so a later disconnect
        // can peace-stop the reconnected cell (I1 / T6b parity with proxy).
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), inbox_rx.recv())
            .await
            .expect("StopWiringRestored within 5s")
            .expect("inbox open");
        assert!(matches!(msg, crate::ColonyMsg::StopWiringRestored { .. }));

        drop(sender); // close mailbox → dispatcher exits
        join.await.unwrap();
    }
}
