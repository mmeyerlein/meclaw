//! Phase-10-B: the I/O sub-task for `TimerCell`.
//!
//! Defines the frame types that travel between handler and I/O (10-A substrate:
//! bounded 64 events, bounded 8 reconfig) plus the `run_io` loop itself. The loop
//! is single-owner over `TimerIo.active` (no mutex) and `select!`s over reconfig
//! + `sleep_until_optional(next)`.

use crate::timer::cell::TimerIo;
use crate::timer::schedule::{ActiveSchedule, ScheduleKind};
use chrono::{DateTime, Timelike, Utc};
use croner::parser::{CronParser, Seconds};
use meclaw_core::Uuid;
use std::future::Future;
use tokio::sync::mpsc;

/// I/O → handler: a single schedule has reached its `sleep_until` point; the
/// handler processes it in `handle_event` (race check + state-before-emit +
/// OriginSink emit).
#[derive(Debug, Clone)]
pub enum TimerEvent {
    /// A schedule has fired. The handler resolves `schedule_id` against
    /// `cell.db`, builds the auto-set headers (incl. `scheduled_at`/`fired_at`)
    /// and emits via `OriginSink`.
    Fire {
        /// PK in `cell.db.schedules`.
        schedule_id: Uuid,
        /// Planned firing time (UTC). Passed through into the auto-set header
        /// `scheduled_at`.
        scheduled_at: DateTime<Utc>,
        /// `true` for an operator trigger (GH #17, `FireNow`): the schedule
        /// fires as it stands, whatever its moment. `false` for a strike of the
        /// sleep arm, which belongs to the moment it slept for -- a one-shot
        /// whose `at` no longer matches `scheduled_at` was replaced (`rearm`,
        /// `modify`) after the strike was queued, and `handle_event` skips it
        /// (GH #904 fix round 1).
        forced: bool,
    },
}

/// Handler → I/O frames. After every successful `add`/`modify`/`remove` op the
/// handler recomputes the active snapshot fresh from `cell.db` and sends it, and
/// the I/O task replaces its working copy and recomputes the next `sleep_until`.
#[derive(Debug, Clone)]
pub enum TimerReconfig {
    /// Complete replacement of the I/O-local active set.
    SetActive(Vec<ActiveSchedule>),
    /// Fire this schedule once, now (GH #17). Not a reconfiguration: the plan is
    /// untouched and the working copy is not read. It travels on this channel
    /// because this channel IS the handler-to-I/O direction, and the firing has
    /// to originate in the I/O task: the handler holds no `OriginSink`, so an
    /// emission it made itself could not be the one a cron tick makes.
    FireNow {
        /// PK of the schedule to fire. Resolved against `cell.db` by
        /// `handle_event`, exactly as for a `sleep_until` firing.
        schedule_id: Uuid,
    },
}

/// GH #956: handler → I/O, "this strike did not take; strike it again". A
/// one-shot leaves the working copy the moment its strike is pushed (the sleep
/// arm below drops it locally), so a strike whose state step then fails in
/// `handle_event` -- `load_schedule` or `mark_completed` running out of
/// `query_timeout_ms` -- left the row `active` in `cell.db` and planned
/// nowhere: nothing fired until the next op snapshot or the next boot, and a
/// test under suite load waited its 30 s out (#956, candidate b). The handler
/// sends this frame instead, and the I/O task strikes the same moment once
/// more after [`REPLAN_DELAY`].
///
/// Its own channel, not [`TimerReconfig`]: the handler only holds the reconfig
/// sender while it handles a mailbox message, and the strike it has to plan
/// again arrives as an I/O event -- the catch-up strike after a boot arrives
/// before any message at all. The sender lives in the cell from `split_io` on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TimerReplan {
    /// PK in `cell.db.schedules`.
    pub schedule_id: Uuid,
    /// The moment the failed strike was for. The strike again carries it
    /// unchanged, so the stale-strike guard of GH #904 and the `late` mark of
    /// GH #922 read it exactly as they read the first one.
    pub scheduled_at: DateTime<Utc>,
}

