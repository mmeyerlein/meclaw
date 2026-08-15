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

/// What a watchdog trip does to the process (`colony.json` `watchdog_on_trip`).
///
/// The default is [`WatchdogOnTrip::Exit`] and it is the production contract of
/// issue #6: a trip drives the graceful shutdown path and ends the process with a
/// non-zero exit code, so a supervisor restarts and an alert fires.
///
/// [`WatchdogOnTrip::LogOnly`] exists for boxes where a trip is more likely to be
/// a measurement artefact than a fault — a debug build on a loaded developer
/// machine, a scenario suite running dozens of colonies back to back. It
/// downgrades **silence** to a loud structured log line and keeps the colony
/// running. It does **not** downgrade a colony task that is GONE (heartbeat
/// channel closed = the task panicked or returned): that trip stays fatal under
/// both policies, because there is nothing left to keep running for and hiding it
/// would mask a real death.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WatchdogOnTrip {
    /// Production default: graceful shutdown, non-zero exit (issue #6).
    #[default]
    Exit,
    /// Report the trip loudly and keep the colony running (silence trips only).
    LogOnly,
}

impl std::fmt::Display for WatchdogOnTrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchdogOnTrip::Exit => f.write_str("exit"),
            WatchdogOnTrip::LogOnly => f.write_str("log-only"),
        }
    }
}

/// One watchdog trip, with the evidence the supervisor actually holds (GH #84).
///
/// Before this existed the trip said only "heartbeat lost for N periods of M ms",
/// which names the deadline but not what missed it. The three questions an
/// operator has are: was the colony task still there, how long was it really
/// silent, and was the supervisor itself on time. The last one is the
/// discriminator: the supervisor is a Tokio task in the same process, so if IT
/// was late the missing heartbeats are not evidence that the colony loop was
/// wedged — the whole process was off CPU and both tasks starved together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogTrip {
    /// Consecutive silent supervisor periods that produced the trip (`threshold`).
    pub silent_periods: u32,
    /// The configured length of one supervisor period.
    pub period: std::time::Duration,
    /// Wall time since the last observed heartbeat (or since arming, if none).
    pub silent_for: std::time::Duration,
    /// Heartbeats drained since the supervisor was armed.
    pub beats_seen: u64,
    /// `true` when the heartbeat channel is closed: the colony task is GONE
    /// (panicked or returned), not merely quiet.
    pub colony_task_gone: bool,
    /// Wall time since the supervisor was armed.
    pub armed_for: std::time::Duration,
}

impl WatchdogTrip {
    /// The deadline the trip was measured against: `silent_periods × period`.
    pub fn nominal_window(&self) -> std::time::Duration {
        self.period
            .checked_mul(self.silent_periods)
            .unwrap_or(std::time::Duration::MAX)
    }

    /// How much longer the silence actually lasted than the nominal deadline.
    ///
    /// Structurally `>= 0`: the supervisor evaluates one period per tick and its
    /// ticks are at least `period` apart. A lag near zero means the supervisor
    /// ran on schedule and only the colony went quiet. A lag on the order of the
    /// window itself means the supervisor's own ticks were late — the process,
    /// not the colony loop, lost the CPU.
    pub fn supervisor_lag(&self) -> std::time::Duration {
        self.silent_for.saturating_sub(self.nominal_window())
    }

    /// One word for what was starved, derived from the evidence above.
    ///
    /// * `colony_task_gone` — the heartbeat channel is closed; the task is dead.
    /// * `process_scheduling` — the supervisor's own periods came in at least
    ///   twice as slow as configured, so the whole process was descheduled and
    ///   the colony loop is not singled out by this observation.
    /// * `colony_loop` — the supervisor kept its schedule and the colony loop
    ///   alone stopped iterating.
    pub fn starved(&self) -> &'static str {
        if self.colony_task_gone {
            "colony_task_gone"
        } else if self.supervisor_lag() >= self.nominal_window() {
            "process_scheduling"
        } else {
            "colony_loop"
        }
    }

    /// Does this trip end the process under `policy`?
    ///
    /// A gone colony task is fatal under every policy (see [`WatchdogOnTrip`]).
    pub fn is_fatal(&self, policy: WatchdogOnTrip) -> bool {
        self.colony_task_gone || policy == WatchdogOnTrip::Exit
    }
}

