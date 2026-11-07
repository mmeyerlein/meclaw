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

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

fn soft(cell: &str, way: &str) -> i64 {
    let row = canonical(&json!({"model": "test/m", "input_soft": 200_000,
                                "input_hard": 300_000, "chars_per_token": 3.5}));
    let st = json!({format!("catalog:{cell}"): row, "context_window:quick": "50000"});
    // `window_key` reads the running message's `ctx` (`sa_stage`), a name the
    // pure half does not bind: the probe binds an empty one first, the way
    // `gh_d38_v1_two_notes_on_one_item_keep_the_latest` binds `AUDIENCE`.
    let probe = format!(
        "(globals().__setitem__('ctx', {{}}), cut_budget('{cell}', '{way}', ARGS)['soft'])[1]"
    );
    let (got, err) = policy_scope(json!({"role": "talky"}), &probe, st);
    got.as_i64()
        .unwrap_or_else(|| panic!("{cell}/{way}: {got} {err}"))
}

/// Pin: a window reported on the quick way never cuts the brain's row.
#[test]
fn another_ways_window_never_cuts_the_row() {
    if !shipped() {
        return;
    }
    assert_eq!(soft("brain", "brain"), 200_000);
}

/// Lock (OR-d38-3.v.21): the cells of one way share its window -- the
/// answer names no cell.
#[test]
fn the_cells_of_one_way_share_its_window() {
    if !shipped() {
        return;
    }
    for cell in ["quick", "quick_spec", "quick_bridge"] {
        assert_eq!(soft(cell, "quick"), 50_000, "{cell}");
    }
}
