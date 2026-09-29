//! GH #904 (PP-7): the curator's cache clock keeps ONE row.
//!
//! The policy ordered the clock under a fresh id per brain call
//! (`uuid5("cache:" + call)`) and removed the call before; the timer never
//! deletes a row (`docs/cell-types.md` `timer`), so `schedules` grew by one row
//! per call -- in a long-running curator colony without bound. Now the policy
//! re-arms one order under one id (`add` with `rearm`).
//!
//! Measured at the receiver: the shipped curator hive runs in one process
//! (`support/curator_hive.rs`), and every order it sends to `./clock` is handed
//! to the timer's own op parser and `timer::db::add_schedule` over an in-memory
//! `schedules` table with the timer's own DDL -- the table the `clock` cell's
//! `cell.db` holds. The strike is the timer's: `mark_completed` first
//! (state-before-emit), then the emission of the row as it stands.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_cells::timer::db::{
    AddOutcome, add_schedule, load_active_filter_past, load_schedule, mark_completed,
    setup_timer_schema,
};
use meclaw_cells::timer::op::TimerOp;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_core::serde_json::json;

/// The timer beside the hive: the orders the curator sent, applied in order.
struct Clock {
    db: rusqlite::Connection,
    seen: usize,
    outcomes: Vec<AddOutcome>,
}

impl Clock {
    fn new() -> Self {
        let db = rusqlite::Connection::open_in_memory().expect("sqlite");
        setup_timer_schema(&db).expect("ddl");
        Self {
            db,
            seen: 0,
            outcomes: Vec::new(),
        }
    }

    /// Every order the hive sent since the last look, through the timer's
    /// own parser and `add_schedule`. A `remove` would fail the lock: the
    /// clock has no business removing anything any more.
    fn take(&mut self, h: &Hive) {
        for m in &h.clock[self.seen..] {
            let op = TimerOp::parse(&json!(m.body)).expect("an order the timer takes");
            match op {
                TimerOp::Add { row, rearm } => {
                    assert!(rearm, "the curator re-arms: {:?}", m.body);
                    self.outcomes
                        .push(add_schedule(&self.db, &row, rearm).expect("add"));
                }
                other => panic!("the curator orders only `add` now, got {other:?}"),
            }
        }
        self.seen = h.clock.len();
    }

    fn rows(&self) -> i64 {
        self.db
            .query_row("SELECT count(*) FROM schedules", [], |r| r.get(0))
            .unwrap()
    }

    fn rowid(&self) -> i64 {
        self.db
            .query_row("SELECT rowid FROM schedules", [], |r| r.get(0))
            .unwrap()
    }

    /// The strike: state before emit, as `timer::cell::handle_event` does it,
    /// then the row's own emission to the hive.
    fn strike(&self, h: &mut Hive) {
        let now = chrono::Utc::now();
        let due = load_active_filter_past(&self.db, now).expect("active");
        assert_eq!(due.len(), 1, "one order stands: {due:?}");
        let row = load_schedule(&self.db, due[0].schedule_id)
            .unwrap()
            .expect("row");
        let ScheduleKind::At(at) = row.kind else {
            panic!("the cache clock is a one-shot: {row:?}");
        };
        assert_eq!(mark_completed(&self.db, row.schedule_id).unwrap(), 1);
        let order = Msg {
            body: obj(json!({
                "schedule_id": row.schedule_id.to_string(),
                "schedule_name": row.schedule_name,
                "at": at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "emit_body": row.emit_body,
            })),
            ..Msg::default()
        };
        h.fire(&order);
    }
}

fn expiry(i: u32) -> String {
    format!("2099-01-01T{:02}:{:02}:00Z", i / 60, i % 60)
}

#[test]
fn the_curator_clock_keeps_one_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let mut clock = Clock::new();
    let mut rowid = None;
    let mut struck = 0;
    for i in 0..50u32 {
        turn(
            &mut h,
            "s1",
            &format!("t{i}"),
            &format!("q{i}"),
            &format!("a{i}"),
            json!({"cache_expires_at": expiry(i)}),
        );
        clock.take(&h);
        assert_eq!(clock.rows(), 1, "after call {i}: one row");
        let r = *rowid.get_or_insert(clock.rowid());
        assert_eq!(
            clock.rowid(),
            r,
            "re-armed in place, the firing order holds (GH #613)"
        );
        let row = load_schedule(&clock.db, clock_id(&clock)).unwrap().unwrap();
        assert_eq!(row.status, "active", "after call {i}: armed");
        assert_eq!(
            row.emit_body["curator_call"],
            json!(h.state("last_call")),
            "the clock stands on the newest call"
        );
        assert!(
            matches!(row.kind, ScheduleKind::At(t)
                if t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true) == expiry(i)),
            "at the newest call's moment: {row:?}"
        );
        if i == 24 {
            // The cache went cold once: the order strikes, the curator hears it.
            clock.strike(&mut h);
            struck += 1;
            assert_eq!(h.state("armed_call"), "", "the struck order is disarmed");
            assert!(
                load_active_filter_past(&clock.db, chrono::Utc::now())
                    .unwrap()
                    .is_empty(),
                "a struck one-shot is planned no more"
            );
            assert_eq!(clock.rows(), 1, "and it stays one row");
            // The strike starts a rebuild; its summary comes back, so the calls
            // that follow run on a settled hive.
            while !h.summ.is_empty() {
                h.answer("summary", "stop");
            }
        }
    }
    assert_eq!(struck, 1);
    assert_eq!(clock.outcomes.len(), 50, "one order per call");
    assert_eq!(clock.outcomes[0], AddOutcome::Inserted);
    assert!(
        clock.outcomes[1..]
            .iter()
            .all(|o| *o == AddOutcome::Rearmed),
        "every later call re-arms (the one after the strike included): {:?}",
        clock.outcomes
    );
    assert_eq!(clock.rows(), 1, "50 calls, one row");
}

fn clock_id(clock: &Clock) -> meclaw_core::Uuid {
    let id: String = clock
        .db
        .query_row("SELECT schedule_id FROM schedules", [], |r| r.get(0))
        .unwrap();
    meclaw_core::Uuid::parse_str(&id).unwrap()
}
