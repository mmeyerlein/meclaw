//! D58 V5 (d58-2) -- a recall quote of an assistant answer that runs over
//! several lines is sent once.
//!
//! WHY this file exists: `recall_in_wall` reads a recall result line by line.
//! The memory hive renders a quote of an answer with paragraphs or a list as
//! a head row `  assistant on <day>: "First line` that carries only the
//! opening quote mark, the following lines WITHOUT indent, the last one
//! ending on the closing mark. The head row has one quote mark, so the
//! function returned False and the block stayed, although the whole quote,
//! opening to closing mark, hashes byte for byte to a raw element of the
//! wall. On the RV5 candidate every recall block left standing was of this
//! kind (reader note on the V5 rest: 69 of 69 over the k10/k30/k60 probe
//! windows; the one multi-line answer sat in 3, 8 and 10 recall blocks).
//! The pin uses the same line shape: a one-line user quote that matches, and
//! an answer of five lines (paragraph, blank line, two list items, blank
//! line, closing question).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, json};

const RECALL_ID: &str = "call_recall_000000000000v5ml";
const ASKED: &str = "Where could we go this summer?";
const FIRST: &str = "A week at the Baltic Sea would suit you well.";
const ITEM: &str = "- **Places:** Usedom or Ruegen, both easy by train.";
const LAST: &str = "Shall I look up the train times for Friday?";

/// The answer as the wall books it: a paragraph, a list, a closing question.
fn answer() -> String {
    format!("{FIRST}\n\n{ITEM}\n- **When:** early June, before the school holidays.\n\n{LAST}")
}

/// The recall result as the memory hive renders it: the user quote on one
/// row, the answer's head row with the opening mark only, its further lines
/// unindented, the closing mark at the end of the last one.
fn recalled() -> String {
    format!(
        "WHAT WAS SAID\n  user on 2026-11-08: \"{ASKED}\"\n  assistant on 2026-11-08: \"{}\"",
        answer()
    )
}

/// The request of `t3` as text: the multi-line answer in `t1`, a recall of
/// both turns of `t1` in the earlier round `t2`.
fn sent(knob: &str) -> String {
    let mut h =
        Hive::with(&[("policy", "recall_in_wall", json!(knob))]).reporting_composite("talky");
    turn(&mut h, "s1", "t1", ASKED, &answer(), json!({}));
    let pair = json!([
        user("What did you suggest for the holidays?"),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &recalled())
    ]);
    let c2 = h.curate("s1", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", json!({}), json!([said("The Baltic Sea.")]));
    let c = h.curate("s1", "t3", 0, json!([user("And?")]), mode("Be brief."));
    sj::to_string(&c.messages()).expect("messages")
}

/// Pin: an answer of several lines, recalled in an earlier round together
/// with its question, reaches the model once -- the recall pair goes whole.
#[test]
fn a_recall_of_a_multiline_answer_is_sent_once() {
    if !shipped() {
        return;
    }
    let text = sent("1");
    for line in [ASKED, FIRST, ITEM, LAST] {
        assert_eq!(
            text.matches(line).count(),
            1,
            "{line:?} once, as the raw turn: {text}"
        );
    }
    assert!(
        !text.contains(RECALL_ID),
        "the pair goes whole, no orphaned call: {text}"
    );
}

/// Lock: off, the window of before -- the turn and its recall both stand, so
/// the fixture does put the multi-line quote into the window.
#[test]
fn off_the_recall_of_a_multiline_answer_stays() {
    if !shipped() {
        return;
    }
    let text = sent("0");
    for line in [ASKED, FIRST, ITEM, LAST] {
        assert_eq!(text.matches(line).count(), 2, "{line:?}: {text}");
    }
    assert!(text.contains(RECALL_ID), "{text}");
}
