//! GH #1526 (d58-3 grenzen, N6, review minor 3) -- a `view_share` out of
//! (0, 1] does not lift the bound of a view.
//!
//! WHY this file exists: `view_share` is a set knob without bounds at the
//! door. A set value of `0` or `1.5` made the share of the view None, the
//! view lost its budget and the turns came whole -- a judge with a small
//! window was handed the whole conversation, the case the strand removes.
//! The rule: a `view_share` out of (0, 1] stands for the shipped share and is
//! named on stderr; the view is the one the shipped share gives.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// Turns of the person, each this long: together far past any share of the
/// target's window below.
const TURNS: usize = 8;
const TURN_CHARS: usize = 3000;

/// The talky hive with `over` on the history, eight long turns on the wall
/// and one target whose catalog row names a window.
fn conversation(over: Option<Value>) -> Hive {
    let mut h = match over {
        Some(v) => Hive::with(&[("history", "view_share", v)]),
        None => Hive::with(&[]),
    }
    .reporting_composite("talky");
    h.report_row(
        "talky",
        "decide",
        json!({"model": "test/judge", "input_soft": 8000, "input_hard": 16000,
               "chars_per_token": 3}),
    );
    for i in 1..=TURNS {
        let text = format!("{}{}", i, "w".repeat(TURN_CHARS));
        let call = h.curate(
            "s1",
            &format!("t{i}"),
            0,
            json!([user(&text)]),
            mode("Be brief."),
        );
        h.tap(
            &call,
            "stop",
            json!({"input_soft": 250000}),
            json!([said("ok")]),
        );
    }
    h
}

/// The one `view` the hive answers for the target, asked without a share.
fn view(h: &mut Hive) -> Value {
    h.out.clear();
    h.stderr.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t99", "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "decide", "turns": TURNS}}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    Value::Object(views[0].body.clone())
}

#[test]
fn a_view_share_out_of_range_takes_the_shipped_share() {
    if !shipped() {
        return;
    }
    let shipped_view = view(&mut conversation(None));
    let shipped_budget = shipped_view["budget_tokens"].as_i64();
    assert!(
        shipped_budget.is_some_and(|b| b > 0),
        "the shipped share sizes the view: {shipped_view}"
    );
    assert_eq!(
        shipped_view["truncated"],
        json!(true),
        "the shipped share holds less than the eight turns: {shipped_view}"
    );
    for bad in [json!(0), json!(1.5), json!(-0.2)] {
        let mut h = conversation(Some(bad.clone()));
        let got = view(&mut h);
        assert_eq!(
            got["budget_tokens"].as_i64(),
            shipped_budget,
            "view_share {bad} sizes the view by the shipped share: {got}"
        );
        assert_eq!(
            got["messages"], shipped_view["messages"],
            "view_share {bad} hands the turns of the shipped share, not all of them"
        );
        assert!(
            h.stderr.iter().any(|l| l.contains("view_share")),
            "view_share {bad} is named on stderr: {:?}",
            h.stderr
        );
    }
}
