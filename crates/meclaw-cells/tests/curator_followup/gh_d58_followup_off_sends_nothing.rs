//! D58 followup (GH #1479, contract point 3) -- off (shipped) the cell sends
//! nothing.
//!
//! WHY this file exists: the cell ships off (`enabled` "0"), like the
//! curator's `wellbeing_line` "0" it replaces; the set switches it on (strand
//! U1 moves the switch of soul2me's set from `curator_form` to the cell).
//! Off, a classed fact above the bound leaves no message in any child and
//! nothing at the rim but the section itself (once, as before the branch),
//! and the cell does not touch its store. The other knobs ship with it: the
//! least confidence (0.6), the text form of the note (it carries the claim,
//! `{claim}`), the instance's time zone ("" reads as UTC) and the test clock
//! (`now_utc` "", the wall clock in a colony).
//!
//! Red until the cell exists.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_followup_off_sends_nothing() {
    if !shipped() {
        return;
    }
    let params = child_config(CELL)["params"].clone();
    assert_eq!(params[KNOB_ON], json!("0"), "shipped off: {params}");
    assert_eq!(params[KNOB_MIN].as_f64(), Some(0.6), "{params}");
    assert!(
        params[KNOB_TEXT]
            .as_str()
            .is_some_and(|t| t.contains("{claim}")),
        "the text form carries the claim: {params}"
    );
    assert_eq!(params[KNOB_ZONE], json!(""), "{params}");
    assert_eq!(params[KNOB_CLOCK], json!(""), "{params}");

    let mut a = Assistant::with(&[(KNOB_CLOCK, json!(NOON))]);
    a.leave(
        "./talky",
        memory_out(
            "text-1",
            "t1",
            json!([fact(
                "Ada",
                "has_symptom",
                "Ada feels unwell and slept badly.",
                Some(0.9),
                true
            )]),
        ),
    );
    let into_cells: Vec<&String> = a.into.iter().map(|(c, _)| c).collect();
    assert!(a.into.is_empty(), "nothing into a child: {into_cells:?}");
    // The section itself still leaves at the rim, once, as before the branch.
    assert_eq!(a.out.len(), 1, "the sidecar leaves once: {:?}", a.out);
    assert_eq!(a.out[0].route(), "sidecar", "{:?}", a.out[0].hop);
    assert!(
        a.ops.is_empty(),
        "off, the store stays untouched: {:?}",
        a.ops
    );
    // Nor does the end of the session that heard it (OR-d58-2.followup.19).
    session_ends(&mut a, "./talky", "text-1");
    assert!(
        a.into.is_empty(),
        "nothing into a child at the end: {:?}",
        a.into
    );
    assert!(a.ops.is_empty(), "off, the end reads nothing: {:?}", a.ops);
}
