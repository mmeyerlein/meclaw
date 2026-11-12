//! GH #1451 (D38 V1/V3, fix round 1, review o15-Rv1 I3) -- the rebuild's
//! state list (`summary_state_cond`) follows the two knobs like the
//! window's state block.
//!
//! WHY this file exists: V1 and V3 were built on the turn path AND in the
//! rebuild (OR-d38-3.v.13, v.16: `state_of_rows` with the rebuild's
//! handle `live` and `ctx.needs`), but no pin drove a rebuild ("lesson 4 of
//! c1: pin the rebuilt path too"). A rebuild starts on a strike of the clock:
//! the edge `./clock -> ./policy` lifts only `audience_set` into the context,
//! so the rebuild knows no `needs` and no handle of a running session. Hence: one line per
//! `(subject, predicate)` (V1 a); a scheduled (`foresight`) line stays out of
//! the list (V1 b, nothing asks for it); the session's delegation row is
//! "open: a delegation". A row parked under a handle stays in the list
//! (OR-d38-3.o15.8): the rebuild reads the whole delegation prefix -- one
//! colony, one member -- and keeps the rows whose own audience the round may
//! see (D58 live: always, the knob `delegation_park_by_call` fell).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const OLD: &str = "Ada will call his mother tonight.";
const NEW: &str = "Ada will call his mother at 8 pm.";
const SCHEDULED: &str = "Ada wants a reminder at 6 pm.";

fn fact(h: &mut Hive, back_s: i64, predicate: &str, claim: &str, kind: &str) {
    let mut v = json!({"subject": "Ada", "predicate": predicate, "claim": claim,
                       "source": "person", "stance": "asserted"});
    if !kind.is_empty() {
        v["fact_kind"] = json!(kind);
    }
    h.mark(backdated(back_s), "s1", "t1", "fact", &canonical(&v));
}

fn delegation(h: &mut Hive, scope: &str, id: &str, audience: Option<&str>) {
    let at = backdated(5);
    let until = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp_millis();
    let mut row = json!({"delegation_id": id,
                         "round": format!("t1~{until}~0000abcd"),
                         "until": until,
                         "parked_at": at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()});
    if let Some(aud) = audience {
        row["audience_set"] = json!(aud);
    }
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![format!("pending:delegation:{scope}#t1"), canonical(&row)],
    )
    .expect("a state row");
}

/// The state line the summarizer is handed in the rebuild.
fn rebuild_line(knobs: &str, scope: &str, audience: Option<&str>) -> String {
    clear_of_midnight();
    let mut over: Vec<(&str, &str, Value)> = vec![
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!("1")),
        ("policy", "state_lines", json!(8)),
    ];
    for k in ["state_one_per_subject", "state_scheduled_on_need"] {
        over.push(("policy", k, json!(knobs)));
    }
    let mut h = Hive::with(&over);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    fact(&mut h, 30, "calls", OLD, "");
    fact(&mut h, 20, "calls", NEW, "");
    fact(&mut h, 10, "reminder_at", SCHEDULED, "foresight");
    delegation(&mut h, scope, "d-1", audience);
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("- person (2026-11-07) x/y: kept. [asserted]", "stop");
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    transcript
        .lines()
        .find(|l| l.starts_with("[state -- "))
        .unwrap_or("")
        .to_string()
}

/// Pin: in the rebuild one line per subject, no scheduled line, the
/// session's delegation open.
#[test]
fn the_rebuild_lists_one_line_per_subject_and_no_schedule() {
    if !shipped() {
        return;
    }
    let line = rebuild_line("1", "s1", None);
    assert!(line.contains(NEW), "the younger line: {line}");
    assert!(
        !line.contains(OLD),
        "the older line of the subject goes: {line}"
    );
    assert!(
        !line.contains(SCHEDULED),
        "a rebuild asks for nothing scheduled: {line}"
    );
    assert!(line.contains("open: a delegation"), "{line}");
}

/// Pin (OR-d38-3.o15.8): a rebuild knows no handle (v.13), yet an open
/// delegation parked under a handle stays in the list to keep -- the row's
/// own audience is the round's.
#[test]
fn the_rebuild_keeps_an_open_delegation_of_a_call() {
    if !shipped() {
        return;
    }
    let line = rebuild_line("1", "call-9", Some(ROUND_E));
    assert!(line.contains("open: a delegation"), "{line}");
}

/// Lock: a handle's row of another audience, or of before (no audience), is
/// not the round's.
#[test]
fn the_rebuild_keeps_no_row_of_another_audience() {
    if !shipped() {
        return;
    }
    for aud in [Some("[\"member:x\"]"), None] {
        let line = rebuild_line("1", "call-9", aud);
        assert!(!line.contains("open: a delegation"), "{aud:?}: {line}");
    }
}

/// Lock (D58 live, contract point 7): with the other knobs off (shipped) the
/// rebuild reads the park all the same -- a row of the session and a row of a
/// handle, each with the round's audience: there is no knob left that keys a
/// park by the session alone.
#[test]
fn off_the_rebuild_reads_the_park_of_the_session_and_of_a_handle() {
    if !shipped() {
        return;
    }
    for scope in ["s1", "call-9"] {
        let line = rebuild_line("0", scope, Some(ROUND_E));
        assert!(line.contains("open: a delegation"), "{scope}: {line}");
    }
}

/// Lock: off (shipped), the list of before -- both lines and the schedule.
#[test]
fn off_the_rebuild_lists_as_before() {
    if !shipped() {
        return;
    }
    let line = rebuild_line("0", "s1", None);
    for part in [OLD, NEW, SCHEDULED, "open: a delegation"] {
        assert!(line.contains(part), "{part:?} in {line:?}");
    }
}
