//! D58 judge (GH #1480, contract point 1) -- a view reads and leaves no
//! trace in the ledger.
//!
//! WHY this file exists: the short way of the view is a READ. Planned
//! against the way of a window through `curator/policy` (plan review B1: at
//! least eight messages, and it writes blocks, the wall and
//! `pending:<call>`), the view goes through `./history` alone: no
//! `./intake`, no lead, no plan, no row in `calls`, no `pending`, no window
//! bound -- at most two ledger rounds (rows, state and contributions in the
//! first, blocks in the second), every operation a `select`, and every table
//! of the ledger holds afterwards exactly the rows it held before.
//!
//! Runs the shipped `./history` against the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;
// harness_fold: #[path = "support/history_view.rs"]
#[allow(unused_imports)]
use crate::history_view; // harness_fold: mod history_view;

use curator_hive::*;
use history_judge::*;
use history_view::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_judge_a_view_reads_and_leaves_no_trace_in_the_ledger() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    said_turn(&mut h, 3, "s-now", "t2", "its concerts", "");
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let _ = part(&h, &params, "decide", "affinity", hints_value(), "member:e");
    let before = ledger_counts(&h);

    let run = view_ask(&h, &params, "decide", "s-now", "t3");
    let view = the_view(&run);
    assert!(
        !leaf(&view, "decide", "affinity").is_null(),
        "check of the setup: the view read the contributions: {:?}",
        view.body
    );
    assert!(
        run.store.len() <= 2,
        "at most two ledger rounds, {} taken: {:?}",
        run.store.len(),
        run.store.iter().map(|m| m.hop.clone()).collect::<Vec<_>>()
    );
    let ops = ledger_operations(&run);
    assert!(
        !ops.is_empty(),
        "check of the setup: the view reads the ledger"
    );
    for op in &ops {
        assert_eq!(op["operation"], json!("select"), "a view only reads: {op}");
        assert_ne!(op["table"], json!("calls"), "a view opens no call: {op}");
    }
    assert_eq!(
        ledger_counts(&h),
        before,
        "every table of the ledger holds the rows it held before the view"
    );
}
