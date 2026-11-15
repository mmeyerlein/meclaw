//! GH #1469 (wave d58-2 close, inventory 39; errand review M1, first gap) --
//! a second errand of the same turn is not parked over the first.
//!
//! WHY this file exists: the park of an errand is keyed by the turn of the
//! person (`pending:delegation:<scope>#<turn>`). A second errand handed on in
//! the same turn was parked over the first with an unconditional set: the
//! first errand's row was gone without a word, and with it its `until` and
//! everything its tap would say at it -- its outcome was never filed. Now,
//! with `errand_outcome` "1", the park is written only while none stands
//! (with "0" it is the set of before: review close I1, held by
//! `curator_errands/gh_d58_errand_off_is_byte_identical.rs`, second test);
//! a second errand of the turn
//! leaves the first's row as it is and gets the mark `errand_outcome`
//! "not followed" under its own round, naming the errand that is followed.
//! A round delivered again finds its own row and writes no such mark.
//!
//! Mutant: the park as the unconditional set of before -> the first test
//! red (the row names the second errand, no mark).
//!
//! Runs the shipped curator hive behind the clock of the bank; no model.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_errand.rs"]
#[allow(unused_imports)]
use crate::curator_errand; // harness_fold: mod curator_errand;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use chrono::Duration;
use curator_errand::*;
use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The round of an errand of the turn `label` that is already there: the
/// lane of `hand_on` without the person's turn before it.
fn round_of(h: &mut Hive, label: &str, hex8: &str, minutes: i64) -> (String, String) {
    let deadline = start() + Duration::minutes(minutes);
    let inner = format!("{label}~{}~{hex8}", deadline.timestamp_millis());
    let id = format!("dg-{hex8}");
    let text = format!("[delegation item_{hex8} from the caller: find out about {label}]");
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": S, "turn_id": inner, "iter": "0", "channel": "test",
               "audience_set": ROUND_E, "delegation_id": id}),
        json!({"session_id": S, "turn_id": inner, "iter": "0", "phase": "",
               "delegation_id": id}),
        json!({"messages": [user(&text)], "system": mode("Be brief.")}),
    );
    (inner, id)
}

fn parked(h: &Hive, label: &str) -> Value {
    sj::from_str(&h.state(&park_key(label))).unwrap_or(Value::Null)
}

fn outcome_marks_of(h: &Hive, inner: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = 'errand_outcome' AND turn_id = '{inner}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
    .collect()
}

#[test]
fn a_second_errand_of_the_turn_leaves_the_first_parked_and_a_mark() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    assert_eq!(
        parked(&h, "p01")["delegation_id"],
        json!(e.id),
        "check of the setup: the first errand is parked"
    );

    let (inner, id) = round_of(&mut h, "p01", "bbbb2222", 7);
    assert_eq!(
        h.routed("brain").len(),
        1,
        "check of the setup: the second errand leaves as one call: {:?}",
        h.stderr
    );
    let row = parked(&h, "p01");
    assert_eq!(
        row["delegation_id"],
        json!(e.id),
        "the second errand of the turn is not parked over the first: {row}"
    );
    assert_eq!(
        row["round"],
        json!(e.inner),
        "the first's row stands whole: {row}"
    );
    let marks = outcome_marks_of(&h, &inner);
    assert_eq!(
        marks,
        vec![json!({"delegation_id": id, "outcome": "not followed", "followed": e.id})],
        "the second errand says it is not followed, once, under its own round: {:?}",
        h.stderr
    );
    assert!(
        outcome_marks(&h, &e).is_empty(),
        "the first errand has no outcome yet: {:?}",
        outcome_marks(&h, &e)
    );
}

#[test]
fn a_round_delivered_again_finds_its_own_park_and_leaves_no_mark() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let before = h.state(&park_key("p01"));
    let (inner, _) = round_of(&mut h, "p01", "aaaa1111", 5);
    assert_eq!(inner, e.inner, "check of the setup: the same round again");
    assert_eq!(
        h.state(&park_key("p01")),
        before,
        "the park stands as it was"
    );
    assert!(
        outcome_marks_of(&h, &inner).is_empty(),
        "a round that finds its own park says nothing: {:?}",
        outcome_marks_of(&h, &inner)
    );
}
