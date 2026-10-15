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
    /// GH #1216: the loop's last word. An orderly shutdown has ended its
    /// drain (quiescent, or cut at its budget, or never opened) and the
    /// teardown begins: closing the writer, joining the cells, the process
    /// ending. None of that is a loop iteration, so there is nothing left to
    /// supervise, and the supervisor stands down on hearing it — disarmed by
    /// this EVENT, never by a timer. Sent once, from the teardown only: a hang
    /// DURING the drain is still silence on an armed supervisor, and a colony
    /// task that dies without saying it (panic, abort) still closes its
    /// channel unannounced and trips as `colony_task_gone`.
    StandDown,
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

/// GH #968: the slot a work-item label lands in when the heartbeat channel has
/// no room for it -- the writing half, held by every [`WorkPulse`].
///
/// Measured: a `/colony/graph` read declares itself with one labelled beat
/// and then blocks. When that one beat met a full channel (a boot burst the
/// supervisor had not drained yet, or a relay that had stopped reading), the
/// `try_send` dropped it, the supervisor's last word stayed `Parked`, and the
/// trip came out `starved=colony_loop` -- the fatal verdict, for a loop that
/// had named exactly what it was inside (#968, `gh571`). A channel can only
/// drop its NEWEST word; this slot keeps exactly that word: "last label wins",
/// no room needed, nothing to block on.
///
/// A `tokio::sync::watch` channel, not shared state: the colony writes, the
/// supervisor reads, the same one-way channel discipline as the heartbeat.
/// Only a label the heartbeat channel REFUSED is written here, which is what
/// makes it newer than every beat the channel still holds: those were queued
/// before the channel filled, so before the refusal.
///
/// Known limit (review M-1 of #968): it is not newer than a beat the channel
/// refused AFTER it. A label lands here (channel full), the read ends, and its
/// `Parked` beat meets the same full channel and is dropped -- unlabelled
/// beats have no slot. The supervisor then reads `Working` plus the label
/// although the last word was `Parked`. The same holds while the supervisor
/// drains and the loop refills the room it made. Either way a real hang right
/// after reads as `slow_work_item` and turns fatal only after the work-item
/// budget (10 x the window), and the next idle beat (100 ms) of any living
/// loop corrects the picture.
#[derive(Clone, Debug)]
pub struct LabelSlot(std::sync::Arc<tokio::sync::watch::Sender<Option<WorkItem>>>);

/// GH #968: the reading half of a [`LabelSlot`], held by the supervisor.
#[derive(Debug)]
pub struct LabelSlotReader(tokio::sync::watch::Receiver<Option<WorkItem>>);

/// GH #968: a fresh label slot, both halves.
pub fn label_slot() -> (LabelSlot, LabelSlotReader) {
    let (tx, rx) = tokio::sync::watch::channel(None);
    (LabelSlot(std::sync::Arc::new(tx)), LabelSlotReader(rx))
}

impl LabelSlot {
    /// Put `label` in the slot, replacing whatever the supervisor has not read
    /// yet. Never blocks, never fails: a reader that is gone is a watchdog that
    /// is gone, and there is nobody left to tell.
    pub fn put(&self, label: WorkItem) {
        self.0.send_replace(Some(label));
    }
}

impl LabelSlotReader {
    /// The label put since the last call, if any.
    pub fn take(&mut self) -> Option<WorkItem> {
        match self.0.has_changed() {
            Ok(true) => self.0.borrow_and_update().clone(),
            _ => None,
        }
    }

    /// Another reader on the same slot -- a test relay that has to see what the
    /// supervisor sees. Starts where this one stands.
    pub fn observe(&self) -> Self {
        Self(self.0.clone())
    }

    /// Wait until a label is put; `None` once the writing side is gone.
    pub async fn changed(&mut self) -> Option<WorkItem> {
        match self.0.changed().await {
            Ok(()) => self.0.borrow_and_update().clone(),
            Err(_) => None,
        }
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
    /// GH #968: where the label goes when `tx` has no room for it.
    slot: Option<LabelSlot>,
    /// GH #1176: the work-item budget of the supervisor this pulse reports
    /// to, so an operation timeout under it can be set ABOVE it (a hang is
    /// declared stuck first). `None` for a pulse with no supervisor.
    budget: Option<std::time::Duration>,
    /// Where the supervisor reads the CPU denial from, when it is not the
    /// kernel ([`RunDelays::read`]) -- a test's host (GH #1199). The scan
    /// timeout under the budget credits the denial from the same source the
    /// supervisor deducts it from.
    denial: Option<DenialSource>,
}

/// A host's CPU denial as the supervisor reads it ([`RunDelays`]), shared by a
/// supervisor and the pulses that report to it.
pub(crate) type DenialSource = std::sync::Arc<dyn Fn() -> RunDelays + Send + Sync>;

impl WorkPulse {
    /// A pulse that reports to `tx` under `label`.
    pub fn new(tx: Option<tokio::sync::mpsc::Sender<Beat>>, label: WorkItem) -> Self {
        Self {
            tx,
            label,
            slot: None,
            budget: None,
            denial: None,
        }
    }

    /// GH #968: the same pulse with a slot for the label a full channel would
    /// drop. `None` keeps the pulse as it was.
    pub fn with_slot(mut self, slot: Option<LabelSlot>) -> Self {
        self.slot = slot;
        self
    }

    /// GH #1176: the same pulse, knowing the work-item budget of its
    /// supervisor ([`WatchdogTrip::work_item_budget`]).
    pub fn with_budget(mut self, budget: std::time::Duration) -> Self {
        self.budget = Some(budget);
        self
    }

    /// The work-item budget of the supervisor, when the pulse knows it.
    pub fn budget(&self) -> Option<std::time::Duration> {
        self.budget
    }

    /// GH #1199: the same pulse, reading the CPU denial where its supervisor
    /// reads it -- a test's host instead of the kernel.
    #[cfg(test)]
    pub(crate) fn with_denial(mut self, denial: DenialSource) -> Self {
        self.denial = Some(denial);
        self
    }

    /// GH #1199: the CPU denial of this process now, from the source the
    /// supervisor reads ([`RunDelays::read`] unless a test named another).
    pub fn read_denial(&self) -> RunDelays {
        self.denial
            .as_ref()
            .map_or_else(RunDelays::read, |read| read())
    }

    /// A pulse that reports nowhere — the default for every call site with no
    /// heartbeat wired (tests, `--validate`, the boot growth, which runs before
    /// the supervisor is armed).
    pub fn silent() -> Self {
        Self {
            tx: None,
            label: WorkItem::new("<unlabelled>"),
            slot: None,
            budget: None,
            denial: None,
        }
    }

    /// One non-blocking beat. Never awaits, never blocks, never panics: a full
    /// channel means the supervisor has not drained this period yet and needs
    /// only one beat per period -- but it needs the newest LABEL, so a label
    /// the channel refuses goes to the slot (GH #968).
    pub fn tick(&self) {
        let Some(t) = &self.tx else {
            return;
        };
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(Beat::WorkingOn(w))) =
            t.try_send(Beat::WorkingOn(self.label.clone()))
            && let Some(slot) = &self.slot
        {
            slot.put(w);
        }
    }

    /// The same pulse under a narrower label — one mutation, one cell at a time.
    pub fn with_label(&self, label: WorkItem) -> Self {
        Self {
            tx: self.tx.clone(),
            label,
            slot: self.slot.clone(),
            budget: self.budget,
            denial: self.denial.clone(),
        }
    }

    /// The label this pulse currently reports under.
    pub fn label(&self) -> &WorkItem {
        &self.label
    }

    /// GH #1216: say [`Beat::StandDown`] — the colony loop's last word, sent
    /// by the shutdown teardown before it starts. Never blocks the teardown:
    /// the common case is room in the channel. A full channel (a supervisor
    /// that has not drained this period, or one not yet armed) gets the word
    /// from a task of its own that waits for room, so it is never dropped and
    /// still lands BEHIND every beat already queued; a supervisor that is gone
    /// makes that task end at once.
    pub fn stand_down(&self) {
        let Some(t) = &self.tx else {
            return;
        };
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(word)) = t.try_send(Beat::StandDown)
        {
            let t = t.clone();
            tokio::spawn(async move {
                let _ = t.send(word).await;
            });
        }
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
/// factor bounds the suppression, it does not remove it. The bar is counted in
/// evaluated supervisor periods ([`WatchdogTrip::budget_periods`]), so a
/// supervisor that runs late delays it in wall time and never lifts it; the
/// CPU the host denied the process stretches it by at most
/// [`WORK_ITEM_DENIAL_FACTOR`] (GH #1216). The same number of periods bounds
/// the credit a parked loop gets for a late supervisor (tv-br M10o).
///
/// The verdict lands at the latest one window after the bar (tv-br M10q-a):
/// tokio keeps the tick grid for up to 5 ms of delay and the supervisor reads
/// `now` after the tick, so at the tick edge `silent_for` can lie
/// milliseconds below the evaluated periods (`silent_for=9999ms`, gh47) and
/// the trip at the bar still reads `slow_work_item`; the next trip, one
/// window later, does not. Pinned in
/// `the_bounds_hold_one_window_late_at_the_tick_edge`.
pub const WORK_ITEM_BUDGET_FACTOR: u32 = 10;

/// The most a CPU denial may stretch a work item's budget in wall time (GH
/// #1216, tv-br M10l): the denial deduction leaves at least
/// `1 / WORK_ITEM_DENIAL_FACTOR` of every silence charged to the item, whatever
/// the kernel reports as denied. (The lag deduction leaves every evaluated
/// period charged; the item is charged the smaller of the two remainders.)
///
/// The denial is read process-wide (the largest `run_delay` growth of any
/// thread, [`RunDelays::max_growth_since`]) and cannot be pinned to the item's
/// own thread -- tokio tasks move between workers. So a deadlock next to a
/// thread the host keeps off the CPU is credited that thread's denial. Without
/// this floor a host denying the whole silence would hide a hung item for
/// ever; with it, an in-flight item is `stuck_work_item` at the latest one
/// window after `WORK_ITEM_DENIAL_FACTOR × budget_periods` evaluated periods
/// however late the supervisor runs (tv-br M10o; the window is the tick edge,
/// see [`WORK_ITEM_BUDGET_FACTOR`]) -- `WORK_ITEM_DENIAL_FACTOR ×
/// work_item_budget` of silence plus at most one window where it keeps its
/// schedule.
pub const WORK_ITEM_DENIAL_FACTOR: u32 = 5;

/// The part of `denied` a silence of `silent_for` is not charged: the denial,
/// capped at `silent_for × (1 − 1/WORK_ITEM_DENIAL_FACTOR)` -- see
/// [`WORK_ITEM_DENIAL_FACTOR`]. One rule for the supervisor's verdict and for
/// the scan timeout that must stay above it (GH #1199).
pub(crate) fn denial_deduction(
    silent_for: std::time::Duration,
    denied: std::time::Duration,
) -> std::time::Duration {
    let floor = silent_for / WORK_ITEM_DENIAL_FACTOR;
    denied.min(silent_for.saturating_sub(floor))
}

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
    /// GH #1172: supervisor periods actually evaluated since the last observed
    /// heartbeat. Equals `silent_periods` on the first trip of a silence and
    /// keeps growing across the non-fatal trips that follow it, because the
    /// silence clock is not reset by a trip (see [`run_watchdog`]). It is what
    /// [`Self::supervisor_lag`] is measured against.
    pub observed_periods: u32,
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
    /// GH #1216 (tv-br M10h): CPU the host denied this process inside the
    /// silence -- the largest growth of any thread's `run_delay` (time spent
    /// runnable but off the CPU, `/proc/self/task/*/schedstat`; CFS quota
    /// throttling counts) since the first silent period. Measured by the
    /// kernel, never inferred from a clock; `ZERO` where there is no procfs.
    /// The evidence as read; the item is credited at most
    /// [`Self::denial_deduction`] of it. See [`Self::work_item_silence`].
    pub cpu_denied: std::time::Duration,
}

impl WatchdogTrip {
    /// The deadline the trip was measured against: `silent_periods × period`.
    pub fn nominal_window(&self) -> std::time::Duration {
        self.period
            .checked_mul(self.silent_periods)
            .unwrap_or(std::time::Duration::MAX)
    }

