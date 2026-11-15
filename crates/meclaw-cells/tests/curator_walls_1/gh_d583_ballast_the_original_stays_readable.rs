//! GH #1528 (d58-3 ballast) -- a round shown as one line is still readable
//! as it was.
//!
//! WHY this file exists: the one line of a marked round says that something
//! was asked, not what was said. The original window is never deleted (the
//! owner on the curator, 11.11., in translation: "the original context
//! window is never deleted anyway"): the ids in the line are the way back, and `history_read`
//! brings each block back unchanged, so a model asked "did I ask you the time
//! today already?" can answer from the line and, when it needs the words,
//! from the ledger.
//!
//! Pinned on the wall `phone_asked`, the judge on: after the rebuild every
//! block of a marked round is shown in the one-line form, and `history_read`
//! with its id answers with the block's text whole.

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
    let (wall, keys) = phone_asked::wall();
    let (mut h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    for key in &keys.asked_out {
        for i in round_of(&wall, key) {
            let p = &wall.pieces[i];
            assert_eq!(
                o.shown(&wall, i),
                Shown::Form(Form::Asked),
                "{}: {} is shown as asked\n{}",
                o.who(),
                p.key,
                o.view()
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
}
