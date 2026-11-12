//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 1, "the
//! constraint") -- a side call stays in when its condition does not hold.
//!
//! WHY this file exists: only the cut (`cut_tools`) knows whether a call
//! gets a required tool round -- it reads the tool leaves of the window and
//! the channel's own scope, which the question to the lead does not carry.
//! So the lead names a side call under `must_call` for every turn whose stamp
//! needs a tool, and the policy drops it where the cut leaves no tool open:
//! no side call, no `calls` row, no snapshot. Measured shape: the channel
//! allows only a tool outside the target's class, so the cut opens none.

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
fn gh_d58_moveout_a_side_call_stays_in_when_it_does_not() {
    if !shipped() {
        return;
    }
    // The lead cannot see the cut: it names the entry under `must_call`.
    let v = verdict(
        lead_knobs(json!({})),
        "simple",
        &["tool"],
        &["weather"],
        LIVE_H,
    );
    assert_eq!(
        sides_of(&v, MUST_CALL).len(),
        1,
        "the verdict names the side call under its condition: {v}"
    );

    // The cut opens no tool: the condition does not hold, nothing goes.
    let mut h = moveout_hive(&[]);
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("will it rain in Hamburg tomorrow?")],
               "tool_scope": {"allow": ["calendar"]}}),
    );
    let main = main_calls(&h);
    assert_eq!(
        main.len(),
        1,
        "the main call goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        main[0].hop.get("interim").is_none(),
        "no required tool round -- the setup holds: {:?}",
        main[0].hop
    );
    assert!(side_calls(&h).is_empty(), "no side call: {:?}", h.out);
    assert_eq!(
        h.routed("quick").len(),
        1,
        "no second call of any kind: {:?}",
        h.out
    );
    let rows = h.rows("SELECT call_id, trigger FROM calls ORDER BY call_id");
    assert!(
        rows.iter()
            .all(|r| r[1] != json!("side") && r[1] != json!("bridge")),
        "no calls row of a side call: {rows:?}"
    );

    // `external` without an errand: the shipped lead names that entry only
    // with an errand, so the policy's side of it is driven directly -- an
    // entry under `external` stays in while the call stays in the composite,
    // and goes when it leaves.
    let side = json!([{"when": EXTERNAL, "target": "quick", "window": "turn", "text": "x"}]);
    for (external, want) in [(false, 0), (true, 1)] {
        let (got, err) = policy_scope(
            json!({}),
            "len(holding_entries(ARGS[0], ARGS[1], False))",
            json!([side, external]),
        );
        assert_eq!(got, json!(want), "external {external}: {err}");
    }
}
