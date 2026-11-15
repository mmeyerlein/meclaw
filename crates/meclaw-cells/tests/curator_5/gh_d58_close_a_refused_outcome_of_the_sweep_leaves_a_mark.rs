//! GH #1469 (wave d58-2 close, inventory 42; errand review m1) -- a refused
//! outcome of the sweep leaves a mark.
//!
//! WHY this file exists: past its deadline the sweep takes the parked row of
//! an errand without an answer and files its outcome in the answer of the
//! delete (OR-d58-2.errand.14 and .15: delete first, then the outcome). When
//! the store refuses that write, the row is gone and no later sweep or
//! curate files the outcome again; the loss was said on stderr alone. The
//! order stays (OR-d58-2.errand.18); the loss is now booked in the ledger as
//! one `store_error` mark of the step `swept` that names the refused legs --
//! the rule a refused delete already follows (GH #1225).
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

/// The values of the `store_error` marks in the ledger.
fn store_errors(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'store_error' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
        .collect()
}

#[test]
fn a_refused_outcome_of_the_sweep_leaves_a_store_error_mark() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[("policy", "keep_recent", json!(1))]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    let key = park_key(&e.label);

    // Twenty minutes on, the sweep takes the row; the store refuses the wall
    // row of its outcome.
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("eo-w0".to_string(), "store_error".to_string())];
    strike(&mut h, start() + Duration::minutes(20), "b1");

    assert!(
        h.refuse_legs.is_empty(),
        "check of the setup: the outcome of the sweep met the refusal: {:?}",
        h.stderr
    );
    assert!(
        swept_keys(&h).contains(&key),
        "check of the setup: the sweep took the parked row: {:?}",
        swept_keys(&h)
    );
    assert!(
        outcome_rows(&h, &e).is_empty(),
        "the refused wall row is not there: {:?}",
        outcome_rows(&h, &e)
    );
    let marks = store_errors(&h);
    let swept: Vec<&Value> = marks
        .iter()
        .filter(|m| m["step"] == json!("swept"))
        .collect();
    assert_eq!(
        swept.len(),
        1,
        "one `store_error` mark of the step `swept` says the loss: {marks:?} {:?}",
        h.stderr
    );
    assert_eq!(
        swept[0]["legs"],
        json!({"eo-w0": "store_error"}),
        "the mark names the refused leg of the outcome: {marks:?}"
    );

    // A later sweep files nothing more and marks nothing more.
    strike(&mut h, start() + Duration::minutes(40), "b2");
    assert_eq!(
        store_errors(&h)
            .iter()
            .filter(|m| m["step"] == json!("swept"))
            .count(),
        1,
        "the loss is said once: {:?}",
        store_errors(&h)
    );
}
