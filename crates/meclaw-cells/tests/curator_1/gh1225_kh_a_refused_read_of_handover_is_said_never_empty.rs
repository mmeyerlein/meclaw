//! GH #1225 (A3, EA3, OR-KF-31) -- a read `./handover` was refused is never "no rows".
//!
//! WHY this file exists: `./handover` took the rows of a read the store
//! refused (`query_timeout`, `store_degraded`) for an empty answer. A close
//! whose wall read failed said "closed without a turn" and left no note; a
//! session that came back after another one was taken for a new session and
//! got the leaf of one; a renewal was composed without its summary. Now a
//! refused read is sent once more (the phase with `-again`, `error_code` in
//! the header); refused again, the flow says so -- on stderr, with a `marks`
//! row `store_error` `{step, legs, call?}` (the form of every curator cell,
//! OR-KF-31) and, on a renewal, `error_code` on the block it hands on -- and
//! composes nothing from what it did not read. Refused once, the second
//! attempt runs as if nothing happened.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn close(h: &mut Hive, session: &str) {
    h.summ.clear();
    h.lane(
        "in_close",
        json!({"session_id": session, "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": []}),
    );
}

fn renew(h: &mut Hive, session: &str, call_id: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": session, "call_id": call_id, "channel": "test",
               "audience_set": ROUND_E}),
        json!({"call_id": call_id, "session_id": session, "renewal_n": 1}),
        json!({"messages": []}),
    );
    h.routed("sidecar")
}

/// The `store_error` marks of the ledger, their values parsed.
fn store_errors(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'store_error' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("")).unwrap_or(Value::Null))
        .collect()
}

fn stderr(h: &Hive) -> String {
    h.stderr.join("\n")
}

fn set_marks_of(h: &Hive, session: &str) -> usize {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = 'handover' AND session_id = '{session}'"
    ))
    .into_iter()
    .filter(|r| r[0].as_str().unwrap_or("").contains("\"state\":\"set\""))
    .count()
}

fn state_value(h: &Hive, key: &str) -> String {
    h.state(key)
}

/// The store refuses the read twice: the first time the leg `leg` with
/// `query_timeout`, the second time every leg of the retry (`<phase>-again`).
fn refuse_twice(h: &mut Hive, leg: &str, phase: &str) {
    h.ledger_may_refuse = true;
    h.refuse_legs
        .push((leg.to_string(), "query_timeout".to_string()));
    h.refuse_phase = Some(format!("{phase}-again"));
}

#[test]
fn a_close_whose_wall_read_is_refused_twice_says_so_and_leaves_no_note() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-a",
        "a1",
        "kh3 a: the ferry leaves at nine.",
        "kh3 a noted.",
        json!({}),
    );
    h.stderr.clear();
    refuse_twice(&mut h, "r-wall", "close");
    close(&mut h, "s-a");
    let err = stderr(&h);
    assert!(
        !err.contains("closed without a turn"),
        "a refused read is no empty session: {err}"
    );
    assert!(h.summ.is_empty(), "no note from a wall it did not read");
    let marks = store_errors(&h);
    assert_eq!(marks.len(), 1, "one store_error mark: {marks:?} {err}");
    assert_eq!(marks[0]["step"], "close", "{marks:?}");
    assert!(
        marks[0]["legs"].get("r-wall").is_some(),
        "the mark names the refused read: {marks:?}"
    );
    assert!(err.contains("refused"), "said on stderr: {err}");
}

#[test]
fn a_close_whose_wall_read_is_refused_once_makes_its_note() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-a",
        "a1",
        "kh3 a: the ferry leaves at nine.",
        "kh3 a noted.",
        json!({}),
    );
    h.ledger_may_refuse = true;
    h.refuse_legs
        .push(("r-wall".to_string(), "query_timeout".to_string()));
    close(&mut h, "s-a");
    assert_eq!(h.summ.len(), 1, "the note is asked for: {:?}", h.stderr);
    let req = sj::to_string(&h.summ[0].body).unwrap_or_default();
    assert!(
        req.contains("the ferry leaves at nine"),
        "the second read's words: {req}"
    );
    assert!(store_errors(&h).is_empty(), "nothing failed in the end");
}

/// `s-a` served, then `s-b` (with a topic to hand over), then `s-a` comes back.
fn returning(h: &mut Hive) {
    turn(
        h,
        "s-a",
        "a1",
        "kh3 a: the ferry leaves at nine.",
        "kh3 a noted.",
        json!({}),
    );
    h.mark(
        chrono::Utc::now(),
        "s-a",
        "a1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "kh3 the ferry"})),
    );
    turn(
        h,
        "s-b",
        "b1",
        "kh3 b: anything new?",
        "kh3 b noted.",
        json!({}),
    );
    assert_eq!(state_value(h, "handover_for"), "s-b");
    assert_eq!(set_marks_of(h, "s-b"), 1, "s-b got a leaf: {:?}", h.stderr);
}

