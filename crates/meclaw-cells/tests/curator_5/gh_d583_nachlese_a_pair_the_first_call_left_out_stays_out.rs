//! d58-3 nachlese (GH #1539, re-check m3 on review I2) -- a recall pair the
//! first call of a round left out with a reason stays out in the second,
//! even before the first call's tap.
//!
//! WHY this file exists: an earlier call of the round names in its `sent`
//! list (and its cut) what it saw -- sent, or left out with a `why`. A
//! second call that comes before the first one's tap finds that decision in
//! no plan; it reads it off the first call's cut: the block ids it sent
//! (`ids`) and those it left out (`dropped`). The pin beside this file
//! holds the sent case; this one the left-out case: what the first call
//! left out stays out, and the second call begins with the first one's
//! bytes.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's id form: `call_recall_` plus 16 hex.
const AMBIENT_ID: &str = "call_recall_00000000000d583c";
const TOOL_ID: &str = "call_tool_nlout_0001";
const TOLD: &str = "My sister Anna moved to Hamburg.";
const REPLY: &str = "Hamburg is lovely.";

fn prefix_break(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let est: Value = sj::from_str(&h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null);
    est["prefix_break"].clone()
}

fn has_pair(call: &Msg) -> bool {
    call.messages().iter().any(|m| m["id"] == AMBIENT_ID)
}

/// Pin: the first call of `t2` leaves the pair out (its quotes stand raw in
/// the window, `recall_in_wall`); the second call of the round, with no tap
/// between them, leaves it out as well and begins with the bytes of the
/// first; no prefix break.
#[test]
fn gh_d583_nachlese_a_pair_the_first_call_left_out_stays_out() {
    if !shipped() {
        return;
    }
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ]);
    t.turn("t1", TOLD, REPLY);
    let a = t.call(
        "t2",
        0,
        json!([
            user("Read the file, please."),
            tool_call(AMBIENT_ID, "memory_recall"),
            tool_result(
                AMBIENT_ID,
                &format!("WHAT WAS SAID\n  user: \"{TOLD}\"\n  assistant: \"{REPLY}\"")
            )
        ]),
    );
    let why: Vec<Value> = sent_of(&t.h, &a)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.get("why").is_some())
        .collect();
    assert!(
        !has_pair(&a) && why.iter().any(|e| e["why"] == json!("recall_in_wall")),
        "check of the setup: the first call leaves the pair out with a reason: {:?} {why:?} {:?}",
        a.messages(),
        t.h.stderr
    );
    // No tap: the first call's plan is not written when the second comes.
    let b = t.call(
        "t2",
        1,
        json!([
            tool_call(TOOL_ID, "read_file"),
            tool_result(TOOL_ID, "file 1: abc")
        ]),
    );
    assert!(
        !has_pair(&b),
        "the second call leaves the pair out: {:?}",
        b.messages()
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
