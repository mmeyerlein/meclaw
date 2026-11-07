//! GH #1448 (D38 C2, R-38-4 Nr. 2) -- a turn only appends its new blocks.
//!
//! WHY this file exists: the plan of the window is built ahead of the turn
//! (after the answer, at a rebuild, at the clock). The turn itself adds only
//! what is new -- the person's words, the legs of its own call -- and never
//! builds the blocks of before again: the next answer leaves the blocks the
//! plan held byte for byte in their place, and the call's snapshot is the
//! plan of before plus the turn's own words (and the call's own system
//! part, state line and notes, rebuilt each call).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;
use std::collections::BTreeSet;

#[test]
fn the_next_answer_keeps_the_blocks_of_before_in_place() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let first = plan_blocks(&h, ROUND_E);
    assert!(
        !first.is_empty(),
        "the plan holds the first turn: {}",
        h.plan_in(ROUND_E)
    );
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    let second = plan_blocks(&h, ROUND_E);
    assert!(
        second.len() >= first.len() && second[..first.len()] == first[..],
        "the blocks of before stay in place, byte for byte:\n{first:?}\n{second:?}"
    );
    let added: BTreeSet<String> = block_ids(&second[first.len()..]).into_iter().collect();
    let want: BTreeSet<String> = [
        block_id("words", &user("q2")),
        block_id("answer", &said("a2")),
    ]
    .into_iter()
    .collect();
    assert_eq!(added, want, "the second turn adds its own two blocks");
}

#[test]
fn the_call_is_the_plan_of_before_and_the_persons_words() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let before: BTreeSet<String> = block_ids(&plan_blocks(&h, ROUND_E)).into_iter().collect();
    assert!(!before.is_empty(), "{}", h.plan_in(ROUND_E));
    let call = turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    let snap = snapshot_of(&h, &call).expect("the call's plan snapshot");
    let ids: BTreeSet<String> = block_ids(&snap).into_iter().collect();
    assert!(
        before.is_subset(&ids),
        "the call sends the plan of before: {before:?} {ids:?}"
    );
    // OR-d38-3.o10.2: the call's own blocks (system part, state line, notes)
    // are rebuilt each call and never part of the plan.
    let own = ["frame:", "state:", "note:"];
    let new: BTreeSet<String> = ids
        .difference(&before)
        .filter(|i| !own.iter().any(|k| i.starts_with(k)))
        .cloned()
        .collect();
    assert_eq!(
        new,
        [block_id("words", &user("q2"))].into_iter().collect(),
        "the turn appends only its words"
    );
}
