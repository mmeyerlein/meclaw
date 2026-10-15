//! GH #922 — a one-shot marked `catch_up` that a stop made the timer miss
//! fires once after the next boot, marked `late`; an unmarked one does not.
//!
//! The acceptance sentence of the issue, measured at the receiver (the sink
//! a colony delivers to) and at the seam (the row in the timer's `cell.db`):
//!
//! 1. Colony 1 takes two `at` rows due a few seconds ahead — one with
//!    `catch_up: true` (armed through `add … rearm: true` on a fresh id, which
//!    is an insert: the stale-strike guard of GH #904 never sees
//!    `at != scheduled_at` here — a catch-up is planned as `At(at)` and struck
//!    for exactly that `at`, so the guard cannot drop it), one without — and
//!    refuses `catch_up` on a `cron` row with `invalid_params` /
//!    `catch_up_cron_unsupported`. It is stopped before either row is due.
//! 2. With the colony down both moments pass; both rows are still `active`
//!    (nothing fired while it was down).
//! 3. Colony 2 boots on the same `cell.db`: exactly one strike arrives — the
//!    marked row, with `late: true` and its own `scheduled_at` — and the row
//!    is `completed`; the unmarked row stays silent (and `active`, as before).
//! 4. Colony 3 boots once more: nothing arrives.
//!
//! "Exactly once" and "nothing" are judged by a marker over the strike's own
//! path (TV-A1), never by a window: a `trigger` on a far-off marker row enters
//! the timer's I/O task and leaves through the same events channel, the same
//! `handle_event` and the same emit as every strike; whatever the timer pushed
//! into that channel before it took the trigger reaches `/sink` first, and
//! after the marker the receiver is empty (`try_recv`).
//!
//! The marker covers that FIFO path and nothing else (FIFO holds 1:1, not
//! transitively). Two ways a second strike could come lie off it:
//!
//! * A strike still waiting in the I/O task's sleep arm. The `select!` there
//!   is `biased` towards the reconfig arm the trigger enters through
//!   (`timer/io.rs`), so a trigger already queued when the task first turns
//!   overtakes the boot's catch-up. Colony 3 therefore waits for an anchor
//!   first: a fresh missed `catch_up` row whose moment lies after the marked
//!   row's. The sleep arm pushes moments earliest first, every row of one
//!   moment in one loop with no `select!` in between, so any strike due at or
//!   before the anchor is in the channel before the anchor is -- and the
//!   trigger is sent only once the anchor reached `/sink`. (Colony 2 and
//!   `a_strike_before_its_edge_is_not_lost` need no extra anchor: their first
//!   strike is one.)
//! * A replan (`TimerReplan`, struck again after `REPLAN_DELAY`, a second
//!   after the marker). A strike that emitted is never replanned (the replan
//!   branch is the not-emitted one, `marked.is_none() && !forced`); the case
//!   left is a mark the verdict reports as not landed although it did. No
//!   window can wait for that strike; two guards carry it. The handler's
//!   status check returns on a row that is no longer `active`, so a strike
//!   loaded after the mark landed stops there. A strike that loaded the row
//!   before the mark landed reaches the gate `one_shot_strike_may_emit`: its
//!   mark finds no `active` row (`marked == Some(0)`) and it never emits.
//!   Pinned by `a_catch_up_one_shot_emits_only_after_its_completion_mark_landed`
//!   (`timer/cell.rs`).
//!
//! GH #956 (quarantined until then, red once in a strand gate under suite
//! load) adds two locks, one per cause the flake could have had:
//!
//! * `a_strike_before_its_edge_is_not_lost` -- the catch-up strike sleeps zero
//!   (`io.rs`, a past `At`), so it leaves the moment the timer is spawned. The
//!   helper used to spawn the timer and add the edge `/timer → /sink` after it;
//!   a strike routed in between dies as `no_route` with its row already
//!   `completed`, and the receiver waits its 30 s out. The edge now comes first.
//! * `a_failed_persist_plans_the_one_shot_again` -- a strike whose state step
//!   runs out of `query_timeout_ms` left its row `active` and planned nowhere
//!   (the I/O task drops a one-shot when it pushes the strike). The handler
//!   now plans it again (`TimerReplan`), measured for both steps.

