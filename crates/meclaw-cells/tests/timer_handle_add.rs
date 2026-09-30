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
            catch_up: false,
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
            catch_up: false,
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

/// GH #904 (PP-7): `add` with `rearm: true` over a fired (`completed`)
/// one-shot arms it again in place -- no error, a snapshot that plans it, the
/// new moment and emission, the same `rowid`, still one row. Without the
/// flag the same op is the collision it always was (GH #690).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_add_with_rearm_arms_a_fired_order_again_in_place() {
    use chrono::TimeZone;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "cache".into(),
            kind: ScheduleKind::At(chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap()),
            emit_to: Path::new("/dst"),
            emit_body: json!({"curator_call": "c1"}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        },
    )
    .unwrap();
    meclaw_cells::timer::db::mark_completed(&conn, id).unwrap();
    let rowid_of = |c: &rusqlite::Connection| -> i64 {
        c.query_row(
            "SELECT rowid FROM schedules WHERE schedule_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    };
    let rowid = rowid_of(&conn);
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let mut order = json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "cache",
        "at": "2099-01-01T00:05:00Z",
        "emit_to": "/dst",
        "emit_body": {"curator_call": "c2"},
    });
    // Without the flag: the fired order is another order (GH #690, unchanged).
    let msg = build_op_msg(order.clone());
    let sink = sink_from(&msg, out_tx.clone());
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("without rearm: the collision")
        .unwrap();
    assert_eq!(em.content["header"]["error_code"], "schedule_id_exists");

    order["rearm"] = json!(true);
    let msg = build_op_msg(order);
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), out_rx.recv())
            .await
            .is_err(),
        "a rearm is no error"
    );
    let rc = tokio::time::timeout(Duration::from_secs(1), rc_rx.recv())
        .await
        .expect("a rearm answers with SetActive")
        .unwrap();
    let TimerReconfig::SetActive(snap) = rc else {
        panic!("a rearm answers with SetActive, got {rc:?}");
    };
    assert!(
        snap.iter().any(|s| s.schedule_id == id),
        "planned: {snap:?}"
    );
    let (row, rowid_after, n) = db
        .call(move |c| {
            let row = load_schedule(c, id)?;
            let rid: i64 = c.query_row(
                "SELECT rowid FROM schedules WHERE schedule_id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )?;
            let n: i64 = c.query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))?;
            Ok::<_, rusqlite::Error>((row, rid, n))
        })
        .await
        .unwrap();
    let row = row.expect("row present");
    assert_eq!(row.status, "active");
    assert_eq!(row.emit_body, json!({"curator_call": "c2"}));
    assert_eq!(rowid_after, rowid, "in place");
    assert_eq!(n, 1, "one row");
}

/// GH #904 (PP-7, second half): an error answer to an op that named a
/// `schedule_id` names it back -- here an `add` whose `at` has passed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handle_add_error_names_the_schedule_id() {
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
        "schedule_name": "cache",
        "at": "2000-01-01T00:00:00Z",
        "emit_to": "/dst",
        "emit_body": {},
        "rearm": true,
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("an error emission")
        .unwrap();
    assert_eq!(em.content["header"]["error_code"], "at_in_past");
    assert_eq!(
        em.content["header"]["schedule_id"],
        json!(id.to_string()),
        "the refused order is named: {}",
        em.content
    );
    let n: i64 = db
        .call(|c| c.query_row("SELECT count(*) FROM schedules", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(n, 0, "refused before any write");
}

/// GH #904 (fix round 1, review I-1): a strike for the OLD moment that is
/// already queued when a `rearm` replaces the one-shot must not hit the new
/// order. The I/O loop pushes `Fire { scheduled_at: t }` into the event
/// channel (`io.rs`, sleep arm) and the handler processes ops and events in
/// arrival order; the curator re-arms its one clock id on every brain call, so
/// a call landing exactly as the cache goes cold queues `rearm` behind such a
/// strike's wake -- or in front of its handling. Before the fix the stale
/// strike loaded the replaced row (`status = active`), marked the NEW order
/// completed and emitted the NEW `curator_call`: an early rebuild, and the
/// order for the new moment never fired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_strike_does_not_fire_the_order_a_rearm_put_in_its_place() {
    use chrono::TimeZone;
    use meclaw_cells::timer::io::TimerEvent;
    use meclaw_core::OriginSink;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    let old_at = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
    let new_at = chrono::Utc.with_ymd_and_hms(2099, 1, 1, 0, 5, 0).unwrap();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "cache".into(),
            kind: ScheduleKind::At(old_at),
            emit_to: Path::new("/dst"),
            emit_body: json!({"curator_call": "c1"}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        },
    )
    .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    // The next call re-arms the one id for a later moment.
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "cache",
        "at": "2099-01-01T00:05:00Z",
        "emit_to": "/dst",
        "emit_body": {"curator_call": "c2"},
        "rearm": true,
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), out_rx.recv())
            .await
            .is_err(),
        "a rearm is no error"
    );

    // The strike the sleep arm pushed for the OLD moment arrives now.
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/t"), 32);
    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at: old_at,
            forced: false,
        },
        &origin,
        &mut db,
    )
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), origin_rx.recv())
            .await
            .is_err(),
        "the stale strike must not emit the new order"
    );
    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present");
    assert_eq!(row.status, "active", "the new order is still armed");
    assert!(
        matches!(row.kind, ScheduleKind::At(t) if t == new_at),
        "armed for the new moment: {:?}",
        row.kind
    );
    assert_eq!(row.emit_body, json!({"curator_call": "c2"}));

    // Its own strike fires it, once.
    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at: new_at,
            forced: false,
        },
        &origin,
        &mut db,
    )
    .await;
    let em = tokio::time::timeout(Duration::from_secs(1), origin_rx.recv())
        .await
        .expect("the new order fires at its own moment")
        .unwrap();
    assert_eq!(em.content["curator_call"], json!("c2"), "{}", em.content);
    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present");
    assert_eq!(row.status, "completed");
}

/// GH #904 review M-2: an op that named no `schedule_id` gets the error body
/// it always got -- no `header.schedule_id` key, nothing else added. The one
/// op error without an id is the parse error of an op that lacks it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_without_a_schedule_id_keeps_its_old_shape() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_name": "x",
        "at": "2000-01-01T00:00:00Z",
        "emit_to": "/dst",
        "emit_body": {},
    }));
    let sink = sink_from(&msg, out_tx);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("an error emission")
        .unwrap();
    let detail = em.content["meta"]["detail"].clone();
    assert_eq!(
        em.content,
        json!({
            "header":   { "error_code": "parse_error", "msg_type": "timer_op_error" },
            "messages": [],
            "meta":     { "detail": detail },
        }),
        "byte-identical to the pre-#904 error body"
    );
}
