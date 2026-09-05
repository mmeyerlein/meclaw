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
//!
//! GH #165 — the second observer. Silence carries no information about its own
//! cause: a loop that is wedged and a loop that never got the CPU are the same
//! observation. The supervisor cannot settle it by looking at itself, because it
//! is `sleep`-driven and has nothing to do: a starved runtime still wakes a timer
//! roughly on schedule, which is how a real incident produced `supervisor_lag=0`
//! while the box was compiling on sixteen cores. So the substrate runs a SECOND
//! task that has to FINISH WORK on the same clock — [`run_liveness_witness`] —
//! and judges it by the identical rule it judges the colony by. A colony trip
//! whose witness failed the same test is not evidence about the colony, and it
//! never ends the process.

/// How many beats the colony→supervisor heartbeat channel buffers (GH #571).
///
/// The channel shipped at 8. The loop emits three beats per handled event — the
/// top of the iteration, the `Parked` before the `select!`, the arm's own
/// declaration — and the supervisor drains it once per `watchdog_period_ms`,
/// so a burst of a handful of events filled it inside a few milliseconds. Every
/// beat after that was dropped by `try_send`, and what survived in the buffer was
/// the OLDEST word, not the newest: a loop that had just said `Working` was read
/// as one whose last word was `Parked`. `in_flight_work` came out false, the trip
/// said `starved=colony_loop`, and that verdict ends the process under the
/// shipped `watchdog_on_trip = exit`. A talking loop was killed for being silent.
///
/// 256 buffers the bursts a real colony produces — a timer tick that fans out
/// across a member's cells, a mutation instantiating a subtree — while staying
/// small enough to be irrelevant in memory (a `Beat` is a discriminant plus, in
/// the labelled case, one `Arc<str>` word). It is a constant, not a
/// `colony.json` knob: nothing an operator can usefully tune, and a channel that
/// can be configured too small is the same defect with a config file in front of
/// it.
pub const HEARTBEAT_CAPACITY: usize = 256;

/// What the colony loop says about itself when it beats (GH #165).
///
/// The pre-#165 heartbeat was a bare `()`: it proved that an iteration had
/// begun and nothing else. Silence therefore had no cause attached — a loop
/// wedged in a deadlock and a loop halfway through a legitimate half-second
/// operation produced the identical observation, and the watchdog killed both.
///
/// The loop now declares its phase BEFORE it blocks, which is the only moment at
/// which a blocked loop can say anything at all. Everything between a `Working`
/// and the next `Parked` is ONE work item, so the supervisor can ask "how long
/// has it been on this item" instead of only "how long has it been quiet".
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Beat {
    /// The loop has finished its work item and is about to wait for the next
    /// event. An idle colony sits here and is woken ~10×/s by the heartbeat
    /// interval arm, so silence in this phase means the loop failed to come back
    /// from a wait with nothing in it — there is no operation to blame.
    Parked,
    /// The loop has taken an event (or is flushing durable writes) and is inside
    /// that one work item. Silence in this phase has a named cause: the item has
    /// not returned yet. That is slow, and it is only a defect once the item
    /// exceeds a budget of its own.
    Working,
    /// GH #439: the same declaration as [`Beat::Working`], plus the name of the
    /// operation the loop is inside. A trip that happens inside a named item can
    /// then say what it was inside instead of only how long it was quiet.
    ///
    /// The supervisor normalises this to `Working` for every judgement it makes
    /// — a label changes the diagnosis, never the verdict — and remembers the
    /// last label it saw until the loop parks.
    WorkingOn(WorkItem),
}

/// The name of the work item the colony loop declared before it blocked
/// (GH #439).
///
/// `Arc<str>` because a long instantiation clones the label once per cell and
/// the supervisor keeps the last one; a fresh `String` per beat would allocate
/// on the one path that must stay cheap enough to run inside a hot loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem(std::sync::Arc<str>);

impl WorkItem {
    /// A label for one work item. One line, no JSON, no private data — it is
    /// rendered into a log line an operator reads.
    pub fn new(label: impl Into<std::sync::Arc<str>>) -> Self {
        Self(label.into())
    }

    /// The label as it will be rendered.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A cheap, cloneable handle the mutation path pulses while it works (GH #439).
///
/// Sync by construction: [`WorkPulse::tick`] is a plain `fn` around `try_send`,
/// exactly like [`beat`](crate::colony) itself, so it also works inside the
/// SYNCHRONOUS staging functions in `mutation/{apply,stage,subtree}.rs` — which
/// is where the expensive half of an instantiation actually lives. A pulse that
/// blocked while saying "I am still working" would be the defect it reports.
#[derive(Clone)]
pub struct WorkPulse {
    tx: Option<tokio::sync::mpsc::Sender<Beat>>,
    label: WorkItem,
}

impl WorkPulse {
    /// A pulse that reports to `tx` under `label`.
    pub fn new(tx: Option<tokio::sync::mpsc::Sender<Beat>>, label: WorkItem) -> Self {
        Self { tx, label }
    }

    /// A pulse that reports nowhere — the default for every call site with no
    /// heartbeat wired (tests, `--validate`, the boot growth, which runs before
    /// the supervisor is armed).
    pub fn silent() -> Self {
        Self {
            tx: None,
            label: WorkItem::new("<unlabelled>"),
        }
    }

    /// One non-blocking beat. Never awaits, never blocks, never panics: a full
    /// channel means the supervisor has not drained this period yet and needs
    /// only one beat per period.
    pub fn tick(&self) {
        if let Some(t) = &self.tx {
            let _ = t.try_send(Beat::WorkingOn(self.label.clone()));
        }
    }

