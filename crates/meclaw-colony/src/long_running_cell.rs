//! Phase-10 Long-Running-Cell-Trait — dual-task pattern (handler + I/O).
//! See `docs/meclaw-overview.md` § "Long-Running-Cells: Doppel-Task"
//! (Z.85–110, Z.695–730). RPITIT, not object-safe, monomorphized per
//! cell type (Phase-6.5 pattern, symmetric to `StatefulCell`).

use crate::DbConn;
use meclaw_core::{Message, OriginSink, OutputSink};
use std::future::Future;
use tokio::sync::mpsc;

/// Long-running cell trait — dual-task substrate (Phase 10).
///
/// Monomorphized per cell type by `cell_task_long_running<L>`. The trait
/// is **not** object-safe (RPITIT), matching the existing `Cell` /
/// `StatefulCell` pattern.
///
/// The outer task `cell_task_long_running` spawns two sub-tasks:
/// 1. **I/O sub-task** runs `run_io` with the owned `Io` state — performs
///    the unbounded I/O (long-poll, sleep_until, SSE-read), pushes
///    provider events into the internal `events` mpsc. **No** cell state,
///    **no** `outputs_tx`, **no** `cell.db` access.
/// 2. **Handler sub-task** runs the substrate-internal `handler_loop`,
///    selects over the external mailbox + the internal events channel,
///    dispatches to `handle` / `handle_event`. Holds `DbConn`, owns the
///    cell value, is the only sub-task that emits to topology — via
///    `OutputSink` for mailbox-driven emissions and `OriginSink` for
///    event-originated (source) emissions.
///
/// Reconfigure hints (Handler → I/O) flow through a second internal mpsc
/// — `handle` receives `&mpsc::Sender<Self::Reconfig>` to dispatch them.
///
/// See `docs/meclaw-overview.md` § "Long-Running-Cells: Doppel-Task" for
/// the canonical specification.
pub trait LongRunningCell: Send + Sized {
    /// Event frame pushed from the I/O sub-task into the internal channel.
    type Event: Send + 'static;
    /// Hint from handler to I/O sub-task (e.g. timer "schedule changed").
    type Reconfig: Send + 'static;
    /// Owned I/O state that the I/O sub-task takes by value.
    type Io: Send + 'static;

    /// Extract the owned I/O state from the cell. Called once per spawn,
    /// before the I/O sub-task is `tokio::spawn`'d. Sync — no `.await`
    /// (RespawnFn corridor stays await-free per Phase-5 tripwire).
    fn split_io(&mut self) -> Self::Io;

    /// I/O loop. Takes the owned `Io` state by value, pushes events,
    /// receives reconfigure hints. No cell-state access, no `outputs_tx`,
    /// no `cell.db`. Static-style: not a method on `self`.
    ///
    /// **Lifetime contract (A1′):** `run_io` runs for the *entire lifetime*
    /// of the cell — it polls/waits forever and returns only when the cell as
    /// a whole tears down (handler closes the internal channels, disconnect, or
    /// panic). A clean, voluntary return from `run_io` while the cell is still
    /// live is a **contract violation**: it would silence the I/O side while
    /// the handler keeps running and open the latent "io-finish-first" loss
    /// class (the outer `select!` over both join handles could win on the I/O
    /// completion and abort the surviving handler with unprocessed events). All
    /// real impls (`proxy`/`timer`/`mcp`) loop endlessly; this invariant
    /// forbids the class for future implementors.
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send;

    /// Startup work, run in the handler task BEFORE the first mailbox message
    /// and before the first I/O event. Default: nothing.
    ///
    /// This is the slot for anything a cell must settle about its OWN previous
    /// life — crash recovery over `cell.db` being the motivating case. Doing
    /// that from an I/O event instead is a race: the `select!` over mailbox and
    /// events has no ordering, so a message can be handled first and the
    /// recovery then mistakes this life's freshly written rows for orphans of
    /// the last one (P8/P10 flake, harness cell).
    ///
    /// Deliberately independent of the I/O sub-task: the handler must never
    /// wait for a signal that a dead or slow `run_io` may never send.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        _sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async {}
    }

    /// Handle a message from the external mailbox. `reconfig_tx` lets the
    /// handler dispatch a reconfigure hint to the I/O sub-task.
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a;

    /// Handle a provider event from the I/O sub-task. `OriginSink` is used
    /// because events have no parent message context (source emissions
    /// per overview Z.852).
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DbConn;
    use meclaw_core::{Message, OriginSink, OutputSink};
    use std::future::Future;
    use tokio::sync::mpsc;

    struct TrivialMock;
    struct TrivialIo;

    impl LongRunningCell for TrivialMock {
        type Event = ();
        type Reconfig = ();
        type Io = TrivialIo;

        fn split_io(&mut self) -> Self::Io {
            TrivialIo
        }

        // The explicit `+ Send` is load-bearing — AFIT (`async fn` in a trait)
        // does not bind `Send` to the returned future, but the generic
        // `tokio::spawn` in `cell_task_long_running` needs it
        // (see overview § Output path / § Long-running cells: double task).
        // `clippy::manual_async_fn` is a false positive here.
        #[allow(clippy::manual_async_fn)]
        fn run_io(
            _io: Self::Io,
            _events_tx: mpsc::Sender<Self::Event>,
            _reconfig_rx: mpsc::Receiver<Self::Reconfig>,
        ) -> impl Future<Output = ()> + Send {
            async {}
        }

        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a mut self,
            _msg: Message,
            _sink: &'a OutputSink,
            _db: &'a mut DbConn,
            _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
        ) -> impl Future<Output = ()> + Send + 'a {
            async {}
        }

        #[allow(clippy::manual_async_fn)]
        fn handle_event<'a>(
            &'a mut self,
            _event: Self::Event,
            _sink: &'a OriginSink,
            _db: &'a mut DbConn,
        ) -> impl Future<Output = ()> + Send + 'a {
            async {}
        }
    }

    #[test]
    fn trivial_mock_compiles_against_trait() {
        let _ = TrivialMock;
    }
}
