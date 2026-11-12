//! GH #1448 (D38 C2, R-38-4 Nr. 2) -- the plan is built after the answer.
//!
//! WHY this file exists: every turn rendered its window from scratch
//! (`render`, `state_block`, `note_block`, `guard_cut` on the call's path),
//! and only a rebuild (`rb-data`) wrote a plan. Stage 1 of the curator is a
//! plan of building blocks `{id, kind, prio, origin, tokens, audience,
//! valid_until}`, written ahead of the next turn: after each answer (`tap`)
//! the round's plan (`state.window_plan:<rk>`, the one plan form -- `blocks`
//! beside the rebuild's keys) holds the person's words and the answer of the
//! turn as blocks of their own, each pointing at its wall row.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn find<'a>(blocks: &'a [Value], id: &str) -> Option<&'a Value> {
    blocks.iter().find(|b| b["id"] == json!(id))
}

#[test]
fn the_answer_leaves_the_turn_in_the_plan() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let call = h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    let before = block_ids(&plan_blocks(&h, ROUND_E));
    assert!(
        !before.contains(&block_id("answer", &said("a1"))),
        "no answer before it was given: {before:?}"
    );
    h.tap(&call, "stop", json!({}), json!([said("a1")]));

    let blocks = plan_blocks(&h, ROUND_E);
    assert!(
        !blocks.is_empty(),
        "the plan of the round holds its blocks after the answer: {}",
        h.plan_in(ROUND_E)
    );
    let ids = block_ids(&blocks);
    let mut uniq = ids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), ids.len(), "every id once: {ids:?}");
    for b in &blocks {
        for k in BLOCK_KEYS {
            assert!(b.get(k).is_some(), "block without `{k}`: {b}");
        }
        assert!(b["prio"].is_i64(), "prio is a whole number: {b}");
        assert!(
            b["tokens"].as_i64().is_some_and(|t| t >= 0),
            "tokens a count: {b}"
        );
        let src = &b["origin"]["source"];
        assert!(src.is_string(), "origin names its source: {b}");
        assert!(
            b["origin"].get("seq").is_some() || b["origin"].get("hash").is_some(),
            "origin points at a seq or a hash: {b}"
        );
    }
    let canon = round_canon(ROUND_E).expect("a round");
    for (kind, el) in [("words", user("q1")), ("answer", said("a1"))] {
        let id = block_id(kind, &el);
        let b = find(&blocks, &id).unwrap_or_else(|| panic!("{id} in the plan: {ids:?}"));
        assert_eq!(b["kind"], json!(kind), "{b}");
        assert_eq!(b["origin"]["source"], json!("wall"), "{b}");
        assert_eq!(
            b["origin"]["hash"],
            json!(sha256_hex(&canonical(&el))),
            "the block points at its wall row: {b}"
        );
        assert_eq!(b["audience"], json!(canon), "the round's audience: {b}");
        assert!(
            b["tokens"].as_i64().is_some_and(|t| t > 0),
            "a block of text has tokens: {b}"
        );
    }
}

/// Before the first rebuild the plan written after an answer is blocks
/// only: no cover, no keep -- every step that reads the rebuild's keys reads
/// them as without a plan (the window of before, byte for byte).
#[test]
fn before_the_first_rebuild_the_plan_holds_blocks_only() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let plan = h.plan_in(ROUND_E);
    assert!(plan["blocks"].is_array(), "the plan is written: {plan}");
    for k in ["cover", "keep", "summary", "sum_from", "sum_to", "marks_to"] {
        assert!(plan.get(k).is_none(), "no rebuild key `{k}` yet: {plan}");
    }
}
