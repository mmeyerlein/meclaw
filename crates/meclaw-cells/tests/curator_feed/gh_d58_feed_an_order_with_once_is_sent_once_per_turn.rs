//! D58 feed (GH #1481, plan `u-feed.md` contract points 1 and 2) -- an order
//! with `once` is sent once per turn.
//!
//! WHY this file exists: the curator's own feeds that must not come twice
//! built a claim each (`pending:fact_tool:…`, `pending:interrupt:…`). An
//! order says it with a key of its own, `once`: the curator claims
//! `pending:feed:<live>#<turn>#<once>` (with `parked_at`, so the sweep takes
//! it) and sends only where the claim took -- after exactly one round trip
//! for the claim, then the send with its mark. The same key in the same turn
//! sends nothing more; another key, or the same key in another turn, sends.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn once_order(h: &mut Hive, turn: &str, once: &str) {
    let mut hop = order_head(Some(FEED_LIVE), turn, "fact");
    hop["once"] = json!(once);
    order(h, hop, fact_body("The taxi is booked for ten."));
}

#[test]
fn gh_d58_feed_an_order_with_once_is_sent_once_per_turn() {
    if !shipped() {
        return;
    }
    let mut h = feed_hive(&[]);
    let steps = policy_steps(&h);
    once_order(&mut h, "t1", "k1");
    assert_eq!(feeds(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        policy_steps(&h) - steps,
        2,
        "one round trip for the claim, then the send: {:?}",
        h.ledger_steps
    );
    let key = claim_key(FEED_LIVE, "t1", "k1");
    assert_eq!(claims(&h), vec![key.clone()]);
    let row: Value = meclaw_core::serde_json::from_str(&h.state(&key)).unwrap_or(Value::Null);
    assert!(
        row.get("parked_at")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "the claim carries `parked_at` for the sweep: {row}"
    );
    assert!(
        row.get("session").is_none() && row.get("hop").is_none(),
        "a claim is no parked turn (`brain_busy` reads `session`): {row}"
    );

    // The same key in the same turn: nothing more, no second mark.
    once_order(&mut h, "t1", "k1");
    assert!(feeds(&h).is_empty(), "once per turn: {:?}", h.out);
    assert_eq!(feed_marks(&h).len(), 1, "{:?}", feed_marks(&h));

    // Another key in the same turn, and the first key in the next turn.
    once_order(&mut h, "t1", "k2");
    assert_eq!(
        feeds(&h).len(),
        1,
        "another key: {:?} {:?}",
        h.out,
        h.stderr
    );
    once_order(&mut h, "t2", "k1");
    assert_eq!(
        feeds(&h).len(),
        1,
        "the next turn: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(feed_marks(&h).len(), 3, "{:?}", feed_marks(&h));
    assert_eq!(
        claims(&h),
        vec![
            claim_key(FEED_LIVE, "t1", "k1"),
            claim_key(FEED_LIVE, "t1", "k2"),
            claim_key(FEED_LIVE, "t2", "k1"),
        ]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
