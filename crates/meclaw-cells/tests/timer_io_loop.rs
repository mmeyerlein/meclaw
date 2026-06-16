//! Phase-10-B T8/T9: `run_io`-Loop. T8: Skelett — bei leerer Active-Menge
//! bleibt `sleep_until_optional` `pending`; SetActive mit future-`at` weckt
//! nicht; Reconfig-Channel-Close terminiert. T9: cron-`*/1`-Fire kommt
//! innerhalb 2.5 s; once feuert genau einmal, dann ist `active` lokal leer.

use chrono::{Duration as ChDur, TimeZone, Utc};
use meclaw_cells::timer::cell::TimerIo;
use meclaw_cells::timer::io::{TimerEvent, TimerReconfig, run_io};
use meclaw_cells::timer::schedule::{ActiveSchedule, ScheduleKind};
use meclaw_core::Uuid;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_on_empty_active_stays_pending_then_reacts_to_setactive_with_future_at() {
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let io = TimerIo { active: vec![] };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    // Innerhalb 100 ms KEIN Event (active leer → sleep_until = pending).
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events_rx.recv())
            .await
            .is_err()
    );

    // SetActive mit once weit in der Zukunft — immer noch kein Event.
    let future_at = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
    rc_tx
        .send(TimerReconfig::SetActive(vec![ActiveSchedule {
            schedule_id: Uuid::now_v7(),
            kind: ScheduleKind::At(future_at),
        }]))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events_rx.recv())
            .await
            .is_err()
    );

    // Sauberer Shutdown via Reconfig-Channel-Close. `run_io` matched
    // `reconfig_rx.recv() == None` → `break`; events_tx droppt; join terminiert.
    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .expect("run_io hung after reconfig close")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_emits_fire_for_every_second_cron_and_keeps_active() {
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let id = Uuid::now_v7();
    let io = TimerIo {
        active: vec![ActiveSchedule {
            schedule_id: id,
            kind: ScheduleKind::Cron("*/1 * * * * *".into()),
        }],
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    let ev = tokio::time::timeout(Duration::from_millis(2500), events_rx.recv())
        .await
        .expect("kein Fire innerhalb 2.5s")
        .unwrap();
    let TimerEvent::Fire { schedule_id, .. } = ev;
    assert_eq!(schedule_id, id, "Fire trug fremde schedule_id");

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(3), join)
        .await
        .expect("run_io hung")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_drops_once_locally_after_fire() {
    // once auf jetzt+200ms → Fire kommt → active sollte danach leer sein
    // (run_io feuert NICHT erneut innerhalb 800 ms Window).
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let id = Uuid::now_v7();
    let when = Utc::now() + ChDur::milliseconds(200);
    let io = TimerIo {
        active: vec![ActiveSchedule {
            schedule_id: id,
            kind: ScheduleKind::At(when),
        }],
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    let _ev = tokio::time::timeout(Duration::from_millis(800), events_rx.recv())
        .await
        .expect("once-Fire fehlt")
        .unwrap();
    // Kein zweiter Fire innerhalb der naechsten 800 ms.
    assert!(
        tokio::time::timeout(Duration::from_millis(800), events_rx.recv())
            .await
            .is_err(),
        "once feuerte ein zweites Mal"
    );

    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .expect("run_io hung")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_terminates_when_events_channel_consumer_drops() {
    // Defensive: wenn events_rx droppt (Handler tot), terminiert run_io
    // beim naechsten Fire-Send-Failure. Hier *2 s* Window mit cron-*/1*.
    let (events_tx, events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let io = TimerIo {
        active: vec![ActiveSchedule {
            schedule_id: Uuid::now_v7(),
            kind: ScheduleKind::Cron("*/1 * * * * *".into()),
        }],
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));
    drop(events_rx);
    tokio::time::timeout(Duration::from_secs(3), join)
        .await
        .expect("run_io ignorierte events-Channel-Close")
        .unwrap();
    drop(rc_tx);
}
