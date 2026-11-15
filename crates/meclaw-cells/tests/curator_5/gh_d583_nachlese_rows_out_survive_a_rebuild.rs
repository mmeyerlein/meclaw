//! d58-3 nachlese (GH #1539, review I1) -- the rows a recall block lost
//! stay lost over a rebuild.
//!
//! WHY this file exists: a recall block that loses the quotes the window
//! holds raw (`recall_in_wall` "1") keeps the rows left out beside its
//! decision in the plan (`rows_out`). A rebuild builds every recall block of
//! an earlier round anew from the wall and marks it decided -- without the
//! rows. From then on the block was neither a candidate nor held, and every
//! call sent it whole, the quotes beside the raw words they quote: the gain
//! of the form held only until the first rebuild (the cache runs out
//! regularly). Now the rebuilt block takes the rows of the block of the same
//! id before it; a later call still cuts them with what stands raw in the
//! window, so a quote whose words a summary covers comes back.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const RECALL_ID: &str = "call_recall_0000000000nlrebd";
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

fn seq_of(t: &Talk, turn: &str, agg: &str) -> i64 {
    t.h.rows(&format!(
        "SELECT {agg}(seq) FROM wall WHERE turn_id = '{turn}'"
    ))
    .first()
    .and_then(|r| r[0].as_i64())
    .unwrap_or_else(|| panic!("the rows of {turn}"))
}

/// Pin: a mixed recall block left its quotes out; a rebuild whose cover
/// lies before the quoted round keeps them out -- the next call sends the
/// fact and no quote of the wall.
#[test]
fn gh_d583_nachlese_rows_out_survive_a_rebuild() {
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
    let a = t.call("t3", 0, json!([user("Tell me more.")]));
    let first = recall_of(&a);
    let shown = first["text"].as_str().unwrap_or_default().to_string();
    assert!(
        shown.contains(FACT) && !shown.contains(ASKED) && !shown.contains(ANSWER),
        "check of the setup: the block stays without its quoted rows: {first}"
    );
    t.answer(&a, "stop", json!([said("That is all I know.")]));
    t.strike("- The person told items 1 to 6.");
    assert!(
        t.cover() > 0 && t.cover() < seq_of(&t, "t1", "min"),
        "check of the setup: the rebuild covers the items, not the quoted round: cover {} \
         {:?}",
        t.cover(),
        t.h.stderr
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
        "the quotes the window holds raw stay out after the rebuild: {after}"
    );
    let raw = meclaw_core::serde_json::to_string(&c.messages()).expect("messages");
    assert!(
        raw.contains(ASKED) && raw.contains(ANSWER),
        "check of the setup: the quoted words stand raw in the window: {raw}"
    );
}
