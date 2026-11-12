//! D58 target (GH #1483, contract point 4) -- a rebuild counts with the
//! target that ordered it.
//!
//! WHY this file exists: a rebuild starts on a strike of the clock, which
//! carries no usage; it priced its claim at a catalogue row read under a
//! fixed name (`catalog:brain`), whichever model's call had ordered it.
//! Planner's addendum of 11.11.: the wall's values come from the target
//! that holds its tree (`system: delta`, like `strong` in simple-agent).
//! Here the target that orders the rebuild is that target: the order it
//! leaves at the clock carries its name (`emit_body.target`), and the
//! rebuild reads its row -- measured at the store: the probe asks for
//! `catalog:<target>` and for no fixed name and no other target's row. The
//! answer of a target without a tree is the case of
//! `gh_d58_target_tap_and_rebuild_take_the_wall_values_of_the_tree_holding_target`.
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
use meclaw_core::serde_json::{self as sj, json};

#[test]
fn gh_d58_target_a_rebuild_counts_with_the_target_that_ordered_it() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(&mut h, "north", json!({"system": "full"}));
    announce_profile(&mut h, "south", json!({"system": "delta"}));
    turn_for(&mut h, "north", "t1", TALK[0].0, TALK[0].1);
    turn_for(&mut h, "south", "t2", TALK[1].0, TALK[1].1);
    let order = h.clock_order_for(ROUND_E).unwrap_or_else(|| {
        panic!(
            "check of the setup: the last answer left an order: {:?}",
            h.stderr
        )
    });
    assert_eq!(
        order.body["emit_body"]["target"],
        json!("south"),
        "the order carries the target of the call that left it: {:?}",
        order.body
    );
    let before = h.ledger_ops.len();
    h.fire(&order);
    while !h.summ.is_empty() {
        h.answer("A summary.", "stop");
    }
    let ops = sj::to_string(
        &h.ledger_ops[before..]
            .iter()
            .map(|(_, op)| op.clone())
            .collect::<Vec<_>>(),
    )
    .expect("json");
    assert!(
        ops.contains("catalog:south"),
        "the rebuild reads the row of the target that ordered it: {ops}"
    );
    assert!(
        !ops.contains("catalog:brain") && !ops.contains("catalog:north"),
        "and no other row: {ops}"
    );
}
