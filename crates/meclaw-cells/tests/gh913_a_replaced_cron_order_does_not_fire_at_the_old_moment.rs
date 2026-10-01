//! GH #913 — a cron order replaced in place does not fire at a moment of its
//! old expression.
//!
//! The I/O task pushes a strike for the moment it slept for; the handler takes
//! the mailbox before the event channel (`cell_task` select is biased that
//! way). An op that changes the row's expression -- `modify` with a new
//! `cron`, or `add … rearm: true` -- and is handled while that strike sits in
//! the channel used to meet the strike afterwards, load the NEW row, and emit
//! the new order at the OLD moment. The one-shot twin of this was GH #904.
//!
//! The in-flight order is made deterministic, not waited for: a second
//! connection holds the write lock of the timer's `cell.db` across the old
//! moment ([`WriteHold`]). The change op is sent while the hold stands, so the
//! handler is inside the op, parked on its UPDATE (busy timeout 30 s, query
//! timeout set to 5 s in the spawn, the hold is under 2 s), when the I/O task
//! pushes the old moment's strike (the I/O task has no database). The hold
//! ends half a second after the old moment; then the op completes, and the
//! queued strike is handled against the changed row. The handler's debug line
//! for the dropped strike is caught ([`SkipLines`]): it is the proof that the
//! strike really was in the queue, so a run in which the I/O task woke only
//! after the release cannot pass as green without having tested anything.
//!
//! Measured at the receiver: the sink the colony delivers to, the
//! `scheduled_at` and `schedule_name` the strike carries, and the instant it
//! arrived.
//!
//! 1. `modify` to another expression with a strike in flight: nothing at the
//!    old moment, the new order once at its own first moment.
//! 2. `add … rearm: true` to another expression and body: the same.
//! 3. `modify` that keeps the expression and renames: the strike in flight is
//!    the order's own moment and fires, once, as renamed.
//! 4. `trigger` (`FireNow`, GH #17) fires at once, off the expression.
//! 5. The handler seam without a clock: the same cases straight through
//!    `handle` and `handle_event`, `iteration_n` untouched by a dropped strike.

use chrono::{DateTime, Utc};
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{insert_schedule, load_schedule, setup_timer_schema};
use meclaw_cells::timer::io::{TimerEvent, TimerReconfig};
use meclaw_cells::timer::schedule::{ScheduleKind, ScheduleRow};
use meclaw_colony::{CellFactory, DbConn, LongRunningCell};
use meclaw_core::{
    Body, CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::{ColonyHandle, topologies::phase_3a::CaptureCell};
use serde_json::{Map, json};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The order before the change: every fourth second (0, 4, …, 56 -- 60 is a
/// multiple of 4, so the period holds across the minute turn).
const OLD: &str = "*/4 * * * * *";
/// The order after the change: the seconds in between (2, 6, …, 58). No
/// moment in common with [`OLD`], so the old moment is off the new expression
/// and the new order's first moment is two seconds after it.
const NEW: &str = "2/4 * * * * *";

fn ms(n: i64) -> chrono::Duration {
    chrono::Duration::milliseconds(n)
}

/// One colony on `cell_dir`: `/sink` first (anti-cascade), then the timer
/// through its factory, then the edge.
async fn boot(cell_dir: &std::path::Path) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;
    // The hold only works while it is shorter than the op's query timeout (an
    // interrupted UPDATE answers `query_timeout` and changes nothing), so the
    // premise is set here rather than taken from the default.
    let spawned = Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/timer"),
            json!({"query_timeout_ms": 5000}),
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
    h.add_edge(Uuid::now_v7(), Path::new("/timer"), Path::new("/sink"))
        .await;
    (h, recv_rx)
}

/// A timer op as a dispatcher delivers it: one `tool_call` turn, answered
/// with an ack once the op is stored.
fn op_call(args: serde_json::Value, call_id: &str) -> Message {
    MessageBuilder::new(Path::new("/timer"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": call_id
        }]})))
        .build()
}

