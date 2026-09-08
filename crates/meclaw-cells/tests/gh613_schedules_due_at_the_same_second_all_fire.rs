//! GH #613: when two schedules of one `timer` cell come due at the same second,
//! every one of them fires, once each, in schedule order.
//!
//! The measured case was a live `clock` cell holding `*/20 * * * * *` next to
//! `0 */15 * * * *`. In two hours the 20-second schedule fired 272 times and the
//! quarter-hour one zero times, while its row stayed `active` and a manual
//! `trigger` worked — the loop picked one winner per instant and planned the
//! loser strictly after that second, so it jumped a whole period without a word.
//!
//! These tests drive `run_io` directly, the way `timer_io_loop.rs` does: the
//! events channel is the receipt, no colony, no `cell.db`. Failure markers are
//! the 30-second convention; the discriminators are the schedule ids and the
//! `scheduled_at` values, not the wall clock.

use chrono::{DateTime, Duration as ChDur, Timelike, Utc};
use meclaw_cells::timer::cell::TimerIo;
use meclaw_cells::timer::io::{TimerEvent, TimerReconfig, run_io};
use meclaw_cells::timer::schedule::{ActiveSchedule, ScheduleKind};
use meclaw_core::Uuid;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure marker only — a fire that needs longer than this is a hang, not a
/// slow machine (CLAUDE.md, 30-second convention).
const FIRE_MARKER: Duration = Duration::from_secs(30);

fn every_second(id: Uuid) -> ActiveSchedule {
    ActiveSchedule {
        schedule_id: id,
        kind: ScheduleKind::Cron("* * * * * *".into()),
    }
}

