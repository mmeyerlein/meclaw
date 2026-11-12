//! D58 feed (GH #1481, plan `u-feed.md` contract point 1, review F-W3) --
//! `at_turn_end` is passed on untouched.
//!
//! WHY this file exists: the curator never waited for a turn boundary -- the
//! renewal carries `hop.at_turn_end` "1" and the voice takes it at its next
//! boundary (policy, phase `live-facts`). An order that asks for the turn's
//! end gets exactly that: the curator sends at once and hands the head field
//! on as it came, whatever its value; an order without it gets none added.
//! A hold in the policy would need a state of its own the plan rules out.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_feed_at_turn_end_is_passed_on_untouched() {
    if !shipped() {
        return;
    }
    let mut h = feed_hive(&[]);
    for (turn, value) in [("t1", Some("1")), ("t2", Some("0")), ("t3", None)] {
        let mut hop = order_head(Some(FEED_LIVE), turn, "renew");
        if let Some(v) = value {
            hop["at_turn_end"] = json!(v);
        }
        order(&mut h, hop, json!({"topic": "garden"}));
        let sent = feeds(&h);
        assert_eq!(
            sent.len(),
            1,
            "{turn}: sent at once, not held: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert_eq!(
            sent[0].hop.get("at_turn_end"),
            value.map(|v| json!(v)).as_ref(),
            "{turn}: the field as the order gave it: {:?}",
            sent[0].hop
        );
        assert_eq!(body_of(&sent[0]), json!({"topic": "garden"}));
    }
    assert_eq!(feed_marks(&h).len(), 3, "{:?}", feed_marks(&h));
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
