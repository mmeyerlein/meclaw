//! GH #1469 (wave d58-2 close, inventory 39; errand review M1, third gap) --
//! a lost end of an errand is written again.
//!
//! WHY this file exists: the tap of an errand's round says at the parked row
//! how the round ended (`answered`, or `ended` "failed"), by a read and a
//! compare-and-set on the value read (phase `errand-end`). When another
//! writer changed the row between the two -- the tap of an earlier call of
//! the round, a curate filing an outcome -- the compare-and-set took nothing,
//! and nobody wrote the end again: a lost `answered` read as `no answer`
//! once the deadline passed, beside the answer itself. Now the write is a
//! step of its own (phase `errand-put`); a write that took nothing reads the
//! row again and adds the end to what stands. Every retry follows a write of
//! someone else, so the chain ends: the row took it, went, or holds it.
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

#[test]
fn an_answer_whose_end_lost_its_compare_and_set_is_written_again() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let key = park_key(&e.label);

    // Between the read of the row and the write of its end, another writer
    // changes it: the end of an earlier call of the round, said at the row.
    h.sql_at_phase = Some((
        "errand-put".to_string(),
        format!(
            "UPDATE state SET value = json_set(value, '$.ended', 'failed') WHERE key = '{key}'"
        ),
    ));
    answers(
        &mut h,
        start() + Duration::minutes(1),
        &e,
        "Here is what I found.",
    );
    assert!(
        h.sql_at_phase.is_none(),
        "check of the setup: the end of the errand is written in a step of its own, and \
         the other write landed before it: {:?}",
        h.stderr
    );
    let row: Value = sj::from_str(&h.state(&key)).unwrap_or(Value::Null);
    assert_eq!(
        row["answered"],
        json!(1),
        "the answer is said at the row although its first write took nothing: {row}"
    );
    assert_eq!(
        row["ended"],
        json!("failed"),
        "the other writer's word stands beside it: {row}"
    );

    // Past the deadline, the next turn files no outcome beside the answer.
    let call = ask(&mut h, start() + Duration::minutes(10), "p02");
    assert!(
        outcome_lines(&call, &e).is_empty() && outcome_rows(&h, &e).is_empty(),
        "an answered errand has no outcome: {:?} {:?}",
        outcome_lines(&call, &e),
        outcome_rows(&h, &e)
    );
}
