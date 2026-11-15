//! GH #1465 (D58, the bank) -- a released block shrinks and a pinned one
//! stays.
//!
//! WHY this file exists: the model trims its own window through the sidecar
//! of an answer (GH #892): a block it releases leaves the window but for
//! one line that names where to read it again, a block it pins stays whole
//! whatever a rebuild would collect. On the wall `released` of the bank,
//! with every role the wall is made for, each released block of the raw
//! part stands as its one-line form after a rebuild and its text is gone,
//! each pinned block stands byte for byte -- the one that was released and
//! pinned in the same answer among them --, and the `gc` mark of the
//! rebuild counts the released blocks.
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

use curator_bank::released::{pinned, released};
use curator_bank::*;
use curator_hive::shipped;
use meclaw_core::serde_json::json;

#[test]
fn a_released_block_shrinks_and_a_pinned_one_stays() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let wall = bank.named("released");
    let (gone, kept) = (released(wall), pinned(wall));
    assert_eq!(gone.len(), 2, "the wall counts two released blocks");
    assert_eq!(kept.len(), 2, "the wall counts two pinned blocks");
    let runs = run_walls(&bank, |w| w.name == "released", &mut found);
    assert!(!runs.is_empty(), "the wall released runs");
    for (wall, o) in &runs {
        let window = o.window();
        let mut raw = 0;
        for i in &gone {
            let (p, shown) = (&wall.pieces[*i], o.shown(wall, *i));
            if !o.raw(*i) {
                continue;
            }
            raw += 1;
            if shown != Shown::Form(Form::Released) {
                found.note(format!(
                    "released: {}: the released block {} is not shown in its one-line form: \
                     {shown:?}",
                    o.who(),
                    p.key
                ));
            }
            if window.iter().any(|t| t.contains(&p.key)) {
                found.note(format!(
                    "released: {}: the text of the released block {} is still in the window",
                    o.who(),
                    p.key
                ));
            }
        }
        if raw == 0 {
            found.note(format!(
                "released: {}: no released block lies in the raw part of the plan",
                o.who()
            ));
        }
        for i in &kept {
            let (p, shown) = (&wall.pieces[*i], o.shown(wall, *i));
            if shown != Shown::Whole {
                found.note(format!(
                    "pinned: {}: the pinned block {} does not stand byte for byte: {shown:?}",
                    o.who(),
                    p.key
                ));
            }
        }
        let counted = o.gc.first().map(|m| m["classes"]["released"].clone());
        if counted != Some(json!(raw)) {
            found.note(format!(
                "mark: {}: the gc mark counts {counted:?} released blocks, the window shows \
                 {raw}",
                o.who()
            ));
        }
    }
    found.finish();
}
