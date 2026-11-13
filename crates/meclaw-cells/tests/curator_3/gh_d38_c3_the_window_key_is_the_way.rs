//! GH #1450 (D38 C3b, fix round 1 of #1451, review o15-Rv1 I4,
//! OR-d38-3.v.21) -- the reported window that cuts a catalogue row is kept
//! per WAY, not per cell.
//!
//! WHY this file exists: the cut clamps a row's `input_soft` to the window
//! the model reported (`context_window:<way>`, review m1 of c3b). The tap
//! would key it per target cell where it knows the cell that answered -- but
//! the answer names none: talky's edges `./brain`, `./quick` and
//! `./quick_bridge` -> `./curator` set only `hop.route` ("in_llm"), the
//! llm cell's hop carries no sender. So `quick`, `quick_spec` and
//! `quick_bridge` share `context_window:quick`; another WAY's window never
//! cuts a row. This pins the foreign case both ways: a quick window never
//! clamps the brain row, and does clamp a quick-way cell's row.
//!
//! D58 target (contract point 7, 10): the answer names its target
//! (`curator_target`), so the window is kept per target
//! (`catalog:<target>:context_window`) and two targets never share one --
//! the foreign case both ways again: another target's window never clamps
//! a row, the target's own always does.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::json;

/// The soft line of a call to `cell` when the model of `window_of` reported
/// a window of 50 000 tokens.
fn soft(cell: &str, window_of: &str) -> i64 {
    let row = canonical(&json!({"model": "test/m", "input_soft": 200_000,
                                "input_hard": 300_000, "chars_per_token": 3.5}));
    let st = json!({format!("catalog:{cell}"): row,
                    format!("catalog:{window_of}:context_window"): "50000"});
    let probe = format!("cut_budget('{cell}', ARGS)['soft']");
    let (got, err) = policy_scope(curator_profiles::set_knobs("talky", json!({})), &probe, st);
    got.as_i64()
        .unwrap_or_else(|| panic!("{cell}/{window_of}: {got} {err}"))
}

/// Pin: a window another target's model reported never cuts the row.
#[test]
fn another_ways_window_never_cuts_the_row() {
    if !shipped() {
        return;
    }
    assert_eq!(soft("brain", "quick"), 200_000);
    for cell in ["quick_spec", "quick_bridge"] {
        assert_eq!(soft(cell, "quick"), 200_000, "{cell}");
    }
}

/// Lock (D58 target, contract point 7): each target's own reported window
/// cuts its own row -- the answer names its target (`curator_target`), so
/// no two targets share a window.
#[test]
fn each_target_has_its_own_window() {
    if !shipped() {
        return;
    }
    for cell in ["brain", "quick", "quick_spec", "quick_bridge"] {
        assert_eq!(soft(cell, cell), 50_000, "{cell}");
    }
}