    /// The same pulse under a narrower label — one mutation, one cell at a time.
    pub fn with_label(&self, label: WorkItem) -> Self {
        Self {
            tx: self.tx.clone(),
            label,
        }
    }

    /// The label this pulse currently reports under.
    pub fn label(&self) -> &WorkItem {
        &self.label
    }
}

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

/// How much longer than the idle deadline a DECLARED, in-flight work item may
/// take before it counts as stuck (GH #165).
///
/// Not a widening of the watchdog window: the idle bar — "a parked loop must come
/// back within `threshold × period`" — is untouched, and it is the bar that
/// catches a colony task which stopped iterating. This is a second, separate bar
/// for the case the first one has no business judging: a loop that told us it
/// entered an operation and has not returned from it. `colony.db` migrations, a
/// mutation instantiating cells, an `fsync` on a contended disk are all legitimate
/// half-second operations, and the reported incident (`beats_seen` at the nominal
/// rate for 235 s, then one 499 ms iteration) is exactly that shape.
///
/// A work item that outlives even this bar is a wedge and is fatal again: the
/// factor bounds the suppression, it does not remove it.
pub const WORK_ITEM_BUDGET_FACTOR: u32 = 10;

/// What the independent, work-doing witness had to say about the same window
/// (GH #165).
///
/// The witness is a second Tokio task on the same runtime that must complete a
/// small unit of REAL work — a trip through the run queue plus a fixed CPU
/// quantum — once per supervisor period. It is judged by the identical rule the
/// colony is judged by: `threshold` consecutive periods without a completed unit.
///
/// Why a witness and not a bigger window: widening the deadline only moves the
/// point at which the same fallacy is committed. What the deadline is missing is
/// not length, it is a control — an observer that fails when the runtime fails
/// and succeeds when it does not. The supervisor is not that control (it sleeps),
/// and the colony loop cannot be its own control (it is the thing in question).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HostWitness {
    /// No witness was wired. The trip carries no corroboration either way — the
    /// pre-#165 state of knowledge, kept honest instead of hidden.
    Absent,
    /// The witness kept completing its work units while the colony went quiet.
    /// The runtime was handing out CPU to a task that had to earn it, so the
    /// colony loop is alone in its silence.
    Kept,
    /// The witness failed the SAME test in the SAME window. Whatever stopped the
    /// colony loop also stopped a task that has nothing to do with the colony —
    /// the observation does not implicate the colony and must not end it.
    Failed,
}

impl std::fmt::Display for HostWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostWitness::Absent => f.write_str("absent"),
            HostWitness::Kept => f.write_str("kept"),
            HostWitness::Failed => f.write_str("failed"),
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// GH #165: the loop's own last word before it went quiet. `true` = it had
    /// declared a work item and never came back from it; `false` = it was parked
    /// on the select with nothing in flight, where silence has no explanation.
    pub in_flight_work: bool,
    /// GH #165: was an independent work-doing witness wired for this window.
    /// `false` = no corroboration is available, not "the host was fine".
    pub witness_present: bool,
    /// GH #165: the longest run of consecutive supervisor periods in which the
    /// witness failed to complete a work unit, counted over the same window that
    /// produced this trip. Compared against `silent_periods` — the identical bar
    /// the colony just failed — so the two observers are held to one rule.
    pub witness_worst_misses: u32,
    /// GH #439: the label of the work item that was in flight when the loop went
    /// quiet, if it declared one. `None` = a `Working` beat without a name, or a
    /// parked loop — a parked loop is inside nothing.
    pub work_item: Option<WorkItem>,
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

    /// The witness's verdict on this window (GH #165).
    ///
    /// Derived, not stored twice: the witness is `Failed` exactly when it missed
    /// as many consecutive periods as the colony did — the same bar, so neither
    /// observer is privileged.
    pub fn witness(&self) -> HostWitness {
        if !self.witness_present {
            HostWitness::Absent
        } else if self.witness_worst_misses >= self.silent_periods {
            HostWitness::Failed
        } else {
            HostWitness::Kept
        }
    }

    /// The budget a DECLARED, in-flight work item gets before it counts as stuck:
    /// [`WORK_ITEM_BUDGET_FACTOR`] × the idle window.
    pub fn work_item_budget(&self) -> std::time::Duration {
        self.nominal_window()
            .checked_mul(WORK_ITEM_BUDGET_FACTOR)
            .unwrap_or(std::time::Duration::MAX)
    }

    /// One word for what was starved, derived from the evidence above.
    ///
    /// * `colony_task_gone` — the heartbeat channel is closed; the task is dead.
    /// * `host_runtime` (GH #165) — the independent witness failed the same test
    ///   in the same window: a task with no relation to the colony also stopped
    ///   completing work, so the runtime, not the loop, is what stopped.
    /// * `slow_work_item` (GH #165) — the loop declared a work item before it
    ///   blocked and is still inside it, under [`Self::work_item_budget`]. That is
    ///   an operation taking long, not a loop that stopped: a mutation
    ///   instantiating cells, a migration, an `fsync` on a contended disk.
    /// * `stuck_work_item` (GH #165) — the same declared item outlived even that
    ///   budget. An operation that never returns is a wedge whatever its name.
    /// * `process_scheduling` — the supervisor's own periods came in at least
    ///   twice as slow as configured, so the whole process was descheduled and
    ///   the colony loop is not singled out by this observation.
    /// * `colony_loop` — every control held: the supervisor kept its schedule, the
    ///   witness kept finishing work, and the loop was parked with nothing in
    ///   flight when it stopped answering. This is the only silence that
    ///   implicates the colony.
    ///
    /// The witness outranks `supervisor_lag` because it is the stronger claim:
    /// `supervisor_lag` says a sleeper woke on time, the witness says a worker
    /// got through its work. A sleeper waking on time is compatible with a host
    /// that cannot get anything done — that compatibility is the whole defect
    /// GH #165 is about.
    pub fn starved(&self) -> &'static str {
        if self.colony_task_gone {
            "colony_task_gone"
        } else if self.witness() == HostWitness::Failed {
            "host_runtime"
        } else if self.in_flight_work {
            if self.silent_for < self.work_item_budget() {
                "slow_work_item"
            } else {
                "stuck_work_item"
            }
        } else if self.supervisor_lag() >= self.nominal_window() {
            "process_scheduling"
        } else {
            "colony_loop"
        }
    }

    /// Does this trip end the process under `policy`?
    ///
    /// A gone colony task is fatal under every policy (see [`WatchdogOnTrip`]) —
    /// that finding is a proof (the channel is closed), not an inference.
    ///
    /// Every other trip is an inference, and GH #165 makes it conditional: the
    /// process ends only when the evidence actually implicates the colony loop —
    /// a parked loop that stopped answering (`colony_loop`), or a declared work
    /// item that outlived even [`Self::work_item_budget`] (`stuck_work_item`). A
    /// window in which the witness failed the same test, in which the
    /// supervisor's own ticks were late, or in which the loop was demonstrably
    /// inside an operation it had announced, says something about the host or
    /// about one slow operation — and a watchdog that kills on it causes the
    /// outage it exists to prevent.
    pub fn is_fatal(&self, policy: WatchdogOnTrip) -> bool {
        if self.colony_task_gone {
            return true;
        }
        if policy != WatchdogOnTrip::Exit {
            return false;
        }
        matches!(self.starved(), "colony_loop" | "stuck_work_item")
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
             in_flight_work={} work_item_budget={}ms \
             witness={} witness_missed={}/{} \
             beats_seen={} armed_for={}ms colony_task={} work_item={}]",
            self.silent_periods,
            self.period.as_millis(),
            self.starved(),
            self.silent_for.as_millis(),
            self.nominal_window().as_millis(),
            self.supervisor_lag().as_millis(),
            self.in_flight_work,
            self.work_item_budget().as_millis(),
            self.witness(),
            self.witness_worst_misses,
            self.silent_periods,
            self.beats_seen,
            self.armed_for.as_millis(),
            if self.colony_task_gone {
                "gone"
            } else {
                "alive"
            },
            // GH #439: appended, never prepended — the issue-#6 prefix stays
            // byte-stable for the log scans that grep for it.
            self.work_item
                .as_ref()
                .map(WorkItem::as_str)
                .unwrap_or("none"),
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

