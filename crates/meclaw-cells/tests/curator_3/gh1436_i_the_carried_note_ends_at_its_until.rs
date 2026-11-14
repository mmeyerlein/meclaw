//! GH #1436 (d54g) -- the carried note ends at its `until`.
//!
//! WHY this file exists: the after-effect of a crisis is bounded -- the set
//! names its end (`until`: a number of seconds after the turn, or `day`, the
//! end of the turn's day in the policy's `timezone`, UTC without one); the
//! curator sets no number of its own (R-IG-1). The row carries the instant,
//! and past it no window shows the note. The ending is shown by moving the
//! stored instant behind the clock (no test waits for a clock).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const NOTE: &str = "The person may be in danger: answer with care.";
const CARRY: &str = "Earlier the person spoke of a crisis and it was answered. Do not bring it up unasked; if they come back to it, stay with it.";

fn crisis() -> Value {
    json!([{"id": "route.risk", "source": "route.risk", "text": NOTE, "priority": 0}])
}

fn talk(h: &mut Hive, session: &str, turn: &str, notes: Value) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk",
                     "turn_notes": notes});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("How are you?")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call: {:?} {:?}", h.out, h.stderr);
    let call = calls[0].clone();
    h.tap(&call, "stop", json!({}), json!([said("Fine, and you?")]));
    call
}

fn shows(call: &Msg, text: &str) -> bool {
    texts(call).iter().any(|t| t.contains(text))
}

fn instant(v: &Value) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap_or(""))
        .unwrap_or_else(|e| panic!("{v} is no RFC 3339 instant: {e}"))
        .with_timezone(&chrono::Utc)
}

/// The carried row's `(at, until)` after a crisis turn in `s1` under `until`.
fn born(h: &mut Hive) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    talk(h, "s1", "t1", crisis());
    let rows = h.rows("SELECT at, until FROM notes WHERE source = 'carry.route.risk'");
    assert_eq!(rows.len(), 1, "{rows:?} {:?}", h.stderr);
    (instant(&rows[0][0]), instant(&rows[0][1]))
}

#[test]
fn a_duration_ends_that_many_seconds_after_the_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        (
            "policy",
            "carry_notes",
            json!({"route.risk": {"text": CARRY, "until": 3600}}),
        ),
    ]);
    let (at, until) = born(&mut h);
    assert_eq!(until - at, chrono::Duration::seconds(3600), "{at} {until}");
}

#[test]
fn day_ends_at_the_end_of_the_turns_day() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        (
            "policy",
            "carry_notes",
            json!({"route.risk": {"text": CARRY, "until": "day"}}),
        ),
        ("policy", "timezone", json!("UTC")),
    ]);
    let (at, until) = born(&mut h);
    let next_midnight = at
        .date_naive()
        .succ_opt()
        .expect("a next day")
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc();
    assert_eq!(until, next_midnight, "the end of the turn's day: {at}");
}

#[test]
fn the_carried_note_ends_at_its_until() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "notes_share", json!(0.04)),
        (
            "policy",
            "carry_notes",
            json!({"route.risk": {"text": CARRY, "until": 3600}}),
        ),
    ]);
    born(&mut h);
    let open = talk(&mut h, "s2", "t1", json!([]));
    assert!(shows(&open, CARRY), "before its end: {:?}", texts(&open));
    // Its end moved behind the clock: the next window shows it no more.
    let past = backdated(5).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    h.db.execute(
        "UPDATE notes SET until = ?1 WHERE source = 'carry.route.risk'",
        rusqlite::params![past],
    )
    .expect("an update");
    let ended = talk(&mut h, "s3", "t1", json!([]));
    assert!(
        !shows(&ended, CARRY),
        "past its until no window shows it: {:?}",
        texts(&ended)
    );
}
