//! d58-3 nachlese (GH #1539, re-check m2 on review I1) -- the rows a recall
//! block first lost while a rebuild ran stay lost after that rebuild.
//!
//! WHY this file exists: a rebuild builds every recall block of an earlier
//! round anew and took the rows each block left out (`rows_out`) only from
//! the plan it started from -- the plan as it stood at its claim. A call that
//! runs while the summarizer works decides a block for the first time and
//! its tap writes the rows into the plan that stands; the rebuild then wrote
//! its own plan over it, the block `decided` without rows, and every call
//! after sent it neither cut nor held -- the quotes of the wall raw in the
//! window -- until the next rebuild.
//!
//! Two ways lead there. (1) The block is of the newest round of the claim:
//! `rebuild_blocks` leaves it undecided, the next call decides it anew and
//! cuts the same rows. (2) The block is of an older round and was still
//! undecided at the claim: a call of a target whose profile sets `recall`
//! false sends no recall row, so it decides no recall block; a round of such
//! a target between the recall round and the claim leaves the block
//! undecided and no longer of the newest round, and `rebuild_blocks` marks
//! it `kept` without rows. The rebuild therefore takes the rows of its
//! decided blocks from the plan that stands when it ends (`held_rows_of`,
//! the plan read with the rebuild's end); the second pin is red without
//! that and green with it.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;
use curator_target::{announce_profile, turn_for};
use meclaw_core::serde_json::{Value, json};

const RECALL_ID: &str = "call_recall_0000000000nlrebe";
const ASKED: &str = "Where did Rita go last week?";
const ANSWER: &str = "Rita went to the Baltic Sea with her daughter.";
const FACT: &str = "Rita visited = the Baltic Sea";

fn recalled() -> String {
    format!("WHAT I KNOW\n  {FACT}\nWHAT WAS SAID\n  user: \"{ASKED}\"\n  assistant: \"{ANSWER}\"")
}

/// The recall result as `call` sent it.
fn recall_of(call: &Msg) -> Value {
    call.messages()
        .into_iter()
        .find(|m| m["type"] == "tool_result" && m["id"] == RECALL_ID)
        .unwrap_or(Value::Null)
}

/// The recall blocks of the plan that stands that carry `rows_out`.
fn held_rows(t: &Talk) -> Vec<Value> {
    plan_blocks(&t.h, ROUND_E)
        .into_iter()
        .filter(|b| b["kind"] == json!("recall") && b.get("rows_out").is_some())
        .collect()
}

fn seq_of(t: &Talk, turn: &str, agg: &str) -> i64 {
    t.h.rows(&format!(
        "SELECT {agg}(seq) FROM wall WHERE turn_id = '{turn}'"
    ))
    .first()
    .and_then(|r| r[0].as_i64())
    .unwrap_or_else(|| panic!("the rows of {turn}"))
}

