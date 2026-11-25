//! D58 lead (GH #1474, contract point 3) -- without a lead no event and no
//! note leaves.
//!
//! WHY this file exists: the curator tells its switch what only the window
//! sees -- a note at every closed turn (`turn_end`), at an escalation, at a
//! consult the speaking model started (`lead_note`), and the delta stamp
//! (GH #1584: since the speculation is gone it rides `lead_note` too). With
//! the switch a hive of its own, every one of them crosses the curator's
//! boundary; in a composite without a lead each would end as a dead letter
//! `hive_no_route` there, one per turn at least. Without a lead none is
//! sent: whole turns, an errand the speaking model starts itself and a delta
//! stamp reach the curator, and nothing
//! leaves on a lane to a lead.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) without a lead;
//! no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_lead_without_a_lead_no_event_and_no_note_leaves() {
    if !shipped() {
        return;
    }
    let mut h = bare(&[("intake", "errand_tool", json!("ask_advice"))]);
    let mut seen = Vec::new();
    // Two whole turns: each closes with a note to the lead when one is wired.
    let first = turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
    seen.extend(to_lead(&h));
    turn(&mut h, "s1", "t2", "how are you?", "fine", json!({}));
    seen.extend(to_lead(&h));
    // The speaking model starts an errand itself: a note `errand` when wired.
    let call = h.curate(
        "s1",
        "t3",
        0,
        json!([user("plan my week")]),
        mode("Be brief."),
    );
    seen.extend(to_lead(&h));
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("e1", "ask_advice")]),
    );
    seen.extend(to_lead(&h));
    // The delta stamp, as a composite with a fast voice sends it.
    let ctx = json!({"session_id": "s1", "turn_id": "t4", "iter": "0",
                     "curator_call": first.hop["curator_call"], "audience_set": ROUND_E});
    h.out.clear();
    h.lane(
        "in_delta_stamp",
        ctx,
        json!({"session_id": "s1", "turn_id": "t4"}),
        json!({"messages": [], "stamp": {"turn_id": "t4", "path": "simple",
                                          "text_norm": "plan my week"}}),
    );
    seen.extend(to_lead(&h));
    assert!(
        seen.is_empty(),
        "without a lead no event and no note leaves the curator: {seen:?}"
    );
    assert!(
        h.lead_ops.is_empty(),
        "without a lead nobody touches its store: {:?}",
        h.lead_ops
    );
}
