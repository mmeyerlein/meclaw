//! D58 target (GH #1483, contract point 7) -- a full-tree call clears every
//! delta target.
//!
//! WHY this file exists: a target with `system: delta` is sent only the
//! families of the system part that moved; its cell holds the rest. What it
//! was sent last is kept per target (`system_hash_sent`). The rule of before
//! stays general: a call that sends the whole tree clears what every delta
//! target was sent, so the next call of each of them carries the whole tree
//! again. Two delta targets and one full one: after the full call, both
//! delta targets get every family once more.
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
use meclaw_core::serde_json::json;

fn families(c: &Msg) -> Vec<String> {
    c.body
        .get("system")
        .and_then(|s| s.as_object())
        .map(|s| s.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn gh_d58_target_a_full_tree_call_clears_every_delta_target() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(&mut h, "left", json!({"system": "delta"}));
    announce_profile(&mut h, "right", json!({"system": "delta"}));
    announce_profile(&mut h, "whole", json!({"system": "full"}));

    let c = turn_for(&mut h, "left", "t1", TALK[0].0, TALK[0].1);
    // The whole tree: what the first call of a target carries.
    let all = families(&c);
    assert!(
        all.len() >= 2,
        "check of the setup: a system part of several families: {all:?} {:?}",
        h.stderr
    );
    turn_for(&mut h, "right", "t2", TALK[1].0, TALK[1].1);
    let c = turn_for(&mut h, "left", "t3", TALK[2].0, TALK[2].1);
    assert!(
        families(&c).len() < all.len(),
        "check of the setup: a delta target is sent what moved only: {:?} {:?}",
        c.body.get("system"),
        h.stderr
    );

    let c = turn_for(&mut h, "whole", "t4", TALK[3].0, TALK[3].1);
    assert_eq!(families(&c), all, "the full target gets the whole tree");

    let c = turn_for(&mut h, "left", "t5", "And tomorrow?", "Rain.");
    assert_eq!(
        families(&c),
        all,
        "after a full call the first delta target gets the whole tree again"
    );
    let c = turn_for(&mut h, "right", "t6", "And the day after?", "Sun.");
    assert_eq!(
        families(&c),
        all,
        "and so does the second -- every delta target is cleared"
    );
}
