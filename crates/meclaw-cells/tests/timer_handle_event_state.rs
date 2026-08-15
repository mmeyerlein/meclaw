//! Phase-10-B T11: the `handle_event` persist path (state before emit / phase-5
//! canon). Firing a repeating schedule takes `iteration_n` in `cell.db` from 0 → 1
//! while status stays `active`. Firing a one-shot sets status='completed'.
//! Race check: a removed row → no persist. The emit follows in T15.

use chrono::Utc;
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{insert_schedule, load_schedule, mark_removed, setup_timer_schema};
use meclaw_cells::timer::io::TimerEvent;
use meclaw_cells::timer::schedule::{ScheduleKind, ScheduleRow};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{CellEmission, OriginSink, Path, Uuid};
use serde_json::Map;
use tokio::sync::mpsc;

fn cron_row(id: Uuid, cron: &str) -> ScheduleRow {
    ScheduleRow {
        schedule_id: id,
        schedule_name: "x".into(),
        kind: ScheduleKind::Cron(cron.into()),
        emit_to: Path::new("/dst"),
        emit_body: serde_json::json!({}),
        emit_headers: Map::new(),
        status: "active".into(),
        iteration_n: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_event_for_repeating_bumps_iteration_n_in_db_before_emit() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(&conn, &cron_row(id, "*/1 * * * * *")).unwrap();
    let mut db = DbConn::wrap(conn, None);

    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (origin_tx, _origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/t"), 32);

    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at: Utc::now(),
        },
        &sink,
        &mut db,
    )
    .await;

    let loaded = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.iteration_n, 1,
        "a repeating fire must bump iteration_n"
    );
    assert_eq!(loaded.status, "active");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_event_for_once_marks_completed_and_does_not_bump_iteration() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    let mut row = cron_row(id, "*/1 * * * * *");
    row.kind = ScheduleKind::At(Utc::now() + chrono::Duration::seconds(60));
    insert_schedule(&conn, &row).unwrap();
    let mut db = DbConn::wrap(conn, None);

    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (origin_tx, _origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/t"), 32);

    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at: Utc::now(),
        },
        &sink,
        &mut db,
    )
    .await;

    let loaded = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, "completed");
    assert_eq!(loaded.iteration_n, 0, "once must not bump iteration_n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_event_race_check_skips_persist_when_row_removed_between_sleep_and_fire() {
    // Sleep window between the I/O fire push and the handler's handle_event: a
    // remove op happened in this window → status='removed'. The fire must then NOT
    // bump iteration_n or set status to 'completed'.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(&conn, &cron_row(id, "*/1 * * * * *")).unwrap();
    mark_removed(&conn, id).unwrap();
    let mut db = DbConn::wrap(conn, None);

    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (origin_tx, _origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/t"), 32);

    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at: Utc::now(),
        },
        &sink,
        &mut db,
    )
    .await;

    let loaded = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.status, "removed",
        "the race check must leave the removed row alone"
    );
    assert_eq!(loaded.iteration_n, 0);
}
