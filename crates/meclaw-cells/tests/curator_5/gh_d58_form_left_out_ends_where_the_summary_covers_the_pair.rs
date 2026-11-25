//! GH #1494 (D58 close, form review M-2, part `left_out`) -- the list of what
//! the calls left out ends where a summary covers it.
//!
//! WHY this file exists: a recall pair a call leaves out is named in the
//! plan's `left_out` by its wall hashes, so no later call and no rebuild
//! takes it in again. Every rebuild carried the list on whole, also for
//! pairs its cover had passed long ago -- rows no window reads any more. The
//! list only grew, with every decision of the conversation, in a plan every
//! call reads. Now a rebuild keeps a hash only while a wall row it reads
//! above its new cover holds it (or a model pin it keeps): no number, the
//! cover is the bound.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const RECALL_ID: &str = "call_recall_0000000000close19";
const SAID: &str = "My sister Anna moved to Hamburg.";

fn result_of_said() -> Value {
    tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
}

fn left_out(t: &Talk) -> Vec<String> {
    t.h.plan_in(ROUND_E)["left_out"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

#[test]
fn gh_d58_form_left_out_ends_where_the_summary_covers_the_pair() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "keep_recent", json!(2)),
    ]);
    t.turn("t1", SAID, "Hamburg is lovely.");
    let c2 = t.call(
        "t2",
        0,
        json!([
            user("Tell me about Anna."),
            tool_call(RECALL_ID, "memory_recall"),
            result_of_said()
        ]),
    );
    t.answer(&c2, "stop", json!([said("She likes it there.")]));
    t.turn("t3", "And?", "She moved in March.");
    let pair = sha256_hex(&canonical(&result_of_said()));
    assert!(
        left_out(&t).contains(&pair),
        "check of the setup: the pair is left out and named: {:?}",
        left_out(&t)
    );
    for i in 4..=9 {
        t.turn(
            &format!("t{i}"),
            &format!("The person tells item {i}."),
            &format!("Noted item {i}."),
        );
    }
    t.strike("- Anna moved to Hamburg; the person told items 4 to 6.");
    let pair_to =
        t.h.rows("SELECT max(seq) FROM wall WHERE turn_id = 't2'")
            .first()
            .and_then(|r| r[0].as_i64())
            .expect("the rows of t2");
    assert!(
        t.cover() >= pair_to,
        "check of the setup: the rebuild covers the round of the pair: cover {} < {pair_to} {:?}",
        t.cover(),
        t.h.stderr
    );
    assert!(
        !left_out(&t).contains(&pair),
        "a pair below the cover is no longer named: {:?}",
        left_out(&t)
    );
    // The window after it: the pair stays out, the summary holds the round.
    let c = t.call("t10", 0, json!([user("Go on.")]));
    let text = meclaw_core::serde_json::to_string(&c.messages()).expect("messages");
    assert!(!text.contains(RECALL_ID), "{text}");
}
