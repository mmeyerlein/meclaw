//! D58 feed (GH #1481, plan `u-feed.md` contract point 1) -- an ordered feed
//! leaves at once, without a curate.
//!
//! WHY this file exists: every `live_feed` the curator sent came out of one
//! of its own triggers (a closed turn, a tool result, a partial); nothing
//! from outside could put a fact into a running session without waiting for
//! a turn's end. The lane `in_feed` takes an order: the head names the
//! session (`live`, `session_id`, `turn_id`, the round), the section and the
//! orderer; the curator sends the body unchanged as the body of ONE
//! `live_feed`, at once, with no model call, and books its mark `feed` in the
//! same step (one message to the store). Further head fields of the orderer
//! travel with it (the tool fact's `fact_source` today). The round comes
//! from the head: the context of this order carries none.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

const FACT: &str = "The pharmacy has the prescription ready.";

#[test]
fn gh_d58_feed_an_ordered_feed_leaves_at_once_without_a_curate() {
    if !shipped() {
        return;
    }
    let mut h = feed_hive(&[]);
    let mut hop = order_head(Some(FEED_LIVE), "t1", "fact");
    hop["fact_source"] = json!("tool");
    let steps = policy_steps(&h);
    order_in(&mut h, json!({"audience_set": null}), hop, fact_body(FACT));

    let sent = feeds(&h);
    assert_eq!(sent.len(), 1, "one feed: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        h.out.len(),
        1,
        "nothing but the feed leaves -- no model call, no curate: {:?}",
        h.out
    );
    let f = &sent[0];
    for (k, v) in [
        ("live", json!(FEED_LIVE)),
        ("session_id", json!(SESSION)),
        ("turn_id", json!("t1")),
        ("section", json!("fact")),
        (SENDER_KEY, json!(SENDER)),
        ("fact_source", json!("tool")),
        ("audience_set", json!(ROUND_E)),
    ] {
        assert_eq!(f.hop.get(k), Some(&v), "head field {k}: {:?}", f.hop);
    }
    assert!(
        f.hop.get("at_turn_end").is_none(),
        "no turn end asked, none added: {:?}",
        f.hop
    );
    assert_eq!(
        body_of(f),
        fact_body(FACT),
        "the body leaves as it was ordered"
    );
    assert_eq!(
        policy_steps(&h) - steps,
        1,
        "mark and send in one step: {:?}",
        h.ledger_steps
    );

    let marks = feed_marks(&h);
    assert_eq!(marks.len(), 1, "one mark feed: {marks:?} {:?}", h.stderr);
    assert_eq!(marks[0].session, SESSION);
    assert_eq!(marks[0].turn, "t1");
    assert_eq!(marks[0].audience, round_e(), "the round of the head");
    assert!(
        claims(&h).is_empty(),
        "no `once`, no claim: {:?}",
        claims(&h)
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