/// Pin: the clock strikes and the summarizer is asked; before it answers,
/// the call of `t3` decides the mixed recall block of `t2` for the first
/// time (its quotes out) and its tap writes that. The rebuild ends after
/// it -- the next call still sends the fact and no quote of the wall.
#[test]
fn gh_d583_nachlese_rows_out_decided_while_a_rebuild_runs_survive_it() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(3)),
    ]);
    for i in 1..=6 {
        t.turn(
            &format!("f{i}"),
            &format!("The person tells item {i}."),
            &format!("Noted item {i}."),
        );
    }
    t.turn("t1", ASKED, ANSWER);
    let c2 = t.call(
        "t2",
        0,
        json!([
            user("What do you know about Rita?"),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(RECALL_ID, &recalled())
        ]),
    );
    t.answer(&c2, "stop", json!([said("She was at the sea.")]));
    assert!(
        held_rows(&t).is_empty(),
        "check of the setup: no block has lost rows before the claim: {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    // The clock strikes: claim, plan, the summarizer asked -- not answered.
    let order = t
        .h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("check of the setup: an order at the clock: {:?}", t.h.clock));
    t.h.fire(&order);
    assert!(
        !t.h.summ.is_empty(),
        "check of the setup: the summarizer is asked: {:?}",
        t.h.stderr
    );
    // A call while the summarizer works: it decides the block of t2.
    let a = t.call("t3", 0, json!([user("Tell me more.")]));
    let first = recall_of(&a);
    let shown = first["text"].as_str().unwrap_or_default().to_string();
    assert!(
        shown.contains(FACT) && !shown.contains(ASKED) && !shown.contains(ANSWER),
        "check of the setup: the block goes without its quoted rows: {first}"
    );
    t.answer(&a, "stop", json!([said("That is all I know.")]));
    assert!(
        !held_rows(&t).is_empty(),
        "check of the setup: the tap wrote the rows into the plan that stands: {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    // The summarizer answers: the rebuild ends.
    t.h.answer("- The person told items 1 to 6.", "stop");
    assert!(
        t.cover() > 0 && t.cover() < seq_of(&t, "t1", "min"),
        "check of the setup: the rebuild covers the items, not the quoted round: cover {} \
         {:?}",
        t.cover(),
        t.h.stderr
    );
    let recall: Vec<Value> = plan_blocks(&t.h, ROUND_E)
        .into_iter()
        .filter(|b| b["kind"] == json!("recall"))
        .collect();
    assert!(
        !recall.is_empty()
            && recall
                .iter()
                .all(|b| b.get("decided").is_none() && b.get("rows_out").is_none()),
        "the rebuild leaves the block of the newest round of its claim undecided: {recall:?}"
    );
    let c = t.call("t4", 0, json!([user("Thanks.")]));
    let after = recall_of(&c);
    let text = after["text"].as_str().unwrap_or_default().to_string();
    assert!(
        text.contains(FACT),
        "the block stays after the rebuild: {after}"
    );
    assert!(
        !text.contains(ASKED) && !text.contains(ANSWER),
        "the rows decided while the rebuild ran stay out after it: {after} {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    t.answer(&c, "stop", json!([said("You are welcome.")]));
    assert!(
        !held_rows(&t).is_empty(),
        "the call after the rebuild decides the block anew and its tap writes the rows: {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    let raw = meclaw_core::serde_json::to_string(&c.messages()).expect("messages");
    assert!(
        raw.contains(ASKED) && raw.contains(ANSWER),
        "check of the setup: the quoted words stand raw in the window: {raw}"
    );
}

/// Pin (re-check m2): the recall round `t2`, then a round of a target whose
/// profile sets `recall` false -- the block of `t2` stays undecided and is
/// no longer of the newest round. The clock strikes; before the summarizer
/// answers, the call of `t3` decides the block for the first time (its
/// quotes out) and its tap writes that. The rebuild ends after it -- the
/// block stands decided with its rows, and the next call still sends the
/// fact and no quote of the wall.
#[test]
fn gh_d583_nachlese_rows_out_of_an_older_round_decided_while_a_rebuild_runs_survive_it() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(3)),
    ]);
    // Two targets: the main one (with recall) the curates without a target
    // name, and `fast` beside it.
    t.h.name_main_target();
    for i in 1..=6 {
        t.turn(
            &format!("f{i}"),
            &format!("The person tells item {i}."),
            &format!("Noted item {i}."),
        );
    }
    t.turn("t1", ASKED, ANSWER);
    let c2 = t.call(
        "t2",
        0,
        json!([
            user("What do you know about Rita?"),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(RECALL_ID, &recalled())
        ]),
    );
    t.answer(&c2, "stop", json!([said("She was at the sea.")]));
    // A round of a target that sends no recall row: it decides no block.
    announce_profile(&mut t.h, "fast", json!({"recall": false}));
    turn_for(&mut t.h, "fast", "t2b", "And the weather?", "It was sunny.");
    let recall: Vec<Value> = plan_blocks(&t.h, ROUND_E)
        .into_iter()
        .filter(|b| b["kind"] == json!("recall"))
        .collect();
    assert!(
        !recall.is_empty()
            && recall
                .iter()
                .all(|b| b.get("decided").is_none() && b.get("rows_out").is_none()),
        "check of the setup: the round without recall leaves the block of t2 undecided: \
         {recall:?}"
    );
    // The clock strikes: claim, plan, the summarizer asked -- not answered.
    let order = t
        .h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("check of the setup: an order at the clock: {:?}", t.h.clock));
    t.h.fire(&order);
    assert!(
        !t.h.summ.is_empty(),
        "check of the setup: the summarizer is asked: {:?}",
        t.h.stderr
    );
    // A call while the summarizer works: it decides the block of t2.
    let a = t.call("t3", 0, json!([user("Tell me more.")]));
    let first = recall_of(&a);
    let shown = first["text"].as_str().unwrap_or_default().to_string();
    assert!(
        shown.contains(FACT) && !shown.contains(ASKED) && !shown.contains(ANSWER),
        "check of the setup: the block goes without its quoted rows: {first}"
    );
    t.answer(&a, "stop", json!([said("That is all I know.")]));
    assert!(
        !held_rows(&t).is_empty(),
        "check of the setup: the tap wrote the rows into the plan that stands: {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    // The summarizer answers: the rebuild ends.
    t.h.answer("- The person told items 1 to 6.", "stop");
    assert!(
        t.cover() > 0 && t.cover() < seq_of(&t, "t1", "min"),
        "check of the setup: the rebuild covers the items, not the quoted round: cover {} \
         {:?}",
        t.cover(),
        t.h.stderr
    );
    assert!(
        !held_rows(&t).is_empty(),
        "the rebuild's plan keeps the rows the call decided while it ran: {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    let c = t.call("t4", 0, json!([user("Thanks.")]));
    let after = recall_of(&c);
    let text = after["text"].as_str().unwrap_or_default().to_string();
    assert!(
        text.contains(FACT),
        "the block stays after the rebuild: {after}"
    );
    assert!(
        !text.contains(ASKED) && !text.contains(ANSWER),
        "the rows decided while the rebuild ran stay out after it: {after} {:?}",
        plan_blocks(&t.h, ROUND_E)
    );
    t.answer(&c, "stop", json!([said("You are welcome.")]));
    let raw = meclaw_core::serde_json::to_string(&c.messages()).expect("messages");
    assert!(
        raw.contains(ASKED) && raw.contains(ANSWER),
        "check of the setup: the quoted words stand raw in the window: {raw}"
    );
}
