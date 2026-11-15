//! d58-3 treue (T1, GH #1471) -- the frame of an earlier answer holds within
//! the turn.
//!
//! WHY this file exists: a turn keeps the session boundary its first call
//! set (`turn_legend`, `kept`), so a later call of the turn only adds to the
//! window and never changes what stood before (GH #1471: "message 0 of 13
//! differs"). The frame of an own answer of an earlier session is part of
//! that boundary: when the earlier session speaks again while the turn runs,
//! its start and end move, and a frame read from the wall of then would
//! change in the middle of the turn. The answer keeps the frame of the
//! turn's first call, byte for byte, and every call of the turn begins with
//! the whole window before it.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "How is the weather in Berlin today?";
const WEATHER: &str = "In Berlin it is 12 degrees and sunny today.";
const AGAIN: &str = "Hello again from the other phone.";

fn framed_answer(msgs: &[Value]) -> String {
    msgs.iter()
        .find(|m| m["text"].as_str().unwrap_or("").ends_with(WEATHER))
        .unwrap_or_else(|| panic!("the earlier answer stays: {msgs:#?}"))["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn gh_d583_treue_the_frame_of_an_earlier_answer_holds_within_the_turn() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // The form on (`recall_in_wall`, `snapshot_window`): the turn keeps its cut.
    let mut t = Talk::told(0);
    let base = backdated(50);
    let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
    t.h.row(at(0), "s0", "t1", "user", &user(ASK), 0);
    t.h.row(at(10), "s0", "t1", "assistant", &said(WEATHER), 1);
    let since = base.format("%Y-%m-%d %H:%M UTC").to_string();
    let frame =
        format!("[your own answer in an earlier session, since {since} -- said then, not now]\n");

    let id = "call_tool_treue_0001";
    let first = t.call("t1", 0, json!([user("Read the file, please.")]));
    let before = framed_answer(&first.messages());
    assert!(
        before.starts_with(&frame),
        "the first call of the turn frames the earlier answer: {before:?}"
    );
    t.answer(&first, "tool_calls", json!([tool_call(id, "read_file")]));
    // The earlier session speaks again while the turn runs: it no longer
    // ends before the running one.
    t.h.row(chrono::Utc::now(), "s0", "t2", "user", &user(AGAIN), 0);
    let second = t.call(
        "t1",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    let msgs = second.messages();
    assert_eq!(
        framed_answer(&msgs),
        before,
        "the second call keeps the frame of the first, byte for byte"
    );
    assert!(
        msgs.iter().any(|m| {
            let t = m["text"].as_str().unwrap_or("");
            t.starts_with("[other session, since ") && t.ends_with(AGAIN)
        }),
        "check of the setup: the new line of the earlier session stands as said meanwhile: {msgs:#?}"
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "every call of the turn begins with the whole window before it"
    );
}
