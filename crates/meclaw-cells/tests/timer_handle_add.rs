//! Phase-10-B T13: `handle` add-Branch. INSERT in cell.db + on-dup-Error
//! (`schedule_id_exists`) + SetActive-Snapshot nach Erfolg. Invalid-cron-
//! Body → `invalid_cron`-Error (Korrektur A: Prefix-`"cron:"`-Mapping).

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_add_fresh_inserts_row_and_sends_setactive() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut _out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "n",
        "cron": "*/1 * * * * *",
        "emit_to": "/dst",
        "emit_body": {},
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present");
    assert_eq!(row.status, "active");
    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("kein SetActive innerhalb 1s")
        .unwrap();
    let TimerReconfig::SetActive(snap) = rc;
    assert!(
        snap.iter().any(|s| s.schedule_id == id),
        "SetActive-Snapshot enthaelt id nicht: {:?}",
        snap.iter().map(|s| s.schedule_id).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_add_dup_emits_schedule_id_exists_error_to_reply_to() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();

    let id = Uuid::now_v7();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "existing".into(),
            kind: ScheduleKind::Cron("*/1 * * * * *".into()),
            emit_to: Path::new("/dst"),
            emit_body: json!({}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
        },
    )
    .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "dup",
        "cron": "*/1 * * * * *",
        "emit_to": "/dst",
        "emit_body": {},
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("keine Error-Emission")
        .unwrap();
    assert_eq!(em.target, Path::new("/reply"));
    assert_eq!(
        em.content["header"]["error_code"], "schedule_id_exists",
        "got: {}",
        em.content
    );
    // KEIN SetActive bei Fehler.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rc_rx.recv())
            .await
            .is_err(),
        "SetActive darf bei Fehler NICHT gesendet werden"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_add_invalid_cron_emits_invalid_cron_error() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "x",
        "cron": "not a cron",
        "emit_to": "/dst",
        "emit_body": {},
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("keine Error-Emission")
        .unwrap();
    assert_eq!(em.target, Path::new("/reply"));
    assert_eq!(
        em.content["header"]["error_code"], "invalid_cron",
        "got: {}",
        em.content
    );
    // Kein DB-Insert passiert.
    assert!(
        db.call(move |c| load_schedule(c, id))
            .await
            .unwrap()
            .is_none(),
        "ungueltige cron-Op darf keine Row erzeugen"
    );
}
