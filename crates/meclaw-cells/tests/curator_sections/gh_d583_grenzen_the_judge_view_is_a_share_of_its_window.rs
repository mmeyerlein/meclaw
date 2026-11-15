//! GH #1526 (d58-3 grenzen, N6) -- the view a judge asks for is a share of
//! the judge's own window.
//!
//! WHY this file exists: `./history` sized every `view` by the asker's
//! `budget_tokens`, 1 500 when none came and never over 4 000 -- two fixed
//! numbers, whatever window the judging model has. A judge with a large
//! window saw no more of the conversation than one with a small window, and
//! one with a small window could be handed more than it holds. The rule
//! (contract point 2): the view is `view_share` x the window of the target
//! it is asked for (the catalog row `./policy` reports to `./history`), and
//! no cap of 4 000. Two targets with two windows here: the large one gets a
//! view past the old cap, the small one less than the old default.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

/// The old cap of a view, in tokens, at the core's three characters a token.
const OLD_CAP_CHARS: usize = 4000 * 3;
/// The old default of a view without `budget_tokens`, in characters.
const OLD_DEFAULT_CHARS: usize = 1500 * 3;
/// Turns of the person, each this long.
const TURNS: usize = 8;
const TURN_CHARS: usize = 3000;

/// The talky hive with eight long turns on the wall and two targets whose
/// catalog rows name a large and a small window.
fn conversation() -> Hive {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    h.report_row(
        "talky",
        "decide",
        json!({"model": "test/large", "input_soft": 40000, "input_hard": 80000,
               "chars_per_token": 3}),
    );
    h.report_row(
        "talky",
        "quick",
        json!({"model": "test/small", "input_soft": 4000, "input_hard": 8000,
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

/// The characters of the messages of the one `view` the hive answers for
/// `target`, asked without a token number.
fn view_chars(h: &mut Hive, target: &str) -> usize {
    h.out.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t99", "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": target, "turns": TURNS}}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    let msgs = views[0].body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    msgs.iter()
        .map(|m| m["text"].as_str().map(str::len).unwrap_or(0))
        .sum()
}

#[test]
fn the_judge_view_is_a_share_of_its_window() {
    if !shipped() {
        return;
    }
    let mut h = conversation();
    let large = view_chars(&mut h, "decide");
    assert!(
        large > OLD_CAP_CHARS,
        "a judge of 40 000 tokens sees past the old cap of 4 000 tokens: {large} chars"
    );
    let small = view_chars(&mut h, "quick");
    assert!(
        small < OLD_DEFAULT_CHARS,
        "a judge of 4 000 tokens sees less than the old default of 1 500 tokens: {small} chars"
    );
    assert!(small > 0, "and still sees the conversation");
}