    /// How much longer the silence lasted than the supervisor periods that were
    /// actually evaluated inside it: the supervisor's own lateness.
    ///
    /// Structurally `>= 0`. A lag near zero means the supervisor ran on schedule
    /// and only the colony went quiet. A lag on the order of the window itself
    /// means the supervisor's own ticks were late — the process, not the colony
    /// loop, lost the CPU.
    ///
    /// GH #1172: measured against [`Self::observed_periods`], not against the
    /// first window. The silence clock survives non-fatal trips (the escalation
    /// from `slow_work_item` to `stuck_work_item` is built on that), so the old
    /// `silent_for - nominal_window` grew by one window on every repeated trip
    /// of the same item with a punctual supervisor: the lab trip that opened the
    /// issue said `supervisor_lag=4739ms` for 40 evaluated periods in 5239 ms,
    /// of which 1239 ms were the supervisor's.
    pub fn supervisor_lag(&self) -> std::time::Duration {
        let evaluated = self
            .period
            .checked_mul(self.observed_periods)
            .unwrap_or(std::time::Duration::MAX);
        self.silent_for.saturating_sub(evaluated)
    }

    /// The silence charged to the in-flight work item: [`Self::silent_for`]
    /// minus [`Self::supervisor_lag`], i.e. `min(silent_for, observed_periods ×
    /// period)`.
    ///
    /// GH #1172: what the supervisor sleeps through is not silence of the work
    /// item — on a host without CPU the item could not have spoken either.
    ///
    /// Fail-closed by construction: the lag deduction is bounded per evaluated
    /// period, so every period the supervisor does evaluate charges one full
    /// period to the item -- as long as `silent_for` reaches them; at the tick
    /// edge it lies milliseconds below (see [`WORK_ITEM_BUDGET_FACTOR`]).
    /// Without a CPU denial a hung item is stuck at the latest one window after
    /// `work_item_budget / period` evaluated periods however late each of them
    /// is; a supervisor that lags permanently delays the verdict in wall time
    /// and can never hide the item. Subtracting a lag derived from the total silence instead
    /// (the pre-#1172 `supervisor_lag`) would have left every repeated trip at
    /// one window of charged silence — no item could ever have been stuck.
    ///
    /// GH #1216 (tv-br M10h): and minus the CPU the host denied
    /// ([`Self::denial_deduction`] of [`Self::cpu_denied`]), whichever of the
    /// two deductions is larger (they overlap: a supervisor that was off the
    /// CPU is part of the denial). A gap in scheduling is not a hang -- under
    /// a CPU quota the supervisor and the witness each got their slice on
    /// time while a 14 s instantiation got a sliver of one core, and the wall
    /// clock declared it stuck after 5 s (`stuck_work_item supervisor_lag=0ms
    /// witness=kept`, TV-F7, 4 of 5 runs). The item is charged with the time
    /// it could have used.
    ///
    /// The denial is bounded, not exact: it is read process-wide (the largest
    /// growth of any thread), so a hung item is credited the denial of every
    /// other thread too -- a deadlock beside a throttled thread, a spin that
    /// itself loses `1 − quota` of the CPU. The deduction for the denial is
    /// therefore capped at `silent_for × (1 − 1/WORK_ITEM_DENIAL_FACTOR)`
    /// (tv-br M10l). The denial deduction always leaves a fifth of the
    /// silence charged, the lag deduction every evaluated period, and the item
    /// is charged the smaller remainder: at least a fifth of the smaller of
    /// `silent_for` and the evaluated periods (tv-br M10o, M10q-a). A hung item
    /// is therefore stuck at the latest one window after
    /// [`WORK_ITEM_DENIAL_FACTOR`] × [`Self::budget_periods`] evaluated
    /// periods, however much the host denies and however late the supervisor
    /// runs -- [`WORK_ITEM_DENIAL_FACTOR`] × [`Self::work_item_budget`] of
    /// silence plus at most one window where the supervisor keeps its schedule
    /// (the window is the tick edge, see [`WORK_ITEM_BUDGET_FACTOR`]). The
    /// denial delays the verdict by at most that factor and never hides the
    /// item.
    pub fn work_item_silence(&self) -> std::time::Duration {
        self.silent_for
            .saturating_sub(self.supervisor_lag().max(self.denial_deduction()))
    }

    /// The part of [`Self::cpu_denied`] the item is not charged: the denial,
    /// capped at `silent_for × (1 − 1/WORK_ITEM_DENIAL_FACTOR)` (GH #1216, tv-br
    /// M10l). See [`WORK_ITEM_DENIAL_FACTOR`].
    pub fn denial_deduction(&self) -> std::time::Duration {
        denial_deduction(self.silent_for, self.cpu_denied)
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
        self.period
            .checked_mul(self.budget_periods())
            .unwrap_or(std::time::Duration::MAX)
    }

    /// [`Self::work_item_budget`] counted in evaluated supervisor periods:
    /// `silent_periods × WORK_ITEM_BUDGET_FACTOR` (GH #1216, tv-br M10o).
    pub fn budget_periods(&self) -> u32 {
        self.silent_periods.saturating_mul(WORK_ITEM_BUDGET_FACTOR)
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
    ///   Both compare [`Self::work_item_silence`] (GH #1172), so a supervisor
    ///   that was itself off the CPU does not push an item over its budget.
    /// * `process_scheduling` — the supervisor's own ticks fell at least one
    ///   window behind the periods it evaluated ([`Self::supervisor_lag`]; on the
    ///   first trip of a silence: periods at least twice as slow as configured),
    ///   so the whole process was descheduled and the colony loop is not singled
    ///   out by this observation. GH #1172: on a follow-up trip of the same
    ///   silence a punctual supervisor no longer reads as late, so a parked
    ///   silence that outlasts its first trip is `colony_loop`. The lag is
    ///   summed over the silence and only a beat resets it, so a parked loop
    ///   is credited it for at most [`Self::budget_periods`] evaluated periods
    ///   (tv-br M10o): from there on the silence is `colony_loop` however late
    ///   the supervisor is (unless the witness failed too: `host_runtime`).
    /// * `colony_loop` — the loop was parked with nothing in flight when it
    ///   stopped answering, the witness kept finishing work, and the supervisor
    ///   kept its schedule -- or its lag is no longer credited, because the same
    ///   silence already lasts [`Self::budget_periods`] evaluated periods
    ///   (tv-br M10o, M10q-a). This is the only silence that
    ///   implicates the colony. Its limit: a cell that blocks its runtime worker
    ///   synchronously can hold the colony task in that worker's LIFO slot,
    ///   which no other worker may steal, while the witness runs on another
    ///   worker (GH #866) -- which is why neither the spawn path of `code` and
    ///   `bash` nor a cell's `cell.db` close runs on a worker. The other stdio
    ///   children (`StdioChild::spawn`) still spawn and journal on the worker
    ///   (`reg:stdio-child-spawn-off-worker` in `docs/defer-register.md`).
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
            // GH #1172: the budget is compared against the silence charged to
            // the item, never against the supervisor's own lateness.
            if self.work_item_silence() < self.work_item_budget() {
                "slow_work_item"
            } else {
                "stuck_work_item"
            }
        } else if self.supervisor_lag() >= self.nominal_window()
            && self.observed_periods < self.budget_periods()
        {
            // GH #1216 (tv-br M10i): a parked loop is not granted the CPU
            // denial. It declared nothing in flight, so it has nothing to do
            // but answer; and a wall-clock silence a millisecond short of the
            // window (`silent_for=9999ms`, the beat at the edge of a tick)
            // must not read as a denial (M10h did, and parked trips went
            // log-only one window late: gh47, seven unit pins).
            //
            // tv-br M10o: and the supervisor's lateness is credited for at
            // most `budget_periods` evaluated periods, the bar of an item in
            // flight. The lag is summed over the silence and only a beat
            // resets it, so without the bound one stall of a window, or a
            // supervisor late on every tick, kept a real wedge log-only for
            // ever.
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
    /// outage it exists to prevent. That holds up to the bounds: a late
    /// supervisor and an announced operation are credited for at most
    /// [`Self::budget_periods`] evaluated periods ([`Self::work_item_budget`]
    /// for the operation, stretched by at most [`WORK_ITEM_DENIAL_FACTOR`] for
    /// a CPU denial); past them the trip is fatal however late the supervisor
    /// is (tv-br M10o, M10q-a).
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
             beats_seen={} armed_for={}ms colony_task={} work_item={} cpu_denied={}ms]",
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
            // GH #1216: appended like GH #439's label, after it.
            self.cpu_denied.as_millis(),
        )
    }
}

/// GH #1216 (tv-br M10h): the kernel's `run_delay` of every thread of this
/// process, in ns -- the time each thread spent runnable but off the CPU
/// (`/proc/self/task/<tid>/schedstat`, second field; a CFS quota's throttled
/// time counts, measured on a 20 % scope: 2.40 s of 3.00 s).
///
/// This is the evidence the supervisor and the witness cannot give: under a
/// CPU quota both get their slice on time while the colony's worker waits for
/// its own. Read only at the edges of a silence, never per period, and from
/// procfs (memory, no disk): a read cannot block on anything the colony does.
/// Empty where there is no procfs -- the growth is then zero, the pre-#1216
/// verdict.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunDelays(std::collections::BTreeMap<u32, u64>);

impl RunDelays {
    /// This process's threads, now.
    pub fn read() -> Self {
        let mut out = std::collections::BTreeMap::new();
        let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
            return Self(out);
        };
        for entry in dir.flatten() {
            let Some(tid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
                continue;
            };
            if let Some(ns) = std::fs::read_to_string(entry.path().join("schedstat"))
                .ok()
                .as_deref()
                .and_then(parse_run_delay)
            {
                out.insert(tid, ns);
            }
        }
        Self(out)
    }

    /// From explicit `(tid, run_delay ns)` rows -- a test's or a fixture's.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, u64)>) -> Self {
        Self(rows.into_iter().collect())
    }

    /// The largest growth of any one thread's `run_delay` since `base`. A
    /// thread born since counts from zero (all its delay is inside the
    /// window); one that is gone counts nothing. The MAX, not the sum: the
    /// threads wait side by side, and the question is how long the host kept
    /// a runnable thread of this process off the CPU.
    pub fn max_growth_since(&self, base: &Self) -> std::time::Duration {
        let ns = self
            .0
            .iter()
            .map(|(tid, now)| now.saturating_sub(base.0.get(tid).copied().unwrap_or(0)))
            .max()
            .unwrap_or(0);
        std::time::Duration::from_nanos(ns)
    }
}

/// The `run_delay` (ns) of one `schedstat` line: `<on-cpu> <run_delay> <slices>`.
pub fn parse_run_delay(line: &str) -> Option<u64> {
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Counts consecutive missed heartbeats. After `threshold` misses in a row →
/// `Stop`. Any received heartbeat resets the counter. Pure — no I/O, no tasks.
pub struct Watchdog {
    threshold: u32,
    consecutive_misses: u32,
}

impl Watchdog {
    /// `threshold` consecutive misses trigger a `Stop`. An idle colony beats
    /// twice per supervisor period (`ColonyConfig::heartbeat_interval`, GH
    /// #1099); the production supervisor uses `threshold = 5` (~0.5 s of
    /// silence at the default 100 ms period).
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
/// compiled away would make the witness a second sleeper). Returns the value only
/// the full quantum produces, so a test can see that the work ran.
fn witness_work_unit() -> u64 {
    let mut acc: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..1024u64 {
        acc = acc.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i) ^ (acc >> 29);
    }
    std::hint::black_box(acc)
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
    heartbeat_rx: tokio::sync::mpsc::Receiver<Beat>,
    trip_tx: tokio::sync::mpsc::Sender<WatchdogTrip>,
    threshold: u32,
    period: std::time::Duration,
    armed_rx: tokio::sync::oneshot::Receiver<()>,
    on_trip: WatchdogOnTrip,
    witness_rx: Option<tokio::sync::mpsc::Receiver<()>>,
) {
    run_watchdog_with_label_slot(
        heartbeat_rx,
        trip_tx,
        threshold,
        period,
        armed_rx,
        on_trip,
        witness_rx,
        None,
    )
    .await
}

/// GH #968: the last word the supervisor has heard -- phase, label, count --
/// and the one place it is read from the two channels the loop speaks on.
///
/// GH #165: the LAST word wins. Within one period the loop may say `Working`
/// then `Parked`; what matters at trip time is the phase it was in when it
/// stopped talking. GH #439: a labelled beat is a `Working` beat that also
/// names its operation; it is normalised to `Working` so every judgement
/// (`in_flight_work`, the budget, the fatality rule) reads exactly as before
/// -- a label changes the diagnosis, never the verdict. The label is kept
/// across bare `Working` beats and cleared by `Parked`.
#[derive(Debug)]
pub(crate) struct LastWord {
    pub(crate) phase: Beat,
    pub(crate) label: Option<WorkItem>,
    pub(crate) beats_seen: u64,
    /// GH #1216: the loop said [`Beat::StandDown`]; the supervisor returns
    /// without a verdict.
    pub(crate) stood_down: bool,
}

/// What one [`LastWord::drain`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Drained {
    /// At least one word arrived this period.
    pub(crate) received: bool,
    /// The heartbeat channel is closed: the colony task is gone.
    pub(crate) gone: bool,
}

