//! GH #1494 (D58 close, recall review M4) -- a decision on a recall pair that
//! the plan did not keep is made again, the same way.
//!
//! WHY this file exists: "decided once, and it stays" rests on the plan the
//! tap writes after the answer (`t-plan`, compare-and-set). Two ways leave
//! the decision unwritten: no answer comes back, or a rebuild wrote the plan
//! in between and the tap's write loses. The next call then finds the pair
//! as rows the plan holds no block of -- it must decide again and leave the
//! pair out again, never send it as a plain new row in the middle of the
//! window. Both ways are pinned here.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, json};

const RECALL_ID: &str = "call_recall_0000000000close30";
const SAID: &str = "My sister Anna moved to Hamburg.";

/// `t1` says [`SAID`]; `t2` carries a recall pair that quotes it and is
/// answered.
fn two_turns() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    turn(&mut h, "s1", "t1", SAID, "Hamburg is lovely.", json!({}));
    let pair = json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("She likes it there.")]));
    h
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

fn left_out(h: &Hive) -> Vec<sj::Value> {
    h.plan_in(ROUND_E)["left_out"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The call after the deciding one: the pair stays out, the person's words
/// stand once; after its answer the plan names the pair.
fn the_next_call_leaves_it_out_again(h: &mut Hive) {
    let c4 = h.curate("s1", "t4", 0, json!([user("Go on.")]), mode("Be brief."));
    let t = text(&c4);
    assert!(!t.contains(RECALL_ID), "the pair stays out: {t}");
    assert_eq!(t.matches(SAID).count(), 1, "once, as the raw turn: {t}");
    h.tap(&c4, "stop", json!({}), json!([said("Fine.")]));
    assert_eq!(
        left_out(h).len(),
        2,
        "the decision of the call after is kept: the result and its call"
    );
}

/// Pin: the deciding call gets no answer.
#[test]
fn without_an_answer_the_next_call_decides_again() {
    if !shipped() {
        return;
    }
    let mut h = two_turns();
    let c3 = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    assert!(
        !text(&c3).contains(RECALL_ID),
        "check of the setup: the call decides: {}",
        text(&c3)
    );
    assert!(
        left_out(&h).is_empty(),
        "check of the setup: without an answer the plan names nothing"
    );
    the_next_call_leaves_it_out_again(&mut h);
}

/// Pin: the tap of the deciding call loses its write to a plan written in
/// between.
#[test]
fn a_lost_plan_write_is_decided_again() {
    if !shipped() {
        return;
    }
    let mut h = two_turns();
    let c3 = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    assert!(!text(&c3).contains(RECALL_ID), "{}", text(&c3));
    // Another writer of the round's plan lands right before the tap's
    // compare-and-set: the same plan, a newer moment.
    let mut plan = h.plan_in(ROUND_E);
    let at = plan["as_of_ms"].as_i64().unwrap_or(0) + 1;
    plan["as_of_ms"] = json!(at);
    h.sql_at_phase = Some((
        "done".to_string(),
        format!(
            "UPDATE state SET value = '{}' WHERE key = 'window_plan:{}'",
            canonical(&plan).replace('\'', "''"),
            round_key(ROUND_E)
        ),
    ));
    h.tap(&c3, "stop", json!({}), json!([said("She moved in March.")]));
    assert!(
        h.sql_at_phase.is_none(),
        "check of the setup: the other write landed"
    );
    assert!(
        left_out(&h).is_empty(),
        "check of the setup: the tap lost its write, the plan names nothing: {}",
        h.plan_in(ROUND_E)
    );
    the_next_call_leaves_it_out_again(&mut h);
}
