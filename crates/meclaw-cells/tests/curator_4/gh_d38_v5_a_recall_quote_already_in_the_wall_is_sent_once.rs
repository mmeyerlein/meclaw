//! GH #1451 (D38 V5, T-17, OR-38-84 b) -- a recall quote already in the wall
//! is sent once.
//!
//! WHY this file exists: in the S8 drift runs (P1a, `dup.py`) EVERY recall
//! quote of the window was a turn that already stood raw as a user/assistant
//! row of the same wall (100 % of the quotes), and the recall rows carried
//! 57-75 % of the wall's characters. Doubled context is a window cause
//! (R-38-3 no. 1). With `recall_in_wall` "1" stage 1 leaves out a recall
//! block whose every row quotes an element raw in the window -- matched by
//! the hash of the quoted bytes, never by resemblance; a recall of a turn
//! outside the wall stays a block (OR-38-65 b). Shipped "0", byte-identical.
//!
//! Fix round 1 (review o15-Rv1 I2): only a recall of an EARLIER round goes,
//! and as a whole pair (`tool_call` and result). The model's own
//! `memory_recall` of the running round stays: without it the next request
//! shows neither its call nor the result, and the model calls it again up
//! to `tool_call_limit`. The snapshot names the drop (review m1).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, json};

const RECALL_ID: &str = "call_recall_00000000000000v5";
const SAID: &str = "My sister Anna moved to Hamburg.";
const ELSEWHERE: &str = "We sold the old car last spring.";

fn quote(text: &str) -> String {
    format!("WHAT WAS SAID\n  user: \"{text}\"")
}

/// The recall pair in turn `t2` (an earlier round) or, `running`, in the
/// running turn `t3`'s own tool loop; the request of `t3` as text, and the
/// cut of its snapshot.
fn sent_in(knob: &str, quoted: &str, running: bool) -> (String, String) {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "recall_in_wall", json!(knob)),
    ]);
    turn(&mut h, "s1", "t1", SAID, "Hamburg is lovely.", json!({}));
    let pair = json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &quote(quoted))
    ]);
    let c = if running {
        h.curate("s1", "t3", 0, pair, mode("Be brief."))
    } else {
        let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
        h.tap(&c2, "stop", json!({}), json!([said("She likes it there.")]));
        h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."))
    };
    let id = c.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let cut = h
        .rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r.first().and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    (sj::to_string(&c.messages()).expect("messages"), cut)
}

fn sent(knob: &str, quoted: &str) -> String {
    sent_in(knob, quoted, false).0
}

/// Pin: the quote of a turn raw in the wall, recalled in an earlier round,
/// reaches the model once -- its pair goes whole, the call with the result.
#[test]
fn a_recall_quote_already_in_the_wall_is_sent_once() {
    if !shipped() {
        return;
    }
    let text = sent("1", SAID);
    assert_eq!(
        text.matches(SAID).count(),
        1,
        "once, as the raw turn: {text}"
    );
    assert!(
        !text.contains(RECALL_ID),
        "the pair goes whole, no orphaned call: {text}"
    );
}

/// Pin (review o15-Rv1 m1): the snapshot's cut names the dropped pair with
/// its reason, so a reader tells "dropped" from "never there".
#[test]
fn the_snapshot_names_the_dropped_recall() {
    if !shipped() {
        return;
    }
    let (_, cut) = sent_in("1", SAID, false);
    let v: sj::Value = sj::from_str(&cut).unwrap_or_else(|_| panic!("a cut: {cut:?}"));
    let ids: Vec<String> = v["dropped"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|d| d["why"] == "recall_in_wall")
        .map(|d| d["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(ids.len(), 2, "the result and its call: {cut}");
    assert!(ids.iter().any(|id| id.starts_with("recall:")), "{cut}");
    assert!(ids.iter().any(|id| id.starts_with("tool_call:")), "{cut}");
    let (_, off) = sent_in("0", SAID, false);
    assert!(
        !off.contains("recall_in_wall"),
        "off, the cut of before: {off}"
    );
}

/// Lock (review o15-Rv1 I2): a recall of the running round stays -- the
/// model sees its own call and result.
#[test]
fn a_recall_of_the_running_round_stays() {
    if !shipped() {
        return;
    }
    let (text, _) = sent_in("1", SAID, true);
    assert_eq!(text.matches(SAID).count(), 2, "{text}");
    assert!(text.contains(RECALL_ID), "{text}");
}

/// Lock: a quote of a turn outside the wall stays; off, the window of before.
#[test]
fn a_quote_from_outside_the_wall_and_off_stay() {
    if !shipped() {
        return;
    }
    assert!(sent("1", ELSEWHERE).contains(ELSEWHERE));
    assert_eq!(sent("0", SAID).matches(SAID).count(), 2);
}
