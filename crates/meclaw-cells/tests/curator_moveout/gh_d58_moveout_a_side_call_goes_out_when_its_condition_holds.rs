//! D58 moveout (GH #1476, plan `u-moveout.md` contract points 1 and 2) -- a
//! side call goes out when its condition holds.
//!
//! WHY this file exists: the curator decided itself when a bridge went
//! (`way in (consult, research) or tool_round`) and what kind it was. Now the
//! lead's verdict carries `side`, a list of entries with a condition: under
//! `external` (the main call leaves the composition) and under `must_call`
//! (the cut gives the call a required tool round). The policy only checks
//! the condition and sends the side call -- a model call of its own with its
//! own `calls` row (trigger `side`, not `bridge`) and its own snapshot, its
//! kind in the neutral head field `call_kind`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_moveout_a_side_call_goes_out_when_its_condition_holds() {
    if !shipped() {
        return;
    }
    // The verdict names the side call and its condition, never the curator.
    let v = verdict(lead_knobs(json!({})), "complex", &[], &["web"], LIVE_H);
    let ext = sides_of(&v, EXTERNAL);
    assert_eq!(
        ext.len(),
        1,
        "a complex turn's verdict carries one side entry under `external`: {v}"
    );
    assert_eq!(ext[0]["target"], json!("quick"), "talky's side target: {v}");
    assert!(
        ext[0]["text"].as_str().is_some_and(|t| !t.is_empty()),
        "the entry carries its text: {v}"
    );
    let v = verdict(
        lead_knobs(json!({})),
        "simple",
        &["tool"],
        &["weather"],
        LIVE_H,
    );
    let must = sides_of(&v, MUST_CALL);
    assert_eq!(
        must.len(),
        1,
        "a tool turn's verdict carries one side entry under `must_call`: {v}"
    );
    assert!(sides_of(&v, EXTERNAL).is_empty(), "{v}");
    // Under speculation the side call has a cell of its own (`bridge_target`).
    let v = verdict(
        lead_knobs(json!({"quick_speculative": "1"})),
        "simple",
        &["tool"],
        &["weather"],
        LIVE_H,
    );
    assert_eq!(
        sides_of(&v, MUST_CALL)[0]["target"],
        json!("quick_bridge"),
        "{v}"
    );

    // The condition holds: the side call goes, as a call of its own.
    let mut h = moveout_hive(&[]);
    for (turn, stamp, ask) in [
        (
            "t1",
            errand_stamp("complex"),
            "plan my week around the move",
        ),
        (
            "t2",
            errand_stamp("research"),
            "find everything on the topic",
        ),
        (
            "t3",
            tool_stamp("weather"),
            "will it rain in Hamburg tomorrow?",
        ),
    ] {
        seam(&mut h, turn, "0", stamp, json!({"messages": [user(ask)]}));
        let sides = side_calls(&h);
        assert_eq!(
            sides.len(),
            1,
            "{turn}: one side call: {:?} {:?}",
            h.out,
            h.stderr
        );
        let s = &sides[0];
        assert!(
            s.hop.get("bridge").is_none(),
            "{turn}: `bridge` left the head: {:?}",
            s.hop
        );
        assert_eq!(
            trigger_of(&h, s),
            SIDE,
            "{turn}: its own calls row, trigger `side`"
        );
        assert!(
            snapshot_of(&h, s).is_some(),
            "{turn}: its own snapshot row: {:?}",
            s.hop
        );
        assert_eq!(
            s.body["tool_scope"],
            json!({"allow": []}),
            "{turn}: no tool"
        );
        assert_eq!(
            main_calls(&h).len(),
            1,
            "{turn}: beside one main call: {:?}",
            h.out
        );
    }
}
