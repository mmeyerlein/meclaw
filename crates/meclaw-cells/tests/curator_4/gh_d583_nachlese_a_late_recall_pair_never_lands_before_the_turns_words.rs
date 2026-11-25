//! d58-3 nachlese (GH #1539, B1 break 4) -- a late recall pair never lands
//! before the turn's words.
//!
//! WHY this file exists: the collector's recall pair of a round
//! (`call_recall_` + 16 hex) moves in front of the person's words of its
//! round (`ambient_first`). When the pair stands in no plan at a later call
//! of the round -- it came after the first call, or the first call left it
//! out and its plan was not written yet (S8 run, diag B1: two calls 270 ms
//! apart) -- the later call took it for a pair never decided and set it in
//! front of the words the first call had already sent: a break inside the
//! prefix. Such a pair now waits for the next round; there it is decided
//! once, in front of the words of a round that lies behind the prefix kept.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's id form: `call_recall_` plus 16 hex.
const AMBIENT_ID: &str = "call_recall_00000000000d583a";
const TOOL_ID: &str = "call_tool_nllate_0001";
const ELSEWHERE: &str = "We sold the old car last spring.";

fn text(c: &Msg) -> String {
    sj::to_string(&c.messages()).expect("messages")
}

fn prefix_break(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let est: Value = sj::from_str(&h.state(&format!("window_est:{id}"))).unwrap_or(Value::Null);
    est["prefix_break"].clone()
}

/// Pin: the second call of a tool round brings the recall pair late. It
/// begins with the window of the first call, without the pair; the next
/// round sends the pair, and no call breaks its prefix.
#[test]
fn gh_d583_nachlese_a_late_recall_pair_never_lands_before_the_turns_words() {
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
    let a = t.call("t2", 0, json!([user("Read the file, please.")]));
    t.answer(&a, "tool_calls", json!([tool_call(TOOL_ID, "read_file")]));
    let b = t.call(
        "t2",
        1,
        json!([
            tool_call(AMBIENT_ID, "memory_recall"),
            tool_result(
                AMBIENT_ID,
                &format!("WHAT WAS SAID\n  user: \"{ELSEWHERE}\"")
            ),
            tool_call(TOOL_ID, "read_file"),
            tool_result(TOOL_ID, "file 1: abc")
        ]),
    );
    assert!(
        !text(&b).contains(AMBIENT_ID),
        "the late pair waits: {}",
        text(&b)
    );
    assert_eq!(prefix_break(&t.h, &b), json!(0), "{:?}", t.h.stderr);
    assert_eq!(t.breaks(), Vec::<String>::new());
    t.answer(&b, "stop", json!([said("All read.")]));
    let c = t.call("t3", 0, json!([user("And the car?")]));
    assert!(
        text(&c).contains(AMBIENT_ID) && text(&c).contains(ELSEWHERE),
        "the next round sends the pair: {}",
        text(&c)
    );
    assert_eq!(prefix_break(&t.h, &c), json!(0), "{:?}", t.h.stderr);
    // The bytes: everything in front of the pair is what `b` sent there --
    // the pair lands behind the prefix the next round keeps, not inside it.
    let msgs_b = b.messages();
    let msgs_c = c.messages();
    let p = msgs_c
        .iter()
        .position(|m| m["id"] == AMBIENT_ID)
        .expect("the pair in the next round");
    assert!(
        p <= msgs_b.len()
            && sj::to_string(&msgs_c[..p]).expect("messages")
                == sj::to_string(&msgs_b[..p]).expect("messages"),
        "the next round begins with the bytes `b` sent in front of the pair:\n b: {}\n c: {}",
        text(&b),
        text(&c)
    );
    t.answer(&c, "stop", json!([said("Sold, yes.")]));
    assert_eq!(t.breaks(), Vec::<String>::new());
    let w = t.window_figure();
    assert_eq!(w["prefix_breaks"], json!(0), "{w}");
}

/// Lock: the pair that comes with the first call of its round is sent with
/// it, in front of the person's words.
#[test]
fn a_pair_with_the_first_call_is_sent() {
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
    assert!(text(&a).contains(AMBIENT_ID), "{}", text(&a));
}
