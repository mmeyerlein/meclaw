//! Deep-Audit F3 — colony heartbeat watchdog policy.
//!
//! The colony is a single Tokio task (`colony_task`); if it panics, every cell
//! dies with it (a heavier class than a `one_for_one` cell panic). This module is
//! the PURE policy half: it counts consecutive missed heartbeats and decides when
//! to stop. The plumbing (a heartbeat interval arm inside the `colony_task`
//! select-loop + a supervisor task in `meclaw-cli`) lives outside this module.
//!
//! Honest limit: the watchdog detects colony-task DEATH (panic → loop gone →
//! heartbeats stop) and a fully-wedged loop (blocked in an `.await` → ticks stop).
//! It does NOT reliably detect the F1 backpressure deadlock when that deadlock
//! lives in a cell wait-cycle while the colony loop keeps polling — there the
//! heartbeat keeps flowing. F1 is the separate post-MVP roadmap posten.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum WatchdogAction {
    Continue,
    Stop,
}

/// Counts consecutive missed heartbeats. After `threshold` misses in a row →
/// `Stop`. Any received heartbeat resets the counter. Pure — no I/O, no tasks.
pub struct Watchdog {
    threshold: u32,
    consecutive_misses: u32,
}

impl Watchdog {
    /// `threshold` consecutive misses trigger a `Stop`. The colony emits ~10
    /// heartbeats/second; the production supervisor uses `threshold = 5`
    /// (~0.5 s of silence).
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold,
            consecutive_misses: 0,
        }
    }

    /// Advance one supervisor period. `received` = at least one heartbeat arrived
    /// since the previous `tick`. Returns `Stop` once `threshold` consecutive
    /// misses accumulate.
    pub fn tick(&mut self, received: bool) -> WatchdogAction {
        if received {
            self.consecutive_misses = 0;
            return WatchdogAction::Continue;
        }
        self.consecutive_misses += 1;
        if self.consecutive_misses >= self.threshold {
            WatchdogAction::Stop
        } else {
            WatchdogAction::Continue
        }
    }
}

/// Deep-Audit F3 supervisor loop: every `period`, drain the heartbeat channel and
/// feed the `Watchdog`; on `threshold` consecutive empty periods send `()` on
/// `stop_tx` (one-shot, clean stop — NO restart) and return. Returns early if the
/// heartbeat channel closes (the colony is gone) by counting that as a miss too.
/// Lives here (not inline in the caller) so the plumbing is unit-testable.
pub async fn run_watchdog(
    mut heartbeat_rx: tokio::sync::mpsc::Receiver<()>,
    stop_tx: tokio::sync::oneshot::Sender<()>,
    threshold: u32,
    period: std::time::Duration,
) {
    let mut wd = Watchdog::new(threshold);
    let mut iv = tokio::time::interval(period);
    iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        iv.tick().await;
        // Drain every tick buffered this period; `received` = at least one.
        let mut received = false;
        while heartbeat_rx.try_recv().is_ok() {
            received = true;
        }
        if wd.tick(received) == WatchdogAction::Stop {
            let _ = stop_tx.send(());
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn five_consecutive_misses_trigger_stop() {
        let mut wd = Watchdog::new(5);
        for _ in 0..4 {
            assert_eq!(wd.tick(false), WatchdogAction::Continue);
        }
        assert_eq!(wd.tick(false), WatchdogAction::Stop);
    }

    #[test]
    fn a_received_heartbeat_resets_the_counter() {
        let mut wd = Watchdog::new(5);
        for _ in 0..4 {
            wd.tick(false);
        }
        // A heartbeat arrives → counter resets → no Stop yet.
        assert_eq!(wd.tick(true), WatchdogAction::Continue);
        for _ in 0..4 {
            assert_eq!(wd.tick(false), WatchdogAction::Continue);
        }
        assert_eq!(wd.tick(false), WatchdogAction::Stop);
    }

    #[test]
    fn healthy_stream_never_stops() {
        let mut wd = Watchdog::new(5);
        for _ in 0..100 {
            assert_eq!(wd.tick(true), WatchdogAction::Continue);
        }
    }

    /// Supervisor plumbing: when heartbeats cease, `run_watchdog` fires the stop
    /// signal after `threshold` empty periods.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_stops_when_heartbeats_cease() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(run_watchdog(hb_rx, stop_tx, 5, Duration::from_millis(10)));
        let res = tokio::time::timeout(Duration::from_secs(5), stop_rx).await;
        assert!(
            matches!(res, Ok(Ok(()))),
            "watchdog must fire the stop signal when heartbeats cease, got {res:?}"
        );
        drop(hb_tx);
    }

    /// Supervisor plumbing: while heartbeats keep flowing, `run_watchdog` never
    /// fires the stop signal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_does_not_stop_while_heartbeats_flow() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(run_watchdog(hb_rx, stop_tx, 5, Duration::from_millis(10)));
        // Emit heartbeats faster than the period for well over `threshold` periods.
        for _ in 0..40 {
            let _ = hb_tx.try_send(());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            stop_rx.try_recv().is_err(),
            "watchdog must NOT stop while heartbeats flow"
        );
    }
}
