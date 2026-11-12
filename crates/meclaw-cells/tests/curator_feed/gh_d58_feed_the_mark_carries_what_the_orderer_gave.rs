//! D58 feed (GH #1481, plan `u-feed.md` contract points 1 and 4, review
//! F-W4, T-W3) -- the mark carries what the orderer gave.
//!
//! WHY this file exists: three feeds had three body forms and the tool fact
//! booked its tool in a mark of its own (`fact_from_tool {tool}`); an orderer
//! from outside (the lead, strand `moveout`) has figures to book with its
//! feed (the words of a partial, the seconds of a horizon). The order names
//! them in one body field, `mark`: the curator takes that field out of the
//! body -- the feed leaves without it, every other field as ordered -- and
//! lays it, uninterpreted, into the mark `feed` beside the section, the
//! orderer and `once`. The mark stands in the order's session, turn and
//! round.

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
fn gh_d58_feed_the_mark_carries_what_the_orderer_gave() {
    if !shipped() {
        return;
    }
    let mut h = feed_hive(&[]);
    let given = json!({"delta_n": 4, "probe": {"nested": ["as", "given"]}});
    let mut hop = order_head(Some(FEED_LIVE), "t1", "fact");
    hop["once"] = json!("k1");
    order(
        &mut h,
        hop,
        json!({"section": "fact", "payload": "It rains at three.", "mark": given.clone()}),
    );
    let sent = feeds(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        body_of(&sent[0]),
        json!({"section": "fact", "payload": "It rains at three."}),
        "the field `mark` stays with the curator, the rest leaves as ordered"
    );
    let marks = feed_marks(&h);
    assert_eq!(marks.len(), 1, "{marks:?} {:?}", h.stderr);
    assert_eq!(
        marks[0].value,
        json!({"section": "fact", SENDER_KEY: SENDER, "once": "k1", "mark": given}),
        "section, orderer, once and the orderer's mark field, untouched"
    );
    assert_eq!(marks[0].session, SESSION);
    assert_eq!(marks[0].turn, "t1");
    assert_eq!(marks[0].audience, round_e());
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
