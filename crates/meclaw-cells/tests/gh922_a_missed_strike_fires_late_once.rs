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
use meclaw_testing::{ColonyHandle, topologies::phase_3a::CaptureCell};
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
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no message at /sink within 30s")
        .expect("sink channel closed")
}

/// Asserts that nothing reaches `/sink` for `secs`.
async fn silence(rx: &mut mpsc::Receiver<Message>, secs: u64, why: &str) {
    let extra = tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await;
    assert!(
        !matches!(extra, Ok(Some(_))),
        "{why}, got {:?}",
        extra.map(|m| m.map(|m| m.headers.hop.clone()))
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

    // Both rows are due at the same moment, a lead ahead that the three ops
    // and the stop fit into comfortably.
    let lead = chrono::Duration::seconds(3);
    let at = chrono::Utc::now() + lead;
    let at_s = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let marked = Uuid::now_v7();
    let unmarked = Uuid::now_v7();

    // --- 1. colony 1: two orders and one refusal, then stopped in time.
    let (h, mut rx) = boot(&cell_dir).await;
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
    assert!(
        chrono::Utc::now() < at,
        "setup outran the {lead} lead; the stop has to come before the moment"
    );

    // --- 2. down: both moments pass, nothing fires.
    let wait = (at - chrono::Utc::now()).to_std().unwrap_or_default() + Duration::from_millis(1500);
    tokio::time::sleep(wait).await;
    assert_eq!(
        status_of(&cell_dir, marked),
        "active",
        "nothing fired while down"
    );
    assert_eq!(
        status_of(&cell_dir, unmarked),
        "active",
        "nothing fired while down"
    );

    // --- 3. colony 2: exactly the marked row, once, late.
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
    silence(
        &mut rx,
        2,
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

    // --- 4. colony 3: nothing is caught up twice.
    let (h, mut rx) = boot(&cell_dir).await;
    silence(&mut rx, 2, "a second restart catches nothing up").await;
    h.shutdown().await;
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
/// two-second silence after it checks the "once".
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
    // either (a replan of a strike that did land would).
    silence(&mut rx, 2, "the catch-up strike comes exactly once").await;
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
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("[{step}] no strike within 30s"))
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

    // 2. The same moment is struck again, and this time it takes.
    let again = next_strike(&mut events_rx, step).await;
    let TimerEvent::Fire {
        schedule_id,
        scheduled_at,
        forced,
    } = again.clone();
    assert_eq!(
        (schedule_id, scheduled_at, forced),
        (id, at, false),
        "[{step}] the strike again is for the moment that failed"
    );
    cell.handle_event(again, &sink, &mut db).await;
    let out = tokio::time::timeout(Duration::from_secs(30), origin_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("[{step}] no emission within 30s"))
        .expect("origin channel closed");
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
    let _ = tokio::time::timeout(Duration::from_secs(30), io_join).await;
}
