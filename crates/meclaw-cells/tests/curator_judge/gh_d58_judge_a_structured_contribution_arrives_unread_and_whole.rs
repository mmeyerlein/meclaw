//! D58 judge (GH #1480, contract point 3) -- a structured contribution
//! arrives unread and whole.
//!
//! WHY this file exists: the screen topics are structured
//! (`{name, purpose, candidates: [{key, describe}]}`), and the route needs
//! names and keys exactly, as options and to bind the answer back (plan
//! review W3). The curator does not read a contribution (R-58-14, no word
//! lists: what a topic means is the model's): it checks no shape, drops no
//! entry, cuts at no count of its own (R-IG-1: limits only from the catalogue,
//! the curator's budget or a carrier's cap) and keeps every key. Today's
//! `in_screen_topics` cut at 64 topics and 32 candidates and dropped what it
//! did not know; a contribution comes back byte for byte as a value.
//!
//! Runs the shipped `./history` against the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;
// harness_fold: #[path = "support/history_view.rs"]
#[allow(unused_imports)]
use crate::history_view; // harness_fold: mod history_view;

use curator_hive::*;
use history_judge::*;
use history_view::*;
use meclaw_core::serde_json::{Value, json};

/// 65 topics with 33 candidates each -- one over each of the old bounds --,
/// a topic without a name, a candidate without a key, keys of no known
/// shape, a number, a null and nested lists.
fn odd_value() -> Value {
    let mut topics: Vec<Value> = (0..65)
        .map(|i| {
            json!({"name": format!("topic-{i:02}"), "purpose": format!("purpose {i}"),
                   "candidates": (0..33).map(|k| json!({"key": format!("k{k}"),
                                                        "describe": format!("block {k}")}))
                                        .collect::<Vec<_>>()})
        })
        .collect();
    topics.push(json!({"purpose": "a topic without a name"}));
    topics.push(
        json!({"name": "odd", "candidates": [{"describe": "no key"}],
                       "weight": 0.5, "seen": null, "tags": [["a", 1], [true]]}),
    );
    json!({"topics": topics, "version": 3, "note": "ñ€✓ \"quoted\"\nline"})
}

#[test]
fn gh_d58_judge_a_structured_contribution_arrives_unread_and_whole() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    let _ = part(&h, &params, "decide", "presenter", odd_value(), "");

    let view = the_view(&view_ask(&h, &params, "decide", "s-now", "t2"));
    let got = leaf(&view, "decide", "presenter");
    assert!(
        got["text"].is_string(),
        "the structured value is one leaf, its JSON text: {got}"
    );
    assert_eq!(
        leaf_value(&got),
        odd_value(),
        "every topic, every candidate and every key, unread and whole"
    );
}
