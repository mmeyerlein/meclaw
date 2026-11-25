//! d58-3 nachlese (GH #1539, B2) -- a mixed recall block sends no quote of
//! the wall.
//!
//! WHY this file exists: `recall_in_wall` decided a recall block whole. A
//! block left only when EVERY row was a quote of a raw turn of the window; a
//! block with one fact beside its quotes stayed whole, quotes and all. In
//! the S8 run (diag B2) the one block left standing held a fact and two
//! quotes of the wall and went out in three windows: 6 of 6 recall quotes
//! sent twice (100 %, target <= 5 %). With the form on, such a block now
//! loses the quoted rows one by one; its head, the fact and the caveat stay,
//! and the hop says how many rows it shows of how many.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_0000000000nlmixa";
const ASKED: &str = "Where did Rita go last week?";
const ANSWER: &str = "Rita went to the Baltic Sea with her daughter.";
const FACT: &str = "Rita visited = the Baltic Sea";
const CAVEAT: &str = "(2 older rows not shown)";

/// The recall result: a fact, the two turns of `t1` quoted, a caveat.
fn recalled() -> String {
    format!(
        "WHAT I KNOW\n  {FACT}\nWHAT WAS SAID\n  user: \"{ASKED}\"\n  assistant: \"{ANSWER}\"\n  {CAVEAT}"
    )
}

/// The first call of `t3`: `t1` asked and answered, `t2` carried a recall
/// pair of both turns and a fact.
fn third(knob: &str) -> Msg {
    let mut h = Hive::with(&[
        ("policy", "recall_in_wall", json!(knob)),
        ("policy", "snapshot_window", json!("1")),
    ]);
    turn(&mut h, "s1", "t1", ASKED, ANSWER, json!({}));
    let pair = json!([
        user("What do you know about Rita?"),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &recalled())
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("She was at the sea.")]));
    h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."))
        .clone()
}

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

/// Pin: the block stays with its fact and caveat, without the two quotes
/// the window holds raw; the hop names the rows it shows of its rows.
#[test]
fn gh_d583_nachlese_a_mixed_recall_block_sends_no_quote_of_the_wall() {
    if !shipped() {
        return;
    }
    let c = third("1");
    let t = text(&c);
    assert!(t.contains(RECALL_ID), "the block with a fact stays: {t}");
    assert!(t.contains(FACT), "the fact stays: {t}");
    assert!(t.contains(CAVEAT), "the caveat stays: {t}");
    for quoted in [ASKED, ANSWER] {
        assert_eq!(
            t.matches(quoted).count(),
            1,
            "{quoted:?} once, as the raw turn: {t}"
        );
    }
    let cuts: Vec<Value> = c
        .hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == "recall_in_wall")
        .collect();
    assert_eq!(cuts.len(), 1, "one entry for the block: {cuts:?}");
    assert_eq!(cuts[0]["shown"], json!(1), "{cuts:?}");
    assert_eq!(cuts[0]["total"], json!(3), "{cuts:?}");
    assert_eq!(cuts[0]["unit"], json!("rows"), "{cuts:?}");
}

/// Lock: off, the window of before -- the block whole, each quote twice.
#[test]
fn off_the_mixed_block_stays_whole() {
    if !shipped() {
        return;
    }
    let t = text(&third("0"));
    for quoted in [ASKED, ANSWER] {
        assert_eq!(t.matches(quoted).count(), 2, "{quoted:?}: {t}");
    }
    assert!(t.contains(FACT), "{t}");
}