use chrono::Timelike;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::timer::db::{insert_schedule, setup_timer_schema};
use meclaw_cells::timer::schedule::{ScheduleKind, ScheduleRow};
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::{ColonyHandle, deadline, topologies::phase_3a::CaptureCell, wait};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// One colony on the given timer directory: `/sink` first (anti-cascade),
/// then the edge, then the timer through its factory. Every boot uses the
/// same `cell_dir`, so the second and third boot resume the first one's
/// `cell.db` — the restart the issue is about.
///
/// GH #956: the edge comes BEFORE the timer. A missed `catch_up` one-shot is
/// struck the moment the timer's I/O task starts (a past `At` sleeps zero),
/// and a strike that reaches the colony before `/timer → /sink` exists is a
/// `no_route` dead letter -- with its row already `completed`, so it never
/// comes again. Spawn-then-edge only held while the test won that race.
async fn boot(cell_dir: &std::path::Path) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;
    h.add_edge(Uuid::now_v7(), Path::new("/timer"), Path::new("/sink"))
        .await;
    let spawned = Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/timer"),
            json!({}),
            h.runtime().outputs_tx,
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("spawn timer");
    h.register_spawned(Path::new("/timer"), spawned).await;
    (h, recv_rx)
}

/// A timer op as a dispatcher delivers it: one `tool_call` turn, so every
/// op is answered (ack or error) and the test knows it was stored.
fn op_call(args: meclaw_core::JsonValue, call_id: &str) -> Message {
    MessageBuilder::new(Path::new("/timer"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": call_id
        }]})))
        .build()
}

async fn receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    deadline::hang("the next message at /sink", rx.recv())
        .await
        .expect("sink channel closed")
}

/// A one-shot a day ahead, written straight into `cell.db` before a boot: the
/// marker of [`nothing_before_the_marker`]. It never strikes by itself within
/// a test; only its `trigger` fires it.
fn insert_marker(cell_dir: &std::path::Path, id: Uuid) {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: format!("probe-marker-{id}"),
            kind: ScheduleKind::At(chrono::Utc::now() + chrono::Duration::days(1)),
            emit_to: Path::new("/sink"),
            emit_body: json!({"messages": []}),
            emit_headers: serde_json::Map::from_iter([(
                "msg_type".to_string(),
                json!("probe_marker"),
            )]),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        },
    )
    .unwrap();
}

/// TV-A1: nothing but the marker reaches `/sink` from here on. `trigger` on the
/// marker row enters the I/O task as `FireNow` and leaves it as the same
/// `TimerEvent::Fire` every sleep strike is, through the same events channel,
/// `handle_event` and emit to `/sink` -- so every strike the timer pushed into
/// that channel before it took the trigger arrives first, and fails the test
/// here. The op's own ack is the one other message allowed; after both,
/// `try_recv` is empty. A strike still waiting in the sleep arm is not covered
/// (the trigger may overtake it, see the module doc): the caller sends the
/// marker only after a strike that orders it.
async fn nothing_before_the_marker(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<Message>,
    marker: Uuid,
    why: &str,
) {
    h.send(op_call(
        json!({"op": "trigger", "schedule_id": marker.to_string()}),
        "call-marker",
    ))
    .await;
    let (mut acked, mut struck) = (false, false);
    while !(acked && struck) {
        let m = receipt(rx).await;
        let hop = &m.headers.hop;
        match hop["msg_type"].as_str() {
            Some("timer_op_ack") if !acked => acked = true,
            Some("probe_marker") if !struck && hop["schedule_id"] == marker.to_string() => {
                struck = true
            }
            _ => panic!("{why}: {hop:?} reached /sink before the marker"),
        }
    }
    let after = rx.try_recv();
    assert!(
        matches!(after, Err(mpsc::error::TryRecvError::Empty)),
        "{why}: the receiver is not empty after the marker: {:?}",
        after.map(|m| m.headers.hop)
    );
}

