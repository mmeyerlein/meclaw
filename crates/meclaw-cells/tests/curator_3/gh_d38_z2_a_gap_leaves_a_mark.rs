//! GH #1441 (D38 Z2) -- a gap the push looks up leaves a `gap` mark.
//!
//! WHY this file exists: the curator's `stats` counts kind `gap` (GH #926) and
//! argus judges `gap_rate` off it, so every gap the model names in its answer
//! (section `gap`) has to leave exactly one mark in the ledger: the turn it
//! was said in, its id, its words and whether it was looked up (`searched`).
//! The plan of D38 Z2 read the writer as missing (orientation of 04.11.); the
//! push writes it since GH #1225 (phase `gap`, if absent). This file is the
//! lock on that contract for the adaptive recall: one gap, one mark, one
//! lookup; a gap without words, no mark.
//!
//! Runs the shipped curator in the test hive (`support/curator_hive.rs`); no
//! memory and no model -- the lookup is the `recall` message that leaves.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
}

fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
}

/// One round with the collector's ask first, so the push knows the tier of
/// this surface's memory leg.
fn round(h: &mut Hive, session: &str, turn: &str, text: &str) {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": session, "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    );
    assert_eq!(
        h.routed("recall").len(),
        1,
        "the round's ask leaves: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = h.curate(session, turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said("Nice.")]));
}

/// The model's section `gap`, as the parent's splitter hands it to the curator.
fn gap(h: &mut Hive, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "curator_call": "c-1",
               "channel": "test", "engine": "", "audience_set": ROUND_E}),
        json!({"section": "gap"}),
        json!({"messages": [], "section": "gap", "payload": {"payload": text}}),
    );
    h.routed("recall")
}

fn gap_marks(h: &Hive) -> Vec<(String, Value)> {
    h.rows("SELECT turn_id, value FROM marks WHERE kind = 'gap' ORDER BY seq")
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                sj::from_str(r[1].as_str().unwrap_or("{}")).expect("the mark is JSON"),
            )
        })
        .collect()
}

#[test]
fn a_gap_leaves_one_mark_and_one_lookup() {
    if !shipped() || !push_shipped() {
        return;
    }
    let mut h = talky();
    round(&mut h, "s1", "t1", "I was at the lake with my brother");
    let asks = gap(&mut h, "s1", "t1", "the name of the brother");
    let marks = gap_marks(&h);
    assert_eq!(
        marks.len(),
        1,
        "one gap, one mark: {marks:?} {:?}",
        h.stderr
    );
    let (turn, value) = &marks[0];
    assert_eq!(turn, "t1", "the mark names the turn the gap was said in");
    assert_eq!(value["text"], json!("the name of the brother"), "{value}");
    assert!(
        value["id"].as_str().is_some_and(|s| !s.is_empty()),
        "{value}"
    );
    // Review d38_o3-r1 m6: the harness knows the stage (`round` ran), so
    // the gap is looked up -- `searched` is 1, and it is asked once.
    assert_eq!(
        value["searched"],
        json!(1),
        "the mark says the gap was looked up: {value}"
    );
    assert_eq!(asks.len(), 1, "a searched gap is asked once: {:?}", h.out);
    assert_eq!(
        asks[0].hop["recall_query"],
        json!("the name of the brother"),
        "the gap is its own question"
    );
}

#[test]
fn a_gap_without_words_leaves_no_mark() {
    if !shipped() || !push_shipped() {
        return;
    }
    let mut h = talky();
    round(&mut h, "s1", "t1", "hello");
    let asks = gap(&mut h, "s1", "t1", "   ");
    assert!(asks.is_empty(), "{:?}", h.out);
    assert!(gap_marks(&h).is_empty(), "no words, no mark");
}
