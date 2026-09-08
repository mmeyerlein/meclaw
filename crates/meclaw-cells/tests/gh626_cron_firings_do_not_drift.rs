//! GH #626: a cron firing lands on its second and stays there.
//!
//! `docs/cell-types.md` § `timer` promises that firing happens exactly at the
//! configured second. It did not: the loop recomputed the next occurrence from
//! the instant it woke — the firing time plus the wake latency — and croner
//! carries the sub-second part of the instant it is asked about into its answer,
//! so that fraction became the anchor for the next tick and grew with every one
//! of them.
//!
//! Driving `run_io` directly, the way `timer_io_loop.rs` does: the events
//! channel is the receipt. Failure markers are the 30-second convention; the
//! discriminators are the `scheduled_at` values.

use chrono::{DateTime, Duration as ChDur, Utc};
use meclaw_cells::timer::cell::TimerIo;
use meclaw_cells::timer::io::{TimerEvent, TimerReconfig, run_io};
use meclaw_cells::timer::schedule::{ActiveSchedule, ScheduleKind};
use meclaw_core::Uuid;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker only — a fire that needs longer than this is a hang, not a
/// slow machine (CLAUDE.md, 30-second convention).
const FIRE_MARKER: Duration = Duration::from_secs(30);

async fn next_fire(rx: &mut mpsc::Receiver<TimerEvent>, what: &str) -> DateTime<Utc> {
    let ev = tokio::time::timeout(FIRE_MARKER, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("no fire within 30s while waiting for {what}"))
        .unwrap_or_else(|| panic!("run_io closed the events channel while waiting for {what}"));
    let TimerEvent::Fire { scheduled_at, .. } = ev;
    scheduled_at
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consecutive_cron_firings_land_on_whole_seconds_exactly_one_apart() {
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let io = TimerIo {
        active: vec![ActiveSchedule {
            schedule_id: Uuid::now_v7(),
            kind: ScheduleKind::Cron("* * * * * *".into()),
        }],
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    let mut previous: Option<DateTime<Utc>> = None;
    for round in 0..3 {
        let at = next_fire(&mut events_rx, "a cron fire").await;
        assert_eq!(
            at.timestamp_subsec_nanos(),
            0,
            "fire {round} was planned for {at:?}, which is not a whole second — \
             the sub-second part is the wake latency the previous round leaked \
             into the plan"
        );
        if let Some(p) = previous {
            assert_eq!(
                at - p,
                ChDur::seconds(1),
                "fire {round} must sit exactly one second after its predecessor; \
                 anything else is drift accumulating tick by tick"
            );
        }
        previous = Some(at);
    }

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("run_io hung after reconfig close")
        .unwrap();
}
