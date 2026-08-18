//! GH #231: a one-shot `at` that has already passed when the op is processed.
//!
//! The op travels brain → dispatcher → timer cell and is INSERTed, and only
//! then does `send_setactive_snapshot` re-read the active set with a fresh
//! `Utc::now()`. A schedule whose lead time ran out **in flight** — 16–30 ms
//! per hop is normal — was dropped from that set, and `compute_next_occurrence`
//! refused it too. No error, no dead letter: the ack said yes and nothing ever
//! fired. A user asking an agent to remind them "in one second" hit exactly
//! this.
//!
//! # The ruling: refuse the op, do not fire it
//!
//! `docs/cell-types.md` § `timer` states both halves of the answer. "Past
//! firings are discarded … a one-off schedule whose time already lies in the
//! past (at creation or restart time) is not scheduled and only logged" rules
//! out firing it late. And the validation clause on `add` gives the reason the
//! silence is wrong: an invalid cron is rejected so that no silently stored,
//! never-firing schedule comes into being —
//! which is precisely what a past `at` produced. So the past one-shot is a
//! refused op, not a late firing: `at_in_past`, no row, no snapshot.
//!
//! Constructed explicitly rather than raced against the wall clock: the `at`
//! values below are decades away on either side, so the branch under test is
//! decided by the fixture and not by how long the test took to get here.

use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{insert_schedule, load_schedule, setup_timer_schema};
use meclaw_cells::timer::io::TimerReconfig;
use meclaw_cells::timer::schedule::{ScheduleKind, ScheduleRow};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use serde_json::{Map, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Long past — the state an `at` reaches while the op is still in flight, with
/// the margin blown up so no scheduling delay can decide the test.
const LONG_PAST: &str = "2000-01-01T00:00:00Z";
/// Comfortably ahead, for the control case.
const FAR_FUTURE: &str = "2099-01-01T00:00:00Z";

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

/// The core of the issue: the op is answered, and it leaves nothing behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_of_a_past_one_shot_answers_at_in_past_and_stores_nothing() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "remind-me-in-one-second",
        "at": LONG_PAST,
        "emit_to": "/dst",
        "emit_body": {},
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("a past one-shot must be answered, not swallowed")
        .unwrap();
    assert_eq!(em.target, Path::new("/reply"));
    assert_eq!(
        em.content["header"]["error_code"], "at_in_past",
        "got: {}",
        em.content
    );
    assert_eq!(
        em.content["header"]["msg_type"], "timer_op_error",
        "got: {}",
        em.content
    );

    // Nothing stored — the same rule the invalid-cron branch follows: a refused
    // op does not leave a row that could never fire.
    assert!(
        db.call(move |c| load_schedule(c, id))
            .await
            .unwrap()
            .is_none(),
        "a refused one-shot must not create a row"
    );
    // And no snapshot: there is nothing for the I/O task to plan.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rc_rx.recv())
            .await
            .is_err(),
        "SetActive must NOT be sent for a refused op"
    );
}

/// The control case: the very same lane with a time still ahead commits and
/// reaches the I/O task's plan. Without this, "refuses everything" would pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_of_a_future_one_shot_still_commits_and_is_planned() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, mut rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let msg = build_op_msg(json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "later",
        "at": FAR_FUTURE,
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
    let TimerReconfig::SetActive(snap) = rc else {
        panic!("an add op must answer with SetActive, got {rc:?}");
    };
    assert!(
        snap.iter().any(|s| s.schedule_id == id),
        "the future one-shot must reach the I/O plan"
    );
    assert!(
        out_rx.try_recv().is_err(),
        "a committed raw-body add stays unacked and emits no error"
    );
}

/// GH #81's lane carries the refusal too — a tool loop that asked for the
/// reminder closes on the error instead of waiting for a result that the old
/// behaviour was never going to send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refusal_reaches_a_tool_loop_as_a_tool_result() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);

    let id = Uuid::now_v7();
    let args = json!({
        "op": "add",
        "schedule_id": id.to_string(),
        "schedule_name": "remind-me",
        "at": LONG_PAST,
        "emit_to": "/dst",
        "emit_body": {},
    })
    .to_string();
    let msg = build_op_msg(json!({
        "messages": [{
            "id": "call-231", "origin": "assistant", "type": "tool_call", "text": args
        }]
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("the tool lane must be answered")
        .unwrap();
    assert_eq!(em.content["header"]["error_code"], "at_in_past");
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["messages"][0]["type"], "tool_result");
    assert_eq!(em.content["messages"][0]["id"], "call-231");
}

/// The same trap through `modify`: moving an existing one-shot to a time that
/// has already passed used to leave an active row nothing would ever plan.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modify_that_moves_a_one_shot_into_the_past_is_refused() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();

    let id = Uuid::now_v7();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "one-shot".into(),
            kind: ScheduleKind::At(FAR_FUTURE.parse().unwrap()),
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
        "op": "modify",
        "schedule_id": id.to_string(),
        "at": LONG_PAST,
    }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    let em = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .expect("a past modify must be answered")
        .unwrap();
    assert_eq!(
        em.content["header"]["error_code"], "at_in_past",
        "got: {}",
        em.content
    );

    // The stored plan is untouched — a refused modify changes nothing.
    let row = db
        .call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present");
    let ScheduleKind::At(at) = row.kind else {
        panic!("the row must still be a one-shot");
    };
    assert_eq!(
        at,
        FAR_FUTURE.parse::<chrono::DateTime<chrono::Utc>>().unwrap(),
        "the refused modify must not have moved the schedule"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rc_rx.recv())
            .await
            .is_err(),
        "SetActive must NOT be sent for a refused modify"
    );
}

