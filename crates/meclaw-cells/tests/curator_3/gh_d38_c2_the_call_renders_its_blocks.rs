//! GH #1448 (D38 C2, Z1.10) -- the call renders the blocks of its plan.
//!
//! WHY this file exists: the window plan is built ahead of the turn (after
//! the answer, at a rebuild) as a list of blocks, and the turn only appends
//! its new ones. The messages of a call are rendered from that list, so the
//! list has to hold every wall row the window reads: a row that lands below
//! the plan's `blocks_to` (an earlier stamp, written after the plan) is a
//! new block of the call like the person's words -- never left out in
//! silence, never sent without its block in the call's snapshot. A plan
//! that holds every row renders the window of before, byte for byte (the
//! c0 corpus holds that side).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const FIRST_ANSWER: &str = "the first answer stands here";
const LATE: &str = "a late row of an earlier stamp";

#[test]
fn a_plan_that_holds_every_row_renders_the_whole_wall() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t1", "first words", FIRST_ANSWER, json!({}));
    let call = turn(&mut h, "s1", "t2", "second words", "a2", json!({}));
    let texts = texts(&call);
    for want in ["first words", FIRST_ANSWER, "second words"] {
        assert!(texts.iter().any(|t| t.contains(want)), "{want}: {texts:?}");
    }
}

#[test]
fn a_row_below_the_plans_end_is_a_block_of_the_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t1", "first words", FIRST_ANSWER, json!({}));
    let to = h.plan_in(ROUND_E)["blocks_to"].as_i64().unwrap_or(0);
    let (seq, _) = h.row(
        chrono::Utc::now() - chrono::Duration::seconds(3600),
        "s1",
        "t0",
        "user",
        &user(LATE),
        0,
    );
    assert!(seq < to, "the row lands below the plan's end: {seq} {to}");
    let call = turn(&mut h, "s1", "t2", "second words", "a2", json!({}));
    let texts = texts(&call);
    assert!(
        texts.iter().any(|t| t.contains(LATE)),
        "the late row is in the window: {texts:?}"
    );
    let snap = snapshot_of(&h, &call).expect("the call's plan snapshot");
    let late = block_id("words", &user(LATE));
    assert!(
        block_ids(&snap).contains(&late),
        "the late row is a block of the call: {late} {snap:?}"
    );
}
