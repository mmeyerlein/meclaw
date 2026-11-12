//! D58 target (GH #1483, planner's addendum of 11.11.) -- without one
//! tree-holding target the instance knobs rule the wall.
//!
//! WHY this file exists: the wall's values come from the target that holds
//! its tree. When no reported target holds it, or several do and the call
//! goes to none of them, there is no such source: the knobs of the curator
//! instance rule the wall, and the curator books a mark (`target_unknown`,
//! `missing` "wall") -- no new state. A composite with several tree-holding
//! targets and others sets its wall values on the instance. Shown on the
//! wall target: the instance says 20 tokens, the tree-holding targets say
//! 0 (off). An answer of a third target orders the wall's own rebuild
//! (`curate_mark` "target", the instance's value) and is marked; an answer
//! of one of the tree holders takes its own 0 and orders none.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

/// The `curate_mark` of the newest order the answer of a call for `target`
/// armed, and the `target_unknown` marks that say `wall`.
fn after_answer(h: &mut Hive, target: &str, turn: &str) -> (Value, usize) {
    let c = say(h, Some(target), turn, TALK[2].0);
    let since = h.clock.len();
    answer(h, &c, "stop", wall_usage(), json!([said(TALK[2].1)]));
    let mark = adds_since(h, since)
        .last()
        .map(|m| m.body["emit_body"]["curate_mark"].clone())
        .unwrap_or(Value::Null);
    let wall = marks_of_kind(h, TARGET_MARK)
        .iter()
        .filter(|m| m["missing"] == json!("wall"))
        .count();
    (mark, wall)
}

fn hive(holders: &[&str]) -> Hive {
    let mut h = Hive::with(&[("policy", "window_target", json!(20))]);
    for t in holders {
        announce_profile(&mut h, t, json!({"system": "delta", "window_target": 0}));
    }
    announce_profile(&mut h, "fast", json!({"system": "full"}));
    for (i, t) in holders.iter().enumerate() {
        turn_for(&mut h, t, &format!("h{i}"), TALK[0].0, TALK[0].1);
    }
    h
}

#[test]
fn gh_d58_target_without_one_tree_holding_target_the_instance_knobs_rule_the_wall() {
    if !shipped() {
        return;
    }
    for holders in [vec![], vec!["left", "right"]] {
        let mut h = hive(&holders);
        let (mark, wall) = after_answer(&mut h, "fast", "t9");
        assert_eq!(
            mark,
            json!("target"),
            "{holders:?}: the instance's wall target rules: {:?}",
            h.stderr
        );
        assert_eq!(wall, 1, "{holders:?}: one mark says the wall had no source");
    }
    let mut h = hive(&["left", "right"]);
    let (mark, wall) = after_answer(&mut h, "left", "t9");
    assert_ne!(
        mark,
        json!("target"),
        "a tree holder's own call takes its own wall values (off): {:?}",
        h.stderr
    );
    assert_eq!(wall, 0, "its own source: no mark");
}
