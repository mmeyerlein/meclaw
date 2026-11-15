//! d58-3 nachlese (GH #1539, B2) -- the rows a recall block loses hold the
//! prefix in the round and after it.
//!
//! WHY this file exists: a recall block that loses the quotes the window
//! holds raw (`recall_in_wall` "1") is decided once, at the first call after
//! its round (GH #1463). A later call must show the same bytes: the call of
//! a tool round and the calls of the next rounds see the block as the first
//! one sent it, or every call writes the cache anew. The plan keeps the rows
//! left out beside the decision (`rows_out`).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_0000000000nlmixb";
const TOOL_ID: &str = "call_tool_nlrows_0001";
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

fn prefix_break(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let est: Value = sj::from_str(&h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null);
    est["prefix_break"].clone()
}

/// Pin: the first call of `t3` decides the rows, the second call of its
/// tool round and the call of `t4` send the block in the same bytes; no
/// call breaks its prefix.
#[test]
fn gh_d583_nachlese_recall_in_wall_rows_hold_the_prefix_in_the_round() {
    if !shipped() {
        return;
    }
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
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
    let a = t.call("t3", 0, json!([user("Read the file, please.")]));
    let first = recall_of(&a);
    let shown = first["text"].as_str().unwrap_or_default().to_string();
    assert!(
        shown.contains(FACT),
        "check of the setup: the block stays: {first}"
    );
    assert!(
        !shown.contains(ASKED) && !shown.contains(ANSWER),
        "the first call after its round leaves the quoted rows out: {first}"
    );
    assert_eq!(prefix_break(&t.h, &a), json!(0));
    t.answer(&a, "tool_calls", json!([tool_call(TOOL_ID, "read_file")]));
    let b = t.call(
        "t3",
        1,
        json!([
            tool_call(TOOL_ID, "read_file"),
            tool_result(TOOL_ID, "file 1: abc")
        ]),
    );
    assert_eq!(recall_of(&b), first, "the tool round sends the same bytes");
    assert_eq!(prefix_break(&t.h, &b), json!(0), "{:?}", t.h.stderr);
    t.answer(&b, "stop", json!([said("All read.")]));
    let c = t.call("t4", 0, json!([user("Thanks.")]));
    assert_eq!(recall_of(&c), first, "and so does the next round");
    assert_eq!(prefix_break(&t.h, &c), json!(0), "{:?}", t.h.stderr);
    t.answer(&c, "stop", json!([said("You are welcome.")]));
    assert_eq!(t.breaks(), Vec::<String>::new());
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}
