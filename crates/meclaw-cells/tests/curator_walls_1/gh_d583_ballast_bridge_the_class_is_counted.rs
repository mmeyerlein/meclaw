//! GH #1528 (d58-3 ballast, R-39-5) -- the `gc` mark of the rebuild counts
//! every bridge it left out in the class `ballast`, one block each.
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

#[test]
fn the_class_is_counted() {
    let (wall, keys) = phone_bridge::wall();
    let (_, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let mark = o.gc_mark();
    assert_eq!(
        mark["classes"][BALLAST],
        meclaw_core::serde_json::json!(keys.gone.len()),
        "{}: the gc mark counts the bridges left out: {mark}",
        o.who()
    );
}
