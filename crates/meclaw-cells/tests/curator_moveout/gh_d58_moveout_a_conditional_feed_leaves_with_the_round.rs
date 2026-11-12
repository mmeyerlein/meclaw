//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 6) -- a
//! conditional feed leaves with the round.
//!
//! WHY this file exists: the waiting-time advise (`horizon`, GH #1400) is a
//! decision about the conversation, and it moves to the lead. The lead
//! computes it at the time of the question from its own booked durations and
//! hands it to the curator as an entry of the verdict's `feed`: `{when,
//! section, body}` under the same two conditions as a side call. Where the
//! condition holds, the policy sends the body unchanged as a `live_feed` WITH
//! the sends of the round (held with them, gone with them), over the one
//! road of strand `feed` -- its mark is `feed`, no more `horizon_sent`. For
//! the voice nothing changes: `seconds`, `way` and `family` as before.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_feed::{body_of, feed_marks_of, marks_of};
use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_moveout_a_conditional_feed_leaves_with_the_round() {
    if !shipped() {
        return;
    }
    let knobs = lead_knobs(json!({"horizon_advise": "1",
                                  "horizon_defaults_ms": json!({"weather": 2000}).to_string()}));
    let v = verdict(knobs, "simple", &["tool"], &["weather"], LIVE_H);
    let entries = feeds_of_verdict(&v, MUST_CALL);
    assert_eq!(
        entries.len(),
        1,
        "the lead names the advise under `must_call`: {v}"
    );
    assert_eq!(entries[0]["section"], json!("horizon"), "{v}");
    assert_eq!(
        entries[0]["body"]["seconds"],
        json!(2),
        "from the lead's defaults: {v}"
    );
    assert_eq!(entries[0]["body"]["family"], json!("weather"), "{v}");

    let mut h = moveout_hive(&horizon_knobs(json!({"weather": 2000})));
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("will it rain in Hamburg tomorrow?")]}),
    );
    let feeds = horizons(&h);
    assert_eq!(feeds.len(), 1, "one advise: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        body_of(&feeds[0]),
        json!({"section": "horizon", "seconds": 2, "way": "quick", "family": "weather"}),
        "the body the voice reads, unchanged"
    );
    assert!(
        feeds[0].hop.get("at_turn_end").is_none(),
        "{:?}",
        feeds[0].hop
    );
    assert_eq!(feeds[0].hop["live"], json!(LIVE_H));
    assert_eq!(feeds[0].hop["turn_id"], json!("t1"));
    assert_eq!(
        (side_calls(&h).len(), main_calls(&h).len()),
        (1, 1),
        "it left in the same step as the round's calls: {:?}",
        h.out
    );
    assert_eq!(
        feed_marks_of(&h, "horizon").len(),
        1,
        "booked as every sent feed: the mark `feed`"
    );
    assert!(
        marks_of(&h, "horizon_sent").is_empty(),
        "the old mark is gone"
    );

    // No tool needed: the lead names no entry, no advise.
    seam(
        &mut h,
        "t2",
        "0",
        json!({"live": LIVE_H, "path": "simple", "families": "weather"}),
        json!({"messages": [user("thank you")]}),
    );
    assert!(horizons(&h).is_empty(), "{:?}", h.out);

    // The lead names the entry (above: a tool stamp gives it under
    // `must_call`), but the cut opens no tool -- the channel allows only a
    // tool outside the target's class (as in
    // `…a_side_call_stays_in_when_it_does_not`): the policy's condition does
    // not hold, nothing of the entry goes.
    seam(
        &mut h,
        "t3",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("and on Sunday?")],
               "tool_scope": {"allow": ["calendar"]}}),
    );
    let main = main_calls(&h);
    assert_eq!(main.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        main[0].hop.get("interim").is_none(),
        "no required tool round -- the setup holds: {:?}",
        main[0].hop
    );
    assert!(horizons(&h).is_empty(), "no advise: {:?}", h.out);
    assert_eq!(
        feed_marks_of(&h, "horizon").len(),
        1,
        "the one mark of t1, none for t3"
    );

    // The policy's side of the condition, alike for `feed` and `side`
    // (`holding_entries`): an `external` entry goes only when the call leaves
    // the composite -- the lead never names one without an errand, so the
    // shipped lead cannot drive this -- and every entry that holds goes
    // (review A M1).
    let entries = json!([{"when": EXTERNAL, "section": "a"}, {"when": MUST_CALL, "section": "b"},
                         {"when": "never", "section": "c"}]);
    for (external, must_call, want) in [
        (false, false, json!([])),
        (true, false, json!(["a"])),
        (false, true, json!(["b"])),
        (true, true, json!(["a", "b"])),
    ] {
        let (got, err) = policy_scope(
            json!({}),
            "[e['section'] for e in holding_entries(ARGS[0], ARGS[1], ARGS[2])]",
            json!([entries, external, must_call]),
        );
        assert_eq!(
            got, want,
            "external {external}, must_call {must_call}: {err}"
        );
    }
}