/// GH #165 — the second observer: a task that must FINISH WORK on the same clock
/// the colony is judged by.
///
/// Once per `period` it completes one unit of real work — a trip through the run
/// queue (`yield_now`), a freshly spawned task the runtime has to schedule, and a
/// fixed, deliberately non-elidable CPU quantum — and then reports the completed
/// unit with a non-blocking `try_send`, exactly like the colony heartbeat.
///
/// The three hops are chosen because they are the ones the colony loop also needs
/// and the supervisor does not: the supervisor is woken by the time driver and
/// evaluates a `try_recv`, which a completely saturated host still delivers on
/// time. A unit of work that has to be queued, scheduled and executed does not.
///
/// Cost: one spawn plus a few microseconds of arithmetic per period — 10×/s at
/// the default. Deliberately no I/O: a witness that touched the disk would perturb
/// the contention it is supposed to measure, and would be a second failure source
/// in the one code path that must never invent a fault.
///
/// Returns when the supervisor is gone (its receiver dropped) or when the runtime
/// refuses the spawn — in both cases there is nothing left to witness for.
pub async fn run_liveness_witness(tx: tokio::sync::mpsc::Sender<()>, period: std::time::Duration) {
    let mut iv = tokio::time::interval(period);
    iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        iv.tick().await;
        // Back of the run queue first: a witness that only ever ran on the timer
        // wake would be as privileged as the supervisor it is meant to correct.
        tokio::task::yield_now().await;
        // A task the runtime has to accept, schedule and drive to completion.
        if tokio::spawn(async { witness_work_unit() }).await.is_err() {
            return;
        }
        if tx.try_send(()).is_err() && tx.is_closed() {
            return;
        }
    }
}

