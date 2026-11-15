//! GH #1526 (d58-3 grenzen, N6) -- `./history` learns the window of a target
//! by a message of `./policy`, never by reading its state.
//!
//! WHY this file exists: to size a judge's view by the judge's window
//! (contract point 2) `./history` must know the catalog row of every target.
//! `./policy` holds those rows (lane `in_catalog`); `./history` reached
//! nothing of it -- no edge led from `./policy` to `./history`, and a read of
//! the policy's `catalog:<target>` state through the ledger would make one
//! cell's state another's input behind the contract. The rule: on every new
//! or changed row `./policy` sends the target's `input_soft` and
//! `chars_per_token` on an edge of its own to `./history`, which keeps them
//! as its table of limits. Here a row that changes changes the view, and the
//! edge and the history's description name the lane.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const TURNS: usize = 8;

fn hive_config() -> Value {
    read_json(&repo("templates/curator/config.json"))
}

/// The view of `decide` asked without a token number: its characters.
fn view_chars(h: &mut Hive) -> usize {
    h.out.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t99", "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "decide", "turns": TURNS}}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    views[0].body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| m["text"].as_str().map(str::len).unwrap_or(0))
        .sum()
}

#[test]
fn the_history_learns_limits_by_message() {
    if !shipped() {
        return;
    }
    // The edge: `./policy` -> `./history`, on a lane the history names.
    let cfg = hive_config();
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let edge = edges
        .iter()
        .find(|e| e["from"] == json!("./policy") && e["to"] == json!("./history"))
        .unwrap_or_else(|| panic!("an edge from ./policy to ./history carries the limits"));
    let cond = edge["condition"].as_str().unwrap_or("").to_string();
    let lane = cond
        .split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("the edge names its lane: {cond}"))
        .to_string();
    let meaning = cell_config("history")["description"]["consumes_meaning"].to_string();
    assert!(
        meaning.contains(&lane),
        "the history's description names the lane `{lane}`: {meaning}"
    );
    // The history never reads the policy's catalog state.
    assert!(
        !script_of("history").contains("catalog:"),
        "./history reads no `catalog:<target>` state of ./policy"
    );

    // A row that changes changes the view.
    let mut h = Hive::with(&[]).reporting_composite("talky");
    for i in 1..=TURNS {
        let text = format!("{}{}", i, "v".repeat(3000));
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
    h.report_row(
        "talky",
        "decide",
        json!({"model": "test/large", "input_soft": 40000, "input_hard": 80000,
               "chars_per_token": 3}),
    );
    let large = view_chars(&mut h);
    h.report_row(
        "talky",
        "decide",
        json!({"model": "test/small", "input_soft": 4000, "input_hard": 8000,
               "chars_per_token": 3}),
    );
    let small = view_chars(&mut h);
    assert!(
        small < large,
        "the second row, a tenth of the window, shrinks the view: {large} -> {small}"
    );
}
