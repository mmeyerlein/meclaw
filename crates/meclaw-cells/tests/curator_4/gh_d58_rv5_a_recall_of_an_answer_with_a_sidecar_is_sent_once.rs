//! GH #1458 (d58-1 RV5) -- a recall quote of an assistant answer that carries
//! a sidecar block is sent once.
//!
//! WHY this file exists: `said_hashes` hashed the raw text of the wall's
//! assistant elements, sidecar block included, while the memory hive quotes
//! only the spoken words. No assistant quote ever matched, so with
//! `recall_in_wall` "1" no recall block holding one was left out: in the S8
//! run 100 % of the doubled recall blocks stayed (reader note leser-v5, k10:
//! 0 of 10 recall blocks cut, 6 with the strip; k30: 0 of 29, 24). The pin
//! uses an answer as the model writes it -- words, blank line, a ```sidecar
//! fence -- and a recall that quotes the words alone.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, json};

const RECALL_ID: &str = "call_recall_0000000000000rv5";
const WORDS: &str = "Good morning, Rita! Nice to have you here. How are you this early?";
const SIDECAR: &str =
    "```sidecar\n{\"memory\":{\"nothing_new\":true,\"name\":\"Morning greeting\"}}\n```";

/// The answer as the wall books it: the words, then the sidecar block.
fn answer() -> String {
    format!("{WORDS}\n\n{SIDECAR}")
}

/// The request of `t3` as text: the answer with its sidecar in `t1`, a
/// recall of its words in the earlier round `t2`.
fn sent(knob: &str) -> String {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "recall_in_wall", json!(knob)),
    ]);
    turn(&mut h, "s1", "t1", "Good morning!", &answer(), json!({}));
    let pair = json!([
        user("What did you say this morning?"),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(
            RECALL_ID,
            &format!("WHAT WAS SAID\n  assistant on 2026-11-08: \"{WORDS}\"")
        )
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("I greeted you.")]));
    let c = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    sj::to_string(&c.messages()).expect("messages")
}

/// Pin: the words of an answer with a sidecar, recalled in an earlier round,
/// reach the model once -- the recall pair goes whole.
#[test]
fn a_recall_of_an_answer_with_a_sidecar_is_sent_once() {
    if !shipped() {
        return;
    }
    let text = sent("1");
    assert_eq!(
        text.matches(WORDS).count(),
        1,
        "once, as the answer: {text}"
    );
    assert!(
        !text.contains(RECALL_ID),
        "the pair goes whole, no orphaned call: {text}"
    );
}

/// Lock: off, the window of before -- the answer and the recall both stand.
#[test]
fn off_the_recall_of_an_answer_stays() {
    if !shipped() {
        return;
    }
    let text = sent("0");
    assert_eq!(text.matches(WORDS).count(), 2, "{text}");
    assert!(text.contains(RECALL_ID), "{text}");
}
