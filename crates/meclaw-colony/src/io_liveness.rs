//! Issue #7 — per-I/O-task liveness marks for long-running cells.
//!
//! The Deep-Audit F3 heartbeat proves exactly ONE task: the `colony_task`
//! select-loop. The I/O sub-tasks of the long-running cells (`proxy`, `timer`,
//! `mcp`, `slack`) contribute nothing to that proof — their dual-task `select!`
//! reports when a side DIES, never when a side merely STALLS. Measured in a
//! production colony: a proxy hung for over 15 minutes while the service was
//! `active`, `/health` answered 200 and the log stayed empty.
//!
//! This module carries the missing signal: a mark that an I/O task sets after
//! every successful external round trip. It travels on the SAME mechanism the
//! heartbeat uses — a non-blocking `try_send` into the colony's own inbox, whose
//! task owns the resulting map. No `Mutex`, no `RwLock`, no atomics: the mark is
//! a message, and the colony task is its single owner (concurrency model — one
//! task per actor, state lives in the task).
//!
//! What the mark is NOT: a verdict. `/health` reports the age of the last
//! successful round trip per cell and nothing else — a timer whose next cron is
//! tomorrow is legitimately quiet, and only an operator (or a later watchdog
//! policy) can say which silence is a fault.

use crate::ColonyMsg;
use meclaw_core::Path;
use std::time::SystemTime;
use tokio::sync::mpsc;

/// The reporting end of a long-running cell's I/O-liveness mark.
///
/// Handed to the I/O sub-task by `cell_task_long_running` via
/// [`crate::long_running_cell::LongRunningCell::attach_liveness`]. Cheap to
/// clone (a `Path` plus an mpsc sender) and inert by default, so a cell type
/// that reports nothing, and every test that builds an I/O config by hand, keeps
/// working unchanged.
#[derive(Clone, Debug, Default)]
pub struct IoLivenessMark {
    /// `None` = disabled (no colony to report to). Reporting is then a no-op.
    inner: Option<(Path, mpsc::Sender<ColonyMsg>)>,
}

impl IoLivenessMark {
    /// A mark that reports to `tx` on behalf of the cell at `path`.
    /// `tx == None` (a cell spawned without a colony inbox, i.e. tests) yields a
    /// disabled mark.
    pub fn new(path: Path, tx: Option<mpsc::Sender<ColonyMsg>>) -> Self {
        Self {
            inner: tx.map(|tx| (path, tx)),
        }
    }

    /// A mark that reports nowhere. The default for I/O configs built outside a
    /// colony (unit tests, fixtures).
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Announce that this I/O task exists and has NOT yet completed a round
    /// trip. Call once, at the top of `run_io`. Without it a cell that never
    /// manages a single successful call would be invisible instead of visibly
    /// having never succeeded — and after a restart the predecessor's mark would
    /// linger as if it were this task's.
    pub fn announce(&self) {
        self.report(None);
    }

    /// Record a successful external round trip, now. Call it where the I/O task
    /// has proof that the far side answered — not where it merely started
    /// waiting.
    pub fn mark_success(&self) {
        self.report(Some(SystemTime::now()));
    }

    fn report(&self, at: Option<SystemTime>) {
        let Some((path, tx)) = &self.inner else {
            return;
        };
        // `try_send`, exactly like the colony heartbeat: an I/O task must never
        // block on the colony inbox (that would make the reporting of a stall
        // depend on the very loop whose health is in question). A full inbox
        // costs one mark; the next round trip reports again.
        let _ = tx.try_send(ColonyMsg::IoLiveness {
            path: path.clone(),
            at,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_disabled_mark_reports_nothing_and_never_panics() {
        let m = IoLivenessMark::disabled();
        m.announce();
        m.mark_success();
    }

    #[tokio::test]
    async fn announce_reports_no_round_trip_yet_and_success_reports_a_time() {
        let (tx, mut rx) = mpsc::channel::<ColonyMsg>(4);
        let m = IoLivenessMark::new(Path::new("/proxy"), Some(tx));
        m.announce();
        m.mark_success();

        match rx.try_recv().expect("announce must be reported") {
            ColonyMsg::IoLiveness { path, at } => {
                assert_eq!(path.as_str(), "/proxy");
                assert!(at.is_none(), "announce must carry no round-trip time");
            }
            _ => panic!("announce must arrive as ColonyMsg::IoLiveness"),
        }
        match rx.try_recv().expect("success must be reported") {
            ColonyMsg::IoLiveness { path, at } => {
                assert_eq!(path.as_str(), "/proxy");
                assert!(at.is_some(), "a successful round trip must carry its time");
            }
            _ => panic!("a success must arrive as ColonyMsg::IoLiveness"),
        }
    }

    /// The reporting path must never block the I/O task, whatever the colony is
    /// doing — a stalled colony inbox is the one situation where the mark
    /// matters most.
    #[tokio::test]
    async fn a_full_inbox_drops_the_mark_instead_of_blocking() {
        let (tx, _rx) = mpsc::channel::<ColonyMsg>(1);
        let m = IoLivenessMark::new(Path::new("/proxy"), Some(tx));
        for _ in 0..10 {
            m.mark_success();
        }
    }
}