/// GH #956: how long the I/O task waits before it strikes a
/// [`TimerReplan`]-ed moment again. The failed step timed out on an overloaded
/// `cell.db`; striking at once would hand the same overload the same query.
/// One second is one cron period at the finest grain the timer plans, and it
/// is the order of the default `query_timeout_ms`.
pub const REPLAN_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// GH #956 review M-2: the longest wait between two strikes of one replanned
/// moment. The wait doubles from [`REPLAN_DELAY`] up to this.
pub const REPLAN_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(60);

/// GH #956 review M-2: how often one moment is struck again before the I/O task
/// gives it up. A `mark_completed` that fails for good (read-only or broken
/// `cell.db`, `Ok(Err(_))` and no timeout) was replanned every second without
/// end; with the back-off the retries of one moment span about four minutes,
/// then the row stays `active` for the next op snapshot or boot -- the state
/// before #956, said out loud.
pub const REPLAN_MAX_ATTEMPTS: u32 = 10;

/// GH #956: how many replan frames the handler may queue before the I/O task
/// picks them up. Sent with `try_send`, never awaited: the I/O task can itself
/// be blocked pushing an event to the handler, and a handler that waited for
/// room here would close that cycle.
pub const REPLAN_CAPACITY: usize = 64;

/// I/O sub-task. Single-owner state (`TimerIo.active`), no mutex.
/// `select!` over (a) the reconfig channel — the snapshot replaces active,
/// recompute — and (b) `sleep_until_optional(next)` — on None it hangs pending,
/// on Some it sleeps, then pushes the fire event and advances locally (T9).
///
/// Phase-10-A lesson (commit `31c15b6`): `events_tx` + `reconfig_rx` are
/// REFERENCED in this body — that is what auto-captures them into the
/// `async move`. On reconfig close (`None`) the loop terminates cleanly. `+ Send`
/// is load-bearing (see the TimerCell::run_io docs).
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    io: TimerIo,
    events_tx: mpsc::Sender<TimerEvent>,
    mut reconfig_rx: mpsc::Receiver<TimerReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let mut active = io.active;
        let liveness = io.liveness;
        // GH #956: the handler's replans and the strikes they wait for. A
        // working copy of its own, not entries in `active`: a replanned strike
        // belongs to the moment that failed, and `compute_next_occurrence`
        // would plan a cron row in `active` strictly after now.
        let mut replan_rx = io.replan_rx;
        let mut retries: Vec<(TimerReplan, tokio::time::Instant)> = Vec::new();
        // GH #956 review M-2: strikes again per moment, for the back-off and
        // its cap. Bounded by the moments that failed since the last op
        // snapshot; a moment given up leaves it.
        let mut attempts: std::collections::HashMap<TimerReplan, u32> =
            std::collections::HashMap::new();
        // Issue #7: announce before the first sleep — a timer that has not fired
        // yet is visibly "no tick yet", not invisible.
        liveness.announce();
        loop {
            let next = compute_next_occurrence(&active, &parser, Utc::now());
            tokio::select! {
                biased;
                maybe_rc = reconfig_rx.recv() => match maybe_rc {
                    Some(TimerReconfig::SetActive(snap)) => {
                        active = snap;
                        // GH #956 review M-2: an op snapshot plans every row
                        // from `cell.db` afresh; the retry counts start over.
                        attempts.clear();
                    }
                    Some(TimerReconfig::FireNow { schedule_id }) => {
                        // GH #17: the operator's trigger enters through the SAME
                        // frame the sleep arm below pushes, so the run that
                        // follows is not "like" a cron-fired one, it IS one --
                        // same event, same handle_event, same OriginSink emit.
                        // `active` stays untouched: a triggered cron keeps its
                        // next occurrence, and a triggered one-shot is dropped by
                        // handle_event's status check when its own time comes.
                        if events_tx
                            .send(TimerEvent::Fire {
                                schedule_id,
                                scheduled_at: Utc::now(),
                                forced: true,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        liveness.mark_success();
                    }
                    None => break,
                },
                maybe_rp = recv_optional(&mut replan_rx) => match maybe_rp {
                    Some(rp) => {
                        if take_replan(&active, &retries, &rp) {
                            match next_replan_delay(&mut attempts, &rp) {
                                Some(delay) => {
                                    retries.push((rp, tokio::time::Instant::now() + delay));
                                }
                                None => tracing::warn!(
                                    schedule_id = %rp.schedule_id,
                                    scheduled_at = %rp.scheduled_at,
                                    attempts = REPLAN_MAX_ATTEMPTS,
                                    "timer: strike failed its state step again and again, \
                                     row left for the next plan"
                                ),
                            }
                        }
                    }
                    // The handler is gone; the reconfig arm ends the loop.
                    None => replan_rx = None,
                },
                _ = sleep_until_instant_optional(retries.iter().map(|(_, due)| *due).min()) => {
                    let now = tokio::time::Instant::now();
                    let mut handler_gone = false;
                    let mut kept = Vec::with_capacity(retries.len());
                    for (rp, due) in std::mem::take(&mut retries) {
                        if due > now || handler_gone {
                            kept.push((rp, due));
                            continue;
                        }
                        if events_tx
                            .send(TimerEvent::Fire {
                                schedule_id: rp.schedule_id,
                                scheduled_at: rp.scheduled_at,
                                forced: false,
                            })
                            .await
                            .is_err()
                        {
                            handler_gone = true;
                            continue;
                        }
                        liveness.mark_success();
                    }
                    retries = kept;
                    if handler_gone {
                        break;
                    }
                }
                _ = sleep_until_optional(next.as_ref().map(|(t, _)| *t)) => {
                    if let Some((t, due)) = next {
                        // GH #613: EVERY schedule due at this instant fires,
                        // once each, in `active` order (= schedule order). The
                        // loop used to pick one winner here and plan the rest
                        // strictly after `t` on the next round, which moved them
                        // a whole period on — silently, every time two schedules
                        // shared a second.
                        let mut handler_gone = false;
                        for &idx in &due {
                            let schedule_id = active[idx].schedule_id;
                            if events_tx
                                .send(TimerEvent::Fire {
                                    schedule_id,
                                    scheduled_at: t,
                                    forced: false,
                                })
                                .await
                                .is_err()
                            {
                                // Handler channel closed → shutdown.
                                handler_gone = true;
                                break;
                            }
                            // Issue #7: a due schedule was delivered — this loop
                            // is demonstrably still turning.
                            liveness.mark_success();
                        }
                        if handler_gone {
                            break;
                        }
                        // Drop the one-shots locally after firing; a repeating
                        // schedule stays in the Vec and the next iteration
                        // computes next > now. Descending, because removing at
                        // `idx` moves everything above it down — ascending would
                        // delete the wrong neighbours. `remove` and not
                        // `swap_remove`: the order of `active` IS the schedule
                        // order the firing order above promises, and swapping
                        // the tail into the hole would scramble it until the
                        // next SetActive.
                        for &idx in due.iter().rev() {
                            if matches!(active[idx].kind, ScheduleKind::At(_)) {
                                active.remove(idx);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// GH #956: whether a replan frame becomes a strike again. Not for a cron row
/// of the working copy: its next occurrence is planned already, and the
/// moment that failed is a tick dropped the way a timed-out cron strike has
/// always been dropped (`handle_event` step 1). Not twice for one moment: a
/// replan that is already waiting covers it.
fn take_replan(
    active: &[ActiveSchedule],
    waiting: &[(TimerReplan, tokio::time::Instant)],
    rp: &TimerReplan,
) -> bool {
    let is_cron = active
        .iter()
        .any(|a| a.schedule_id == rp.schedule_id && matches!(a.kind, ScheduleKind::Cron(_)));
    !is_cron && !waiting.iter().any(|(w, _)| w == rp)
}

/// GH #956 review M-2: the wait before the next strike of `rp`, counted in
/// `attempts` per moment; `None` once the moment has had
/// [`REPLAN_MAX_ATTEMPTS`] strikes again (its count is dropped then).
fn next_replan_delay(
    attempts: &mut std::collections::HashMap<TimerReplan, u32>,
    rp: &TimerReplan,
) -> Option<std::time::Duration> {
    let n = attempts.entry(rp.clone()).or_insert(0);
    if *n >= REPLAN_MAX_ATTEMPTS {
        attempts.remove(rp);
        return None;
    }
    let delay = REPLAN_DELAY
        .saturating_mul(1u32 << (*n).min(16))
        .min(REPLAN_MAX_DELAY);
    *n += 1;
    Some(delay)
}

/// GH #956: the next replan frame, or `pending` forever once there is no
/// channel (a `TimerIo` built without one, or a handler that is gone).
async fn recv_optional(rx: &mut Option<mpsc::Receiver<TimerReplan>>) -> Option<TimerReplan> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// GH #956: sleeps until `t`; on `None` it hangs `pending`, like
/// [`sleep_until_optional`].
async fn sleep_until_instant_optional(t: Option<tokio::time::Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending::<()>().await,
    }
}

/// Sleeps until the point in time `t` (UTC). On `None` it hangs `pending` — the
/// `select!` arm is only woken again by a reconfig.
async fn sleep_until_optional(t: Option<DateTime<Utc>>) {
    match t {
        Some(t) => {
            let dur = (t - Utc::now()).to_std().unwrap_or_default();
            let until = tokio::time::Instant::now() + dur;
            tokio::time::sleep_until(until).await;
        }
        None => std::future::pending::<()>().await,
    }
}

/// Returns the earliest due-or-future occurrence across all schedules together
/// with the indices of **every** schedule that falls on it, in `active` order;
/// `None` when the set holds nothing that will ever come round (empty, or only
/// cron expressions with no next occurrence).
///
/// A cron expression is planned strictly after `now` — missed repeats are not
/// caught up (spec § `timer`). A one-shot in the set is taken as it stands: see
/// the `At` arm below (GH #231).
///
/// GH #613: the tie is the point. Two schedules of one cell whose next
/// occurrence is the same instant used to leave here as one winner, and the
/// loser was re-planned strictly after that instant on the next round, so it
/// skipped a full period without a log line. Every index that ties comes back,
/// and `active` order is the order the caller fires them in.
fn compute_next_occurrence(
    active: &[ActiveSchedule],
    parser: &CronParser,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, Vec<usize>)> {
    let mut best: Option<(DateTime<Utc>, Vec<usize>)> = None;
    for (i, s) in active.iter().enumerate() {
        let next = match &s.kind {
            // GH #626: ask from the whole second, not from the fraction the
            // loop woke in. croner carries the sub-second part of the instant
            // it is asked about into its answer, so the wake latency of one
            // firing became the anchor of the next one and the offset grew
            // tick by tick — against the spec line "firing happens exactly at
            // the configured second". The search is exclusive, so an anchor on
            // the second that has just fired yields the NEXT slot and not that
            // same second again.
            ScheduleKind::Cron(expr) => {
                let anchor = now.with_nanosecond(0).unwrap_or(now);
                parser
                    .parse(expr)
                    .ok()
                    .and_then(|c| c.find_next_occurrence(&anchor, false).ok())
            }
            // GH #231: a one-shot in the working set is DUE, not gone. The set
            // only ever receives one-shots that were still ahead when it was
            // built (`load_active_filter_past` filters, the handler refuses a
            // past `at` outright), so one whose moment arrived while this loop
            // was computing has to fire — dropping it here was the last place
            // an accepted schedule could vanish without a word. A past instant
            // makes `sleep_until_optional` resolve to a zero wait.
            ScheduleKind::At(t) => Some(*t),
        };
        let Some(t) = next else { continue };
        match &mut best {
            // A tie joins the group; iteration is ascending, so the group stays
            // in `active` order.
            Some((bt, due)) if t == *bt => due.push(i),
            Some((bt, _)) if t > *bt => {}
            // Either nothing yet, or this one is strictly earlier: it becomes
            // the new group, alone.
            _ => best = Some((t, vec![i])),
        }
    }
    best
}

#[cfg(test)]
mod tests_utc {
    use super::*;

    /// GH #254 — `docs/cell-types.md`: **cron expressions are evaluated in UTC.**
    ///
    /// This was the one claim of the eight with nothing behind it. What looked
    /// like its pin (`cron_parse_smoke`) builds its own `CronParser` and its own
    /// `Utc` and never touches timer code at all, so it would stay green if this
    /// module were switched to a local clock tomorrow — it pins the third-party
    /// library, not the promise. Every other timer test runs `*/1` or `*/5`,
    /// which are timezone-invariant by construction and therefore cannot tell
    /// the two apart either.
    ///
    /// So the assertion has to use an expression whose answer DIFFERS between
    /// UTC and a local zone: a wall-clock hour. `0 0 9 * * *` next-after
    /// midnight UTC is 09:00 UTC and nothing else; under `Europe/Berlin` the
    /// same instant would resolve to 07:00 UTC.
    ///
    /// **No `TZ` environment manipulation.** Setting an env var is `unsafe` in
    /// Rust 2024 and this test shares its process with every other test in the
    /// binary — a mutation would leak sideways. Injecting a fixed `now` proves
    /// the same thing without touching global state, because
    /// `compute_next_occurrence` takes the instant as a parameter.
    fn cron(expr: &str) -> ActiveSchedule {
        ActiveSchedule {
            schedule_id: Uuid::now_v7(),
            kind: ScheduleKind::Cron(expr.to_string()),
        }
    }

    fn one(expr: &str) -> Vec<ActiveSchedule> {
        vec![cron(expr)]
    }

    fn next_after(expr: &str, now: &str) -> DateTime<Utc> {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let now: DateTime<Utc> = now.parse().expect("a fixed UTC instant");
        let (t, _) = compute_next_occurrence(&one(expr), &parser, now)
            .expect("a daily cron always has a next occurrence");
        t
    }

    #[test]
    fn cron_next_occurrence_is_anchored_in_utc() {
        // Winter and summer, deliberately as a PAIR. One of them alone proves
        // nothing: a zone with no DST could pass it by luck, and `Europe/Berlin`
        // is UTC+1 in January and UTC+2 in July — so a local-clock
        // implementation must get at least one of these two wrong, whatever
        // zone the machine is in.
        assert_eq!(
            next_after("0 0 9 * * *", "2026-01-15T00:00:00Z").to_rfc3339(),
            "2026-01-15T09:00:00+00:00",
            "a 09:00 cron must resolve to 09:00 UTC in winter, not to a local \
             wall clock"
        );
        assert_eq!(
            next_after("0 0 9 * * *", "2026-07-15T00:00:00Z").to_rfc3339(),
            "2026-07-15T09:00:00+00:00",
            "and to the same 09:00 UTC in summer — a local-clock evaluation \
             would shift this one against the winter case above, which is the \
             whole point of testing both"
        );
    }

    /// The counter-check: the interval expressions every other timer test uses
    /// cannot distinguish UTC from anything else, which is why they never
    /// covered this claim.
    #[test]
    fn an_interval_expression_is_timezone_invariant_and_pins_nothing_here() {
        let a = next_after("*/5 * * * * *", "2026-01-15T00:00:00Z");
        assert_eq!(
            a.to_rfc3339(),
            "2026-01-15T00:00:05+00:00",
            "an every-5-seconds cron lands 5 seconds later in EVERY zone — a \
             test built on it stays green under a local clock, which is how \
             this claim went unpinned"
        );
    }

    /// GH #613: two schedules of ONE cell that come due at the same second.
    ///
    /// The measured case was a `clock` cell holding `*/20 * * * * *` next to
    /// `0 */15 * * * *`. Every quarter hour both are due at the same instant,
    /// and only one of them ever fired: the loop picked a single winner, and the
    /// next round planned the loser strictly after that second, so it jumped a
    /// whole period — silently, every quarter hour, forever.
    ///
    /// The instant is injected, so this test says nothing about wall-clock
    /// timing; it says which schedules the loop considers due at one instant.
    /// The non-due `0 0 9 * * *` sits FIRST on purpose: it makes the returned
    /// numbers positions in `active`, not a count, and it pins that the order is
    /// `active` order (= schedule order) and not discovery order.
    #[test]
    fn every_schedule_due_at_the_same_instant_is_returned_in_active_order() {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let now: DateTime<Utc> = "2026-09-08T16:14:59Z"
            .parse()
            .expect("a fixed UTC instant one second before a quarter hour");
        let active = vec![
            cron("0 0 9 * * *"),
            cron("*/20 * * * * *"),
            cron("0 */15 * * * *"),
        ];

        let (t, due) =
            compute_next_occurrence(&active, &parser, now).expect("three crons all have a next");

        assert_eq!(
            t.to_rfc3339(),
            "2026-09-08T16:15:00+00:00",
            "the earliest occurrence is the quarter hour both interval \
             expressions land on"
        );
        assert_eq!(
            due,
            vec![1, 2],
            "both schedules due at that instant must come back, in `active` \
             order; the daily one at index 0 is not due and must not"
        );
    }

    /// The counter-case, so the tie handling above cannot be read as "return
    /// everything": a set with one clear earliest schedule still yields exactly
    /// that one.
    #[test]
    fn a_single_earliest_schedule_comes_back_alone() {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let now: DateTime<Utc> = "2026-09-08T16:14:45Z".parse().expect("a fixed UTC instant");
        let active = vec![cron("0 */15 * * * *"), cron("*/7 * * * * *")];

        let (t, due) = compute_next_occurrence(&active, &parser, now).expect("both have a next");

        assert_eq!(
            t.to_rfc3339(),
            "2026-09-08T16:14:49+00:00",
            "the 7-second slot at :49 is earlier than the quarter hour"
        );
        assert_eq!(
            due,
            vec![1],
            "only the schedule that is actually earliest comes back — a tie is \
             a tie, not a licence to fire the whole set"
        );
    }

    /// A one-shot and a cron can tie too, and the one-shot is not privileged:
    /// both come back, in `active` order. This is the pairing the loop treats
    /// asymmetrically afterwards — the one-shot leaves the working set, the cron
    /// stays — so it is worth pinning that they enter the firing together.
    ///
    /// The injected `now` carries nanoseconds deliberately (GH #626). A one-shot
    /// is stored at millisecond precision and a cron used to inherit the
    /// fraction of whatever instant the loop woke with, so the two could never
    /// meet: the one-shot came out a hair earlier, fired alone, and the cron
    /// lost that second. A `now` without nanoseconds would hide exactly that.
    #[test]
    fn a_one_shot_ties_with_a_cron_and_both_come_back() {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let now: DateTime<Utc> = "2026-09-08T16:14:59.001042943Z"
            .parse()
            .expect("a fixed UTC instant");
        let at: DateTime<Utc> = "2026-09-08T16:15:00Z".parse().expect("a fixed UTC instant");
        let active = vec![
            ActiveSchedule {
                schedule_id: Uuid::now_v7(),
                kind: ScheduleKind::At(at),
            },
            cron("*/20 * * * * *"),
        ];

        let (t, due) = compute_next_occurrence(&active, &parser, now).expect("both are due");

        assert_eq!(t, at);
        assert_eq!(
            due,
            vec![0, 1],
            "the one-shot and the cron are due at the same instant and must \
             both fire"
        );
    }

    /// GH #626: a cron occurrence is anchored on the whole second.
    ///
    /// croner carries the sub-second part of the instant it is asked about into
    /// its answer — asked from `…09.001042943Z`, `* * * * * *` answers
    /// `…10.001042943Z`, not `…10Z`. The loop asks from the instant it woke,
    /// which is the firing time plus the wake latency, so that fraction became
    /// the anchor for the next tick and grew with every one of them. The spec
    /// line it breaks: "firing happens exactly at the configured second".
    #[test]
    fn a_cron_occurrence_is_anchored_on_the_whole_second() {
        let t = next_after("* * * * * *", "2026-09-08T16:14:09.001042943Z");
        assert_eq!(
            t.to_rfc3339(),
            "2026-09-08T16:14:10+00:00",
            "the next second is the whole second, not the whole second plus \
             whatever fraction the caller happened to ask from"
        );
        assert_eq!(
            t.timestamp_subsec_nanos(),
            0,
            "a planned firing carries no sub-second part at all"
        );
    }

    /// The other half of GH #626, and the reason the anchor may be truncated at
    /// all: the search is exclusive, so anchoring on the second that has just
    /// fired yields the NEXT slot and not that same second again. Without this
    /// the fix would trade a drift for a double firing.
    #[test]
    fn an_anchor_on_the_second_that_just_fired_does_not_fire_it_again() {
        // 300 µs after a firing at :00 — the instant the loop wakes with.
        let t = next_after("*/20 * * * * *", "2026-09-08T16:15:00.000300000Z");
        assert_eq!(
            t.to_rfc3339(),
            "2026-09-08T16:15:20+00:00",
            "truncating the anchor to :00 must not re-plan :00"
        );
    }
}

#[cfg(test)]
mod tests_replan {
    use super::*;

    fn rp(id: Uuid, at: &str) -> TimerReplan {
        TimerReplan {
            schedule_id: id,
            scheduled_at: at.parse().unwrap(),
        }
    }

    /// GH #956: a replanned one-shot is struck again once; a cron row of the
    /// working copy is not (its next tick is planned, the failed one is
    /// dropped as before), and a moment already waiting is not queued twice.
    #[test]
    fn a_replan_takes_a_one_shot_once_and_never_a_cron_row() {
        let once = Uuid::now_v7();
        let cron = Uuid::now_v7();
        let active = vec![ActiveSchedule {
            schedule_id: cron,
            kind: ScheduleKind::Cron("*/5 * * * * *".into()),
        }];
        let at = "2026-10-03T10:00:00Z";
        assert!(take_replan(&active, &[], &rp(once, at)));
        assert!(!take_replan(&active, &[], &rp(cron, at)));
        let waiting = vec![(rp(once, at), tokio::time::Instant::now())];
        assert!(!take_replan(&active, &waiting, &rp(once, at)));
        assert!(
            take_replan(&active, &waiting, &rp(once, "2026-10-03T10:00:01Z")),
            "another moment of the same row is its own strike"
        );
    }

    /// GH #956 review M-2 (welle-nachlese Z): a strike whose state step fails
    /// for good -- `mark_completed` on a read-only or broken `cell.db` answers
    /// `Ok(Err(_))`, no timeout -- was planned again every second without end.
    /// The retries of one moment back off from [`REPLAN_DELAY`] and stop after
    /// [`REPLAN_MAX_ATTEMPTS`]; the row stays `active` for the next op
    /// snapshot or boot, the state before #956. Another moment counts afresh,
    /// and an op snapshot (`SetActive`) forgets every count.
    #[test]
    fn a_replan_backs_off_and_gives_up_after_its_cap() {
        let once = Uuid::now_v7();
        let at = rp(once, "2026-10-03T10:00:00Z");
        let mut attempts = std::collections::HashMap::new();
        let mut delays = Vec::new();
        while let Some(d) = next_replan_delay(&mut attempts, &at) {
            delays.push(d);
            assert!(delays.len() <= 64, "a replan without end: {delays:?}");
        }
        assert_eq!(delays.len(), REPLAN_MAX_ATTEMPTS as usize);
        assert_eq!(
            delays[0], REPLAN_DELAY,
            "the first retry keeps its one second"
        );
        assert!(
            delays.windows(2).all(|w| w[0] <= w[1]),
            "backs off: {delays:?}"
        );
        assert!(delays.iter().all(|d| *d <= REPLAN_MAX_DELAY));
        assert_eq!(*delays.last().unwrap(), REPLAN_MAX_DELAY);
        assert!(
            !attempts.contains_key(&at),
            "a moment given up leaves no count behind"
        );
        let other = rp(once, "2026-10-03T10:00:05Z");
        assert_eq!(next_replan_delay(&mut attempts, &other), Some(REPLAN_DELAY));
    }
}
