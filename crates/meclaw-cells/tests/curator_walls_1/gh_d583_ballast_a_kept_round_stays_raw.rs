//! GH #1528 (d58-3 ballast) -- a round the judge keeps stays raw.
//!
//! WHY this file exists: the trust side of the judge. How the person is, a
//! date, a promise -- what carries the later talk -- must stand in the window
//! word for word, whatever the judge said about the rounds around it: the
//! rule "never the person's words" has one exception, a round the judge
//! called `asked`, and no other.
//!
//! Pinned on the wall `phone_asked`, the judge on: the judge was asked about
//! the round in which the person says how they slept and about the round
//! with the daughter's visit, and kept them; after the rebuild both rounds
//! stand whole, no line of the ballast names a block of them, and every
//! round the judge kept stands whole.

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
use curator_hive::Msg;

#[test]
fn a_kept_round_stays_raw() {
    let (wall, keys) = phone_asked::wall();
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    for key in [&keys.wellbeing, &keys.date] {
        let reqs: Vec<&Msg> = h
            .judged
            .iter()
            .filter(|r| request_text(r).contains(key.as_str()))
            .collect();
        assert_eq!(reqs.len(), 1, "{}: the judge is asked about {key}", o.who());
        assert_eq!(verdict_of_the_wall(&wall, reqs[0]), Judged::Keep, "{key}");
    }
    let kept: Vec<usize> = (0..wall.pieces.len())
        .filter(|i| matches!(wall.pieces[*i].what, What::Words | What::Answer))
        .collect();
    for i in kept {
        let p = &wall.pieces[i];
        if wall
            .asked
            .iter()
            .any(|(k, _)| round_of(&wall, k).contains(&i))
        {
            continue;
        }
        assert_eq!(
            o.shown(&wall, i),
            Shown::Whole,
            "{}: the kept row {} stands whole\n{}",
            o.who(),
            p.key,
            o.view()
        );
    }
    for t in o.window() {
        for id in asked_ids(&t) {
            for key in [&keys.wellbeing, &keys.date] {
                for i in round_of(&wall, key) {
                    assert_ne!(
                        id,
                        format!("#{}", wall.pieces[i].id()),
                        "{}: a kept round is in no line of the ballast: {t}",
                        o.who()
                    );
                }
            }
        }
    }
}
