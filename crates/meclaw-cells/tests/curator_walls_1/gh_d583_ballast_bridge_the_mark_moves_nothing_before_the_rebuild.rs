//! GH #1528 (d58-3 ballast, R-39-5) -- the mark of a bridge moves nothing
//! before the next rebuild: the calls after it send what the same wall
//! sends with its bridges unmarked, and the watch of the prefix counts no
//! break.
//!
//! WHY this file exists: a bridge such as "One moment, I am looking it up"
//! is no content, only a consequence of the latency of the look-up (the
//! owner, R-39-5, 14.11.). The curator makes it itself -- its side call
//! beside the round's call -- and marks it when its answer comes back; once
//! the result of the look-up stands on the wall, the bridge is settled
//! ballast. A fixed class, no judge: the switch `ballast_judge` stays as it
//! ships.
//!
//! Pinned on the wall `phone_bridge` (`support/curator_bank/phone_bridge.rs`).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::*;
use curator_hive::texts;
use meclaw_core::serde_json::json;

#[test]
fn the_mark_moves_nothing_before_the_rebuild() {
    let (wall, keys) = phone_bridge::sown();
    let wall = wall.knob("policy", "snapshot_window", json!("1"));
    let run = Run::of("quick");

    let (marked, [m1, m2]) = quiet_turns(&wall, &run);
    let (_, [u1, u2]) = quiet_turns(&wall.clone().unmarked(), &run);

    let marks = marked.rows("SELECT turn_id FROM marks WHERE kind = 'side_answer' ORDER BY seq");
    assert_eq!(
        marks.len(),
        keys.gone.len() + 2,
        "every bridge left its mark: {marks:?}"
    );
    assert_eq!(
        texts(&m1),
        texts(&u1),
        "the first call sends what the unmarked wall sends"
    );
    assert_eq!(
        texts(&m2),
        texts(&u2),
        "the second call sends what the unmarked wall sends"
    );
    for bridge in &keys.gone {
        assert!(
            texts(&m1).iter().any(|t| t.contains(bridge.as_str())),
            "the bridge {bridge} stands before the rebuild"
        );
    }
    let stamp = curate_stamp(&marked);
    assert_eq!(
        stamp["prefix_break"],
        json!(0),
        "the watch of the prefix counts no break: {stamp}"
    );
}
