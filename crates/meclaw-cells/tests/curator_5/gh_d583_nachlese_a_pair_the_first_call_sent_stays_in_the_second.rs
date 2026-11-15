//! d58-3 nachlese (GH #1539, review I2) -- a recall pair the first call of a
//! round sent stays in the second, even before the first call's tap.
//!
//! WHY this file exists: a late recall pair (one in no plan when an earlier
//! call of the round already sent its window) waits for the next round, so
//! it never lands inside the prefix sent. The plan of a call is written by
//! its tap, the snapshot by the call itself -- so a second call that comes
//! before the first one's tap (two calls 270 ms apart in the S8 run) found
//! the pair the first call SENT in no plan and held it back: a break of the
//! prefix and a recall the model just had, gone for the rest of the round.
//! Now a pair an earlier call of the round named in its `sent` list is no
//! late pair: sent stays sent, left out with a reason stays left out.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's id form: `call_recall_` plus 16 hex.
const AMBIENT_ID: &str = "call_recall_00000000000d583b";
const TOOL_ID: &str = "call_tool_nlsent_0001";
const ELSEWHERE: &str = "We sold the old car last spring.";

fn prefix_break(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let est: Value = sj::from_str(&h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null);
    est["prefix_break"].clone()
}

fn at(call: &Msg) -> Option<usize> {
    call.messages().iter().position(|m| m["id"] == AMBIENT_ID)
}

/// Pin: the first call of `t2` sends the pair; the second call of the
/// round, with no tap between them, sends it at the same place and begins
/// with the bytes of the first; no prefix break.
#[test]
fn gh_d583_nachlese_a_pair_the_first_call_sent_stays_in_the_second() {
    if !shipped() {
        return;
    }
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.turn(
        "t1",
        "My sister Anna moved to Hamburg.",
        "Hamburg is lovely.",
    );
    let a = t.call(
        "t2",
        0,
        json!([
            user("Read the file, please."),
            tool_call(AMBIENT_ID, "memory_recall"),
            tool_result(
                AMBIENT_ID,
                &format!("WHAT WAS SAID\n  user: \"{ELSEWHERE}\"")
            )
        ]),
    );
    let p = at(&a).unwrap_or_else(|| panic!("check of the setup: the first call sends the pair"));
    // No tap: the first call's plan is not written when the second comes.
    let b = t.call(
        "t2",
        1,
        json!([
            tool_call(TOOL_ID, "read_file"),
            tool_result(TOOL_ID, "file 1: abc")
        ]),
    );
    assert_eq!(
        at(&b),
        Some(p),
        "the second call sends the pair at the same place"
    );
    let msgs_a = sj::to_string(&a.messages()).expect("messages");
    let msgs_b = sj::to_string(&b.messages()).expect("messages");
    assert!(
        msgs_b.starts_with(msgs_a.trim_end_matches(']')),
        "the second call begins with the bytes of the first:\n first:  {msgs_a}\n second: {msgs_b}"
    );
    assert_eq!(prefix_break(&t.h, &b), json!(0), "{:?}", t.h.stderr);
    assert_eq!(t.breaks(), Vec::<String>::new());
}