fn status_of(cell_dir: &std::path::Path, id: Uuid) -> String {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    conn.query_row(
        "SELECT status FROM schedules WHERE schedule_id = ?1",
        [id.to_string()],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh922_a_missed_strike_fires_late_once() {
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();

    // --- 1. colony 1: two orders and one refusal, then stopped in time.
    let (h, mut rx) = boot(&cell_dir).await;

    // Both rows are due at the same moment. t0 is now, right before the first
    // order, and the lead is a hang marker (TV-Z3/TV-Z11), not a bet on the
    // setup's speed: three acks and a stop take a fraction of it. A setup that
    // outran it fails on the row status below, never on a clock.
    let lead = chrono::Duration::from_std(deadline::scaled(deadline::FLOOR))
        .expect("the lead fits a chrono duration");
    let at = chrono::Utc::now() + lead;
    let at_s = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let marked = Uuid::now_v7();
    let unmarked = Uuid::now_v7();
    h.send(op_call(
        json!({"op": "add", "schedule_id": marked.to_string(),
               "schedule_name": "probe-marked", "at": at_s, "catch_up": true,
               "rearm": true, "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "probe_marked"}}),
        "call-marked",
    ))
    .await;
    let ack = receipt(&mut rx).await;
    assert_eq!(
        ack.headers.hop["msg_type"], "timer_op_ack",
        "{:?}",
        ack.headers.hop
    );

    h.send(op_call(
        json!({"op": "add", "schedule_id": unmarked.to_string(),
               "schedule_name": "probe-unmarked", "at": at_s,
               "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "probe_unmarked"}}),
        "call-unmarked",
    ))
    .await;
    let ack = receipt(&mut rx).await;
    assert_eq!(
        ack.headers.hop["msg_type"], "timer_op_ack",
        "{:?}",
        ack.headers.hop
    );

    h.send(op_call(
        json!({"op": "add", "schedule_id": Uuid::now_v7().to_string(),
               "schedule_name": "probe-cron", "cron": "*/1 * * * * *", "catch_up": true,
               "emit_to": "/sink", "emit_body": {"messages": []}}),
        "call-cron",
    ))
    .await;
    let refusal = receipt(&mut rx).await;
    assert_eq!(refusal.headers.hop["msg_type"], "timer_op_error");
    assert_eq!(
        refusal.headers.hop["error_code"], "invalid_params",
        "catch_up on cron is refused: {:?}",
        refusal.headers.hop
    );
    let Body::Inline(rb) = &refusal.body else {
        panic!("inline refusal expected")
    };
    assert!(
        rb["meta"]["detail"]
            .as_str()
            .is_some_and(|d| d.starts_with("catch_up_cron_unsupported")),
        "the refusal names its reason: {rb}"
    );

    h.shutdown().await;

    // --- 2. down: both moments pass, nothing fires. The wait is for the wall
    // clock to pass the moment, a second past it (the product reads
    // `chrono::Utc::now`, so the wall clock is what has to move).
    wait::until("the wall clock passed the strike", || {
        let passed = chrono::Utc::now() > at + chrono::Duration::seconds(1);
        async move { passed }
    })
    .await;
    assert_eq!(
        status_of(&cell_dir, marked),
        "active",
        "nothing fired while down (a setup that outran the {lead} lead fails here)"
    );
    assert_eq!(
        status_of(&cell_dir, unmarked),
        "active",
        "nothing fired while down (a setup that outran the {lead} lead fails here)"
    );

    // --- 3. colony 2: exactly the marked row, once, late.
    let (marker_2, marker_3) = (Uuid::now_v7(), Uuid::now_v7());
    insert_marker(&cell_dir, marker_2);
    insert_marker(&cell_dir, marker_3);
    let (h, mut rx) = boot(&cell_dir).await;
    let fire = receipt(&mut rx).await;
    assert_eq!(
        fire.headers.hop["msg_type"], "probe_marked",
        "{:?}",
        fire.headers.hop
    );
    assert_eq!(fire.headers.hop["schedule_id"], marked.to_string());
    assert_eq!(fire.headers.hop["late"], true, "{:?}", fire.headers.hop);
    assert_eq!(
        fire.headers.hop["scheduled_at"],
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "the strike names the moment it was due, not the boot"
    );
    nothing_before_the_marker(
        &h,
        &mut rx,
        marker_2,
        "the unmarked row and a second strike stay silent",
    )
    .await;
    h.shutdown().await;
    assert_eq!(
        status_of(&cell_dir, marked),
        "completed",
        "fired = completed"
    );
    assert_eq!(
        status_of(&cell_dir, unmarked),
        "active",
        "an unmarked past row behaves as before: kept, never planned"
    );

    // --- 4. colony 3: nothing is caught up twice. Nothing here is struck
    // first by itself, and a trigger sent right after the boot could overtake
    // a second catch-up of the marked row in the I/O task's biased `select!`
    // (recheck B-M1). The anchor orders it: its moment lies after the marked
    // row's, so a second strike of that row is pushed before the anchor and
    // reaches `/sink` before it (module doc).
    let anchor = Uuid::now_v7();
    // Strictly after `at` by construction, with no second wall-clock read
    // (recheck fix2 H2): `at` carries milliseconds, so the whole second
    // after it is > `at`; and it is <= `at + 1 s`, which step 2 saw the
    // wall clock pass, so it is still a missed (late) moment at the boot.
    let anchor_at = (at + chrono::Duration::seconds(1))
        .with_nanosecond(0)
        .unwrap();
    {
        let mut row = missed_catch_up_row(anchor, anchor_at);
        row.emit_headers
            .insert("msg_type".to_string(), json!("probe_anchor"));
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        insert_schedule(&conn, &row).unwrap();
    }
    let (h, mut rx) = boot(&cell_dir).await;
    let first = receipt(&mut rx).await;
    assert_eq!(
        first.headers.hop["schedule_id"],
        anchor.to_string(),
        "a second restart catches nothing up: {:?} reached /sink before the anchor",
        first.headers.hop
    );
    assert_eq!(first.headers.hop["late"], true, "{:?}", first.headers.hop);
    nothing_before_the_marker(&h, &mut rx, marker_3, "a second restart catches nothing up").await;
    h.shutdown().await;
    assert_eq!(status_of(&cell_dir, anchor), "completed");
}

/// A `catch_up` one-shot whose moment lies well behind the boot, written
/// straight into a fresh `cell.db` -- the state colony 2 above boots on,
/// without the three-second lead.
fn missed_catch_up_row(id: Uuid, at: chrono::DateTime<chrono::Utc>) -> ScheduleRow {
    ScheduleRow {
        schedule_id: id,
        schedule_name: "probe-missed".into(),
        kind: ScheduleKind::At(at),
        emit_to: Path::new("/sink"),
        emit_body: json!({"messages": []}),
        emit_headers: serde_json::Map::from_iter([("msg_type".to_string(), json!("probe_marked"))]),
        status: "active".into(),
        iteration_n: 0,
        catch_up: true,
    }
}

/// GH #956, candidate (a): the catch-up strike leaves the instant the timer is
/// spawned, and the boot that spawns it has the edge in place already, so the
/// strike reaches the receiver -- once, late -- however early it leaves; a
/// marker after it checks the "once" (TV-A1).
/// Red with the old order (spawn, then edge) and the strike let through
/// first: the row is `completed` and the receiver hears nothing (bericht X).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strike_before_its_edge_is_not_lost() {
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let id = Uuid::now_v7();
    let at = (chrono::Utc::now() - chrono::Duration::seconds(60))
        .with_nanosecond(0)
        .unwrap();
    {
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        setup_timer_schema(&conn).unwrap();
        insert_schedule(&conn, &missed_catch_up_row(id, at)).unwrap();
    }
    let marker = Uuid::now_v7();
    insert_marker(&cell_dir, marker);

    let (h, mut rx) = boot(&cell_dir).await;
    let fire = receipt(&mut rx).await;
    assert_eq!(fire.headers.hop["schedule_id"], id.to_string());
    assert_eq!(fire.headers.hop["late"], true, "{:?}", fire.headers.hop);
    assert_eq!(
        fire.headers.hop["scheduled_at"],
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    // GH #956 review M-4 (welle-nachlese Z): "not lost" is half of the
    // promise; the strike that leaves before its edge must not come twice
    // either. The marker judges a second strike pushed before the trigger;
    // a replan (only of a strike whose landed mark the verdict missed) comes
    // later and is stopped by the handler's status check or the gate
    // `one_shot_strike_may_emit` (`marked == Some(0)` never emits, pinned in
    // `timer/cell.rs`), see the module doc.
    nothing_before_the_marker(
        &h,
        &mut rx,
        marker,
        "the catch-up strike comes exactly once",
    )
    .await;
    let dead = h.drain_dead_letters().await;
    assert!(
        dead.is_empty(),
        "the strike was routed, not dead-lettered: {dead:?}"
    );
    h.shutdown().await;
    assert_eq!(status_of(&cell_dir, id), "completed");
}

/// GH #956, candidate (b): a strike whose state step runs out of
/// `query_timeout_ms` is planned again and fires once the database answers.
///
/// Driven by hand instead of through a colony, so each step is an event and
/// not a race: the test plays the substrate (the I/O task's events into
/// `handle_event`), and a second connection holds the lock that makes the
/// step time out -- `EXCLUSIVE` stops the `load_schedule` read, `IMMEDIATE`
/// lets the read through and stops the `mark_completed` write. The row is
/// still `active` and nothing was emitted after the failed step; after the
/// lock is gone the I/O task strikes the same moment again, and that strike
/// emits once, late. Red before the fix: the second strike never comes.
///
/// GH #989: a retry under load may time out as well and is struck once
/// more; the test takes strikes until one emits, and each strike that does
/// not emit must leave the row `active` -- before the fix a step that had
/// landed but outlasted the timeout in the blocking pool's queue was
/// reported `Interrupted`, the row was `completed` without its strike, and
/// public CI waited for an emission that never came.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_persist_plans_the_one_shot_again() {
    for (lock, step) in [("BEGIN EXCLUSIVE", "load"), ("BEGIN IMMEDIATE", "mark")] {
        failed_step_is_planned_again(lock, step).await;
    }
}

async fn next_strike(
    rx: &mut mpsc::Receiver<meclaw_cells::timer::io::TimerEvent>,
    step: &str,
) -> meclaw_cells::timer::io::TimerEvent {
    // The backstop is a watchdog, not the condition: a replanned strike
    // waits up to `REPLAN_MAX_DELAY` once its retries back off (GH #989).
    let backstop = meclaw_cells::timer::io::REPLAN_MAX_DELAY + deadline::scaled(deadline::HANG);
    tokio::time::timeout(backstop, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("HANG: [{step}] no strike within {backstop:?}"))
        .expect("events channel closed")
}

async fn failed_step_is_planned_again(lock: &str, step: &str) {
    use meclaw_cells::timer::cell::TimerCell;
    use meclaw_cells::timer::io::{TimerEvent, TimerReconfig, run_io};
    use meclaw_colony::{DbConn, LongRunningCell};
    use meclaw_core::{CellEmission, OriginSink};

    let td = TempDir::new().unwrap();
    let db_path = td.path().join("cell.db");
    let id = Uuid::now_v7();
    let at = (chrono::Utc::now() - chrono::Duration::seconds(60))
        .with_nanosecond(0)
        .unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    setup_timer_schema(&conn).unwrap();
    insert_schedule(&conn, &missed_catch_up_row(id, at)).unwrap();
    // The cell's query timeout is far below the time a locked step waits
    // for its lock, so the step ends as `Interrupted`, never as a plain
    // `SQLITE_BUSY` (which `load_schedule` would not survive).
    conn.busy_timeout(Duration::from_secs(2)).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_millis(50)));

    let plan = vec![meclaw_cells::timer::schedule::ActiveSchedule {
        schedule_id: id,
        kind: ScheduleKind::At(at),
    }];
    let mut cell = TimerCell::new(Path::new("/timer"), plan, 50);
    let io = cell.split_io();
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let io_join = tokio::spawn(run_io(io, events_tx, rc_rx));
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/timer"), 32);

    let locker = rusqlite::Connection::open(&db_path).unwrap();
    locker.execute_batch(lock).unwrap();

    // 1. The catch-up strike meets the lock: no emission, the row stays.
    let first = next_strike(&mut events_rx, step).await;
    let TimerEvent::Fire {
        schedule_id,
        scheduled_at,
        forced,
    } = first.clone();
    assert_eq!((schedule_id, scheduled_at, forced), (id, at, false));
    cell.handle_event(first, &sink, &mut db).await;
    assert!(
        origin_rx.try_recv().is_err(),
        "[{step}] a strike whose state step failed does not emit"
    );
    let status: String = locker
        .query_row(
            "SELECT status FROM schedules WHERE schedule_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "active",
        "[{step}] the failed step left the row as it was"
    );
    locker.execute_batch("COMMIT").unwrap();

    // 2. The same moment is struck again until a state step takes; that
    //    strike emits once. Event-bound: `handle_event` emits before it
    //    returns, so each strike is judged on its own -- no fixed window. A
    //    loaded host may make a retry's step run past the 50 ms timeout as
    //    well; the I/O task then strikes the moment once more (GH #989), and
    //    a step reported as failed must never have completed the row.
    let mut out = None;
    for attempt in 1..=meclaw_cells::timer::io::REPLAN_MAX_ATTEMPTS {
        let again = next_strike(&mut events_rx, step).await;
        let TimerEvent::Fire {
            schedule_id,
            scheduled_at,
            forced,
        } = again.clone();
        assert_eq!(
            (schedule_id, scheduled_at, forced),
            (id, at, false),
            "[{step}] strike {attempt} again is for the moment that failed"
        );
        cell.handle_event(again, &sink, &mut db).await;
        if let Ok(emitted) = origin_rx.try_recv() {
            out = Some(emitted);
            break;
        }
        let status: String = locker
            .query_row(
                "SELECT status FROM schedules WHERE schedule_id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "active",
            "[{step}] strike {attempt} again did not emit, so it left the row active \
             (a completed row without its strike is a lost one-shot)"
        );
    }
    let out = out.unwrap_or_else(|| panic!("[{step}] no strike again emitted"));
    assert_eq!(out.content["header"]["schedule_id"], id.to_string());
    assert_eq!(
        out.content["header"]["late"], true,
        "[{step}] {:?}",
        out.content
    );
    let status: String = locker
        .query_row(
            "SELECT status FROM schedules WHERE schedule_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "completed", "[{step}]");

    drop(rc_tx);
    let _ = tokio::time::timeout(deadline::scaled(deadline::HANG), io_join).await;
}

/// GH #1181: a `mark_completed` that LANDS but returns after `query_timeout_ms`
/// emits its strike -- a completed row without its strike is a lost one-shot.
///
/// Under full-suite load the mark's `UPDATE` committed while the query clock
/// ran out (a commit waiting on I/O, or the blocking thread descheduled after
/// the statement): `call_with_timeout` answered `Interrupted` because its
/// timer fired while the closure still ran, the handler took the landed mark
/// for a failed one, withheld the strike and planned it again, and the replan
/// found the row `completed` and skipped it. The red line of the issue was
/// exactly that state: no emission, row `completed`.
///
/// Deterministic here: an `AFTER UPDATE` trigger calls a function that sleeps
/// ten times the 50 ms query timeout. SQLite checks for an interrupt between
/// opcodes, not across the trigger's tail and the commit that follows it, so
/// the `UPDATE` commits although the interrupt was sent during the sleep --
/// the same shape as a commit that outlasts the clock on a loaded host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mark_that_lands_past_the_query_timeout_emits_its_strike() {
    use meclaw_cells::timer::cell::TimerCell;
    use meclaw_cells::timer::io::{TimerEvent, TimerReconfig, run_io};
    use meclaw_colony::{DbConn, LongRunningCell};
    use meclaw_core::{CellEmission, OriginSink};

    let query_timeout = Duration::from_millis(50);
    let td = TempDir::new().unwrap();
    let db_path = td.path().join("cell.db");
    let id = Uuid::now_v7();
    let at = (chrono::Utc::now() - chrono::Duration::seconds(60))
        .with_nanosecond(0)
        .unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    setup_timer_schema(&conn).unwrap();
    insert_schedule(&conn, &missed_catch_up_row(id, at)).unwrap();
    let tail = query_timeout * 10;
    conn.create_scalar_function(
        "slow_tail",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |_| {
            std::thread::sleep(tail);
            Ok(1i64)
        },
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER slow_mark AFTER UPDATE OF status ON schedules \
         BEGIN SELECT slow_tail(); END;",
    )
    .unwrap();
    let mut db = DbConn::wrap(conn, Some(query_timeout));

    let plan = vec![meclaw_cells::timer::schedule::ActiveSchedule {
        schedule_id: id,
        kind: ScheduleKind::At(at),
    }];
    let mut cell = TimerCell::new(Path::new("/timer"), plan, 50);
    let io = cell.split_io();
    let (events_tx, mut events_rx) = mpsc::channel::<TimerEvent>(64);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let io_join = tokio::spawn(run_io(io, events_tx, rc_rx));
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/timer"), 32);

    let strike = next_strike(&mut events_rx, "landed mark").await;
    cell.handle_event(strike, &sink, &mut db).await;
    let reader = rusqlite::Connection::open(&db_path).unwrap();
    let status: String = reader
        .query_row(
            "SELECT status FROM schedules WHERE schedule_id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "completed",
        "the mark landed (the trigger only slows it)"
    );
    // Event-bound: `handle_event` emits before it returns.
    let out = origin_rx.try_recv().unwrap_or_else(|_| {
        panic!("the mark landed, so its strike emits (a completed row without its strike is a lost one-shot)")
    });
    assert_eq!(out.content["header"]["schedule_id"], id.to_string());
    assert_eq!(out.content["header"]["late"], true, "{:?}", out.content);

    drop(rc_tx);
    let _ = tokio::time::timeout(deadline::scaled(deadline::HANG), io_join).await;
}
