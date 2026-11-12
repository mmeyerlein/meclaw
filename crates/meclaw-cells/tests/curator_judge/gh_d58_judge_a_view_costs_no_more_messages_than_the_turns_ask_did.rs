//! D58 judge (GH #1480, contract point 1) -- a view costs no more messages
//! than the `turns_ask` did.
//!
//! WHY this file exists: the judge sits in front of every turn; its context
//! must not lengthen the way the stamp waits on. `turns_ask` with
//! `judge: true` cost 6 messages (plan review `plan-review-judge.md`, the flow
//! today, 7): the ask into `./history`, two ledger rounds out and back, the
//! one answer. A view costs the same 6 -- the contributions of the domains
//! are read in the first round, never in a third one. Counted in MESSAGES,
//! never in time (development-rules § 11).
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

/// The number of messages the `turns_ask` with `judge: true` took.
const TURNS_ASK_MESSAGES: usize = 6;

/// The messages of one ask: the ask itself, each ledger message out and its
/// answer back, and every answer of the cell.
fn messages_of(run: &Run) -> usize {
    1 + 2 * run.store.len() + run.out.len()
}

#[test]
fn gh_d58_judge_a_view_costs_no_more_messages_than_the_turns_ask_did() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    said_turn(&mut h, 3, "s-now", "t2", "its concerts", "");
    said_turn(&mut h, 5, "s-now", "t3", "and the price", "");
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let _ = part(&h, &params, "decide", "affinity", hints_value(), "member:e");

    let run = view_ask(&h, &params, "decide", "s-now", "t4");
    let view = the_view(&run);
    assert_eq!(
        said_in(&view).len(),
        6,
        "check of the setup: the view read the turns: {:?}",
        view.body
    );
    assert!(
        messages_of(&run) <= TURNS_ASK_MESSAGES,
        "a view took {} messages, the turns_ask took {TURNS_ASK_MESSAGES}: {} ledger messages, {} answers",
        messages_of(&run),
        run.store.len(),
        run.out.len()
    );

    // A view without a target is no dearer.
    let plain = view_ask(&h, &params, "", "s-now", "t4");
    let _ = the_view(&plain);
    assert!(
        messages_of(&plain) <= TURNS_ASK_MESSAGES,
        "a view without a target took {} messages",
        messages_of(&plain)
    );
}
