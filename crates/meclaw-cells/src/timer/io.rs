//! Phase-10-B: the I/O sub-task for `TimerCell`.
//!
//! Defines the frame types that travel between handler and I/O (10-A substrate:
//! bounded 64 events, bounded 8 reconfig) plus the `run_io` loop itself. The loop
//! is single-owner over `TimerIo.active` (no mutex) and `select!`s over reconfig
//! + `sleep_until_optional(next)`.

use crate::timer::cell::TimerIo;
use crate::timer::schedule::{ActiveSchedule, ScheduleKind};
use chrono::{DateTime, Utc};
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
        // Issue #7: announce before the first sleep — a timer that has not fired
        // yet is visibly "no tick yet", not invisible.
        liveness.announce();
        loop {
            let next = compute_next_occurrence(&active, &parser, Utc::now());
            tokio::select! {
                biased;
                maybe_rc = reconfig_rx.recv() => match maybe_rc {
                    Some(TimerReconfig::SetActive(snap)) => { active = snap; }
                    Some(TimerReconfig::FireNow { schedule_id }) => {
                        // GH #17: the operator's trigger enters through the SAME
                        // frame the sleep arm below pushes, so the run that
                        // follows is not "like" a cron-fired one, it IS one --
                        // same event, same handle_event, same OriginSink emit.
                        // `active` stays untouched: a triggered cron keeps its
                        // next occurrence, and a triggered one-shot is dropped by
                        // handle_event's status check when its own time comes.
                        if events_tx
                            .send(TimerEvent::Fire { schedule_id, scheduled_at: Utc::now() })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        liveness.mark_success();
                    }
                    None => break,
                },
                _ = sleep_until_optional(next.map(|(_, t)| t)) => {
                    if let Some((idx, t)) = next {
                        let sched = &active[idx];
                        let schedule_id = sched.schedule_id;
                        let is_once = matches!(sched.kind, ScheduleKind::At(_));
                        if events_tx
                            .send(TimerEvent::Fire { schedule_id, scheduled_at: t })
                            .await
                            .is_err()
                        {
                            // Handler channel closed → shutdown.
                            break;
                        }
                        // Issue #7: a due schedule was delivered — this loop is
                        // demonstrably still turning.
                        liveness.mark_success();
                        if is_once {
                            // Drop a one-shot locally after firing; a repeating
                            // one stays in the Vec, the next iteration computes
                            // next > now.
                            active.swap_remove(idx);
                        }
                    }
                }
            }
        }
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

/// Returns `(index_in_active, next_time)` of the earliest due-or-future
/// occurrence across all schedules; `None` when the set holds nothing that will
/// ever come round (empty, or only cron expressions with no next occurrence).
///
/// A cron expression is planned strictly after `now` — missed repeats are not
/// caught up (spec § `timer`). A one-shot in the set is taken as it stands: see
/// the `At` arm below (GH #231).
fn compute_next_occurrence(
    active: &[ActiveSchedule],
    parser: &CronParser,
    now: DateTime<Utc>,
) -> Option<(usize, DateTime<Utc>)> {
    let mut best: Option<(usize, DateTime<Utc>)> = None;
    for (i, s) in active.iter().enumerate() {
        let next = match &s.kind {
            ScheduleKind::Cron(expr) => parser
                .parse(expr)
                .ok()
                .and_then(|c| c.find_next_occurrence(&now, false).ok()),
            // GH #231: a one-shot in the working set is DUE, not gone. The set
            // only ever receives one-shots that were still ahead when it was
            // built (`load_active_filter_past` filters, the handler refuses a
            // past `at` outright), so one whose moment arrived while this loop
            // was computing has to fire — dropping it here was the last place
            // an accepted schedule could vanish without a word. A past instant
            // makes `sleep_until_optional` resolve to a zero wait.
            ScheduleKind::At(t) => Some(*t),
        };
        if let Some(t) = next
            && best.map(|(_, bt)| t < bt).unwrap_or(true)
        {
            best = Some((i, t));
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
    fn one(expr: &str) -> Vec<ActiveSchedule> {
        vec![ActiveSchedule {
            schedule_id: Uuid::now_v7(),
            kind: ScheduleKind::Cron(expr.to_string()),
        }]
    }

    fn next_after(expr: &str, now: &str) -> DateTime<Utc> {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let now: DateTime<Utc> = now.parse().expect("a fixed UTC instant");
        let (_, t) = compute_next_occurrence(&one(expr), &parser, now)
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
}
