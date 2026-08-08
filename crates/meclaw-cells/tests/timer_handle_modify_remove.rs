//! Phase-10-B T14: `handle` modify/remove. Unknown → `schedule_not_found`;
//! a modify with a cron update on an at row (or vice versa) → `kind_mismatch`;
//! erfolgreiches modify/remove → SetActive-Snapshot.

use chrono::{TimeZone, Utc};
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{insert_schedule, load_schedule, setup_timer_schema};
use meclaw_cells::timer::io::TimerReconfig;
use meclaw_cells::timer::schedule::{ScheduleKind, ScheduleRow};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use serde_json::{Map, json};
use std::time::Duration;
use tokio::sync::mpsc;

fn build_op_msg(op_body: serde_json::Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/t"))
        .reply_to(Path::new("/reply"))
        .body(Body::Inline(op_body))
        .build()
}

fn sink_from(msg: &meclaw_core::Message, tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/t"),
        msg.id,
        msg.trace_id,
        32,
        meclaw_core::Headers::new(),
        None,
    )
}

fn cron_row(id: Uuid, cron: &str) -> ScheduleRow {
    ScheduleRow {
        schedule_id: id,
        schedule_name: "x".into(),
        kind: ScheduleKind::Cron(cron.into()),
        emit_to: Path::new("/dst"),
        emit_body: json!({}),
        emit_headers: Map::new(),
        status: "active".into(),
        iteration_n: 0,
    }
}

fn at_row(id: Uuid) -> ScheduleRow {
    ScheduleRow {
        schedule_id: id,
        schedule_name: "x".into(),
        kind: ScheduleKind::At(Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap()),
        emit_to: Path::new("/dst"),
        emit_body: json!({}),
        emit_headers: Map::new(),
        status: "active".into(),
        iteration_n: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_modify_unknown_id_emits_schedule_not_found() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let msg = build_op_msg(json!({
        "op": "modify",
        "schedule_id": id.to_string(),
        "cron": "*/5 * * * * *",
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("no error emission")
        .unwrap();
    assert_eq!(em.target, Path::new("/reply"));
    assert_eq!(em.content["header"]["error_code"], "schedule_not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_modify_cron_on_at_row_emits_kind_mismatch() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(&conn, &at_row(id)).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let msg = build_op_msg(json!({
        "op": "modify",
        "schedule_id": id.to_string(),
        "cron": "*/5 * * * * *",
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("no error emission")
        .unwrap();
    assert_eq!(em.content["header"]["error_code"], "kind_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_modify_at_on_cron_row_emits_kind_mismatch() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(&conn, &cron_row(id, "*/1 * * * * *")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let msg = build_op_msg(json!({
        "op": "modify",
        "schedule_id": id.to_string(),
        "at": "2099-06-15T10:00:00Z",
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("no error emission")
        .unwrap();
    assert_eq!(em.content["header"]["error_code"], "kind_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_modify_success_sends_setactive() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(&conn, &cron_row(id, "*/1 * * * * *")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut _out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let msg = build_op_msg(json!({
        "op": "modify",
        "schedule_id": id.to_string(),
        "schedule_name": "renamed",
        "cron": "*/5 * * * * *",
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("no SetActive")
        .unwrap();
    let TimerReconfig::SetActive(snap) = rc;
    assert!(snap.iter().any(|s| s.schedule_id == id));

    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.schedule_name, "renamed");
    assert!(matches!(row.kind, ScheduleKind::Cron(ref c) if c == "*/5 * * * * *"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_remove_unknown_id_emits_schedule_not_found() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let msg = build_op_msg(json!({
        "op": "remove",
        "schedule_id": id.to_string(),
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("no error emission")
        .unwrap();
    assert_eq!(em.content["header"]["error_code"], "schedule_not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_remove_success_marks_status_and_sends_setactive() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(&conn, &cron_row(id, "*/1 * * * * *")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut _out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let msg = build_op_msg(json!({
        "op": "remove",
        "schedule_id": id.to_string(),
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("no SetActive")
        .unwrap();
    let TimerReconfig::SetActive(snap) = rc;
    assert!(
        snap.iter().all(|s| s.schedule_id != id),
        "the removed id must NOT show up in SetActive"
    );

    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "removed");
}