#[test]
fn a_returning_session_whose_read_is_refused_twice_gets_no_leaf() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    returning(&mut h);
    let slot = h.rows("SELECT hash FROM slots WHERE path = 'history.handover'");
    let set_before = set_marks_of(&h, "s-a");
    h.stderr.clear();
    refuse_twice(&mut h, "r-wallb", "new");
    let call = turn(
        &mut h,
        "s-a",
        "a2",
        "kh3 a: back again.",
        "kh3 a noted again.",
        json!({}),
    );
    let err = stderr(&h);
    assert_eq!(
        set_marks_of(&h, "s-a"),
        set_before,
        "a returning session never gets the leaf of a new one: {err}"
    );
    assert_eq!(
        h.rows("SELECT hash FROM slots WHERE path = 'history.handover'"),
        slot,
        "the leaf stands as it was"
    );
    assert_eq!(
        state_value(&h, "handover_for"),
        "s-b",
        "the next call comes to the handover again"
    );
    let marks = store_errors(&h);
    assert_eq!(marks.len(), 1, "one store_error mark: {marks:?} {err}");
    assert_eq!(marks[0]["step"], "new");
    assert!(marks[0]["legs"].get("r-wallb").is_some(), "{marks:?}");
    assert_eq!(marks[0]["call"], call.hop["curator_call"], "{marks:?}");
    assert!(err.contains("refused"), "said on stderr: {err}");

    // The next call of the session reads again and is served as returning.
    turn(
        &mut h,
        "s-a",
        "a3",
        "kh3 a: still here.",
        "kh3 a noted.",
        json!({}),
    );
    assert_eq!(state_value(&h, "handover_for"), "s-a");
    assert_eq!(set_marks_of(&h, "s-a"), set_before, "still no leaf");
}

#[test]
fn a_returning_session_whose_read_is_refused_once_is_served_as_returning() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    returning(&mut h);
    let set_before = set_marks_of(&h, "s-a");
    h.ledger_may_refuse = true;
    h.refuse_legs
        .push(("r-wallb".to_string(), "query_timeout".to_string()));
    turn(
        &mut h,
        "s-a",
        "a2",
        "kh3 a: back again.",
        "kh3 a noted again.",
        json!({}),
    );
    assert_eq!(state_value(&h, "handover_for"), "s-a", "{:?}", h.stderr);
    assert_eq!(set_marks_of(&h, "s-a"), set_before, "no leaf");
    assert!(store_errors(&h).is_empty());
}

#[test]
fn a_renewal_whose_summary_read_is_refused_twice_carries_its_head_only() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-r",
        "r1",
        "kh3 r: the lamp is broken.",
        "kh3 r noted.",
        json!({}),
    );
    h.stderr.clear();
    refuse_twice(&mut h, "r-sum", "renew");
    let blocks = renew(&mut h, "s-r", "call-r");
    let err = stderr(&h);
    assert_eq!(blocks.len(), 1, "the call goes on: {err}");
    assert_eq!(
        blocks[0].hop.get("error_code"),
        Some(&json!("store_error")),
        "the code rides the block: {:?}",
        blocks[0].hop
    );
    let payload = blocks[0].body["payload"].as_str().unwrap_or("");
    assert!(
        !payload.contains("the lamp is broken"),
        "nothing composed from a refused read: {payload}"
    );
    let marks = store_errors(&h);
    assert_eq!(marks.len(), 1, "one store_error mark: {marks:?} {err}");
    assert_eq!(marks[0]["step"], "renew");
    assert!(marks[0]["legs"].get("r-sum").is_some(), "{marks:?}");
    assert_eq!(marks[0]["call"], "call-r");
    assert!(err.contains("refused"), "said on stderr: {err}");
}

#[test]
fn a_renewal_whose_summary_read_is_refused_once_is_whole() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-r",
        "r1",
        "kh3 r: the lamp is broken.",
        "kh3 r noted.",
        json!({}),
    );
    h.ledger_may_refuse = true;
    h.refuse_legs
        .push(("r-sum".to_string(), "query_timeout".to_string()));
    let blocks = renew(&mut h, "s-r", "call-r");
    assert_eq!(blocks.len(), 1, "{:?}", h.stderr);
    assert!(
        blocks[0].hop.get("error_code").is_none(),
        "{:?}",
        blocks[0].hop
    );
    let payload = blocks[0].body["payload"].as_str().unwrap_or("");
    assert!(payload.contains("the lamp is broken"), "{payload}");
    assert!(store_errors(&h).is_empty());
}