/// A timer op as a raw body: stored, never answered. The arming `add` goes
/// this way so its ack cannot race the order's first strike at the sink.
fn op_raw(args: serde_json::Value) -> Message {
    MessageBuilder::new(Path::new("/timer"))
        .body(Body::Inline(args))
        .build()
}

async fn next(rx: &mut mpsc::Receiver<Message>, why: &str) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("no message at /sink within 30s: {why}"))
        .expect("sink channel closed")
}

/// Asserts that nothing reaches `/sink` before `until`.
async fn quiet_until(rx: &mut mpsc::Receiver<Message>, until: DateTime<Utc>, why: &str) {
    let wait = (until - Utc::now()).to_std().unwrap_or_default();
    let extra = tokio::time::timeout(wait, rx.recv()).await;
    assert!(
        !matches!(extra, Ok(Some(_))),
        "{why}, got {:?}",
        extra.map(|m| m.map(|m| m.headers.hop.clone()))
    );
}

async fn sleep_until(t: DateTime<Utc>) {
    tokio::time::sleep((t - Utc::now()).to_std().unwrap_or_default()).await;
}

fn hop_str<'a>(m: &'a Message, key: &str) -> &'a str {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// A hop value, `Null` when the strike does not carry the key.
fn hop_val(m: &Message, key: &str) -> serde_json::Value {
    m.headers.hop.get(key).cloned().unwrap_or_default()
}

/// The moment a strike was due (`scheduled_at`, whole seconds).
fn moment(m: &Message) -> DateTime<Utc> {
    hop_str(m, "scheduled_at")
        .parse()
        .unwrap_or_else(|e| panic!("a strike carries its moment ({e}): {:?}", m.headers.hop))
}

/// The timer's "stale strike skipped" debug lines, each rendered as
/// `field=value` pairs. A tiny subscriber on the `tracing` crate alone: the
/// test crate has no capture helper, and this file needs exactly one fact --
/// that the handler met the strike for the old moment and dropped it.
static SKIPS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Subscriber that keeps the timer's skip lines in [`SKIPS`] and nothing else.
struct SkipLines;

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

impl tracing::Subscriber for SkipLines {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.target().starts_with("meclaw_cells::timer")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Fields(&mut line));
        if line.contains("stale strike skipped") {
            SKIPS.lock().unwrap().push(line);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Installs [`SkipLines`] once per test process (nextest runs one test per
/// process; under `cargo test` the lines of several tests share the list, and
/// [`skipped`] tells them apart by schedule id).
fn capture_skips() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(SkipLines);
    });
}

/// Whether the handler dropped a strike of `id` for `moment`.
fn skipped(id: Uuid, moment: DateTime<Utc>) -> bool {
    let (id, moment) = (id.to_string(), moment.to_string());
    SKIPS
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains(&id) && l.contains(&moment))
}

/// A second connection that holds the write lock of the timer's `cell.db`.
/// The handler's UPDATE waits behind it; its reads go on. See the module
/// comment for why this makes the #913 order deterministic.
struct WriteHold(rusqlite::Connection);

impl WriteHold {
    fn take(db: &std::path::Path) -> Self {
        let c = rusqlite::Connection::open(db).expect("open the timer's cell.db");
        c.busy_timeout(Duration::from_secs(5)).unwrap();
        c.execute_batch("BEGIN IMMEDIATE")
            .expect("take the write lock");
        Self(c)
    }

    fn release(self) {
        self.0
            .execute_batch("ROLLBACK")
            .expect("drop the write lock");
    }
}

/// A colony whose `*/4` order `old` has struck once and then had `change`
/// put in while the strike for its next moment was in flight. Returned after
/// the change's ack.
struct InFlight {
    h: ColonyHandle,
    rx: mpsc::Receiver<Message>,
    _td: TempDir,
    /// The order's schedule id.
    id: Uuid,
    /// The next moment of the old expression after the first strike -- the
    /// one whose strike was in flight.
    old_moment: DateTime<Utc>,
}