impl LastWord {
    /// A colony that has not spoken yet is not credited with work in flight.
    pub(crate) fn new() -> Self {
        Self {
            phase: Beat::Parked,
            label: None,
            beats_seen: 0,
            stood_down: false,
        }
    }

    fn hear(&mut self, beat: Beat) {
        self.beats_seen += 1;
        match beat {
            Beat::WorkingOn(w) => {
                self.phase = Beat::Working;
                self.label = Some(w);
            }
            Beat::Working => self.phase = Beat::Working,
            Beat::Parked => {
                self.phase = Beat::Parked;
                self.label = None;
            }
            Beat::StandDown => self.stood_down = true,
        }
    }

    /// Drain every beat buffered this period, then the label slot. The slot is
    /// read LAST because a label lands there only when the channel refused it,
    /// i.e. after every beat the channel still holds (see [`LabelSlot`]).
    pub(crate) fn drain(
        &mut self,
        heartbeat_rx: &mut tokio::sync::mpsc::Receiver<Beat>,
        slot: Option<&mut LabelSlotReader>,
    ) -> Drained {
        let mut out = Drained {
            received: false,
            gone: false,
        };
        loop {
            match heartbeat_rx.try_recv() {
                Ok(beat) => {
                    out.received = true;
                    self.hear(beat);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    out.gone = true;
                    break;
                }
            }
        }
        if let Some(w) = slot.and_then(LabelSlotReader::take) {
            out.received = true;
            self.hear(Beat::WorkingOn(w));
        }
        out
    }
}

/// [`run_watchdog`] plus the reading half of the colony's [`LabelSlot`]
/// (GH #968): a label the heartbeat channel refused still becomes the
/// supervisor's last word, so a trip inside a declared work item names it
/// even when the channel was full. `None` = [`run_watchdog`].
#[allow(clippy::too_many_arguments)]
pub async fn run_watchdog_with_label_slot(
    heartbeat_rx: tokio::sync::mpsc::Receiver<Beat>,
    trip_tx: tokio::sync::mpsc::Sender<WatchdogTrip>,
    threshold: u32,
    period: std::time::Duration,
    armed_rx: tokio::sync::oneshot::Receiver<()>,
    on_trip: WatchdogOnTrip,
    witness_rx: Option<tokio::sync::mpsc::Receiver<()>>,
    label_slot: Option<LabelSlotReader>,
) {
    run_watchdog_reading_denial(
        heartbeat_rx,
        trip_tx,
        threshold,
        period,
        armed_rx,
        on_trip,
        witness_rx,
        label_slot,
        RunDelays::read,
    )
    .await
}

