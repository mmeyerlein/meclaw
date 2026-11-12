//! D58 feed (GH #1481, plan `u-feed.md` contract point 2, review F-W5) -- a
//! lost claim stays silent.
//!
//! WHY this file exists: the tool fact tries its claim at every curate of a
//! turn from the second iteration on, the interrupt at every partial; a
//! claim that did not take sent nothing and wrote nothing (`ft-send`,
//! `ie-send`: "however many partials say the same"). A mark for every lost
//! claim would be new and would come with every repetition. An order whose
//! claim another chain already holds sends nothing and books nothing; of two
//! equal orders arriving together exactly one goes, with one mark.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn once_head(turn: &str) -> Value {
    let mut hop = order_head(Some(FEED_LIVE), turn, "fact");
    hop["once"] = json!("k1");
    hop
}

#[test]
fn gh_d58_feed_a_lost_claim_stays_silent() {
    if !shipped() {
        return;
    }
    // The claim is held already (another chain took it).
    let mut h = feed_hive(&[]);
    let key = claim_key(FEED_LIVE, "t1", "k1");
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, r#"{"parked_at":"2026-11-10T08:00:00.000000Z"}"#],
    )
    .expect("the claim row");
    order(&mut h, once_head("t1"), fact_body("The doctor called."));
    assert!(
        feeds(&h).is_empty(),
        "a lost claim sends nothing: {:?}",
        h.out
    );
    assert!(
        feed_marks(&h).is_empty(),
        "a lost claim books nothing: {:?}",
        feed_marks(&h)
    );
    assert_eq!(claims(&h), vec![key], "the holder's claim stays");

    // Two equal orders at once: one goes, one mark.
    let mut h = feed_hive(&[]);
    h.out.clear();
    let ctx = json!({"audience_set": ROUND_E});
    h.lanes_together(vec![
        (
            LANE,
            ctx.clone(),
            once_head("t2"),
            fact_body("The doctor called."),
        ),
        (LANE, ctx, once_head("t2"), fact_body("The doctor called.")),
    ]);
    assert_eq!(feeds(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(feed_marks(&h).len(), 1, "{:?}", feed_marks(&h));
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