async fn change_with_a_strike_in_flight(mut change: serde_json::Value) -> InFlight {
    capture_skips();
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (h, mut rx) = boot(&cell_dir).await;
    let id = Uuid::now_v7();
    h.send(op_raw(json!({
        "op": "add", "schedule_id": id.to_string(), "schedule_name": "old",
        "cron": OLD, "emit_to": "/sink", "emit_body": {"messages": []},
        "emit_headers": {"msg_type": "probe_tick"},
    })))
    .await;
    let first = next(&mut rx, "the old order strikes").await;
    assert_eq!(
        hop_str(&first, "msg_type"),
        "probe_tick",
        "{:?}",
        first.headers.hop
    );
    assert_eq!(hop_str(&first, "schedule_name"), "old");
    let old_moment = moment(&first) + chrono::Duration::seconds(4);

    // 1.2 s ahead of the old moment: the previous strike is long handled, and
    // the op has a second to reach the handler before the moment comes.
    sleep_until(old_moment - ms(1200)).await;
    let hold = WriteHold::take(&cell_dir.join("cell.db"));
    change["schedule_id"] = json!(id.to_string());
    h.send(op_call(change, "call-change")).await;
    assert!(
        Utc::now() < old_moment - ms(500),
        "setup outran the lead: the change has to be in the handler before {old_moment}"
    );
    // Half a second past the moment the I/O task has pushed the strike. The
    // handler is parked behind the lock whatever it took first, so nothing
    // reaches the sink before the release. Which came first is read after it:
    // the op's ack has to be the first thing at the sink -- a strike handled
    // ahead of the op would have parked on its own write (`bump_iteration`)
    // and arrive before the ack.
    quiet_until(
        &mut rx,
        old_moment + ms(500),
        "while the change waits for the write lock nothing reaches the sink",
    )
    .await;
    hold.release();
    let ack = next(&mut rx, "the change is acked once the lock is gone").await;
    assert_eq!(
        hop_str(&ack, "msg_type"),
        "timer_op_ack",
        "the first thing after the hold is the change's ack: {:?}",
        ack.headers.hop
    );
    InFlight {
        h,
        rx,
        _td: td,
        id,
        old_moment,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_modify_with_a_strike_in_flight_does_not_fire_at_the_old_moment() {
    let mut s = change_with_a_strike_in_flight(json!({
        "op": "modify", "cron": NEW, "schedule_name": "new",
    }))
    .await;
    let strike = next(&mut s.rx, "the new order strikes").await;
    let arrived = Utc::now();
    assert_ne!(
        moment(&strike),
        s.old_moment,
        "the old moment fired the new order: {:?}",
        strike.headers.hop
    );
    assert_eq!(
        hop_str(&strike, "schedule_name"),
        "new",
        "{:?}",
        strike.headers.hop
    );
    assert_eq!(
        moment(&strike),
        s.old_moment + chrono::Duration::seconds(2),
        "the new expression's first moment after the old one"
    );
    // A stale strike would arrive right behind the ack, half a second after
    // the old moment; the new order's own strike sleeps until two seconds
    // after it.
    assert!(
        arrived >= s.old_moment + chrono::Duration::seconds(1),
        "arrived {arrived}, old moment {}",
        s.old_moment
    );
    assert_eq!(
        hop_val(&strike, "iteration_n"),
        json!(1),
        "the dropped strike did not count: only the old order's first strike did"
    );
    assert!(
        skipped(s.id, s.old_moment),
        "the strike for the old moment was in the queue and dropped there; without \
         this line the I/O task woke only after the release and the run tested nothing"
    );
    quiet_until(
        &mut s.rx,
        s.old_moment + ms(3500),
        "the new order strikes once per moment",
    )
    .await;
    s.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rearm_with_a_strike_in_flight_does_not_fire_at_the_old_moment() {
    let mut s = change_with_a_strike_in_flight(json!({
        "op": "add", "rearm": true, "schedule_name": "second", "cron": NEW,
        "emit_to": "/sink", "emit_body": {"messages": []},
        "emit_headers": {"msg_type": "probe_second"},
    }))
    .await;
    let strike = next(&mut s.rx, "the re-armed order strikes").await;
    let arrived = Utc::now();
    assert_ne!(
        moment(&strike),
        s.old_moment,
        "the old moment fired the re-armed order: {:?}",
        strike.headers.hop
    );
    assert_eq!(
        hop_str(&strike, "msg_type"),
        "probe_second",
        "{:?}",
        strike.headers.hop
    );
    assert_eq!(hop_str(&strike, "schedule_name"), "second");
    assert_eq!(
        moment(&strike),
        s.old_moment + chrono::Duration::seconds(2),
        "the new expression's first moment after the old one"
    );
    assert!(
        arrived >= s.old_moment + chrono::Duration::seconds(1),
        "arrived {arrived}, old moment {}",
        s.old_moment
    );
    assert_eq!(
        hop_val(&strike, "iteration_n"),
        json!(0),
        "a rearm starts the order from iteration 0, and the dropped strike did not count"
    );
    assert!(
        skipped(s.id, s.old_moment),
        "the strike for the old moment was in the queue and dropped there; without \
         this line the I/O task woke only after the release and the run tested nothing"
    );
    quiet_until(
        &mut s.rx,
        s.old_moment + ms(3500),
        "the re-armed order strikes once per moment",
    )
    .await;
    s.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_modify_that_keeps_the_expression_fires_the_strike_in_flight_once() {
    let mut s = change_with_a_strike_in_flight(json!({
        "op": "modify", "cron": OLD, "schedule_name": "renamed",
    }))
    .await;
    let strike = next(&mut s.rx, "the strike in flight is the order's own").await;
    assert_eq!(
        moment(&strike),
        s.old_moment,
        "the expression did not change, so its moment still stands: {:?}",
        strike.headers.hop
    );
    assert_eq!(
        hop_str(&strike, "schedule_name"),
        "renamed",
        "and it fires the order as it stands now"
    );
    assert_eq!(hop_val(&strike, "iteration_n"), json!(1));
    quiet_until(
        &mut s.rx,
        s.old_moment + ms(3500),
        "no second strike for the same moment",
    )
    .await;
    let after = next(&mut s.rx, "the plan goes on").await;
    assert_eq!(moment(&after), s.old_moment + chrono::Duration::seconds(4));
    assert_eq!(hop_str(&after, "schedule_name"), "renamed");
    assert_eq!(hop_val(&after, "iteration_n"), json!(2));
    s.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trigger_fires_at_once_off_the_expression() {
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (h, mut rx) = boot(&cell_dir).await;
    let id = Uuid::now_v7();
    h.send(op_raw(json!({
        "op": "add", "schedule_id": id.to_string(), "schedule_name": "tick",
        "cron": OLD, "emit_to": "/sink", "emit_body": {"messages": []},
        "emit_headers": {"msg_type": "probe_tick"},
    })))
    .await;
    let first = next(&mut rx, "the order strikes").await;
    let t0 = moment(&first);

    sleep_until(t0 + ms(1200)).await;
    h.send(op_call(
        json!({"op": "trigger", "schedule_id": id.to_string()}),
        "call-trigger",
    ))
    .await;
    let ack = next(&mut rx, "the trigger is acked").await;
    assert_eq!(
        hop_str(&ack, "msg_type"),
        "timer_op_ack",
        "{:?}",
        ack.headers.hop
    );
    let fired = next(&mut rx, "the trigger fires").await;
    let at = moment(&fired);
    assert_eq!(
        hop_str(&fired, "msg_type"),
        "probe_tick",
        "{:?}",
        fired.headers.hop
    );
    assert!(
        at > t0 && at < t0 + chrono::Duration::seconds(4),
        "fired at {at}, between two moments of the expression ({t0} and the next)"
    );
    assert_eq!(hop_val(&fired, "iteration_n"), json!(1));
    let after = next(&mut rx, "the plan goes on").await;
    assert_eq!(
        moment(&after),
        t0 + chrono::Duration::seconds(4),
        "a trigger leaves the plan as it was"
    );
    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// The handler seam, without a clock.
// ---------------------------------------------------------------------------

fn op_msg(op: serde_json::Value) -> Message {
    MessageBuilder::new(Path::new("/t"))
        .body(Body::Inline(op))
        .build()
}

async fn op(
    cell: &mut TimerCell,
    db: &mut DbConn,
    rc_tx: &mpsc::Sender<TimerReconfig>,
    body: serde_json::Value,
) {
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let msg = op_msg(body);
    let sink = OutputSink::new(
        out_tx,
        Path::new("/t"),
        msg.id,
        msg.trace_id,
        32,
        meclaw_core::Headers::new(),
        None,
    );
    cell.handle(msg, &sink, db, rc_tx).await;
}

async fn strike(
    cell: &mut TimerCell,
    origin: &OriginSink,
    db: &mut DbConn,
    schedule_id: Uuid,
    scheduled_at: DateTime<Utc>,
    forced: bool,
) {
    cell.handle_event(
        TimerEvent::Fire {
            schedule_id,
            scheduled_at,
            forced,
        },
        origin,
        db,
    )
    .await;
}

async fn iteration(db: &mut DbConn, id: Uuid) -> u64 {
    db.call(move |c| load_schedule(c, id))
        .await
        .unwrap()
        .expect("row present")
        .iteration_n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_handler_drops_the_strike_a_changed_expression_no_longer_has() {
    use chrono::TimeZone;
    let t = |s: u32| Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, s).unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();
    let id = Uuid::now_v7();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "old".into(),
            kind: ScheduleKind::Cron(OLD.into()),
            emit_to: Path::new("/dst"),
            emit_body: json!({"messages": []}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up: false,
        },
    )
    .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/t"), 32);
    let silent = |rx: &mut mpsc::Receiver<CellEmission>| rx.try_recv().is_err();

    op(
        &mut cell,
        &mut db,
        &rc_tx,
        json!({"op": "modify", "schedule_id": id.to_string(), "cron": NEW, "schedule_name": "new"}),
    )
    .await;

    // :04 is a moment of the old expression, not of the new one.
    strike(&mut cell, &origin, &mut db, id, t(4), false).await;
    assert!(
        silent(&mut origin_rx),
        "the old moment must not fire the new order"
    );
    assert_eq!(
        iteration(&mut db, id).await,
        0,
        "a dropped strike leaves iteration_n where it was"
    );

    // :06 is the new expression's own moment.
    strike(&mut cell, &origin, &mut db, id, t(6), false).await;
    let em = origin_rx.try_recv().expect("the new moment fires");
    assert_eq!(em.content["header"]["schedule_name"], "new");
    assert_eq!(em.content["header"]["scheduled_at"], "2030-01-01T12:00:06Z");
    assert_eq!(iteration(&mut db, id).await, 1);

    // An operator trigger fires whatever the moment.
    strike(&mut cell, &origin, &mut db, id, t(5), true).await;
    let em = origin_rx.try_recv().expect("a forced strike fires");
    assert_eq!(em.content["header"]["schedule_name"], "new");

    // A modify that keeps the expression keeps its moments.
    op(
        &mut cell,
        &mut db,
        &rc_tx,
        json!({"op": "modify", "schedule_id": id.to_string(), "cron": NEW, "schedule_name": "same"}),
    )
    .await;
    strike(&mut cell, &origin, &mut db, id, t(10), false).await;
    let em = origin_rx.try_recv().expect("the same expression fires on");
    assert_eq!(em.content["header"]["schedule_name"], "same");
    assert!(silent(&mut origin_rx), "once");
}
