//! GH #1451 (D38 V2, T-7) -- the next call knows the open wellbeing, once.
//! Since D58 followup (GH #1479) against the hive `assistant` and the curator.
//!
//! WHY this file exists: in the R039 acceptance call 2 did not ask after the
//! person, who had said in call 1 that they felt unwell and slept badly. The
//! rule first lived in the curator (`wellbeing_line` in `./history` and
//! `./policy`); it moved to the cell `./followup` of the hive `assistant`,
//! which hears the person's facts in the section `memory` and hands the day's
//! newest open concern to the curators as a note of scope `once`. The
//! statements stay: the fact the sidecar classed `wellbeing: true` (GH #1456:
//! the model sets the class; the predicate is an open key and never decides)
//! at a confidence of at least the bound is the one concern the next call
//! knows, once; an unclassed fact of the same turn is none; under the bound
//! nothing is sent. The knobs ship off.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

const CLAIM: &str = "Ada feels unwell and slept badly.";

#[test]
fn the_cell_declares_its_knobs_shipped_off() {
    if !shipped() {
        return;
    }
    let p = child_config(CELL)["params"].clone();
    assert_eq!(p[KNOB_ON], json!("0"), "shipped off: {p}");
    assert!(p[KNOB_MIN].is_number(), "the bound is a number: {p}");
}

/// Pin: the classed fact of the day, not the unclassed one, reaches the next
/// call once; under the bound nothing is sent.
#[test]
fn the_next_call_knows_the_open_wellbeing_once() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    a.leave(
        "./talky",
        memory_out(
            "text-1",
            "t1",
            json!([
                fact(
                    "Ada",
                    "moves_to",
                    "Elias wants to move to Hamburg.",
                    Some(0.9),
                    false
                ),
                fact("Ada", "has_symptom", CLAIM, Some(0.9), true)
            ]),
        ),
    );
    session_ends(&mut a, "./talky", "text-1");
    let notes = live_notes(&a.notes_into("./talky"));
    assert_eq!(notes.len(), 1, "one concern: {:?}", a.into);
    assert!(
        !notes[0]["text"].as_str().unwrap_or("").contains("Hamburg"),
        "{}",
        notes[0]
    );
    let mut h = curator(&[]);
    deliver_notes(&a, "./talky", &mut h);
    let s2 = seed(&mut h, "call-2", "start", 0);
    assert_eq!(count_in(&s2, CLAIM), 1, "{:?}", s2.messages());
    let s3 = seed(&mut h, "call-3", "start", 0);
    assert_eq!(count_in(&s3, CLAIM), 0, "once: {:?}", s3.messages());

    // Under the confidence preset it is no concern.
    let mut weak = Assistant::on_at(NOON);
    weak.leave(
        "./talky",
        memory_out(
            "text-1",
            "t1",
            json!([fact("Ada", "has_symptom", "Maybe tired.", Some(0.1), true)]),
        ),
    );
    session_ends(&mut weak, "./talky", "text-1");
    assert!(weak.into.is_empty(), "{:?}", weak.into);
}