/// The stored `at` is what the caller asked for. Second-truncation on write
/// moved a schedule up to a second earlier than requested — enough to land an
/// accepted one-shot in the past on its own, without any transit at all.
#[test]
fn a_sub_second_at_survives_the_store_round_trip() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    let at: chrono::DateTime<chrono::Utc> = "2099-01-01T00:00:00.750Z".parse().unwrap();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "precise".into(),
            kind: ScheduleKind::At(at),
            emit_to: Path::new("/dst"),
            emit_body: json!({}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
        },
    )
    .unwrap();
    let ScheduleKind::At(stored) = load_schedule(&conn, id).unwrap().unwrap().kind else {
        panic!("a one-shot must come back as one");
    };
    assert_eq!(
        stored, at,
        "the stored moment must be the requested one, to the millisecond"
    );
}

/// The boundary the op guard draws and the boundary the plan filter draws are
/// the same one: everything accepted as "still ahead of `now`" is kept by a
/// filter reading that same `now`. Together with the handler passing its own
/// clock read into the snapshot, that is what closes the window — an accepted
/// one-shot cannot be dropped by the read that plans it.
#[test]
fn the_plan_filter_keeps_every_at_the_op_guard_would_accept() {
    use chrono::{Duration as ChDur, TimeZone, Utc};
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
    // The smallest lead the guard accepts (`at > now`), and the largest it
    // refuses (`at == now`).
    let just_ahead = now + ChDur::milliseconds(1);
    let exactly_now = now;

    let ahead_id = Uuid::now_v7();
    let now_id = Uuid::now_v7();
    for (id, at) in [(ahead_id, just_ahead), (now_id, exactly_now)] {
        insert_schedule(
            &conn,
            &ScheduleRow {
                schedule_id: id,
                schedule_name: "boundary".into(),
                kind: ScheduleKind::At(at),
                emit_to: Path::new("/dst"),
                emit_body: json!({}),
                emit_headers: Map::new(),
                status: "active".into(),
                iteration_n: 0,
            },
        )
        .unwrap();
    }

    let active = meclaw_cells::timer::db::load_active_filter_past(&conn, now).unwrap();
    let ids: Vec<Uuid> = active.iter().map(|a| a.schedule_id).collect();
    assert!(
        ids.contains(&ahead_id),
        "a one-shot one millisecond ahead is accepted by the guard, so the plan must keep it"
    );
    assert!(
        !ids.contains(&now_id),
        "`at == now` is refused by the guard and dropped by the plan — the same line"
    );
}

/// The last place an accepted one-shot could vanish: it reached the I/O working
/// set while still ahead, and its moment arrived before the loop got round to
/// planning it. A due one-shot in the set fires; it does not disappear.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_one_shot_that_came_due_inside_the_io_set_fires() {
    use meclaw_cells::timer::cell::TimerIo;
    use meclaw_cells::timer::io::{TimerEvent, run_io};
    use meclaw_cells::timer::schedule::ActiveSchedule;

    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let id = Uuid::now_v7();
    // Constructed, not raced: the set is handed a one-shot whose moment has
    // already arrived, which is the state a schedule reaches when its lead runs
    // out between the snapshot and this loop.
    let io = TimerIo {
        active: vec![ActiveSchedule {
            schedule_id: id,
            kind: ScheduleKind::At(LONG_PAST.parse().unwrap()),
        }],
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
    let join = tokio::spawn(run_io(io, events_tx, rc_rx));

    let ev = tokio::time::timeout(Duration::from_secs(5), events_rx.recv())
        .await
        .expect("a due one-shot must fire, not vanish")
        .expect("events channel closed");
    let TimerEvent::Fire { schedule_id, .. } = ev;
    assert_eq!(schedule_id, id);

    // And exactly once — the one-shot leaves the working set after firing.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), events_rx.recv())
            .await
            .is_err(),
        "a one-shot fires once"
    );
    drop(rc_tx);
    tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .expect("run_io hung after reconfig close")
        .unwrap();
}
