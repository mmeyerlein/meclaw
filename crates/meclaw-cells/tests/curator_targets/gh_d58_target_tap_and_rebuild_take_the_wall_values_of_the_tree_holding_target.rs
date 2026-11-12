//! D58 target (GH #1483, planner's addendum of 11.11.) -- tap and rebuild
//! take the wall values of the tree-holding target.
//!
//! WHY this file exists: the wall belongs to every target of a curator at
//! once, so a value the tap of an answer or a rebuild reads has one source
//! per value. It is the target that holds its tree (`system: delta`): the
//! target of the call itself when it holds the tree, else the one reported
//! target that does. A value the cut of a call reads stays the call's. Here
//! the main target holds the tree and names a wall target of 20 tokens; the
//! fast target holds none and names nothing of the wall. After the fast
//! target's answer the tap orders a rebuild of the wall's own
//! (`curate_mark` "target") -- the main target's value -- and the rebuild
//! reads the main target's catalogue row, not the fast one's.
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
fn gh_d58_target_tap_and_rebuild_take_the_wall_values_of_the_tree_holding_target() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(
        &mut h,
        "main",
        json!({"system": "delta", "window_target": 20}),
    );
    announce_profile(&mut h, "fast", json!({"system": "full", "keep_recent": 4}));
    turn_for(&mut h, "main", "t1", TALK[0].0, TALK[0].1);
    turn_for(&mut h, "main", "t2", TALK[1].0, TALK[1].1);

    let c = say(&mut h, Some("fast"), "t3", TALK[2].0);
    let since = h.clock.len();
    answer(&mut h, &c, "stop", wall_usage(), json!([said(TALK[2].1)]));
    let adds = adds_since(&h, since);
    let last = adds.last().unwrap_or_else(|| {
        panic!(
            "check of the setup: the answer armed an order: {:?}",
            h.stderr
        )
    });
    assert_eq!(
        last.body["emit_body"]["curate_mark"],
        json!("target"),
        "the tap after the fast answer reads the main target's wall target: {:?}",
        last.body
    );

    let before = h.ledger_ops.len();
    h.fire(last);
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
        ops.contains("catalog:main"),
        "the rebuild reads the row of the tree-holding target: {ops}"
    );
    assert!(
        !ops.contains("catalog:fast") && !ops.contains("catalog:brain"),
        "and no other: {ops}"
    );
}