/// The witness's fixed work quantum: cheap, deterministic, and impossible for the
/// optimiser to delete (that is what `black_box` is for — a unit of work that got
/// compiled away would make the witness a second sleeper).
fn witness_work_unit() {
    let mut acc: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..1024u64 {
        acc = acc.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i) ^ (acc >> 29);
    }
    let _ = std::hint::black_box(acc);
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
///
/// **Corroboration (GH #165)**: `witness_rx` is the report channel of a
/// [`run_liveness_witness`] task. It is drained on the same tick and counted by
/// the same rule as the heartbeat, and the worst run of consecutive witness
/// misses since the colony's last beat rides along in the trip. A trip whose
/// witness failed the same test is reported and **never** fatal, whatever the
/// policy — the supervisor keeps supervising exactly as it does under
/// `log-only`, so a real wedge that merely coincided with a bad moment is caught
/// by the next window instead of being lost. `None` = no witness wired: the trip
/// then says `witness=absent` rather than pretending the host was fine.
pub async fn run_watchdog(
    mut heartbeat_rx: tokio::sync::mpsc::Receiver<Beat>,
    trip_tx: tokio::sync::mpsc::Sender<WatchdogTrip>,
    threshold: u32,
    period: std::time::Duration,
    armed_rx: tokio::sync::oneshot::Receiver<()>,
    on_trip: WatchdogOnTrip,
    mut witness_rx: Option<tokio::sync::mpsc::Receiver<()>>,
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
    // GH #165 witness accounting. `witness_misses` is the run in progress,
    // `witness_worst` the longest run inside the window currently being judged —
    // reset whenever a colony beat starts a fresh window, and after every trip.
    // GH #165: the loop's last declared phase. `Parked` until it says otherwise —
    // a colony that has not spoken yet is not credited with work in flight.
    let mut last_phase = Beat::Parked;
    // GH #439: the last LABEL the loop declared. Kept across bare `Working`
    // beats (a labelled operation goes on beating under its own name and the
    // loop's top-of-iteration beat is unlabelled), cleared by `Parked` — a
    // parked loop is inside nothing.
    let mut last_label: Option<WorkItem> = None;
    let mut witness_misses: u32 = 0;
    let mut witness_worst: u32 = 0;
    let mut witness_present = witness_rx.is_some();
    // Stale witness units from before arming prove a moment that is already past,
    // exactly like stale heartbeats.
    if let Some(rx) = witness_rx.as_mut() {
        while rx.try_recv().is_ok() {}
    }
    loop {
        iv.tick().await;
        // Drain every tick buffered this period; `received` = at least one. A
        // closed channel is a colony task that is gone — a strictly stronger
        // finding than silence, and it is recorded as such.
        let mut received = false;
        let mut colony_task_gone = false;
        loop {
            match heartbeat_rx.try_recv() {
                Ok(beat) => {
                    received = true;
                    beats_seen += 1;
                    // GH #165: the LAST word wins. Within one period the loop may
                    // say `Working` then `Parked`; what matters at trip time is
                    // the phase it was in when it stopped talking.
                    //
                    // GH #439: a labelled beat is a `Working` beat that also
                    // names its operation. It is normalised to `Working` here so
                    // every judgement below (`in_flight_work`, the budget, the
                    // fatality rule) reads exactly as it did — a label changes
                    // the diagnosis, never the verdict.
                    match beat {
                        Beat::WorkingOn(w) => {
                            last_phase = Beat::Working;
                            last_label = Some(w);
                        }
                        Beat::Working => last_phase = Beat::Working,
                        Beat::Parked => {
                            last_phase = Beat::Parked;
                            last_label = None;
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    colony_task_gone = true;
                    break;
                }
            }
        }
        // GH #165: the witness is evaluated on the SAME tick, before the colony
        // verdict, so the two observers always describe one and the same window.
        let mut witness_received = false;
        if let Some(rx) = witness_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(()) => witness_received = true,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        // The witness task is gone. It stops being evidence; it
                        // does not become evidence of a healthy host.
                        witness_present = false;
                        break;
                    }
                }
            }
        }
        if witness_present {
            if witness_received {
                witness_misses = 0;
            } else {
                witness_misses += 1;
                witness_worst = witness_worst.max(witness_misses);
            }
        }
        let now = tokio::time::Instant::now();
        if received {
            last_beat_at = now;
            // A beat opens a fresh window; only the witness run still in progress
            // carries over into it.
            witness_worst = witness_misses;
        }
        if wd.tick(received) == WatchdogAction::Stop {
            let trip = WatchdogTrip {
                silent_periods: threshold,
                period,
                silent_for: now.saturating_duration_since(last_beat_at),
                beats_seen,
                colony_task_gone,
                armed_for: now.saturating_duration_since(armed_at),
                in_flight_work: last_phase == Beat::Working,
                witness_present,
                witness_worst_misses: witness_worst,
                work_item: last_label.clone(),
            };
            let fatal = trip.is_fatal(on_trip);
            // `try_send`: the caller drains this channel; a full one means it has
            // already been told and the supervisor must not block on saying so.
            let _ = trip_tx.try_send(trip);
            if fatal {
                return;
            }
            // Non-fatal (log-only, or GH #165: a trip the evidence does not pin
            // on the colony): keep supervising. Only the miss counter is reset —
            // `last_beat_at` is NOT, because it is what the escalation is built
            // on: a declared work item that keeps not returning accumulates
            // `silent_for` across windows until it crosses `work_item_budget`
            // and becomes fatal. Faking the silence clock here would have made
            // that escalation impossible and every repeated trip look fresh.
            wd = Watchdog::new(threshold);
            witness_worst = witness_misses;
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
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
            None,
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
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        // Emit heartbeats faster than the period for well over `threshold` periods.
        for _ in 0..40 {
            let _ = hb_tx.try_send(Beat::Parked);
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
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            arm_rx,
            WatchdogOnTrip::Exit,
            None,
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
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            arm_rx,
            WatchdogOnTrip::Exit,
            None,
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
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        // Fill the channel with boot-time heartbeats, then never send again.
        for _ in 0..8 {
            let _ = hb_tx.try_send(Beat::Parked);
        }
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            arm_rx,
            WatchdogOnTrip::Exit,
            None,
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
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
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
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
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
            "witness=absent",
            "witness_missed=0/5",
            "in_flight_work=false",
            "work_item_budget=5000ms",
        ] {
            assert!(line.contains(needle), "missing {needle} in: {line}");
        }
    }

    /// A trip that was inside a DECLARED, named work item, for the GH #439
    /// diagnosis tests below.
    fn a_trip_in_flight() -> WatchdogTrip {
        WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(900),
            beats_seen: 12,
            colony_task_gone: false,
            armed_for: Duration::from_millis(5_000),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
        }
    }

    /// GH #439: a trip inside a NAMED work item names it. The issue-#6 prefix
    /// still survives verbatim — the diagnosis is appended, never prepended.
    #[test]
    fn the_trip_line_names_the_work_item_it_was_inside() {
        let label = "mutation 0198 scope=/os op=add_nodes[3/7] template=memory-hive cell=/os/mem";
        let trip = WatchdogTrip {
            work_item: Some(WorkItem::new(label)),
            ..a_trip_in_flight()
        };
        let line = format!("{trip}");
        assert!(
            line.starts_with(
                "colony heartbeat lost for 5 consecutive supervisor periods of 100 ms"
            ),
            "the issue-#6 prefix must survive verbatim, was: {line}"
        );
        assert!(
            line.contains(&format!("work_item={label}")),
            "a trip inside a named item must name it, was: {line}"
        );
        assert_eq!(trip.starved(), "slow_work_item");
    }

    /// An unnamed item stays unnamed — the field is evidence, not decoration.
    #[test]
    fn an_unnamed_work_item_renders_as_none() {
        let trip = a_trip_in_flight();
        assert!(format!("{trip}").contains("work_item=none"));
    }

    /// GH #439: the supervisor keeps the LAST label the loop declared, and it
    /// judges a labelled beat exactly as it judges a bare `Working` one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_supervisor_carries_the_last_declared_label_into_the_trip() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::LogOnly,
            None,
        ));
        // A short stretch of labelled beats, then silence. Not ONE beat: the
        // supervisor DISCARDS everything buffered before it arms (a beat from
        // before arming proves a moment that is already past), and a single
        // beat racing that drain is a flake, not a test.
        for _ in 0..12 {
            let _ = hb_tx.try_send(Beat::WorkingOn(WorkItem::new(
                "mutation 42 op=add_nodes[1/1]",
            )));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must trip on silence")
            .expect("a trip");
        assert_eq!(
            trip.work_item.as_ref().map(WorkItem::as_str),
            Some("mutation 42 op=add_nodes[1/1]"),
            "the trip must carry the label the loop last declared"
        );
        assert!(
            trip.in_flight_work,
            "a labelled beat is a declared work item"
        );
        assert_ne!(
            trip.starved(),
            "colony_loop",
            "a declared item is never judged as a parked loop: {trip}"
        );
        drop(hb_tx);
    }

    /// A `Parked` beat clears the label: a parked loop is inside nothing, and a
    /// stale name would be worse evidence than none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parking_clears_the_declared_label() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::LogOnly,
            None,
        ));
        // Same reason as above: beat past the arming drain, and END on `Parked`.
        for _ in 0..12 {
            let _ = hb_tx.try_send(Beat::WorkingOn(WorkItem::new("mutation 42")));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let _ = hb_tx.try_send(Beat::Parked);
        tokio::time::sleep(Duration::from_millis(15)).await;
        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must trip on silence")
            .expect("a trip");
        assert_eq!(
            trip.work_item, None,
            "a parked loop must not carry the name of a finished item: {trip}"
        );
        assert!(!trip.in_flight_work);
        drop(hb_tx);
    }

    /// GH #439: the pulse is the mutation path's way of speaking, and it is
    /// SYNC — `try_send`, never an await, so it works inside the synchronous
    /// staging functions. A silent pulse reports nowhere and never panics.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_work_pulse_beats_under_its_label_and_a_silent_one_beats_nowhere() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let pulse = WorkPulse::new(Some(tx), WorkItem::new("mutation 7 op=add_nodes[1/2]"));
        pulse.tick();
        pulse
            .with_label(WorkItem::new("mutation 7 op=add_nodes[2/2]"))
            .tick();
        let first = rx.try_recv().expect("a pulse beats");
        let second = rx.try_recv().expect("a narrowed pulse beats too");
        assert_eq!(
            first,
            Beat::WorkingOn(WorkItem::new("mutation 7 op=add_nodes[1/2]"))
        );
        assert_eq!(
            second,
            Beat::WorkingOn(WorkItem::new("mutation 7 op=add_nodes[2/2]"))
        );
        assert!(rx.try_recv().is_err(), "one tick is one beat");

        // The silent pulse is the default for every call site without a
        // heartbeat; it must be a no-op, not a panic.
        WorkPulse::silent().tick();

        // A full channel is not an error either — the supervisor needs one beat
        // per period, not all of them.
        let (tx2, rx2) = tokio::sync::mpsc::channel::<Beat>(1);
        let p2 = WorkPulse::new(Some(tx2), WorkItem::new("full"));
        for _ in 0..8 {
            p2.tick();
        }
        drop(rx2);
        p2.tick();
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
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
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
        let (_hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            3,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::LogOnly,
            None,
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
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            3,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::LogOnly,
            None,
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
    // ------------------------------------------------------------- GH #165

    /// A witness that keeps the same schedule, for the tests that are about the
    /// colony rather than the host. Feeds far faster than the supervisor period,
    /// so a missed witness period would require the feeder itself to stall for a
    /// whole period — which is the very condition the test would want to know
    /// about anyway.
    fn healthy_witness(period: Duration) -> tokio::sync::mpsc::Receiver<()> {
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(64);
        tokio::spawn(async move {
            loop {
                if tx.send(()).await.is_err() {
                    return;
                }
                tokio::time::sleep(period / 5).await;
            }
        });
        rx
    }

    /// GH #165, the false positive. A starved host is constructed EXPLICITLY —
    /// the witness channel exists and stays open, and nothing ever completes a
    /// work unit on it — so the test costs no load at all and never depends on
    /// what else the machine is doing.
    ///
    /// Both halves are asserted on one and the same silence:
    /// * the pre-#165 detector, which is exactly a trip with no witness wired,
    ///   calls this silence `colony_loop` and ENDS THE PROCESS;
    /// * the corroborated detector calls it `host_runtime` and does not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_starved_host_no_longer_reads_as_a_wedged_colony() {
        // Held for the whole test: an open channel with nothing on it is a
        // witness that is failing, not a witness that is gone.
        let (_hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (_witness_tx, witness_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
            Some(witness_rx),
        ));

        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must still report the silence")
            .expect("a suppressed trip is still a reported trip");

        assert!(
            !trip.colony_task_gone,
            "the heartbeat channel is open — this must be a silence trip: {trip:?}"
        );
        assert_eq!(trip.witness(), HostWitness::Failed, "was: {trip:?}");
        assert_eq!(trip.starved(), "host_runtime", "was: {trip:?}");
        assert!(
            !trip.is_fatal(WatchdogOnTrip::Exit),
            "a host that stopped a second, unrelated worker must not cost the \
             colony its process, even under `exit`: {trip:?}"
        );

        // The pre-#165 detector, on byte-identical silence: no witness wired is
        // precisely the state of knowledge this fix replaces.
        let pre_165 = WatchdogTrip {
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
            ..trip
        };
        assert_eq!(pre_165.starved(), "colony_loop");
        assert!(
            pre_165.is_fatal(WatchdogOnTrip::Exit),
            "the receipt is only worth something if the old detector really did \
             kill on this: {pre_165:?}"
        );

        // And it keeps supervising: a second trip must arrive, so a wedge that
        // merely coincided with a bad moment is not lost.
        let second = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must keep supervising after a suppressed trip")
            .expect("the supervisor must not have returned");
        assert!(!second.is_fatal(WatchdogOnTrip::Exit));
    }

    /// GH #165, the half that matters more: the watchdog is not switched off.
    ///
    /// A colony loop that stopped iterating while an independent worker on the
    /// same runtime kept finishing its work units is a wedge, it is named one,
    /// and it still ends the process under `exit`. The positive receipt for
    /// "ends the process" is that the supervisor RETURNS — its sender drops and
    /// the trip channel closes behind it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_wedged_colony_is_still_fatal_while_the_witness_keeps_working() {
        let period = Duration::from_millis(20);
        // Open, and permanently silent: the colony loop stopped iterating.
        let (_hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            Some(healthy_witness(period)),
        ));

        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("a wedged colony must still be detected")
            .expect("the supervisor must report before it returns");
        assert_eq!(trip.witness(), HostWitness::Kept, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_loop", "was: {trip:?}");
        assert!(
            trip.is_fatal(WatchdogOnTrip::Exit),
            "a corroborated wedge must still end the process: {trip:?}"
        );

        let after = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must stop on a fatal trip, not keep reporting");
        assert!(
            after.is_none(),
            "a fatal trip must end the supervisor, got {after:?}"
        );
    }

    /// The real supervisor with the real [`run_liveness_witness`] task: the
    /// witness is not a test fixture, it actually completes work units on a live
    /// runtime, and a wedged colony next to it is still fatal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_production_witness_corroborates_a_real_wedge() {
        let period = Duration::from_millis(20);
        let (_hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (w_tx, w_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_liveness_witness(w_tx, period / 4));
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            Some(w_rx),
        ));

        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("a wedged colony must still be detected")
            .expect("the supervisor must report before it returns");
        assert_eq!(trip.witness(), HostWitness::Kept, "was: {trip:?}");
        assert!(trip.is_fatal(WatchdogOnTrip::Exit), "was: {trip:?}");
    }

    /// The pure derivation, and the direct rebuttal of the incident record: a
    /// `supervisor_lag` of zero is not the same claim as "the host was fine".
    /// One and the same punctual-supervisor observation flips its verdict on the
    /// witness alone.
    #[test]
    fn a_punctual_supervisor_does_not_outrank_a_failing_witness() {
        // The shape of the reported incident: 5 × 100 ms, 499 ms of silence, the
        // supervisor dead on time.
        let base = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(499),
            beats_seen: 2380,
            colony_task_gone: false,
            armed_for: Duration::from_millis(234_607),
            in_flight_work: false,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
        };
        assert_eq!(base.supervisor_lag(), Duration::ZERO);
        assert_eq!(base.witness(), HostWitness::Kept);
        assert_eq!(base.starved(), "colony_loop");
        assert!(base.is_fatal(WatchdogOnTrip::Exit));

        // Same silence, same punctual supervisor — but a worker that had to
        // finish something missed as many periods as the colony did.
        let starved = WatchdogTrip {
            witness_worst_misses: 5,
            work_item: None,
            ..base
        };
        assert_eq!(starved.supervisor_lag(), Duration::ZERO);
        assert_eq!(starved.witness(), HostWitness::Failed);
        assert_eq!(starved.starved(), "host_runtime");
        assert!(!starved.is_fatal(WatchdogOnTrip::Exit));

        // The bar is the colony's own: fewer misses than the colony's silent
        // periods is not the same failure and does not excuse anything.
        let brief = WatchdogTrip {
            witness_worst_misses: 4,
            work_item: None,
            ..base
        };
        assert_eq!(brief.witness(), HostWitness::Kept);
        assert!(brief.is_fatal(WatchdogOnTrip::Exit));
    }

    /// A trip with no witness wired keeps the pre-#165 verdict exactly — an
    /// embedder that spawns no witness loses nothing and gains no false comfort.
    #[test]
    fn an_absent_witness_changes_no_verdict_and_says_so() {
        let t = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(520),
            beats_seen: 7,
            colony_task_gone: false,
            armed_for: Duration::from_secs(3),
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
        };
        assert_eq!(t.witness(), HostWitness::Absent);
        assert_eq!(t.starved(), "colony_loop");
        assert!(t.is_fatal(WatchdogOnTrip::Exit));
        assert!(!t.is_fatal(WatchdogOnTrip::LogOnly));
        assert!(format!("{t}").contains("witness=absent"));
    }

    /// A witness whose task died stops being evidence — it must not silently
    /// become evidence that the host was healthy. The receipt: the trip says
    /// `absent`, and it is fatal again (the pre-#165 verdict), rather than being
    /// suppressed forever by a witness that will never report again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dead_witness_degrades_to_absent_and_never_to_healthy() {
        let (_hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (witness_tx, witness_rx) = tokio::sync::mpsc::channel::<()>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        drop(witness_tx); // the witness task is gone
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
            Some(witness_rx),
        ));
        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the silence must still be reported")
            .expect("the supervisor must report before it returns");
        assert_eq!(trip.witness(), HostWitness::Absent, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_loop");
        assert!(trip.is_fatal(WatchdogOnTrip::Exit));
    }

    /// The witness's work unit is real work: it must not be a value the compiler
    /// can fold away, or the witness would be a second sleeper and the whole
    /// corroboration would be worthless. Cheap enough to call in a test loop.
    #[test]
    fn the_witness_work_unit_actually_runs() {
        let t0 = std::time::Instant::now();
        for _ in 0..1000 {
            witness_work_unit();
        }
        assert!(
            t0.elapsed() > Duration::ZERO,
            "a work unit that costs nothing measurable is not work"
        );
    }
    /// GH #165, the reported incident, reconstructed from its own record: a
    /// colony beating at the nominal rate for 235 s and then ONE iteration of
    /// 499 ms, with the supervisor dead on time. No load is generated — the
    /// situation is stated, which is the only way a test suite can hold a
    /// starved host still.
    ///
    /// The two halves on one and the same observation:
    /// * pre-#165 (the loop said nothing about itself) → `colony_loop`, fatal;
    /// * with the loop's own declaration → `slow_work_item`, not fatal.
    #[test]
    fn a_declared_work_item_is_slow_and_not_a_wedge() {
        let incident = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(499),
            beats_seen: 2380,
            colony_task_gone: false,
            armed_for: Duration::from_millis(234_607),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
        };
        assert_eq!(incident.supervisor_lag(), Duration::ZERO);
        assert_eq!(incident.work_item_budget(), Duration::from_secs(5));
        assert_eq!(incident.starved(), "slow_work_item");
        assert!(
            !incident.is_fatal(WatchdogOnTrip::Exit),
            "an operation that is taking long is not a reason to end the process: \
             {incident:?}"
        );

        // Exactly what the pre-#165 detector saw and did.
        let blind = WatchdogTrip {
            in_flight_work: false,
            ..incident
        };
        assert_eq!(blind.starved(), "colony_loop");
        assert!(
            blind.is_fatal(WatchdogOnTrip::Exit),
            "the receipt only counts if the old detector really killed on this"
        );
    }

    /// The other side of the same coin, and the reason the suppression is bounded:
    /// a declared work item that outlives its own budget is a wedge with a nicer
    /// name, and it is fatal again.
    #[test]
    fn a_work_item_that_outlives_its_budget_is_fatal_again() {
        let base = WatchdogTrip {
            silent_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(4_999),
            beats_seen: 2380,
            colony_task_gone: false,
            armed_for: Duration::from_secs(300),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
        };
        assert_eq!(base.starved(), "slow_work_item");
        assert!(!base.is_fatal(WatchdogOnTrip::Exit));

        let stuck = WatchdogTrip {
            silent_for: Duration::from_millis(5_000),
            ..base
        };
        assert_eq!(stuck.starved(), "stuck_work_item");
        assert!(stuck.is_fatal(WatchdogOnTrip::Exit));
        assert!(
            !stuck.is_fatal(WatchdogOnTrip::LogOnly),
            "`log-only` still means log-only for everything that is not a dead task"
        );
    }

    /// The escalation as the supervisor actually runs it: a loop that declares a
    /// work item and then never speaks again is reported as slow, again and
    /// again, until its silence crosses the budget — and then the supervisor
    /// ends it. Positive receipt on both ends: at least one non-fatal
    /// `slow_work_item` first, a fatal `stuck_work_item` after, and the
    /// supervisor returning behind it.
    ///
    /// 5 × 10 ms window, 500 ms budget: the whole escalation runs in well under a
    /// second and generates no load.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_silent_work_item_escalates_from_slow_to_stuck() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(32);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            Some(healthy_witness(period)),
        ));
        // After arming: the supervisor discards beats buffered from before it was
        // armed (they describe a moment already past), and that discards their
        // phase with them — a supervisor that has heard nothing assumes `Parked`,
        // which is the verdict that grants no grace.
        tokio::time::sleep(period).await;
        // The loop's last word: it entered a work item. Then nothing, ever.
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");

        let mut saw_slow = false;
        loop {
            let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
                .await
                .expect("the escalation must finish inside the failure marker");
            let Some(trip) = trip else {
                panic!("the supervisor returned without ever reporting a fatal trip");
            };
            if trip.is_fatal(WatchdogOnTrip::Exit) {
                assert_eq!(trip.starved(), "stuck_work_item", "was: {trip:?}");
                assert!(
                    trip.silent_for >= trip.work_item_budget(),
                    "a stuck item must have outlived its budget: {trip:?}"
                );
                break;
            }
            assert_eq!(trip.starved(), "slow_work_item", "was: {trip:?}");
            saw_slow = true;
        }
        assert!(
            saw_slow,
            "the budget must have bought the work item at least one window of grace"
        );
        // Fatal means the supervisor is done.
        let after = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("the supervisor must stop after a fatal trip");
        assert!(after.is_none(), "got {after:?}");
        drop(hb_tx);
    }

    /// A parked loop gets no such grace: it declared that it had nothing in
    /// flight, so its silence has nothing to hide behind. Same silence duration
    /// as the slow-work-item case above, opposite verdict — the phase is doing
    /// the work, not the clock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_parked_loop_that_stops_answering_is_fatal_at_the_idle_bar() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(32);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            Some(healthy_witness(period)),
        ));
        tokio::time::sleep(period).await;
        // Entered a work item, finished it, parked: the loop is idle and quiet.
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");
        hb_tx
            .try_send(Beat::Parked)
            .expect("the supervisor must be listening");

        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("a parked loop that stops answering must be detected")
            .expect("the supervisor must report before it returns");
        assert!(!trip.in_flight_work, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_loop", "was: {trip:?}");
        assert!(
            trip.silent_for < trip.work_item_budget(),
            "the idle bar must fire long before the work-item budget: {trip:?}"
        );
        assert!(trip.is_fatal(WatchdogOnTrip::Exit), "was: {trip:?}");
        drop(hb_tx);
    }

    /// The phase is a claim about the LAST thing the loop said, not about the
    /// first: a loop that entered a work item, finished it and parked must not
    /// keep the work item's budget.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_last_phase_in_a_period_is_the_one_that_counts() {
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            Duration::from_millis(10),
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Both phases inside one supervisor period, in loop order.
        let _ = hb_tx.try_send(Beat::Working);
        let _ = hb_tx.try_send(Beat::Parked);
        let trip = tokio::time::timeout(Duration::from_secs(30), trip_rx.recv())
            .await
            .expect("a trip must arrive")
            .expect("the supervisor must report");
        assert!(
            !trip.in_flight_work,
            "the loop's last word was `Parked`: {trip:?}"
        );
        drop(hb_tx);
    }

    /// GH #571 — the shipped heartbeat capacity carries a burst without losing
    /// the phase the loop is actually in.
    ///
    /// The loop declares three phases per handled event: the top-of-iteration
    /// `Working`, the `Parked` before the `select!`, and the arm's own `Working`.
    /// `beat` is a `try_send`, so once the channel is full every further
    /// declaration is silently dropped — and what survives in the buffer is the
    /// OLDEST word, not the newest. At the capacity the substrate shipped with
    /// that is eight beats, less than three events: a loop in the middle of a
    /// burst was read as one whose last word had been `Parked`. That reading is
    /// `in_flight_work == false` (see [`run_watchdog`], which keeps exactly the
    /// last phase it drained), the trip then says `starved=colony_loop`, and that
    /// verdict ends the process under the shipped `watchdog_on_trip = exit`. It
    /// is how a colony that was talking got killed for being silent at the top of
    /// every minute.
    ///
    /// Both halves are asserted: the old capacity to show the mechanism is real,
    /// [`HEARTBEAT_CAPACITY`] to show the shipped one carries the burst. The
    /// second half is what a future change to the constant is measured against.
    #[test]
    fn the_shipped_heartbeat_capacity_carries_a_burst_of_declarations() {
        /// Events in the burst — the fan-out order of magnitude of one timer tick
        /// across a member's cells.
        const BURST_EVENTS: usize = 60;

        /// One handled event, declared the way the colony loop declares it.
        fn declare_one_event(tx: &Option<tokio::sync::mpsc::Sender<Beat>>) {
            crate::colony::beat(tx, Beat::Working);
            crate::colony::beat(tx, Beat::Parked);
            crate::colony::beat(tx, Beat::Working);
        }

        /// What the supervisor takes away from the channel: it drains everything
        /// buffered in the period and keeps the LAST phase, with a labelled beat
        /// normalised to `Working` exactly as [`run_watchdog`] normalises it.
        fn last_phase_seen(rx: &mut tokio::sync::mpsc::Receiver<Beat>) -> Option<Beat> {
            let mut last = None;
            while let Ok(b) = rx.try_recv() {
                last = Some(match b {
                    Beat::WorkingOn(_) => Beat::Working,
                    other => other,
                });
            }
            last
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let shipped_before = Some(tx);
        for _ in 0..BURST_EVENTS {
            declare_one_event(&shipped_before);
        }
        assert_eq!(
            last_phase_seen(&mut rx),
            Some(Beat::Parked),
            "eight slots hold less than three events, so the supervisor is left \
             on a phase the loop left long ago — the defect GH #571 measured"
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
        let shipped = Some(tx);
        for _ in 0..BURST_EVENTS {
            declare_one_event(&shipped);
        }
        assert_eq!(
            last_phase_seen(&mut rx),
            Some(Beat::Working),
            "the shipped capacity must carry a burst of {BURST_EVENTS} events \
             ({} beats) so the supervisor reads the phase the loop is in",
            BURST_EVENTS * 3
        );
    }
}