impl std::fmt::Display for WatchdogTrip {
    /// One line, prefix byte-stable, diagnosis in brackets.
    ///
    /// The prefix is the sentence issue #6 shipped and the scenario runner greps
    /// for; everything GH #84 adds is appended, so an existing log scan keeps
    /// working and gains the evidence.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "colony heartbeat lost for {} consecutive supervisor periods of {} ms \
             [starved={} silent_for={}ms nominal_window={}ms supervisor_lag={}ms \
             beats_seen={} armed_for={}ms colony_task={}]",
            self.silent_periods,
            self.period.as_millis(),
            self.starved(),
            self.silent_for.as_millis(),
            self.nominal_window().as_millis(),
            self.supervisor_lag().as_millis(),
            self.beats_seen,
            self.armed_for.as_millis(),
            if self.colony_task_gone {
                "gone"
            } else {
                "alive"
            },
        )
    }
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
/// feed the `Watchdog`; on `threshold` consecutive empty periods report a
/// [`WatchdogTrip`] on `trip_tx`. Lives here (not inline in the caller) so the
/// plumbing is unit-testable.
///
/// **Boot gate (issue #6)**: the loop counts nothing until `armed_rx` fires. A
/// colony boot is not a steady state — the colony task hydrates its tables and
/// compiles its validators BEFORE its select-loop (and therefore before its first
/// heartbeat), so a boot that runs long under parallel load used to trip a
/// watchdog that had been armed at spawn time. The caller fires `armed_rx`
/// exactly once, after boot has completed; a caller that drops the sender
/// (failed boot, no colony to guard) leaves the watchdog disarmed forever and it
/// returns without ever tripping.
///
/// **Trip policy (GH #84)**: under [`WatchdogOnTrip::Exit`] — the default and the
/// issue-#6 production contract — the supervisor reports the trip and returns;
/// the caller drives the graceful stop and the non-zero exit. Under
/// [`WatchdogOnTrip::LogOnly`] a **silence** trip is reported and the supervisor
/// keeps supervising (counter reset, the next window measured from now), so a
/// dev box does not lose its colony to a scheduling artefact. A colony task that
/// is GONE (heartbeat channel closed) returns under both policies — see
/// [`WatchdogOnTrip`].
pub async fn run_watchdog(
    mut heartbeat_rx: tokio::sync::mpsc::Receiver<()>,
    trip_tx: tokio::sync::mpsc::Sender<WatchdogTrip>,
    threshold: u32,
    period: std::time::Duration,
    armed_rx: tokio::sync::oneshot::Receiver<()>,
    on_trip: WatchdogOnTrip,
) {
    // Disarmed until boot says otherwise. `Err` = the arming sender was dropped
    // (the boot never finished) → nothing to guard, exit silently.
    if armed_rx.await.is_err() {
        return;
    }
    // Heartbeats buffered during boot prove liveness of a moment that is already
    // past. Drop them so the first evaluated period is a fresh observation.
    while heartbeat_rx.try_recv().is_ok() {}
    let mut wd = Watchdog::new(threshold);
    let mut iv = tokio::time::interval(period);
    iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // GH #84 evidence trail. `armed_at` and `last_beat_at` are read from the same
    // clock the interval runs on, so `silent_for` measures the supervisor's OWN
    // lateness as well as the colony's silence — which is exactly what separates
    // "the colony loop wedged" from "the process lost the CPU".
    let armed_at = tokio::time::Instant::now();
    let mut last_beat_at = armed_at;
    let mut beats_seen: u64 = 0;
    loop {
        iv.tick().await;
        // Drain every tick buffered this period; `received` = at least one. A
        // closed channel is a colony task that is gone — a strictly stronger
        // finding than silence, and it is recorded as such.
        let mut received = false;
        let mut colony_task_gone = false;
        loop {
            match heartbeat_rx.try_recv() {
                Ok(()) => {
                    received = true;
                    beats_seen += 1;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    colony_task_gone = true;
                    break;
                }
            }
        }
        let now = tokio::time::Instant::now();
        if received {
            last_beat_at = now;
        }
        if wd.tick(received) == WatchdogAction::Stop {
            let trip = WatchdogTrip {
                silent_periods: threshold,
                period,
                silent_for: now.saturating_duration_since(last_beat_at),
                beats_seen,
                colony_task_gone,
                armed_for: now.saturating_duration_since(armed_at),
            };
            let fatal = trip.is_fatal(on_trip);
            // `try_send`: the caller drains this channel; a full one means it has
            // already been told and the supervisor must not block on saying so.
            let _ = trip_tx.try_send(trip);
            if fatal {
                return;
            }
            // log-only: keep supervising. Reset the counter AND the reference
            // instant, so the next window is measured from here and not from an
            // ancient heartbeat.
            wd = Watchdog::new(threshold);
            last_beat_at = now;
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

    /// An already-armed watchdog, for the tests that are not about arming.
    fn armed() -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let _ = tx.send(());
        rx
    }

    /// Supervisor plumbing: when heartbeats cease, `run_watchdog` fires the stop
    /// signal after `threshold` empty periods.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_stops_when_heartbeats_cease() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
        ));
        let res = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv()).await;
        assert!(
            matches!(res, Ok(Some(_))),
            "watchdog must report a trip when heartbeats cease, got {res:?}"
        );
        drop(hb_tx);
    }

    /// Supervisor plumbing: while heartbeats keep flowing, `run_watchdog` never
    /// fires the stop signal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_does_not_stop_while_heartbeats_flow() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
        ));
        // Emit heartbeats faster than the period for well over `threshold` periods.
        for _ in 0..40 {
            let _ = hb_tx.try_send(());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            trip_rx.try_recv().is_err(),
            "watchdog must NOT trip while heartbeats flow"
        );
    }

    /// Issue #6, defect 1: a boot may take arbitrarily long and emits no
    /// heartbeat while the colony task is still coming up. An unarmed watchdog
    /// must therefore be unable to trip — no matter how long that takes.
    ///
    /// Semantic discriminator (kept tight on purpose): the armed deadline here is
    /// `5 × 10 ms = 50 ms`, and the simulated boot is 500 ms of total silence —
    /// ten times the deadline. A watchdog armed at spawn time (the pre-fix
    /// behaviour) would have tripped ten times over.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unarmed_watchdog_cannot_trip_during_a_slow_boot() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            arm_rx,
            WatchdogOnTrip::Exit,
        ));
        // The boot: no heartbeat at all, far past the armed deadline.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            trip_rx.try_recv().is_err(),
            "an unarmed watchdog must not trip, however slow the boot"
        );
        drop(arm_tx);
        drop(hb_tx);
    }

    /// Issue #6, defect 1 (the other half): arming is what starts the count, so a
    /// colony that goes silent AFTER boot is still detected. Positive receipt —
    /// the stop signal must actually arrive.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn arming_after_boot_starts_the_count_and_a_silent_colony_stops() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            arm_rx,
            WatchdogOnTrip::Exit,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        arm_tx.send(()).expect("watchdog task alive");
        let res = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv()).await;
        assert!(
            matches!(res, Ok(Some(_))),
            "an armed watchdog must trip on colony silence, got {res:?}"
        );
        drop(hb_tx);
    }

    /// Issue #6, defect 1: heartbeats buffered during boot are proof about a
    /// moment that has already passed. They must not buy the colony a free
    /// period after arming, otherwise the first post-boot deadline is longer
    /// than the configured one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_boot_heartbeats_are_discarded_at_arming() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        // Fill the channel with boot-time heartbeats, then never send again.
        for _ in 0..8 {
            let _ = hb_tx.try_send(());
        }
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            arm_rx,
            WatchdogOnTrip::Exit,
        ));
        arm_tx.send(()).expect("watchdog task alive");
        let res = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv()).await;
        assert!(
            matches!(res, Ok(Some(_))),
            "buffered boot heartbeats must not keep a dead colony alive, got {res:?}"
        );
        drop(hb_tx);
    }

    // --- GH #84: the trip carries evidence, and it has a policy ---

    /// A trip observed by a supervisor that kept its own schedule blames the
    /// colony loop; the same silence observed by a supervisor whose own periods
    /// came in twice as slow blames process scheduling. That distinction is the
    /// whole point of the added evidence, so it is pinned on the pure derivation
    /// rather than on a timing race.
    #[test]
    fn starved_separates_a_late_supervisor_from_a_quiet_colony() {
        let base = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(520),
            beats_seen: 400,
            colony_task_gone: false,
            armed_for: Duration::from_secs(40),
        };
        assert_eq!(base.nominal_window(), Duration::from_millis(500));
        assert_eq!(base.supervisor_lag(), Duration::from_millis(20));
        assert_eq!(base.starved(), "colony_loop");

        // The supervisor's five periods took 3.1 s instead of 0.5 s: it was not
        // running either, so the colony's silence proves nothing about the loop.
        let late = WatchdogTrip {
            silent_for: Duration::from_millis(3100),
            ..base
        };
        assert_eq!(late.supervisor_lag(), Duration::from_millis(2600));
        assert_eq!(late.starved(), "process_scheduling");

        // A closed channel outranks both: the task is gone, not slow.
        let gone = WatchdogTrip {
            colony_task_gone: true,
            ..late
        };
        assert_eq!(gone.starved(), "colony_task_gone");
    }

    /// The trip line keeps the sentence issue #6 shipped (the scenario runner and
    /// every existing log scan grep for it) and appends the diagnosis.
    #[test]
    fn the_trip_line_keeps_its_prefix_and_names_the_evidence() {
        let trip = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(3100),
            beats_seen: 417,
            colony_task_gone: false,
            armed_for: Duration::from_millis(42_300),
        };
        let line = format!("{trip}");
        assert!(
            line.starts_with(
                "colony heartbeat lost for 5 consecutive supervisor periods of 100 ms"
            ),
            "the issue-#6 prefix must survive verbatim, was: {line}"
        );
        for needle in [
            "starved=process_scheduling",
            "silent_for=3100ms",
            "nominal_window=500ms",
            "supervisor_lag=2600ms",
            "beats_seen=417",
            "colony_task=alive",
        ] {
            assert!(line.contains(needle), "missing {needle} in: {line}");
        }
    }

    /// `exit` is the default policy and it is what production keeps.
    #[test]
    fn exit_is_the_default_policy() {
        assert_eq!(WatchdogOnTrip::default(), WatchdogOnTrip::Exit);
        assert_eq!(format!("{}", WatchdogOnTrip::Exit), "exit");
        assert_eq!(format!("{}", WatchdogOnTrip::LogOnly), "log-only");
    }

    /// Silence is fatal under `exit` and survivable under `log-only`; a colony
    /// task that is GONE is fatal under both. The second half is the guard that
    /// keeps `log-only` from being a way to hide a dead colony.
    #[test]
    fn a_gone_colony_task_is_fatal_under_every_policy() {
        let silent = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(520),
            beats_seen: 1,
            colony_task_gone: false,
            armed_for: Duration::from_secs(1),
        };
        assert!(silent.is_fatal(WatchdogOnTrip::Exit));
        assert!(!silent.is_fatal(WatchdogOnTrip::LogOnly));

        let gone = WatchdogTrip {
            colony_task_gone: true,
            ..silent
        };
        assert!(gone.is_fatal(WatchdogOnTrip::Exit));
        assert!(gone.is_fatal(WatchdogOnTrip::LogOnly));
    }

    /// GH #84 half 3: under `log-only` the supervisor reports the silence and
    /// keeps supervising. Positive receipt — a SECOND trip must arrive from the
    /// same supervisor, which can only happen if it did not return after the
    /// first one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn log_only_keeps_supervising_after_a_trip() {
        // The sender is held for the whole test: the channel must stay OPEN, so
        // the trips below are silence trips and not colony-gone trips.
        let (_hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            3,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::LogOnly,
        ));
        for n in 1..=2 {
            let res = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv()).await;
            let trip = res
                .unwrap_or_else(|_| panic!("trip {n} must arrive under log-only"))
                .unwrap_or_else(|| {
                    panic!("trip {n}: the supervisor returned instead of reporting")
                });
            assert!(
                !trip.colony_task_gone,
                "the heartbeat channel is open, so this must be a silence trip: {trip:?}"
            );
            assert!(!trip.is_fatal(WatchdogOnTrip::LogOnly));
        }
    }

    /// The other half: under `log-only`, a colony task that is GONE still ends
    /// the supervisor. The receipt is that the reported trip says so AND that the
    /// supervisor stops (the trip channel closes behind it).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn log_only_still_reports_a_gone_colony_task_as_fatal() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            3,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::LogOnly,
        ));
        drop(hb_tx); // the colony task is gone
        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("a gone colony must be reported")
            .expect("the supervisor must report before it returns");
        assert!(trip.colony_task_gone, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_task_gone");
        assert!(trip.is_fatal(WatchdogOnTrip::LogOnly));
        // The supervisor returned → its sender dropped → the channel is closed.
        let after = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must stop, not keep reporting");
        assert!(
            after.is_none(),
            "a gone colony must end the supervisor under log-only too, got {after:?}"
        );
    }
}
