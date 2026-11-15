//! GH #1528 (d58-3 ballast, R-39-5) -- a bridge whose result stands on the
//! wall leaves the window at the next rebuild, whole: no line, no id.
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
//! A bridge left as a one-line form would still be a sentence of the
//! companion in the window, which a model can pick up ("as I said, I am
//! looking it up"); left out, the round reads as the person's words and the
//! result, the way it was meant. The way back stays (`history_read`).

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
fn a_bridge_with_its_result_leaves_at_the_rebuild() {
    let (wall, keys) = phone_bridge::wall();
    let (_, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let window = o.window();
    for (n, bridge) in keys.gone.iter().enumerate() {
        let i = wall
            .pieces
            .iter()
            .position(|p| p.key == *bridge)
            .expect("the bridge is a row of the wall");
        assert_eq!(
            o.shown(&wall, i),
            Shown::Absent,
            "{}: the bridge {bridge} is gone from the window\n{}",
            o.who(),
            o.view()
        );
        let id = wall.pieces[i].id();
        assert!(
            window
                .iter()
                .all(|t| !t.contains(bridge.as_str()) && !t.contains(&format!("#{id}"))),
            "{}: neither the bridge {bridge} nor its id stand in the window\n{}",
            o.who(),
            o.view()
        );
        let (words, result) = &keys.gone_rounds[n];
        for key in [words, result] {
            let j = wall
                .pieces
                .iter()
                .position(|p| p.key == *key)
                .expect("a row of the round");
            assert_eq!(
                o.shown(&wall, j),
                Shown::Whole,
                "{}: {key} of the round of {bridge} stays whole\n{}",
                o.who(),
                o.view()
            );
        }
    }
}
