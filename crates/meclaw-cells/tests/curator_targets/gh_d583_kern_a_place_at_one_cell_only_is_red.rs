//! D58-3 kern (GH #1524, contract point 6, N14) -- a place set at one curator
//! cell only is a finding, whichever cell it is.
//!
//! WHY this file exists: the pin `…one_place_at_both_cells` reads every
//! shipped composition and holds `place` and `timezone` equal at
//! `curator/policy` and `curator/history`. Its check of the setup proved the
//! reader trips for a place at the policy only; a composition that sets the
//! place at the history only (the judge and the seed get a place, the model's
//! frame none) went unproved. Both sides and both values are proved here on
//! the pin's own reader (`place_findings` in `support/curator_hive.rs`): a
//! reader that looks at one cell only turns this test red.
//!
//! Reads the shipped templates (`support/curator_hive.rs` for the paths).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d583_kern_a_place_at_one_cell_only_is_red() {
    if !shipped() {
        return;
    }
    for cell in ["policy", "history"] {
        for (knob, value) in [("place", "Springfield"), ("timezone", "Europe/Berlin")] {
            let over = json!({ cell: { knob: value } });
            let f = place_findings("built here", &over);
            assert!(
                f.iter().any(|l| l.contains(&format!("`{knob}`"))),
                "a {knob} at {cell} only is a finding: {over} -> {f:?}"
            );
        }
    }
    let both = json!({"policy": {"place": "Springfield", "timezone": "Europe/Berlin"},
                      "history": {"place": "Springfield", "timezone": "Europe/Berlin"}});
    assert!(
        place_findings("built here", &both).is_empty(),
        "the same values at both cells are no finding"
    );
}
