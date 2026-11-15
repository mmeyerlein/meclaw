//! GH #1528 (d58-3 ballast, R-39-5) -- a bridge the rebuild left out stays
//! readable through `history_read` by its id, word for word.
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
fn the_original_stays_readable() {
    let (wall, keys) = phone_bridge::wall();
    let (mut h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    for bridge in &keys.gone {
        let i = wall
            .pieces
            .iter()
            .position(|p| p.key == *bridge)
            .expect("the bridge is a row of the wall");
        let p = &wall.pieces[i];
        assert_eq!(
            o.shown(&wall, i),
            Shown::Absent,
            "{}: {} is gone",
            o.who(),
            p.key
        );
        let read = history_read(&mut h, &wall.session, "bank-read", &p.id());
        assert!(
            read.contains(p.text()),
            "{}: history_read #{} brings {} back whole: {read}",
            o.who(),
            p.id(),
            p.key
        );
    }
}
