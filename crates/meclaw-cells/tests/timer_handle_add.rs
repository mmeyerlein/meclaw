//! Phase-10-B T13: `handle` add-Branch. INSERT in cell.db + on-dup-Error
//! (`schedule_id_exists`) + the SetActive snapshot after success. Invalid-cron
//! Body → `invalid_cron`-Error (Korrektur A: Prefix-`"cron:"`-Mapping).
//! GH #690: a dup is an `add` that names ANOTHER order under a standing id;
//! the same order again is one order (no error, no snapshot), and the same id
//! on a removed row revives it (a snapshot, like a fresh add).

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
        .expect("no SetActive within 1s")
        .unwrap();
    // GH #17 added `FireNow` to the frame, so the binding names the variant it
    // means. The assertion is unchanged: an `add` answers with a snapshot.
    let TimerReconfig::SetActive(snap) = rc else {
        panic!("an add op must answer with SetActive, got {rc:?}");
    };
    assert!(
        snap.iter().any(|s| s.schedule_id == id),
        "the SetActive snapshot does not contain the id: {:?}",
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

    // GH #690: another cron under the standing id -- a different order.
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "dup",
        "cron": "*/5 * * * * *",
        "emit_to": "/dst",
        "emit_body": {},
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("no error emission")
        .unwrap();
    assert_eq!(em.target, Path::new("/reply"));
    assert_eq!(
        em.content["header"]["error_code"], "schedule_id_exists",
        "got: {}",
        em.content
    );
    // NO SetActive on error.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rc_rx.recv())
            .await
            .is_err(),
        "SetActive must NOT be sent on error"
    );
    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present");
    assert!(
        matches!(row.kind, ScheduleKind::Cron(ref s) if s == "*/1 * * * * *"),
        "the standing order is untouched: {row:?}"
    );
}

/// GH #690: the same order again under a standing id is one order -- no
/// error, no snapshot, nothing written; and the same id on a removed row is
/// revived: no error, a snapshot that carries the id, the row active again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_add_of_the_same_order_is_one_order_and_revives_a_removed_one() {
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

    let again = json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "existing",
        "cron": "*/1 * * * * *",
        "emit_to": "/dst",
        "emit_body": {},
    });
    let msg = build_op_msg(again.clone());
    let sink = sink_from(&msg, out_tx.clone());
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), out_rx.recv())
            .await
            .is_err(),
        "the same order again is no error"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rc_rx.recv())
            .await
            .is_err(),
        "and nothing changed, so no SetActive"
    );

    db.call(move |c| meclaw_cells::timer::db::mark_removed(c, id))
        .await
        .unwrap();
    let msg = build_op_msg(again);
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), out_rx.recv())
            .await
            .is_err(),
        "an add on a removed row of the same id is no error"
    );
    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("a revival answers with SetActive")
        .unwrap();
    let TimerReconfig::SetActive(snap) = rc else {
        panic!("a revival answers with SetActive, got {rc:?}");
    };
    assert!(
        snap.iter().any(|s| s.schedule_id == id),
        "revived: {snap:?}"
    );
    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present");
    assert_eq!(row.status, "active");
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
        .expect("no error emission")
        .unwrap();
    assert_eq!(em.target, Path::new("/reply"));
    assert_eq!(
        em.content["header"]["error_code"], "invalid_cron",
        "got: {}",
        em.content
    );
    // No DB insert happens.
    assert!(
        db.call(move |c| load_schedule(c, id))
            .await
            .unwrap()
            .is_none(),
        "an invalid cron op must not create a row"
    );
}
