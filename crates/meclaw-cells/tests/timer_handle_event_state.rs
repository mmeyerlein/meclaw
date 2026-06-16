//! Phase-10-B T11: `handle_event` Persist-Pfad (State-vor-Emit / Phase-5-
//! Kanon). Fire eines repeating Schedules → `iteration_n` geht in `cell.db`
//! von 0 → 1, status bleibt `active`. Fire eines once → status='completed'.
//! Race-Check: removed-Row → kein Persist. Emit folgt in T15.

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
        "repeating fire muss iteration_n bumpen"
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
    assert_eq!(loaded.iteration_n, 0, "once darf iteration_n nicht bumpen");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_event_race_check_skips_persist_when_row_removed_between_sleep_and_fire() {
    // Sleep-Fenster zwischen I/O-Fire-Push und Handler-handle_event: in
    // diesem Fenster passierte ein remove-Op → status='removed'. Fire darf
    // dann NICHT iteration_n bumpen oder status auf 'completed' setzen.
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
        "race-check muss removed-Row in Ruhe lassen"
    );
    assert_eq!(loaded.iteration_n, 0);
}
