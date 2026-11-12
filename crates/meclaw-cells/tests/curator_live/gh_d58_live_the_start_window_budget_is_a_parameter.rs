//! D58 live (GH #1482, plan `u-live.md` contract point 5) -- the size of
//! the start window is a parameter.
//!
//! WHY this file exists: `./history` cut every start window (`seed`) to a
//! fixed 8192 tokens (`SEED_BUDGET`), whatever model the session runs. A
//! number of the code is no knob a composition can turn (R-IG-1: numbers
//! from a catalog or a set, never fixed in a producer). It becomes the param
//! `seed_budget` of `curator/history`, shipped with the same value, so the
//! window is unchanged until a set says otherwise. A catalog row cannot size
//! it yet: the ask for a start window names no target, and the voice cell
//! reports no row (the close names it as open for the voice domain).
//!
//! The hive in one process (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::json;

/// The window's turns: every message of the seed that is not a line of the
/// curator's own (role `developer`).
fn turns_of(seed: &Msg) -> usize {
    seed.messages()
        .iter()
        .filter(|m| m["role"] != json!("developer"))
        .count()
}

#[test]
fn gh_d58_live_the_start_window_budget_is_a_parameter() {
    if !shipped() {
        return;
    }
    let history = cell_config("history");
    assert_eq!(
        history["params"]["seed_budget"],
        json!(8192),
        "templates/curator/history/config.json declares seed_budget, shipped 8192"
    );
    let setting = &history["contract"]["settings"]["seed_budget"];
    assert_eq!(
        setting["default"],
        json!(8192),
        "the setting's default is the param"
    );
    assert_eq!(setting["type"], json!("number"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );

    // The knob cuts the window: six exchanges of about a hundred tokens a
    // row stand in the shipped window, a budget of 150 holds at most one.
    let window = |budget: Option<i64>| {
        let over: Vec<(&str, &str, meclaw_core::serde_json::Value)> = budget
            .map(|b| vec![("history", "seed_budget", json!(b))])
            .unwrap_or_default();
        let mut h = live_hive(&over);
        clear_of_midnight();
        for i in 0..6i64 {
            let words = format!("{i} {}", "the roses by the old garden wall ".repeat(9));
            h.row(
                backdated(50 - 2 * i),
                "s1",
                &format!("t{i}"),
                "user",
                &user(&words),
                1,
            );
            h.row(
                backdated(49 - 2 * i),
                "s1",
                &format!("t{i}"),
                "assistant",
                &said(&words),
                1,
            );
        }
        let seeds = ask_seed(&mut h, &live_keys(LIVE), "start");
        assert_eq!(seeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
        turns_of(&seeds[0])
    };
    let shipped_turns = window(None);
    let small = window(Some(150));
    assert!(
        shipped_turns >= 6,
        "the shipped budget carries the six exchanges: {shipped_turns}"
    );
    assert!(
        small <= 1,
        "seed_budget 150 holds at most one row of about a hundred tokens: {small}"
    );
}
