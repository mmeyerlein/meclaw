//! GH #1528 (d58-3 ballast) -- the bank: on a phone day the time and the
//! weather asked go into one line each, how the person is stays.
//!
//! WHY this file exists: the statements of the bank (`curator_bank.rs`)
//! measure what a rebuild collects against a wall whose every row is counted.
//! The wall `phone_asked` counts the class `ballast`: the time asked three
//! times, the weather twice, how the person slept once. The statements hold
//! the judge's class to the same rules as every other class: its garbage is
//! out of the window and its keywords with it (statement 1), every word of
//! the person and every answer in the raw part stands byte for byte
//! (statement 2), and the `gc` mark counts the class and adds up with the
//! blocks the window shows in the class's one-line form (statement 5).
//!
//! Pinned on the wall `phone_asked`, the judge a stub, the run the rebuild
//! after the cache ran out (`phone_asked::judged_run`): the counts of the
//! wall; statements 1, 2 and 5; every asked round outside the newest rounds
//! shown in the one-line form; the asked round among the newest and the
//! round of how the person slept whole.

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
fn bank_ballast_is_out() {
    let bank = bank();
    let (wall, keys) = phone_asked::wall();

    // The counts of the wall: three times the time, twice the weather, once
    // how the person is.
    let time = wall
        .asked
        .iter()
        .filter(|(_, l)| l.contains("the time"))
        .count();
    let weather = wall
        .asked
        .iter()
        .filter(|(_, l)| l.contains("weather") || l.contains("rain"))
        .count();
    assert_eq!(
        (time, weather),
        (3, 2),
        "the wall asks the time three times, the weather twice"
    );
    assert_eq!(
        keys.asked_out.len(),
        4,
        "four asked rounds lie outside the newest rounds"
    );
    assert_eq!(
        wall.counted(|w| *w == What::Junk(BALLAST)).len(),
        8,
        "their eight rows are counted as ballast"
    );

    let mut found = Findings::default();
    for run in &wall.runs {
        let Some(o) = found.judge("rebuild", || rebuild(&wall, run)) else {
            continue;
        };
        found.judge("statement 1", || garbage_is_out(&bank, &wall, &o));
        found.judge("statement 2", || talk_is_whole(&wall, &o));
        found.judge("statement 5", || marks_count(&bank, &wall, &o));
        found.judge("asked rounds in one line", || {
            for key in &keys.asked_out {
                for i in round_of(&wall, key) {
                    assert_eq!(
                        o.shown(&wall, i),
                        Shown::Form(Form::Asked),
                        "{}: {} is shown as asked\n{}",
                        o.who(),
                        wall.pieces[i].key,
                        o.view()
                    );
                }
            }
        });
        found.judge("recent and wellbeing raw", || {
            for key in [&keys.asked_recent, &keys.wellbeing] {
                for i in round_of(&wall, key) {
                    assert_eq!(
                        o.shown(&wall, i),
                        Shown::Whole,
                        "{}: {} stands whole",
                        o.who(),
                        wall.pieces[i].key
                    );
                }
            }
        });
    }
    found.finish();
}
