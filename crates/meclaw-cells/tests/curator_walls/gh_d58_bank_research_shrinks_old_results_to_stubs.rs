//! GH #1465 (D58, the bank) -- a research: old search results shrink to
//! one-line forms.
//!
//! WHY this file exists: a research is few turns and many long results, and
//! nearly all of its window is results nobody reads twice. On the wall
//! `research` of the bank every result outside `keep_rounds` is a one-line
//! form that names its id after the rebuild, and its keyword is gone; the
//! results of the last round stand whole, the talk stands byte for byte,
//! and the window lies at the aim of the role. The role is a parameter of
//! the run; the wall runs with its presets and once with `gc` "1".
//!
//! Runs the shipped curator hive over the bank (`support/curator_bank.rs`).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::research::{RESULTS, ROUNDS};
use curator_bank::*;
use curator_hive::shipped;

#[test]
fn research_shrinks_old_results_to_stubs() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let runs = run_walls(&bank, |w| w.name == "research", &mut found);
    assert_eq!(
        runs.len(),
        2,
        "the wall research runs twice: {:?}",
        found.lines
    );
    for (wall, o) in &runs {
        let who = o.who();
        let window = o.window();

        // Every old result: a one-line form with its id, the keyword gone.
        let old = wall.counted(|w| matches!(w, What::Junk(_)));
        assert_eq!(old.len(), (ROUNDS - 1) * RESULTS, "the old results");
        for i in old {
            let p = &wall.pieces[i];
            let shown = o.shown(wall, i);
            if shown != Shown::Form(Form::Shortened) {
                found.note(format!(
                    "{who}: the old result {} (#{}) is no one-line form with its id: {shown:?}",
                    p.key,
                    p.id()
                ));
            }
            if window.iter().any(|t| t.contains(&p.key)) {
                found.note(format!(
                    "{who}: the keyword of the old result {} is still in the window",
                    p.key
                ));
            }
        }

        // The results of the last round stand whole.
        let last: Vec<usize> = (0..wall.pieces.len())
            .filter(|i| {
                wall.pieces[*i].kind == "tool_result" && wall.pieces[*i].what == What::Other
            })
            .collect();
        assert_eq!(last.len(), RESULTS, "the results of the last round");
        for i in last {
            let shown = o.shown(wall, i);
            if shown != Shown::Whole {
                found.note(format!(
                    "{who}: the result {} of the last round does not stand whole: {shown:?}",
                    wall.pieces[i].key
                ));
            }
        }

        found.judge(&format!("{who}: the talk"), || talk_is_whole(wall, o));
        found.judge(&format!("{who}: the aim"), || lands_at_aim(o));
    }
    found.finish();
}
