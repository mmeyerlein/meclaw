//! GH #1528 (d58-3 ballast, re-check I-neu-1) -- a round whose block is not
//! read is never judged.
//!
//! WHY this file exists: the mark names the rows the judge was shown
//! (`seen`), and the one line takes exactly those. The policy reads the
//! words of a closed round by hash before it asks the judge; when the store
//! does not give one of those blocks, the judge would be shown the round
//! without that row while the mark still named it -- the line would take a
//! row no model judged (R-583-2). So a round with a block that is not read
//! is not asked about at all: no request, no mark, the round stays raw.
//!
//! Pinned on the wall `phone_asked_beside`, the judge on: the store answers
//! the read of the round `b1-5` without the block of its answer (the bank
//! asserts the read happened), then gives it back. No request names the
//! round's words, the ledger holds no mark of the ballast for the round,
//! and after the rebuild both rows stand whole, named by no line.

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
fn a_block_not_read_asks_nothing() {
    let (wall, keys) = phone_asked::wall_beside();
    assert_eq!(
        wall.unread,
        vec![keys.unread.clone()],
        "the wall hides one block"
    );
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let round = round_of(&wall, &keys.unread_words);
    let turn = wall.pieces[round[0]].turn.clone();

    assert!(
        h.judged
            .iter()
            .all(|r| !request_text(r).contains(&keys.unread_words)),
        "{}: the judge is not asked about a round whose block was not read",
        o.who()
    );
    let marks = h.rows(&format!(
        "SELECT seq, value FROM marks WHERE kind = 'ballast' AND turn_id = '{turn}'"
    ));
    assert!(
        marks.is_empty(),
        "{}: no mark of the ballast for the round {turn}: {marks:?}",
        o.who()
    );

    let window = o.window();
    for i in &round {
        let id = wall.pieces[*i].id();
        assert!(
            window
                .iter()
                .all(|t| !asked_ids(t).contains(&format!("#{id}"))),
            "{}: no line of the ballast names the row {}\n{}",
            o.who(),
            wall.pieces[*i].key,
            o.view()
        );
        assert_eq!(
            o.shown(&wall, *i),
            Shown::Whole,
            "{}: the row {} of the round stands whole\n{}",
            o.who(),
            wall.pieces[*i].key,
            o.view()
        );
    }
}
