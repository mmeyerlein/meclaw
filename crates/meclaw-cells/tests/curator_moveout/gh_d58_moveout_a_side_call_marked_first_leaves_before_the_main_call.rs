//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 1, `first`) --
//! a side call marked `first` leaves before the main call.
//!
//! WHY this file exists: in a tool round the side call and the required call
//! may go to the same stateful cell, which runs one `handle()` at a time; the
//! side call goes FIRST so its sentence is heard while the lookup runs
//! (GH #1260 fix round 1, OR-WG-8). Beside an errand the main call goes
//! elsewhere and the side call follows it. The order is no longer the
//! curator's knowledge of a tool round: the lead marks the entry `first`,
//! the policy sends in that order.

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
fn gh_d58_moveout_a_side_call_marked_first_leaves_before_the_main_call() {
    if !shipped() {
        return;
    }
    let v = verdict(
        lead_knobs(json!({})),
        "simple",
        &["tool"],
        &["weather"],
        LIVE_H,
    );
    let must = sides_of(&v, MUST_CALL);
    assert_eq!(must.len(), 1, "{v}");
    assert!(
        flag(&must[0]["first"]),
        "the tool round's side call is marked first: {v}"
    );
    let v = verdict(lead_knobs(json!({})), "complex", &[], &["web"], LIVE_H);
    let ext = sides_of(&v, EXTERNAL);
    assert_eq!(ext.len(), 1, "{v}");
    assert!(
        !flag(&ext[0]["first"]),
        "the errand's side call is not: {v}"
    );

    let mut h = moveout_hive(&[]);
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("will it rain in Hamburg tomorrow?")]}),
    );
    let (side, main) = (side_calls(&h), main_calls(&h));
    assert_eq!(
        (side.len(), main.len()),
        (1, 1),
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        position(&h, &side[0]) < position(&h, &main[0]),
        "first: the side call leaves before the main call: {:?}",
        h.out
    );

    seam(
        &mut h,
        "t2",
        "0",
        errand_stamp("complex"),
        json!({"messages": [user("plan my week around the move")]}),
    );
    let (side, main) = (side_calls(&h), main_calls(&h));
    assert_eq!(
        (side.len(), main.len()),
        (1, 1),
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        position(&h, &main[0]) < position(&h, &side[0]),
        "not first: the side call follows the main call: {:?}",
        h.out
    );
}
