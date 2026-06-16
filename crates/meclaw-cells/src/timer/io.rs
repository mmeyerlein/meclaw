//! Phase-10-B: I/O-Sub-Task fuer `TimerCell`.
//!
//! Definiert die Frame-Typen, die zwischen Handler und I/O laufen (10-A-
//! Substrat: bounded 64 events, bounded 8 reconfig) sowie die `run_io`-
//! Loop selbst. Die Loop ist Single-Owner auf `TimerIo.active` (kein Mutex)
//! und `select!`'d ueber Reconfig + `sleep_until_optional(next)`.

use crate::timer::cell::TimerIo;
use crate::timer::schedule::{ActiveSchedule, ScheduleKind};
use chrono::{DateTime, Utc};
use croner::parser::{CronParser, Seconds};
use meclaw_core::Uuid;
use std::future::Future;
use tokio::sync::mpsc;

/// I/O → Handler: ein einzelner Schedule hat seinen `sleep_until`-Punkt
/// erreicht; Handler verarbeitet ihn in `handle_event` (race-check +
/// State-vor-Emit + OriginSink-Emit).
#[derive(Debug, Clone)]
pub enum TimerEvent {
    /// Eine Schedule ist gefeuert. Handler resolved `schedule_id` gegen
    /// `cell.db`, baut Auto-Set-Header (inkl. `scheduled_at`/`fired_at`)
    /// und emittiert via `OriginSink`.
    Fire {
        /// PK in `cell.db.schedules`.
        schedule_id: Uuid,
        /// Geplanter Feuer-Zeitpunkt (UTC). Wird in den Auto-Set-Header
        /// `scheduled_at` durchgereicht.
        scheduled_at: DateTime<Utc>,
    },
}

/// Handler → I/O: Full-Snapshot der aktiven Schedule-Menge. Nach jeder
/// erfolgreichen `add`/`modify`/`remove`-Op berechnet der Handler den
/// Snapshot frisch aus `cell.db` und schickt ihn an die I/O-Task, die ihre
/// Arbeitskopie ersetzt + den naechsten `sleep_until` neu rechnet.
#[derive(Debug, Clone)]
pub enum TimerReconfig {
    /// Vollstaendige Ersetzung der I/O-lokalen Active-Menge.
    SetActive(Vec<ActiveSchedule>),
}

/// I/O-Sub-Task. Single-Owner-State (`TimerIo.active`), kein Mutex.
/// `select!` ueber (a) Reconfig-Channel — Snapshot ersetzt active, neu rechnen
/// — und (b) `sleep_until_optional(next)` — bei None haengt pending; bei
/// Some sleep, dann Fire-Event pushen + lokal advance (T9).
///
/// Phase-10-A-Lesson (commit `31c15b6`): `events_tx` + `reconfig_rx` werden
/// in diesem Body REFERENZIERT — damit auto-captured ins `async move`. Bei
/// Reconfig-Close (`None`) terminiert die Loop sauber. `+ Send` ist
/// load-bearing (siehe TimerCell::run_io-Doc).
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    io: TimerIo,
    events_tx: mpsc::Sender<TimerEvent>,
    mut reconfig_rx: mpsc::Receiver<TimerReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        let parser = CronParser::builder().seconds(Seconds::Required).build();
        let mut active = io.active;
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
                            // Handler-Channel zu → Shutdown.
                            break;
                        }
                        if is_once {
                            // once nach Fire lokal verwerfen; repeating bleibt
                            // im Vec, naechste Iteration rechnet next > now.
                            active.swap_remove(idx);
                        }
                    }
                }
            }
        }
    }
}

/// Schlaeft bis zum Zeitpunkt `t` (Utc). Bei `None` haengt `pending` —
/// der `select!`-Arm wird nur durch Reconfig wieder geweckt.
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

/// Liefert `(index_in_active, next_time)` der fruehesten Zukunfts-Occurrence
/// ueber alle Schedules; `None` wenn nichts in der Zukunft liegt (active
/// leer oder alle past-onces — letzteres sollte nie passieren, weil
/// `load_active_filter_past` past-onces dropt; defensiv geprueft).
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