/// [`run_watchdog_with_label_slot`] with the source of the CPU denial named
/// (GH #1216, tv-br M10i): production reads the kernel ([`RunDelays::read`]);
/// a pin hands in a host that denies a known amount, the one thing a unit
/// test cannot make a real kernel do on demand.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_watchdog_reading_denial(
    mut heartbeat_rx: tokio::sync::mpsc::Receiver<Beat>,
    trip_tx: tokio::sync::mpsc::Sender<WatchdogTrip>,
    threshold: u32,
    period: std::time::Duration,
    armed_rx: tokio::sync::oneshot::Receiver<()>,
    on_trip: WatchdogOnTrip,
    mut witness_rx: Option<tokio::sync::mpsc::Receiver<()>>,
    mut label_slot: Option<LabelSlotReader>,
    read_denial: impl Fn() -> RunDelays,
) {
    // Disarmed until boot says otherwise. `Err` = the arming sender was dropped
    // (the boot never finished) → nothing to guard, exit silently.
    if armed_rx.await.is_err() {
        return;
    }
    // GH #968: a label put during boot is as stale as a boot beat.
    // GH #1022: taken BEFORE the channel is drained, not after. The drain makes
    // room, and room in the channel is the first sign of an armed supervisor; a
    // read declared in that instant meets a still-full channel, so its label goes
    // to the slot -- and the old order then took it away here as stale. Measured
    // on a 24-thread lane under `stress-ng --cpu 24`: 3 of 100 runs of gh571
    // `a_trip_inside_a_read_names_the_endpoint`, each with the label in the slot
    // 1-3 ms after the read, one beat ever heard, 120 trips without a name.
    if let Some(slot) = label_slot.as_mut() {
        let _ = slot.take();
    }
    // Heartbeats buffered during boot prove liveness of a moment that is already
    // past. Drop them so the first evaluated period is a fresh observation.
    // GH #1216: all but one — a stand-down is not stale: a colony that shut
    // down before the supervisor was armed has nothing left to guard.
    while let Ok(beat) = heartbeat_rx.try_recv() {
        if beat == Beat::StandDown {
            return;
        }
    }
    let mut wd = Watchdog::new(threshold);
    let mut iv = tokio::time::interval(period);
    iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // GH #84 evidence trail. `armed_at` and `last_beat_at` are read from the same
    // clock the interval runs on, so `silent_for` measures the supervisor's OWN
    // lateness as well as the colony's silence — which is exactly what separates
    // "the colony loop wedged" from "the process lost the CPU".
    let armed_at = tokio::time::Instant::now();
    let mut last_beat_at = armed_at;
    // GH #165 witness accounting. `witness_misses` is the run in progress,
    // `witness_worst` the longest run inside the window currently being judged —
    // reset whenever a colony beat starts a fresh window, and after every trip.
    // GH #165 / GH #439 / GH #968: the loop's last word (see [`LastWord`]).
    let mut last = LastWord::new();
    let mut witness_misses: u32 = 0;
    let mut witness_worst: u32 = 0;
    let mut witness_present = witness_rx.is_some();
    // GH #1172: periods evaluated since the last beat. Reset by a beat only --
    // like `last_beat_at`, and for the same reason: a non-fatal trip must not
    // make a silence that continues look fresh.
    let mut observed_periods: u32 = 0;
    // GH #1216: every thread's `run_delay`, read at the first silent period of
    // a silence (a period late, so the deduction errs on the charged side) and
    // kept, like `last_beat_at`, until a beat ends the silence.
    let mut denial_base: Option<RunDelays> = None;
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
        let Drained {
            received,
            gone: colony_task_gone,
        } = last.drain(&mut heartbeat_rx, label_slot.as_mut());
        // GH #1216: the teardown has begun; it is not a loop iteration and the
        // closed channel that ends it is not a death. Stand down before any
        // verdict on this period.
        if last.stood_down {
            return;
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
            observed_periods = 0;
            denial_base = None;
            // A beat opens a fresh window; only the witness run still in progress
            // carries over into it.
            witness_worst = witness_misses;
        } else {
            observed_periods = observed_periods.saturating_add(1);
            if denial_base.is_none() {
                denial_base = Some(read_denial());
            }
        }
        if wd.tick(received) == WatchdogAction::Stop {
            let silent_for = now.saturating_duration_since(last_beat_at);
            let cpu_denied = denial_base
                .as_ref()
                .map(|base| read_denial().max_growth_since(base))
                .unwrap_or_default()
                .min(silent_for);
            let trip = WatchdogTrip {
                silent_periods: threshold,
                period,
                silent_for,
                observed_periods,
                beats_seen: last.beats_seen,
                colony_task_gone,
                armed_for: now.saturating_duration_since(armed_at),
                in_flight_work: last.phase == Beat::Working,
                witness_present,
                witness_worst_misses: witness_worst,
                work_item: last.label.clone(),
                cpu_denied,
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

    /// GH #968: a label the full heartbeat channel refuses still becomes the
    /// supervisor's last word -- through the slot, read after every beat the
    /// channel holds -- and a label that fits the channel leaves the slot alone.
    #[test]
    fn a_full_heartbeat_channel_keeps_the_latest_label() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Beat>(4);
        let (slot, mut reader) = label_slot();
        while tx.try_send(Beat::Parked).is_ok() {}
        let pulse = WorkPulse::new(Some(tx.clone()), WorkItem::new("colony-read /colony/graph"))
            .with_slot(Some(slot));
        pulse.tick();

        let mut last = LastWord::new();
        let d = last.drain(&mut rx, Some(&mut reader));
        assert_eq!(
            d,
            Drained {
                received: true,
                gone: false
            }
        );
        assert_eq!(last.phase, Beat::Working, "{last:?}");
        assert_eq!(
            last.label.as_ref().map(WorkItem::as_str),
            Some("colony-read /colony/graph"),
            "the refused label is the last word: {last:?}"
        );
        assert_eq!(last.beats_seen, 5, "four queued beats and the label");

        // The slot is spent: the next word is the next word.
        tx.try_send(Beat::Parked).unwrap();
        last.drain(&mut rx, Some(&mut reader));
        assert_eq!(
            (last.phase.clone(), last.label.clone()),
            (Beat::Parked, None)
        );

        // A label with room goes on the channel, not into the slot.
        pulse.tick();
        assert!(
            reader.take().is_none(),
            "a label that fits leaves the slot alone"
        );
        last.drain(&mut rx, Some(&mut reader));
        assert_eq!(last.phase, Beat::Working);
    }

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
        let res = meclaw_testing::deadline::hang(
            "watchdog must report a trip when heartbeats cease",
            trip_rx.recv(),
        )
        .await;
        assert!(
            res.is_some(),
            "watchdog must report a trip when heartbeats cease, got {res:?}"
        );
        drop(hb_tx);
    }

    /// Supervisor plumbing: while heartbeats keep flowing, `run_watchdog` never
    /// fires the stop signal.
    // tv-br M10x: virtual time (AGENTS.md rule 11, time logic). On the wall
    // clock the 5 ms beats raced the 50 ms silence window: a CPU quota that
    // stalls the sender for one window trips a healthy colony (the same window
    // went red in `the_supervisor_carries_the_last_declared_label_into_the_trip`,
    // CPUQuota 80 %, M10w). In virtual time every beat lands half way between
    // two ticks, so each period hears two of them, and the trip after the last
    // one is the positive receipt that the supervisor was watching all along.
    #[tokio::test(start_paused = true)]
    async fn supervisor_does_not_stop_while_heartbeats_flow() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        // Past the arming drain (the supervisor arms and drains at 0 ms, on the
        // first yield), and off the tick instants: beats at 2.5 ms + n × 5 ms.
        tokio::time::sleep(period / 4).await;
        // Heartbeats twice per period for eight times `threshold` periods.
        for _ in 0..40 {
            hb_tx
                .try_send(Beat::Parked)
                .expect("two beats per period fit the channel");
            tokio::time::sleep(period / 2).await;
        }
        assert!(
            trip_rx.try_recv().is_err(),
            "watchdog must NOT trip while heartbeats flow"
        );
        // The last beat (197.5 ms) is heard at the 200 ms tick; five silent
        // periods later, and not one earlier, the same supervisor trips.
        let trip = meclaw_testing::deadline::hang(
            "the supervisor must trip once the beats stop",
            trip_rx.recv(),
        )
        .await
        .expect("a trip");
        assert_eq!(trip.beats_seen, 40, "every beat was heard: {trip}");
        assert_eq!(trip.silent_for, period * 5, "{trip}");
        assert_eq!(trip.armed_for, period * 25, "{trip}");
        drop(hb_tx);
    }

    /// Issue #6, defect 1: a boot may take arbitrarily long and emits no
    /// heartbeat while the colony task is still coming up. An unarmed watchdog
    /// must therefore be unable to trip — no matter how long that takes.
    ///
    /// Semantic discriminator (kept tight on purpose): the armed deadline here is
    /// `5 × 10 ms = 50 ms`, and the simulated boot is 500 ms of total silence —
    /// ten times the deadline. A watchdog armed at spawn time (the pre-fix
    /// behaviour) would have tripped ten times over.
    // welle-val2 v2-fix-b (#1199): virtual time (AGENTS.md rule 11, time logic).
    // In real time "no trip" was read after a 500 ms window -- absence over a
    // window, which a supervisor descheduled past the window would pass for the
    // wrong reason. On the paused clock every wait point is a tokio timer or
    // channel, the 500 ms are exact, and the absence is the fact at that instant.
    #[tokio::test(start_paused = true)]
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
    // welle-val2 v2-fix-b (#1199): virtual time like its other half above; the
    // 100 ms boot is a step of the paused clock, not a sleep before a read.
    #[tokio::test(start_paused = true)]
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
        let res = meclaw_testing::deadline::hang(
            "an armed watchdog must trip on colony silence",
            trip_rx.recv(),
        )
        .await;
        assert!(
            res.is_some(),
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
        let res = meclaw_testing::deadline::hang(
            "buffered boot heartbeats must not keep a dead colony alive",
            trip_rx.recv(),
        )
        .await;
        assert!(
            res.is_some(),
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(520),
            beats_seen: 400,
            colony_task_gone: false,
            armed_for: Duration::from_secs(40),
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(3100),
            beats_seen: 417,
            colony_task_gone: false,
            armed_for: Duration::from_millis(42_300),
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(900),
            beats_seen: 12,
            colony_task_gone: false,
            armed_for: Duration::from_millis(5_000),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
        }
    }

    /// GH #439: a trip inside a NAMED work item names it. The issue-#6 prefix
    /// still survives verbatim — the diagnosis is appended, never prepended.
    #[test]
    fn the_trip_line_names_the_work_item_it_was_inside() {
        let label = "mutation 0198 scope=/os op=add_nodes[3/7] template=memory-hive cell=/os/mem";
        let trip = WatchdogTrip {
            work_item: Some(WorkItem::new(label)),
            cpu_denied: std::time::Duration::ZERO,
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
    // tv-br M10x: virtual time (AGENTS.md rule 11, time logic). On the wall
    // clock twelve 5 ms beats raced the arming drain and a 50 ms silence
    // window; under CPUQuota 80 % the sender stalled past the window and the
    // trip came without a label (build04, M10w: `left: None`). In virtual time
    // the order is fixed: the supervisor arms and drains at 0 ms, the ONE
    // labelled beat lands at 5 ms, the 10 ms tick hears it, and five silent
    // periods trip at 60 ms.
    #[tokio::test(start_paused = true)]
    async fn the_supervisor_carries_the_last_declared_label_into_the_trip() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::LogOnly,
            None,
        ));
        // Past the arming drain: a beat buffered before it would be discarded
        // as stale (it proves a moment that is already past).
        tokio::time::sleep(period / 2).await;
        hb_tx
            .try_send(Beat::WorkingOn(WorkItem::new(
                "mutation 42 op=add_nodes[1/1]",
            )))
            .expect("the supervisor must be listening");
        let trip =
            meclaw_testing::deadline::hang("the supervisor must trip on silence", trip_rx.recv())
                .await
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
        // Exact in virtual time: one beat heard at 10 ms, five silent periods.
        assert_eq!(trip.beats_seen, 1, "{trip}");
        assert_eq!(trip.silent_for, period * 5, "{trip}");
        assert_eq!(trip.armed_for, period * 6, "{trip}");
        drop(hb_tx);
    }

    /// A `Parked` beat clears the label: a parked loop is inside nothing, and a
    /// stale name would be worse evidence than none.
    // tv-br M10o: virtual time (AGENTS.md rule 11, time logic). In real time a
    // CPU quota delayed the supervisor's ticks and the beats, and the verdict
    // followed the scheduler (CPUQuota 80 %, 3 runs: 11 red in five pins).
    // tv-br M10x: the twelve-beat stretch that raced the arming drain is gone;
    // in virtual time one labelled beat (5 ms, heard at 10 ms) and one `Parked`
    // (15 ms, heard at 20 ms) say the same, and `beats_seen` proves both words
    // were heard, so the cleared label is not the default of a deaf supervisor.
    #[tokio::test(start_paused = true)]
    async fn parking_clears_the_declared_label() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::LogOnly,
            None,
        ));
        // Same reason as above: beat past the arming drain, and END on `Parked`.
        tokio::time::sleep(period / 2).await;
        hb_tx
            .try_send(Beat::WorkingOn(WorkItem::new("mutation 42")))
            .expect("the supervisor must be listening");
        tokio::time::sleep(period).await;
        hb_tx
            .try_send(Beat::Parked)
            .expect("the supervisor must be listening");
        let trip =
            meclaw_testing::deadline::hang("the supervisor must trip on silence", trip_rx.recv())
                .await
                .expect("a trip");
        assert_eq!(
            trip.work_item, None,
            "a parked loop must not carry the name of a finished item: {trip}"
        );
        assert!(!trip.in_flight_work);
        assert_eq!(trip.beats_seen, 2, "both words were heard: {trip}");
        assert_eq!(trip.silent_for, period * 5, "{trip}");
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(520),
            beats_seen: 1,
            colony_task_gone: false,
            armed_for: Duration::from_secs(1),
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
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
            let trip =
                meclaw_testing::deadline::hang("a trip must arrive under log-only", trip_rx.recv())
                    .await
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
    // welle-val2 v2-kit (#1199): virtual time (AGENTS.md rule 11, time logic).
    // In real time on two workers the supervisor ran from the spawn on while the
    // test thread still had to reach `drop(hb_tx)`; a test thread descheduled
    // for three periods (30 ms) under load let a SILENCE trip of a still-open
    // channel arrive first (release gate 0.64.0). That trip was right -- the
    // channel was open for all three periods -- so the race was the test's, not
    // the supervisor's. On the paused clock the supervisor is spawned but not
    // polled before the test yields, so the channel is closed before its first
    // tick, every wait point on the path is a tokio timer or channel, and the
    // order is the same on every host.
    #[tokio::test(start_paused = true)]
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
        drop(hb_tx); // the colony task is gone, before the supervisor's first tick
        let trip = meclaw_testing::deadline::hang("a gone colony must be reported", trip_rx.recv())
            .await
            .expect("the supervisor must report before it returns");
        assert!(trip.colony_task_gone, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_task_gone");
        assert!(trip.is_fatal(WatchdogOnTrip::LogOnly));
        // The supervisor returned → its sender dropped → the channel is closed.
        let after = meclaw_testing::deadline::hang(
            "the supervisor must stop, not keep reporting",
            trip_rx.recv(),
        )
        .await;
        assert!(
            after.is_none(),
            "a gone colony must end the supervisor under log-only too, got {after:?}"
        );
    }
    // ------------------------------------------------------------- GH #165

    /// A witness that never misses, for the tests that are about the colony
    /// rather than the host. It keeps its channel full: every supervisor tick
    /// drains it and the feeder refills it at once (the `send` waits for room,
    /// no clock), so a missed witness period would require the feeder itself to
    /// stall for a whole period — which is the very condition the test would
    /// want to know about anyway. OR-V2.v2-fix-b.G.1: no sleep, the same feeder
    /// serves the virtual-time and the real-time tests.
    fn healthy_witness() -> tokio::sync::mpsc::Receiver<()> {
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(64);
        tokio::spawn(async move { while tx.send(()).await.is_ok() {} });
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
    // tv-br M10o: virtual time (AGENTS.md rule 11, time logic). In real time a
    // CPU quota delayed the supervisor's ticks and the beats, and the verdict
    // followed the scheduler (CPUQuota 80 %, 3 runs: 11 red in five pins).
    #[tokio::test(start_paused = true)]
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

        let trip = meclaw_testing::deadline::hang(
            "the supervisor must still report the silence",
            trip_rx.recv(),
        )
        .await
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
            cpu_denied: std::time::Duration::ZERO,
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
        let second = meclaw_testing::deadline::hang(
            "the supervisor must keep supervising after a suppressed trip",
            trip_rx.recv(),
        )
        .await
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
    // tv-br M10o: virtual time (AGENTS.md rule 11, time logic). In real time a
    // CPU quota delayed the supervisor's ticks and the beats, and the verdict
    // followed the scheduler (CPUQuota 80 %, 3 runs: 11 red in five pins).
    #[tokio::test(start_paused = true)]
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
            Some(healthy_witness()),
        ));

        let trip = meclaw_testing::deadline::hang(
            "a wedged colony must still be detected",
            trip_rx.recv(),
        )
        .await
        .expect("the supervisor must report before it returns");
        assert_eq!(trip.witness(), HostWitness::Kept, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_loop", "was: {trip:?}");
        assert!(
            trip.is_fatal(WatchdogOnTrip::Exit),
            "a corroborated wedge must still end the process: {trip:?}"
        );

        let after = meclaw_testing::deadline::hang(
            "the supervisor must stop on a fatal trip, not keep reporting",
            trip_rx.recv(),
        )
        .await;
        assert!(
            after.is_none(),
            "a fatal trip must end the supervisor, got {after:?}"
        );
    }

    /// The real supervisor with the real [`run_liveness_witness`] task: the
    /// witness is not a test fixture, it actually completes work units on a live
    /// runtime, and a wedged colony next to it is still fatal.
    // tv-br M10o: virtual time (AGENTS.md rule 11, time logic). In real time a
    // CPU quota delayed the supervisor's ticks and the beats, and the verdict
    // followed the scheduler (CPUQuota 80 %, 3 runs: 11 red in five pins).
    #[tokio::test(start_paused = true)]
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

        let trip = meclaw_testing::deadline::hang(
            "a wedged colony must still be detected",
            trip_rx.recv(),
        )
        .await
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(499),
            beats_seen: 2380,
            colony_task_gone: false,
            armed_for: Duration::from_millis(234_607),
            in_flight_work: false,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
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
            cpu_denied: std::time::Duration::ZERO,
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
            cpu_denied: std::time::Duration::ZERO,
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(520),
            beats_seen: 7,
            colony_task_gone: false,
            armed_for: Duration::from_secs(3),
            in_flight_work: false,
            witness_present: false,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
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
    // tv-br M10o: virtual time (AGENTS.md rule 11, time logic). In real time a
    // CPU quota delayed the supervisor's ticks and the beats, and the verdict
    // followed the scheduler (CPUQuota 80 %, 3 runs: 11 red in five pins).
    #[tokio::test(start_paused = true)]
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
        let trip =
            meclaw_testing::deadline::hang("the silence must still be reported", trip_rx.recv())
                .await
                .expect("the supervisor must report before it returns");
        assert_eq!(trip.witness(), HostWitness::Absent, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_loop");
        assert!(trip.is_fatal(WatchdogOnTrip::Exit));
    }

    /// The witness's work unit is real work: a unit that skipped its quantum
    /// would make the witness a second sleeper and the whole corroboration
    /// worthless. Proven by its result, the value only all 1024 steps of the
    /// recurrence produce — never by the wall clock (OR-V2.v2-fix-b.G.2).
    #[test]
    fn the_witness_work_unit_actually_runs() {
        let seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut acc = seed;
        for i in 0..1024u64 {
            acc = acc.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i) ^ (acc >> 29);
        }
        assert_ne!(acc, seed, "the quantum moves its accumulator");
        assert_eq!(
            witness_work_unit(),
            acc,
            "the work unit ran every step of its quantum"
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
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(499),
            beats_seen: 2380,
            colony_task_gone: false,
            armed_for: Duration::from_millis(234_607),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
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
            // GH #1172: a silence of 4.999 s at a punctual 100 ms supervisor is
            // 49 evaluated periods, not the five of the first window.
            observed_periods: 49,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(4_999),
            beats_seen: 2380,
            colony_task_gone: false,
            armed_for: Duration::from_secs(300),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: std::time::Duration::ZERO,
        };
        assert_eq!(base.starved(), "slow_work_item");
        assert!(!base.is_fatal(WatchdogOnTrip::Exit));

        let stuck = WatchdogTrip {
            silent_for: Duration::from_millis(5_000),
            observed_periods: 50,
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
    // tv-br M10x: virtual time (AGENTS.md rule 11, time logic). On the wall
    // clock `sleep(period)` raced the arming drain (a drained `Working` leaves
    // the supervisor assuming `Parked`, which is `colony_loop` at once) and the
    // verdicts followed the scheduler. In virtual time the drain is at 0 ms,
    // the beat lands at 5 ms, and the escalation is exact: slow every window
    // until the charged silence crosses the budget.
    #[tokio::test(start_paused = true)]
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
            Some(healthy_witness()),
        ));
        // After arming: the supervisor discards beats buffered from before it was
        // armed (they describe a moment already past), and that discards their
        // phase with them — a supervisor that has heard nothing assumes `Parked`,
        // which is the verdict that grants no grace.
        tokio::time::sleep(period / 2).await;
        // The loop's last word: it entered a work item. Then nothing, ever.
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");

        let saw_slow = std::cell::Cell::new(false);
        let last_trip = std::cell::RefCell::new(None::<String>);
        // GH #1172: one deadline for the whole escalation, not one per trip --
        // a supervisor that never escalates still reports a slow trip every
        // window, and a per-trip marker would let it spin until nextest kills it.
        meclaw_testing::deadline::hang_with(
            "the escalation must finish inside the failure marker",
            async {
                loop {
                    let Some(trip) = trip_rx.recv().await else {
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
                    saw_slow.set(true);
                    *last_trip.borrow_mut() = Some(trip.to_string());
                }
            },
            || (saw_slow.get(), last_trip.borrow().clone()),
        )
        .await;
        assert!(
            saw_slow.get(),
            "the budget must have bought the work item at least one window of grace"
        );
        // Fatal means the supervisor is done.
        let after = meclaw_testing::deadline::hang(
            "the supervisor must stop after a fatal trip",
            trip_rx.recv(),
        )
        .await;
        assert!(after.is_none(), "got {after:?}");
        drop(hb_tx);
    }

    /// A parked loop gets no such grace: it declared that it had nothing in
    /// flight, so its silence has nothing to hide behind. Same silence duration
    /// as the slow-work-item case above, opposite verdict — the phase is doing
    /// the work, not the clock.
    ///
    /// GH #1216 (tv-br M10i): in virtual time. The claim is the verdict of a
    /// punctual supervisor; on the wall clock under a CPU quota of 80 % the
    /// supervisor itself ran 67 ms late on a 50 ms window (`silent_for` 117 ms,
    /// 3 of 3 runs) and rightly said `process_scheduling` -- the host, not the
    /// verdict, was under test.
    #[tokio::test(start_paused = true)]
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
            Some(healthy_witness()),
        ));
        tokio::time::sleep(period).await;
        // Entered a work item, finished it, parked: the loop is idle and quiet.
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");
        hb_tx
            .try_send(Beat::Parked)
            .expect("the supervisor must be listening");

        let trip = meclaw_testing::deadline::hang(
            "a parked loop that stops answering must be detected",
            trip_rx.recv(),
        )
        .await
        .expect("the supervisor must report before it returns");
        assert!(!trip.in_flight_work, "was: {trip:?}");
        assert_eq!(trip.starved(), "colony_loop", "was: {trip:?}");
        // Exact in virtual time: the bar is `threshold × period` = 5 × 10 ms
        // after the tick that heard the beats, not anything up to the budget.
        assert_eq!(trip.silent_for, Duration::from_millis(50), "was: {trip:?}");
        assert_eq!(trip.silent_for, trip.nominal_window(), "was: {trip:?}");
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
    // tv-br M10x: virtual time (AGENTS.md rule 11, time logic). On the wall
    // clock a 10 ms sleep was the only thing between the two beats and the
    // arming drain; a late supervisor drained them as stale and the verdict
    // `Parked` came from a deaf supervisor, not from the last word. Here the
    // drain is at 0 ms, both beats land at 5 ms, and `beats_seen` proves the
    // 10 ms tick heard both.
    #[tokio::test(start_paused = true)]
    async fn the_last_phase_in_a_period_is_the_one_that_counts() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(8);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        tokio::time::sleep(period / 2).await;
        // Both phases inside one supervisor period, in loop order.
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");
        hb_tx
            .try_send(Beat::Parked)
            .expect("the supervisor must be listening");
        let trip = meclaw_testing::deadline::hang("a trip must arrive", trip_rx.recv())
            .await
            .expect("the supervisor must report");
        assert!(
            !trip.in_flight_work,
            "the loop's last word was `Parked`: {trip:?}"
        );
        assert_eq!(trip.beats_seen, 2, "both words were heard: {trip}");
        assert_eq!(trip.silent_for, period * 5, "{trip}");
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

    /// GH #1172 -- a trip of an in-flight work item, as the supervisor builds it
    /// at the 100 ms default period with the shipped threshold of five.
    fn in_flight(silent_ms: u64, observed_periods: u32) -> WatchdogTrip {
        WatchdogTrip {
            silent_periods: 5,
            observed_periods,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(silent_ms),
            beats_seen: 994,
            colony_task_gone: false,
            armed_for: Duration::from_millis(61_263),
            in_flight_work: true,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: Some(WorkItem::new("mutation scope=/os/orgs")),
            cpu_denied: std::time::Duration::ZERO,
        }
    }

    /// GH #1172, the rule: what the supervisor sleeps through is not silence of
    /// the work item. One window of five periods that took 5239 ms because the
    /// supervisor itself was 4739 ms late: the item was observed silent for
    /// 500 ms, so it is slow -- not stuck, and not fatal. The budget is the
    /// shipped 5 s either way.
    #[test]
    fn a_supervisor_stall_is_not_charged_to_the_work_item() {
        let stalled = in_flight(5_239, 5);
        assert_eq!(stalled.work_item_budget(), Duration::from_secs(5));
        assert_eq!(stalled.supervisor_lag(), Duration::from_millis(4_739));
        assert_eq!(stalled.work_item_silence(), Duration::from_millis(500));
        assert_eq!(stalled.starved(), "slow_work_item", "was: {stalled}");
        assert!(
            !stalled.is_fatal(WatchdogOnTrip::Exit),
            "a supervisor that lost the CPU must not end the process on behalf \
             of the item it was not watching: {stalled}"
        );
    }

    /// GH #1172, the measurement: the lag is the silence beyond the periods the
    /// supervisor ACTUALLY evaluated, not beyond the first window. The record
    /// is the lab trip that motivated the issue -- `silent_for=5239ms` on the
    /// eighth consecutive trip of one mutation (the first came at 1009 ms), so
    /// 40 evaluated periods. The old derivation called 4739 ms of that "lag",
    /// which is the escalation itself: subtracting it would leave every
    /// repeated trip at one window of silence and no item could ever be stuck.
    #[test]
    fn the_lag_is_measured_against_the_periods_actually_evaluated() {
        let record = in_flight(5_239, 40);
        assert_eq!(record.supervisor_lag(), Duration::from_millis(1_239));
        assert_eq!(record.work_item_silence(), Duration::from_millis(4_000));
        assert_eq!(record.starved(), "slow_work_item", "was: {record}");
        assert!(!record.is_fatal(WatchdogOnTrip::Exit));

        // Two windows later the same item is still silent: 50 evaluated periods
        // are the budget, whatever the wall clock says.
        let later = in_flight(6_520, 50);
        assert_eq!(later.work_item_silence(), Duration::from_secs(5));
        assert_eq!(later.starved(), "stuck_work_item", "was: {later}");
        assert!(later.is_fatal(WatchdogOnTrip::Exit));
    }

    /// GH #1172, adversarial: the deduction must never hide a hung item. Every
    /// supervisor tick here is late -- the runtime is blocked for 40 ms between
    /// 1 ms windows in which timers can fire, against a 10 ms period -- and the
    /// item declared `Working` and never spoke again. Each evaluated period
    /// still charges one full period of silence, so the item is stuck after
    /// `budget / period` ticks however late each one is (nothing is denied here;
    /// at the tick edge one window later, pinned in
    /// `the_bounds_hold_one_window_late_at_the_tick_edge`); a supervisor that lags
    /// permanently delays the verdict in wall time and never prevents it.
    ///
    /// Virtual time (AGENTS.md rule 11, time logic; welle-val2 v2-fix-b, #1199):
    /// the starved host is the paused clock jumping four periods at once
    /// (`tokio::time::advance`), so no timer can fire inside the jump and every
    /// tick lands late by construction -- the same lateness the blocking
    /// `std::thread::sleep` built in real time, without guessing at the host.
    #[tokio::test(start_paused = true)]
    async fn a_hung_work_item_is_stuck_even_when_every_supervisor_tick_is_late() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(64);
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        tokio::time::sleep(period).await;
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");
        // The starved host: the clock moves four periods while nothing runs,
        // then a 1 ms window opens in which timers can fire.
        tokio::spawn(async {
            loop {
                tokio::time::advance(Duration::from_millis(40)).await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });

        // One deadline for the whole wait (see the escalation test above).
        let last_trip = std::cell::RefCell::new(None::<String>);
        let fatal = meclaw_testing::deadline::hang_with(
            "a hung item must be declared stuck inside the failure marker",
            async {
                loop {
                    let Some(trip) = trip_rx.recv().await else {
                        panic!("the supervisor returned without a fatal trip");
                    };
                    if trip.is_fatal(WatchdogOnTrip::Exit) {
                        break trip;
                    }
                    assert_eq!(trip.starved(), "slow_work_item", "was: {trip}");
                    *last_trip.borrow_mut() = Some(trip.to_string());
                }
            },
            || last_trip.borrow().clone(),
        )
        .await;
        assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
        assert!(
            fatal.supervisor_lag() >= fatal.nominal_window(),
            "the supervisor must really have been late, and measured so: {fatal}"
        );
        assert!(
            fatal.work_item_silence() >= fatal.work_item_budget(),
            "stuck means the charged silence crossed the budget: {fatal}"
        );
        drop(hb_tx);
    }

    /// GH #1172, the side effect on a parked loop, pinned: a silence with
    /// nothing in flight that outlasts a `host_runtime` trip ends the process
    /// under `exit` once the witness has recovered and the supervisor is on
    /// time -- as `colony_loop`, where before #1172 it was `process_scheduling`
    /// and logged only. The old lag (`silent_for - nominal_window`) grew by one
    /// window on every repeated trip of the same silence, so a punctual
    /// supervisor looked late on every follow-up and hid exactly this case.
    /// Fail-closed and intended; the first trip of a silence reads as before.
    #[test]
    fn a_parked_silence_after_a_host_runtime_trip_is_a_fatal_colony_loop() {
        let parked =
            |silent_ms: u64, observed_periods: u32, witness_worst_misses: u32| WatchdogTrip {
                silent_periods: 5,
                observed_periods,
                period: Duration::from_millis(100),
                silent_for: Duration::from_millis(silent_ms),
                beats_seen: 994,
                colony_task_gone: false,
                armed_for: Duration::from_millis(61_263),
                in_flight_work: false,
                witness_present: true,
                witness_worst_misses,
                work_item: None,
                cpu_denied: std::time::Duration::ZERO,
            };

        // First window: the witness failed with the loop -- the host, not fatal.
        let first = parked(520, 5, 5);
        assert_eq!(first.starved(), "host_runtime", "was: {first}");
        assert!(!first.is_fatal(WatchdogOnTrip::Exit));

        // Next window: the witness recovered, the supervisor stayed punctual
        // (10 periods in 1030 ms), the loop is still silent.
        let follow_up = parked(1_030, 10, 0);
        assert!(
            follow_up
                .silent_for
                .saturating_sub(follow_up.nominal_window())
                >= follow_up.nominal_window(),
            "pre-#1172 this follow-up read as process_scheduling: {follow_up}"
        );
        assert_eq!(follow_up.supervisor_lag(), Duration::from_millis(30));
        assert_eq!(follow_up.starved(), "colony_loop", "was: {follow_up}");
        assert!(follow_up.is_fatal(WatchdogOnTrip::Exit));
        assert!(!follow_up.is_fatal(WatchdogOnTrip::LogOnly));

        // A supervisor that really fell a window behind inside the follow-up
        // is still the process, not the loop.
        let late = parked(1_600, 10, 0);
        assert_eq!(late.starved(), "process_scheduling", "was: {late}");
        assert!(!late.is_fatal(WatchdogOnTrip::Exit));
    }

    /// GH #1216 (tv-br M10h), the red of TV-F7: a 14 s instantiation under a
    /// CPU quota, 5000 ms silent with `supervisor_lag=0ms` and `witness=kept`
    /// -- the supervisor and the witness each got their slice on time, the
    /// colony's worker did not. The kernel measured 4 s of that silence as
    /// denied: the item was charged 1 s, it is slow, not stuck. The control
    /// (`denied` zero: no denial read) is the pre-#1216 verdict, unchanged.
    #[test]
    fn a_silence_the_host_denied_is_not_a_stuck_work_item() {
        let hung = in_flight(5_000, 50);
        assert_eq!(hung.starved(), "stuck_work_item", "control: {hung}");
        assert!(hung.is_fatal(WatchdogOnTrip::Exit));

        let denied = WatchdogTrip {
            cpu_denied: Duration::from_millis(4_000),
            ..in_flight(5_000, 50)
        };
        assert_eq!(denied.work_item_silence(), Duration::from_millis(1_000));
        assert_eq!(denied.starved(), "slow_work_item", "was: {denied}");
        assert!(!denied.is_fatal(WatchdogOnTrip::Exit));
        assert!(
            denied.to_string().ends_with(" cpu_denied=4000ms]"),
            "the trip line carries the denial: {denied}"
        );
    }

    /// GH #1216: fail-closed. The denial delays the verdict and never hides
    /// the item: 12 s silent with 6 s denied is 6 s charged, over the budget.
    /// The larger of the two deductions counts, never their sum (a supervisor
    /// off the CPU is part of the same denial).
    #[test]
    fn a_partly_denied_silence_still_ends_stuck() {
        let t = WatchdogTrip {
            cpu_denied: Duration::from_millis(6_000),
            ..in_flight(12_000, 120)
        };
        assert_eq!(t.work_item_silence(), Duration::from_millis(6_000));
        assert_eq!(t.starved(), "stuck_work_item", "was: {t}");
        assert!(t.is_fatal(WatchdogOnTrip::Exit));

        let both = WatchdogTrip {
            cpu_denied: Duration::from_millis(3_000),
            ..in_flight(9_000, 40)
        };
        assert_eq!(both.supervisor_lag(), Duration::from_millis(5_000));
        assert_eq!(both.work_item_silence(), Duration::from_millis(4_000));
    }

    /// GH #1216 (tv-br M10i): a parked loop is judged without the denial. It
    /// declared nothing in flight, so the idle bar stays a wall-clock bar: a
    /// wedge is `colony_loop` with or without a denial, and the beat at the
    /// edge of a tick (`silent_for` a millisecond short of the window, gh47's
    /// `9999ms` and the `499ms` of the incident shape) is no denial either.
    #[test]
    fn a_parked_silence_is_judged_without_the_denial() {
        let parked = |silent_ms: u64, cpu_denied_ms: u64| WatchdogTrip {
            silent_periods: 5,
            observed_periods: 5,
            period: Duration::from_millis(100),
            silent_for: Duration::from_millis(silent_ms),
            beats_seen: 994,
            colony_task_gone: false,
            armed_for: Duration::from_millis(61_263),
            in_flight_work: false,
            witness_present: true,
            witness_worst_misses: 0,
            work_item: None,
            cpu_denied: Duration::from_millis(cpu_denied_ms),
        };
        for (silent_ms, denied_ms) in [(500, 0), (499, 0), (500, 300), (499, 499)] {
            let t = parked(silent_ms, denied_ms);
            assert_eq!(t.starved(), "colony_loop", "was: {t}");
            assert!(t.is_fatal(WatchdogOnTrip::Exit), "was: {t}");
        }
    }

    /// GH #1216 (tv-br M10o, A-M1): the lag credit of a PARKED loop is
    /// bounded like the item's. `supervisor_lag` is summed over the silence and
    /// reset only by a beat, so one stall of a window -- or a supervisor that is
    /// late on every tick -- kept a real wedge `process_scheduling` (log-only)
    /// for ever. A parked loop is credited the supervisor's lateness for at most
    /// [`WatchdogTrip::budget_periods`] evaluated periods (100 ms × 5: 50), the
    /// bar at which an in-flight item without a denial is stuck; from there on
    /// the silence is `colony_loop` and fatal, however late the supervisor is.
    #[test]
    fn a_parked_wedge_is_fatal_after_the_budget_periods_however_late_the_supervisor() {
        let parked = |silent_ms: u64, observed_periods: u32| WatchdogTrip {
            in_flight_work: false,
            work_item: None,
            ..in_flight(silent_ms, observed_periods)
        };
        let t = parked(500, 5);
        assert_eq!(t.budget_periods(), 50);
        assert_eq!(
            t.period * t.budget_periods(),
            t.work_item_budget(),
            "one source: the parked bar is the item's budget in periods"
        );
        // One stall of more than a window, then a punctual supervisor: the lag
        // stays at 600 ms for the rest of the silence.
        for (silent_ms, observed, verdict) in [
            (1_600, 10, "process_scheduling"),
            (5_500, 49, "process_scheduling"),
            (5_600, 50, "colony_loop"),
            (60_600, 600, "colony_loop"),
            // Late on every tick: four periods behind per period evaluated.
            (2_000, 5, "process_scheduling"),
            (19_600, 49, "process_scheduling"),
            (20_000, 50, "colony_loop"),
            (u64::from(u32::MAX), 50, "colony_loop"),
        ] {
            let t = parked(silent_ms, observed);
            assert!(t.supervisor_lag() >= t.nominal_window(), "late: {t}");
            assert_eq!(t.starved(), verdict, "was: {t}");
            assert_eq!(
                t.is_fatal(WatchdogOnTrip::Exit),
                verdict == "colony_loop",
                "was: {t}"
            );
        }
    }

    /// GH #1216 (tv-br M10o, A-M1) end to end: a parked loop that stops
    /// answering while every supervisor tick is late -- the runtime spins for
    /// 40 ms between yields in which timers can fire, against a 10 ms
    /// period. Every trip before the bar is `process_scheduling`
    /// (log-only); the trip at [`WatchdogTrip::budget_periods`] evaluated
    /// periods is `colony_loop` and ends the supervisor. Before M10o the wedge
    /// stayed log-only for ever and this test ran into the hang marker.
    ///
    /// `current_thread` on purpose, as in the in-flight twin above: one
    /// thread is the starved host, the spin is the starvation.
    #[tokio::test]
    async fn a_parked_wedge_is_fatal_even_when_every_supervisor_tick_is_late() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(64);
        // Stale boot beats fill the channel; room again is the event that the
        // armed supervisor drained them (see `supervise`), so the parked word
        // after it is heard.
        for _ in 0..8 {
            hb_tx
                .try_send(Beat::Parked)
                .expect("room before the supervisor runs");
        }
        tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        // Room for all eight is the event that the armed supervisor drained
        // them: `reserve_many` wakes on it, the permits go back at once. A
        // 10 ms poll here ate into the supervisor's 50 ms bar before the next
        // word (CPUQuota 80 %: the supervisor ended before it,
        // OR-V2.v2-fix-b.G2.1).
        drop(
            meclaw_testing::deadline::hang(
                "the armed supervisor drains the stale beats",
                hb_tx.reserve_many(8),
            )
            .await
            .expect("the supervisor must be listening"),
        );
        hb_tx
            .try_send(Beat::Parked)
            .expect("the supervisor must be listening");
        // The starved host: the only thread spins four periods at a time and
        // then yields once, which lets the timer driver fire the late tick.
        tokio::spawn(async {
            loop {
                let until = tokio::time::Instant::now() + Duration::from_millis(40);
                while tokio::time::Instant::now() < until {
                    std::hint::spin_loop();
                }
                tokio::task::yield_now().await;
            }
        });
        let last_trip = std::cell::RefCell::new(None::<String>);
        let fatal = meclaw_testing::deadline::hang_with(
            "a parked wedge must end the supervisor inside the failure marker",
            async {
                loop {
                    let Some(trip) = trip_rx.recv().await else {
                        panic!("the supervisor returned without a fatal trip");
                    };
                    if trip.is_fatal(WatchdogOnTrip::Exit) {
                        break trip;
                    }
                    assert_eq!(trip.starved(), "process_scheduling", "was: {trip}");
                    *last_trip.borrow_mut() = Some(trip.to_string());
                }
            },
            || last_trip.borrow().clone(),
        )
        .await;
        assert_eq!(fatal.starved(), "colony_loop", "was: {fatal}");
        assert_eq!(
            fatal.observed_periods,
            fatal.budget_periods(),
            "was: {fatal}"
        );
        assert!(
            fatal.supervisor_lag() >= fatal.nominal_window(),
            "the supervisor must really have been late, and measured so: {fatal}"
        );
        drop(hb_tx);
    }

    /// GH #1216 (tv-br M10o, A-M2): the in-flight bound without the assumption
    /// of a punctual supervisor. The lag deduction leaves every evaluated
    /// period charged, the denial deduction a fifth of the silence, so the
    /// item is charged at least a fifth of the evaluated periods and is stuck
    /// after at most `WORK_ITEM_DENIAL_FACTOR × budget_periods` of them (100 ms
    /// × 5: 250), however late the supervisor and however much the host denies.
    /// The raster starts at a silence of the evaluated periods; the tick edge
    /// below it (one window later) is
    /// `the_bounds_hold_one_window_late_at_the_tick_edge`.
    #[test]
    fn an_item_is_stuck_after_the_denial_factor_times_the_budget_periods() {
        let bar = in_flight(500, 5).budget_periods() * WORK_ITEM_DENIAL_FACTOR;
        assert_eq!(bar, 250);
        for observed in [5u32, 50, 245, 250, 600] {
            let evaluated = Duration::from_millis(100) * observed;
            for stretch in [1u32, 2, 5, 40] {
                let silent = evaluated * stretch;
                for denied in [Duration::ZERO, silent / 2, silent] {
                    let t = WatchdogTrip {
                        cpu_denied: denied,
                        ..in_flight(silent.as_millis() as u64, observed)
                    };
                    assert!(
                        t.work_item_silence() >= evaluated / WORK_ITEM_DENIAL_FACTOR,
                        "charged less than a fifth of the evaluated periods: {t}"
                    );
                    if observed >= bar {
                        assert_eq!(t.starved(), "stuck_work_item", "was: {t}");
                    }
                }
            }
        }
    }

    /// tv-br M10q-a (re-check R, K1): the bounds at the tick edge. tokio keeps
    /// the tick grid for up to 5 ms of delay and the supervisor reads `now`
    /// after the tick, so `silent_for` can lie a millisecond below the
    /// evaluated periods (`silent_for=9999ms`, gh47). There the trip AT the
    /// bound is still `slow_work_item` -- without a denial at 50 evaluated
    /// periods, with the whole silence denied (s = 1) at 250 -- and the next
    /// trip, one window (5 periods) later, is `stuck_work_item`: the documented
    /// bound is "at the latest one window after", not "after". A code variant
    /// charging `max(silent_for, evaluated)` would turn the first half red.
    #[test]
    fn the_bounds_hold_one_window_late_at_the_tick_edge() {
        let probe = in_flight(0, 0);
        let (budget, window) = (probe.budget_periods(), probe.silent_periods);
        assert_eq!((budget, window), (50, 5));
        let edge = |observed: u32, denied_all: bool| {
            let silent = Duration::from_millis(100) * observed - Duration::from_millis(1);
            WatchdogTrip {
                cpu_denied: if denied_all { silent } else { Duration::ZERO },
                ..in_flight(silent.as_millis() as u64, observed)
            }
        };
        for (bar, denied_all) in [(budget, false), (budget * WORK_ITEM_DENIAL_FACTOR, true)] {
            let at = edge(bar, denied_all);
            assert!(
                at.work_item_silence() < at.work_item_budget(),
                "the edge charges less than the budget: {at}"
            );
            assert_eq!(at.starved(), "slow_work_item", "at the bound: {at}");
            let next = edge(bar + window, denied_all);
            assert_eq!(
                next.starved(),
                "stuck_work_item",
                "one window later: {next}"
            );
            assert!(next.is_fatal(WatchdogOnTrip::Exit));
        }
    }

    /// GH #1216 (tv-br M10i): without a denial nothing changed. Every trip with
    /// `cpu_denied` zero charges the item exactly the pre-#1216 silence
    /// (`silent_for − supervisor_lag`) and gets the verdict that follows from
    /// it -- the host without a quota and with a free CPU sees the old watchdog.
    /// The one change to that verdict is not a denial: since tv-br M10o a
    /// parked loop is credited the supervisor's lateness for at most
    /// `budget_periods` evaluated periods.
    #[test]
    fn without_a_denial_every_verdict_is_the_pre_1216_one() {
        for silent_ms in [0u64, 499, 500, 4_999, 5_000, 9_000, 12_000, 60_000] {
            for observed in [0u32, 1, 5, 40, 50, 120, 600] {
                for in_flight_work in [false, true] {
                    let t = WatchdogTrip {
                        in_flight_work,
                        ..in_flight(silent_ms, observed)
                    };
                    let charged = t.silent_for.saturating_sub(t.supervisor_lag());
                    assert_eq!(t.work_item_silence(), charged, "was: {t}");
                    let old = if !in_flight_work {
                        // tv-br M10o: the parked lag credit ends at the budget
                        // periods (pinned in `a_parked_wedge_is_fatal_…`).
                        if t.supervisor_lag() >= t.nominal_window()
                            && t.observed_periods < t.budget_periods()
                        {
                            "process_scheduling"
                        } else {
                            "colony_loop"
                        }
                    } else if charged < t.work_item_budget() {
                        "slow_work_item"
                    } else {
                        "stuck_work_item"
                    };
                    assert_eq!(t.starved(), old, "was: {t}");
                }
            }
        }
    }

    /// GH #1216 (tv-br M10i): the supervisor end to end with a declared work
    /// item that never pulses again, under several hosts. `denied(share)` is a
    /// host that keeps `share` of every nanosecond off the CPU, one thread,
    /// read through the same seam production reads the kernel through -- on
    /// the tokio clock, so the same host serves a virtual-time pin.
    fn denied(share: f64) -> impl Fn() -> RunDelays {
        let since = tokio::time::Instant::now();
        move || {
            let ns = since.elapsed().as_nanos() as f64 * share;
            RunDelays::from_rows([(1, ns as u64)])
        }
    }

    /// Runs the supervisor (10 ms × 5, budget 500 ms) over an item that entered
    /// work and then went silent, and hands back every trip up to the first
    /// fatal one, or up to `until` of silence when none is fatal.
    /// [`meclaw_testing::deadline::HANG`] is the hang marker, never a verdict.
    ///
    /// The item enters work from inside the supervisor's first silent period,
    /// through the seam: the first reading of the denial sends the `Working`
    /// beat, so the next tick hears it whatever the host does to the test's
    /// own thread. Sent from the test thread after [`supervise`], the word
    /// raced the parked bar of 50 ms in real time: under an 80 % quota the
    /// test thread lost the whole window before it spoke, and the supervisor
    /// rightly judged a parked loop (`colony_loop`, `beats_seen=0`,
    /// `cpu_denied=40ms`; scan S3 on build03, 1 of 5,
    /// OR-V2.v2-fix-scan.C2.1).
    async fn trips_of_a_silent_item(
        read_denial: impl Fn() -> RunDelays + Send + 'static,
        until: Duration,
    ) -> Vec<WatchdogTrip> {
        let (hb_tx, mut trip_rx) = supervise_saying(read_denial, Some(Beat::Working)).await;
        let trips = std::cell::RefCell::new(Vec::<WatchdogTrip>::new());
        meclaw_testing::deadline::hang_with(
            "the supervisor neither escalated nor reached the bar",
            async {
                while let Some(trip) = trip_rx.recv().await {
                    let done = trip.is_fatal(WatchdogOnTrip::Exit) || trip.silent_for >= until;
                    trips.borrow_mut().push(trip);
                    if done {
                        break;
                    }
                }
            },
            || trips.borrow().last().map(|t| t.to_string()),
        )
        .await;
        drop(hb_tx);
        trips.into_inner()
    }

    /// The armed supervisor (10 ms × 5, budget 500 ms, a healthy witness) over
    /// `read_denial`, its stale boot beats drained: the next word it is sent
    /// is heard.
    async fn supervise(
        read_denial: impl Fn() -> RunDelays + Send + 'static,
    ) -> (
        tokio::sync::mpsc::Sender<Beat>,
        tokio::sync::mpsc::Receiver<WatchdogTrip>,
    ) {
        supervise_saying(read_denial, None).await
    }

    /// [`supervise`], and `first_word` -- when there is one -- said from
    /// inside the supervisor's first silent period: its first reading of the
    /// denial sends it, after the arming drain, so the next tick hears it.
    /// No thread outside the supervisor has to win a race against its clock.
    async fn supervise_saying(
        read_denial: impl Fn() -> RunDelays + Send + 'static,
        first_word: Option<Beat>,
    ) -> (
        tokio::sync::mpsc::Sender<Beat>,
        tokio::sync::mpsc::Receiver<WatchdogTrip>,
    ) {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(64);
        // Beats from before arming are discarded (they describe a moment
        // already past): a full channel of them, which the armed supervisor
        // must throw away before the next word is heard.
        for _ in 0..8 {
            hb_tx
                .try_send(Beat::Parked)
                .expect("room before the supervisor runs");
        }
        let unsaid = std::sync::Mutex::new(first_word.map(|beat| (hb_tx.clone(), beat)));
        let read_denial = move || {
            if let Some((tx, beat)) = unsaid.lock().unwrap().take() {
                tx.try_send(beat)
                    .expect("the drained beat channel has room for the first word");
            }
            read_denial()
        };
        let (witness, drained) = a_witness_that_tells_the_end_of_the_drain();
        tokio::spawn(run_watchdog_reading_denial(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            Some(witness),
            None,
            read_denial,
        ));
        // The end of the arming drain is the event, not room in the beat
        // channel: room for all eight comes with the eighth discarded beat,
        // while the drain is still running, and a word sent in that window was
        // discarded as stale too (beats_seen=0, `colony_loop` instead of
        // `stuck_work_item`; coverage run on 652aaddc6, OR-V2.v2-fix-w.1). The
        // supervisor drains the witness only after the beats, so the witness's
        // first room means the next word is heard. No poll: a 10 ms poll ate
        // into the 50 ms bar (OR-V2.v2-fix-b.G2.1).
        meclaw_testing::deadline::hang("the armed supervisor ends its arming drain", drained)
            .await
            .expect("the witness feeder must report the drain");
        (hb_tx, trip_rx)
    }

    /// [`healthy_witness`] with one more word: the channel starts full, and
    /// the feeder reports its first room. Only the armed supervisor reads the
    /// witness, and only after it has discarded the stale beats, so that
    /// report is the end of the arming drain. Then it refills like
    /// [`healthy_witness`].
    fn a_witness_that_tells_the_end_of_the_drain() -> (
        tokio::sync::mpsc::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(64);
        while tx.try_send(()).is_ok() {}
        let (drained_tx, drained_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let Ok(permit) = tx.reserve().await else {
                return;
            };
            permit.send(());
            let _ = drained_tx.send(());
            while tx.send(()).await.is_ok() {}
        });
        (rx, drained_rx)
    }

    /// The order [`a_witness_that_tells_the_end_of_the_drain`] stands on: on
    /// arming, `run_watchdog_reading_denial` reads the witness only after its
    /// stale-beat drain has ended, so the witness's first room means the beats
    /// are gone. No production verdict depends on that order (both drains only
    /// discard the past), and no behaviour shows a swap on its own: with the
    /// two drains swapped every test still passed (0/9), and only with an
    /// `await` between them did the coverage race of OR-V2.v2-fix-w.1 come
    /// back (9/9). So the source holds it, read at stable anchors -- the
    /// function, its `heartbeat_rx.try_recv()` drain loop, the first
    /// `witness_rx.` use that is not a presence check.
    #[tokio::test]
    async fn the_stale_beats_are_drained_before_the_witness_is_read() -> Result<(), String> {
        let src = include_str!("watchdog.rs");
        let start = src
            .find("\npub(crate) async fn run_watchdog_reading_denial(")
            .ok_or("run_watchdog_reading_denial is gone")?;
        let len = src[start + 1..]
            .find("\n}\n")
            .ok_or("run_watchdog_reading_denial has no end")?;
        let body: Vec<&str> = src[start + 1..start + 1 + len].lines().collect();
        let beat_loop = body
            .iter()
            .position(|l| {
                l.trim_start().starts_with("while") && l.contains("heartbeat_rx.try_recv()")
            })
            .ok_or("the stale-beat drain loop is gone")?;
        let indent = &body[beat_loop][..body[beat_loop].find("while").unwrap_or(0)];
        let close = format!("{indent}}}");
        let beat_end = beat_loop
            + body[beat_loop..]
                .iter()
                .position(|l| *l == close)
                .ok_or("the stale-beat drain loop has no end")?;
        let witness_read = body
            .iter()
            .position(|l| {
                l.split("witness_rx.")
                    .skip(1)
                    .any(|rest| !rest.starts_with("is_some()") && !rest.starts_with("is_none()"))
            })
            .ok_or("the witness is never read")?;
        if witness_read <= beat_end {
            return Err(format!(
                "the witness is read (`{}`) before the stale-beat drain ends (`{}`): the first \
                 room in the witness no longer tells the end of the arming drain",
                body[witness_read].trim(),
                body[beat_loop].trim()
            ));
        }
        Ok(())
    }

    /// GH #1216 (tv-br M10l), the deadlock beside a host that denies the whole
    /// silence (s = 1). The denial is read process-wide, so a hung item is
    /// credited every other thread's denial; before the cap it was never
    /// stuck. The cap ends it exactly at `WORK_ITEM_DENIAL_FACTOR ×
    /// work_item_budget` of silence (10 ms × 5, budget 500 ms: 2500 ms), and
    /// every trip before that is `slow_work_item`, log-only. Virtual time:
    /// every wait point is a tokio timer or channel and the denial comes
    /// through the seam, so the bar is exact.
    #[tokio::test(start_paused = true)]
    async fn a_host_that_denies_the_whole_silence_ends_the_item_at_the_cap() {
        let trips = trips_of_a_silent_item(denied(1.0), Duration::MAX).await;
        let (fatal, before) = trips.split_last().expect("at least one trip");
        for t in before {
            assert_eq!(t.starved(), "slow_work_item", "was: {t}");
            assert!(!t.is_fatal(WatchdogOnTrip::Exit), "was: {t}");
        }
        assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
        assert!(fatal.is_fatal(WatchdogOnTrip::Exit), "was: {fatal}");
        assert_eq!(
            fatal.silent_for,
            fatal.work_item_budget() * WORK_ITEM_DENIAL_FACTOR,
            "was: {fatal}"
        );
        assert_eq!(fatal.work_item_silence(), fatal.work_item_budget());
        assert!(
            fatal.cpu_denied > fatal.denial_deduction(),
            "the cap, not the denial, ended the item: {fatal}"
        );
    }

    /// GH #1216 (tv-br M10l), the cap as arithmetic: whatever the kernel
    /// reports as denied, a fifth of the silence is charged. A fully denied
    /// silence one millisecond short of five budgets is slow; at five budgets
    /// it is stuck. Under the cap the denial still counts in full (TV-F7's
    /// 4 s of 5 s).
    #[test]
    fn a_denial_is_credited_at_most_four_fifths_of_the_silence() {
        let full = |silent_ms: u64| WatchdogTrip {
            cpu_denied: Duration::from_millis(silent_ms),
            ..in_flight(silent_ms, (silent_ms / 100) as u32)
        };
        let short = full(24_999);
        assert_eq!(short.denial_deduction(), Duration::from_micros(19_999_200));
        assert_eq!(short.work_item_silence(), Duration::from_micros(4_999_800));
        assert_eq!(short.starved(), "slow_work_item", "was: {short}");
        let at_cap = full(25_000);
        assert_eq!(
            at_cap.work_item_budget() * WORK_ITEM_DENIAL_FACTOR,
            at_cap.silent_for
        );
        assert_eq!(at_cap.work_item_silence(), at_cap.work_item_budget());
        assert_eq!(at_cap.starved(), "stuck_work_item", "was: {at_cap}");
        assert!(at_cap.is_fatal(WatchdogOnTrip::Exit));
        let under = WatchdogTrip {
            cpu_denied: Duration::from_millis(4_000),
            ..in_flight(5_000, 50)
        };
        assert_eq!(under.denial_deduction(), Duration::from_millis(4_000));
    }

    /// GH #1216 (tv-br M10l), the reset: a beat ends a silence and the denial
    /// measured in it. A first silence the host denies in full (s = 1), then a
    /// `Working` beat, then a second silence the host denies nothing: the
    /// second is charged exactly as without a quota -- `cpu_denied` zero and
    /// `stuck_work_item` at the budget, 500 ms. Without the reset of the
    /// denial's base the second silence would be credited the first one's
    /// denial. Virtual time, as above.
    #[tokio::test(start_paused = true)]
    async fn a_beat_ends_the_denial_of_the_silence_before_it() {
        let since = tokio::time::Instant::now();
        let frozen_at = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
        let freeze = std::sync::Arc::clone(&frozen_at);
        let (hb_tx, mut trip_rx) = supervise(move || {
            let ns = (since.elapsed().as_nanos() as u64)
                .min(frozen_at.load(std::sync::atomic::Ordering::SeqCst));
            RunDelays::from_rows([(1, ns)])
        })
        .await;
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");
        let first = meclaw_testing::deadline::hang("the first silence trips", trip_rx.recv())
            .await
            .expect("the supervisor must report");
        assert_eq!(first.starved(), "slow_work_item", "was: {first}");
        assert!(first.cpu_denied > Duration::ZERO, "was: {first}");

        // From here on the host denies nothing; the item enters work again.
        freeze.store(
            since.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::SeqCst,
        );
        hb_tx
            .try_send(Beat::Working)
            .expect("the supervisor must be listening");
        let fatal = meclaw_testing::deadline::hang("the second silence ends stuck", async {
            loop {
                let t = trip_rx.recv().await.expect("the supervisor must report");
                if t.is_fatal(WatchdogOnTrip::Exit) {
                    break t;
                }
            }
        })
        .await;
        assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
        assert_eq!(fatal.cpu_denied, Duration::ZERO, "was: {fatal}");
        assert_eq!(fatal.silent_for, fatal.work_item_budget(), "was: {fatal}");
        drop(hb_tx);
    }

    /// GH #1216 (tv-br M10i), fail-closed under a throttle: a host that denies
    /// half the silence delays the verdict and never hides a hung item -- the
    /// charge keeps growing and the item ends `stuck_work_item` once its half
    /// has outlived the budget.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_host_that_denies_half_the_silence_still_ends_a_hung_item() {
        let trips = trips_of_a_silent_item(denied(0.5), Duration::MAX).await;
        let fatal = trips.last().expect("at least one trip");
        assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
        assert!(fatal.is_fatal(WatchdogOnTrip::Exit));
        assert!(
            fatal.work_item_silence() >= fatal.work_item_budget(),
            "the charged half outlived the budget: {fatal}"
        );
        assert!(fatal.cpu_denied > Duration::ZERO, "was: {fatal}");
    }

    /// GH #1216 (tv-br M10i), the two real hangs on the real kernel reading.
    /// Blocked (a deadlock: the item waits and asks for no CPU itself) and
    /// spinning (the item burns a core and makes no progress). On a free CPU
    /// the process is denied next to nothing and both end `stuck_work_item`
    /// as before #1216; under a quota the reading credits them the process's
    /// denial and the cap ([`WORK_ITEM_DENIAL_FACTOR`]) still ends them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_blocked_and_a_spinning_item_still_end_stuck_on_a_free_cpu() {
        let blocked = trips_of_a_silent_item(RunDelays::read, Duration::MAX).await;
        let fatal = blocked.last().expect("at least one trip");
        assert_eq!(fatal.starved(), "stuck_work_item", "blocked: {fatal}");
        assert!(fatal.is_fatal(WatchdogOnTrip::Exit));

        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let spinner = std::thread::spawn(move || {
            let mut x: u64 = 0;
            while matches!(
                stop_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
            x
        });
        let spun = trips_of_a_silent_item(RunDelays::read, Duration::MAX).await;
        drop(stop_tx);
        let _ = spinner.join();
        let fatal = spun.last().expect("at least one trip");
        assert_eq!(fatal.starved(), "stuck_work_item", "spinning: {fatal}");
        assert!(fatal.is_fatal(WatchdogOnTrip::Exit));
    }

    /// The real kernel's reading plus one thread the host keeps entirely off
    /// the CPU (s = 1): the shape of M1, a hang beside a starved neighbour
    /// whose denial the process-wide reading credits to the item.
    fn with_a_fully_denied_neighbour() -> impl Fn() -> RunDelays {
        let neighbour = denied(1.0);
        move || {
            let mut rows = RunDelays::read();
            for (_, ns) in neighbour().0 {
                rows.0.insert(u32::MAX, ns);
            }
            rows
        }
    }

    /// GH #1216 (tv-br M10l), the two real hangs beside a fully denied
    /// thread: blocked and spinning, on the real kernel's reading, with a
    /// neighbour the host denies the whole silence. Before the cap both were
    /// never stuck; with it both end `stuck_work_item`, and it is the cap that
    /// ends them. Runs unchanged under a CPU quota (the Abnahme runs it at
    /// 80 %).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_blocked_and_a_spinning_item_end_stuck_beside_a_fully_denied_thread() {
        let blocked = trips_of_a_silent_item(with_a_fully_denied_neighbour(), Duration::MAX).await;
        let fatal = blocked.last().expect("at least one trip");
        assert_eq!(fatal.starved(), "stuck_work_item", "blocked: {fatal}");
        assert!(fatal.is_fatal(WatchdogOnTrip::Exit));
        assert!(
            fatal.cpu_denied > fatal.denial_deduction(),
            "blocked: the cap, not the denial, ended the item: {fatal}"
        );

        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let spinner = std::thread::spawn(move || {
            let mut x: u64 = 0;
            while matches!(
                stop_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
            x
        });
        let spun = trips_of_a_silent_item(with_a_fully_denied_neighbour(), Duration::MAX).await;
        drop(stop_tx);
        let _ = spinner.join();
        let fatal = spun.last().expect("at least one trip");
        assert_eq!(fatal.starved(), "stuck_work_item", "spinning: {fatal}");
        assert!(fatal.is_fatal(WatchdogOnTrip::Exit));
        assert!(
            fatal.cpu_denied > fatal.denial_deduction(),
            "spinning: the cap, not the denial, ended the item: {fatal}"
        );
    }

    /// GH #1216: the reading. `schedstat` is `<on-cpu> <run_delay> <slices>`;
    /// the growth is the largest of any thread, a new thread counts from zero,
    /// a gone one not at all.
    #[test]
    fn run_delays_grow_per_thread_and_the_largest_counts() {
        assert_eq!(parse_run_delay("600123 2400456 77\n"), Some(2_400_456));
        assert_eq!(parse_run_delay("garbage"), None);
        let base = RunDelays::from_rows([(1, 1_000_000), (2, 5_000_000), (3, 9)]);
        let now = RunDelays::from_rows([(1, 301_000_000), (2, 6_000_000), (4, 50_000_000)]);
        assert_eq!(now.max_growth_since(&base), Duration::from_millis(300));
        assert_eq!(base.max_growth_since(&base), Duration::ZERO);
    }

    /// GH #1216: on Linux the reading sees this process's own threads.
    #[cfg(target_os = "linux")]
    #[test]
    fn run_delays_read_this_process() {
        assert!(
            !RunDelays::read().0.is_empty(),
            "procfs names at least the thread that reads it"
        );
    }

    // GH #1216 (K8): the supervisor is disarmed by the loop's last word, never
    // by a timer. Virtual time, 10 ms periods, threshold 5: a window of 50 ms.
    // The colony's word lands at 7.5 ms, off the tick instants; the supervisor
    // hears it at the 10 ms tick and returns there, with no verdict, although
    // the channel closes right behind the word — the exact shape of a teardown
    // whose closing task would otherwise read as `colony_task_gone`.
    #[tokio::test(start_paused = true)]
    async fn a_stand_down_disarms_the_supervisor_for_the_teardown() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let start = tokio::time::Instant::now();
        let supervisor = tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        tokio::time::sleep(period / 4).await;
        hb_tx.try_send(Beat::Working).expect("room");
        tokio::time::sleep(period / 2).await;
        hb_tx.try_send(Beat::Parked).expect("room");
        hb_tx.try_send(Beat::StandDown).expect("room");
        drop(hb_tx);
        meclaw_testing::deadline::hang("the supervisor stands down", supervisor)
            .await
            .expect("the supervisor must end normally");
        assert_eq!(
            start.elapsed(),
            period,
            "the supervisor stands down at the first tick after the word"
        );
        assert!(
            trip_rx.recv().await.is_none(),
            "a stand-down is not a death: no trip, the trip channel simply closes"
        );
    }

    // GH #1216 (K8, the other side): a hang INSIDE the drain is silence on an
    // armed supervisor — the loop has not said its last word yet, so it is
    // judged exactly as before, and a stand-down that comes after the trip
    // changes nothing about it. Parked silence: the loop failed to come back
    // from its wait, the fatal verdict, five periods after the last beat.
    #[tokio::test(start_paused = true)]
    async fn a_hang_inside_the_drain_still_trips() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let supervisor = tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        tokio::time::sleep(period / 4).await;
        hb_tx.try_send(Beat::Working).expect("room");
        tokio::time::sleep(period / 2).await;
        hb_tx.try_send(Beat::Parked).expect("room");
        let trip = meclaw_testing::deadline::hang(
            "a loop hung inside its drain must trip",
            trip_rx.recv(),
        )
        .await
        .expect("a trip, not a stand-down");
        assert!(!trip.colony_task_gone, "the task is alive, hung: {trip}");
        assert_eq!(trip.starved(), "colony_loop", "{trip}");
        assert_eq!(trip.beats_seen, 2, "{trip}");
        assert_eq!(trip.silent_for, period * 5, "{trip}");
        assert!(trip.is_fatal(WatchdogOnTrip::Exit), "{trip}");
        // The verdict is final: the supervisor ended at its fatal trip, so the
        // loop's late stand-down finds nobody listening and the trip stands
        // alone -- no second word, no retraction.
        meclaw_testing::deadline::hang("the supervisor ends at its fatal trip", supervisor)
            .await
            .expect("the supervisor must end normally");
        assert!(
            matches!(
                hb_tx.try_send(Beat::StandDown),
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
            ),
            "a stand-down after the trip reaches no supervisor"
        );
        drop(hb_tx);
        assert!(
            trip_rx.recv().await.is_none(),
            "the fatal trip is the supervisor's last word"
        );
    }

    // GH #1216: a colony that shut down before boot had finished stood down
    // before the supervisor was armed. The arming drain throws away stale
    // beats, but not that word — an armed supervisor would otherwise judge the
    // closed channel of a colony that ended in order as a death.
    #[tokio::test(start_paused = true)]
    async fn a_stand_down_before_arming_keeps_the_supervisor_disarmed() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(8);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
        let supervisor = tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            arm_rx,
            WatchdogOnTrip::Exit,
            None,
        ));
        hb_tx.try_send(Beat::Working).expect("room");
        hb_tx.try_send(Beat::StandDown).expect("room");
        drop(hb_tx);
        arm_tx
            .send(())
            .expect("the supervisor waits for its arming");
        meclaw_testing::deadline::hang("the supervisor never arms", supervisor)
            .await
            .expect("the supervisor must end normally");
        assert!(
            trip_rx.recv().await.is_none(),
            "a colony that stood down before arming is not a death"
        );
    }

    // GH #1216: the word is never dropped. A full channel (the supervisor has
    // not drained this period) gets it from a task of its own, BEHIND every
    // queued beat, and the supervisor still stands down instead of reading
    // the closed channel as a death.
    #[tokio::test(start_paused = true)]
    async fn a_stand_down_into_a_full_channel_still_arrives() {
        let period = Duration::from_millis(10);
        let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(2);
        let (trip_tx, mut trip_rx) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
        let supervisor = tokio::spawn(run_watchdog(
            hb_rx,
            trip_tx,
            5,
            period,
            armed(),
            WatchdogOnTrip::Exit,
            None,
        ));
        tokio::time::sleep(period / 4).await;
        while hb_tx.try_send(Beat::Parked).is_ok() {}
        WorkPulse::new(Some(hb_tx.clone()), WorkItem::new("teardown")).stand_down();
        drop(hb_tx);
        meclaw_testing::deadline::hang("the supervisor stands down", supervisor)
            .await
            .expect("the supervisor must end normally");
        assert!(
            trip_rx.recv().await.is_none(),
            "the word that met a full channel was delivered, not dropped"
        );
    }
}
