//! d58-3 namen (GH #1525, contract point 1, R-583-1 form B) -- a new need
//! that recalls costs no code.
//!
//! WHY this file exists: the policy skipped the recall of a stamped turn
//! when `memory` was not among its needs (`recall_unneeded`) and when the
//! stamp surely said no to `memory` (`recall_sure_no`) -- one need name of
//! one composition, written into the code. A composition whose recall hangs
//! on another need (`files`) had no recall at all, or needed a change under
//! `templates/curator/`. The names come from the param `recall_needs` now:
//! a composition that sets `["memory", "files"]` keeps the recall for a
//! stamp with `files` as for one with `memory`, and the sure no reads the
//! yes-value of every listed name from `needs_yes`.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`); no
//! model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The knob a composition with a second recalling need sets.
fn two_needs() -> Value {
    json!({"recall_needs": ["memory", "files"]})
}

fn unneeded(hop: Value) -> Value {
    let (got, err) = policy_scope(two_needs(), "recall_unneeded(ARGS, {})", hop);
    assert!(got.is_boolean(), "a yes or no: {got} {err}");
    got
}

fn sure_no(hop: Value, bound: f64) -> Value {
    let (got, err) = policy_scope(
        two_needs(),
        "recall_sure_no(ARGS[0], ARGS[1])",
        json!([hop, bound]),
    );
    assert!(
        got.is_null() || got.is_number(),
        "a yes-value or none: {got} {err}"
    );
    got
}

#[test]
fn gh_d583_namen_a_new_need_that_recalls_costs_no_code() {
    if !shipped() {
        return;
    }
    // A stamp whose needs name `files`: the recall stays.
    assert_eq!(
        unneeded(json!({"path": "chat", "needs": "files"})),
        json!(false),
        "a stamp with the listed need `files` keeps its recall"
    );
    assert_eq!(
        unneeded(json!({"path": "chat", "needs": "memory"})),
        json!(false),
        "a stamp with `memory` keeps its recall as before"
    );
    // Control: a need no name of the list names lets the recall go.
    assert_eq!(
        unneeded(json!({"path": "chat", "needs": "weather"})),
        json!(true),
        "a stamp without a listed need skips the recall"
    );
    // The sure no: `files` among the needs is never a sure no.
    assert_eq!(
        sure_no(
            json!({"path": "chat", "needs": "files",
                   "needs_yes": "{\"memory\": 0.05, \"files\": 0.6}"}),
            0.3
        ),
        Value::Null,
        "a stamp that names `files` is no sure no"
    );
    // Neither listed need named, both yes-values at most the bound: skip.
    let got = sure_no(
        json!({"path": "chat", "needs": "",
               "needs_yes": "{\"memory\": 0.05, \"files\": 0.1}"}),
        0.3,
    );
    assert!(
        got.as_f64().is_some_and(|v| v <= 0.3),
        "`files` with a yes at most the bound is a sure no: {got}"
    );
}
