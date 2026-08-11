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

/// Handler → I/O: full snapshot of the active schedule set. After every
/// successful `add`/`modify`/`remove` op the handler recomputes the snapshot
/// fresh from `cell.db` and sends it to the I/O task, which replaces its working
/// copy and recomputes the next `sleep_until`.
#[derive(Debug, Clone)]
pub enum TimerReconfig {
    /// Complete replacement of the I/O-local active set.
    SetActive(Vec<ActiveSchedule>),
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

/// Returns `(index_in_active, next_time)` of the earliest future occurrence
/// across all schedules; `None` when nothing lies in the future (active empty or
/// all past one-shots — the latter should never happen because
/// `load_active_filter_past` drops past one-shots; checked defensively).
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
            ScheduleKind::At(t) => {
                if *t > now {
                    Some(*t)
                } else {
                    None
                }
            }
        };
        if let Some(t) = next
            && best.map(|(_, bt)| t < bt).unwrap_or(true)
        {
            best = Some((i, t));
        }
    }
    best
}