async fn next_fire(rx: &mut mpsc::Receiver<TimerEvent>, what: &str) -> (Uuid, DateTime<Utc>) {
    let ev = tokio::time::timeout(FIRE_MARKER, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("no fire within 30s while waiting for {what}"))
        .unwrap_or_else(|| panic!("run_io closed the events channel while waiting for {what}"));
    let TimerEvent::Fire {
        schedule_id,
        scheduled_at,
    } = ev;
    (schedule_id, scheduled_at)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_crons_due_at_the_same_second_both_fire_in_schedule_order() {
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let io = TimerIo {
        active: vec![every_second(first), every_second(second)],
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    // Two consecutive events. Under the old single-winner loop these were the
    // SAME schedule twice, a second apart — which is exactly what the pair of
    // assertions below separates: same instant, different schedule.
    let (id_a, at_a) = next_fire(&mut events_rx, "the first fire").await;
    let (id_b, at_b) = next_fire(&mut events_rx, "the second fire").await;
    assert_eq!(
        (id_a, id_b),
        (first, second),
        "both schedules must fire at the tick, in schedule order; getting the \
         same id twice means one of them was dropped as a missed firing"
    );
    assert_eq!(
        at_a, at_b,
        "the two firings belong to ONE second and must carry the same \
         scheduled_at — a differing one means the second event is the next \
         tick, not the tie partner"
    );

    // And it keeps doing it: the next second is another full pair, one tick on.
    let (id_c, at_c) = next_fire(&mut events_rx, "the third fire").await;
    let (id_d, at_d) = next_fire(&mut events_rx, "the fourth fire").await;
    assert_eq!(
        (id_c, id_d),
        (first, second),
        "the second tick must be a full pair as well, in the same order"
    );
    assert_eq!(at_c, at_d, "the second pair shares one scheduled_at too");
    assert!(
        at_c > at_a,
        "the second pair belongs to a LATER tick — same scheduled_at across \
         both pairs would mean four firings of one second"
    );

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("run_io hung after reconfig close")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_shot_leaving_the_set_does_not_reorder_the_schedules_that_stay() {
    // A one-shot in front of two tied crons. When it has fired it leaves the
    // working set, and the two that stay must keep firing as the pair they are,
    // in the order they were in: the firing order the tie promises is `active`
    // order, so a removal that swaps the tail into the hole would silently
    // reverse it from that moment on.
    //
    // The one-shot here does not tie with anything; the tie between a one-shot
    // and a cron has its own test below.
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let once = Uuid::now_v7();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let at = Utc::now() + ChDur::milliseconds(1500);
    let io = TimerIo {
        active: vec![
            ActiveSchedule {
                schedule_id: once,
                kind: ScheduleKind::At(at),
            },
            every_second(first),
            every_second(second),
        ],
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    // Wait for the one-shot; everything before it is cron traffic.
    let mut seen = 0;
    let (mut id, mut got) = next_fire(&mut events_rx, "the first fire").await;
    while id != once {
        assert!(
            id == first || id == second,
            "an unknown schedule fired before the one-shot"
        );
        seen += 1;
        assert!(seen < 12, "the one-shot never fired");
        (id, got) = next_fire(&mut events_rx, "the one-shot").await;
    }
    assert_eq!(got, at, "the one-shot fires at its own instant");

    // It is gone, and the two that stay are still a pair, still in order.
    for round in 0..2 {
        let (id_a, at_a) = next_fire(&mut events_rx, "the first half of a tick").await;
        let (id_b, at_b) = next_fire(&mut events_rx, "the second half of a tick").await;
        assert_eq!(
            (id_a, id_b),
            (first, second),
            "tick {round} after the one-shot left: both crons must still fire, \
             and in the order they sit in — the same id twice means one was \
             dropped as a missed firing, the reversed pair means the removal \
             swapped the tail into the hole"
        );
        assert_eq!(at_a, at_b, "tick {round} is one instant, not two");
        assert!(
            at_a > at,
            "tick {round} must lie after the one-shot's instant"
        );
    }

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("run_io hung after reconfig close")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_shot_and_a_cron_due_at_the_same_second_both_fire_and_only_the_one_shot_leaves() {
    // The pairing the live report was made of, end to end: a schedule that goes
    // away after firing next to one that does not, both due at one instant.
    //
    // This test could not be written before GH #626. A one-shot is stored at
    // millisecond precision, and a cron used to inherit the sub-second fraction
    // of whatever instant the loop woke with, so the two came out a hair apart
    // and the one-shot always won alone. With the cron anchored on the whole
    // second, an `at` on a whole second meets it exactly.
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let once = Uuid::now_v7();
    let repeating = Uuid::now_v7();
    // A whole second, two ticks out: `* * * * * *` lands on every whole second.
    let at = Utc::now()
        .with_nanosecond(0)
        .expect("truncating to a whole second")
        + ChDur::seconds(2);
    let io = TimerIo {
        active: vec![
            ActiveSchedule {
                schedule_id: once,
                kind: ScheduleKind::At(at),
            },
            every_second(repeating),
        ],
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    // Drain the cron ticks before the collision.
    let mut seen = 0;
    let (mut id, mut got) = next_fire(&mut events_rx, "the first fire").await;
    while got < at {
        assert_eq!(
            id, repeating,
            "before the collision only the cron is due; the one-shot must not \
             fire early"
        );
        seen += 1;
        assert!(seen < 10, "the collision second never arrived");
        (id, got) = next_fire(&mut events_rx, "the fire at the collision second").await;
    }
    assert_eq!(
        (id, got),
        (once, at),
        "at the collision second the one-shot fires first — it sits first in \
         the schedule order"
    );
    let (id, got) = next_fire(&mut events_rx, "the cron half of the collision").await;
    assert_eq!(
        (id, got),
        (repeating, at),
        "and the cron fires at the SAME instant, right after it — this is the \
         firing GH #613 dropped"
    );

    // Only the one-shot left the set.
    for _ in 0..2 {
        let (id, _) = next_fire(&mut events_rx, "a tick after the collision").await;
        assert_eq!(
            id, repeating,
            "after firing, the one-shot is gone and only the cron keeps ticking"
        );
    }

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("run_io hung after reconfig close")
        .unwrap();
}
