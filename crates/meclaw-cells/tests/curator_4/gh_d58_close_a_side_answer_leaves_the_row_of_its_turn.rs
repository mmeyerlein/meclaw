//! GH #1471 (wave d58-2 close, inventory 32; roundpin review M-2) -- the
//! answer of a side call leaves the row its turn is cut by.
//!
//! WHY this file exists: a turn keeps what its first call was cut by in a
//! row of its own (`pending:turn_cut:<round>:<session>:<turn>`), and the tap
//! takes the row down with the answer that ends the turn. The review found
//! that any ending answer of a call of the turn took it down, also the
//! answer of a call beside the turn's own that ends nothing -- the next call
//! of the turn was then cut like a first one, by whatever plan stood. Since
//! D58 moveout a side call is known by its kind (`call_kind` "side") and
//! keeps no projection of its own (`window_est:<call>`, the plan its tap
//! takes the row down by), so its answer, a sentence or an error, never
//! takes the turn's row down; the main call's answer that ends the turn
//! does. Mutant: a side call that keeps the main call's projection -> red.
//! (`turn_cut_key` without the side kind stays green here: the tap of a side
//! answer never asks it -- measured, close-b2.)
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn gh_d58_close_a_side_answer_leaves_the_row_of_its_turn() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    for (turn, finish) in [("t1", "stop"), ("t2", "error")] {
        let mut h = moveout_hive(&[
            ("policy", "recall_in_wall", json!("1")),
            ("policy", "snapshot_window", json!("1")),
        ]);
        seam(
            &mut h,
            turn,
            "0",
            errand_stamp("complex"),
            json!({"messages": [user("plan my week around the move")]}),
        );
        let side = side_calls(&h);
        let main = main_calls(&h);
        assert_eq!(
            (side.len(), main.len()),
            (1, 1),
            "{turn}: check of the setup: a side call beside the main call: {:?} {:?}",
            h.out,
            h.stderr
        );
        let kept = turn_cut_row(&h, ROUND_E, SESSION_M, turn);
        assert!(
            kept.is_object(),
            "{turn}: check of the setup: the turn keeps its row: {kept} {:?}",
            h.stderr
        );
        let (extra, said_it) = if finish == "error" {
            (
                json!({"error_code": "timeout", "error_source": "wire"}),
                json!([]),
            )
        } else {
            (json!({}), json!([said("I am on it.")]))
        };
        answer_call(&mut h, &side[0], finish, extra, said_it);
        assert_eq!(
            turn_cut_row(&h, ROUND_E, SESSION_M, turn),
            kept,
            "{turn}: the side call's `{finish}` leaves the row of the turn: {:?}",
            h.stderr
        );
        // Control: the main call's answer ends the turn and its row.
        answer_call(
            &mut h,
            &main[0],
            "stop",
            json!({}),
            json!([said("Here is your week.")]),
        );
        assert_eq!(
            turn_cut_row(&h, ROUND_E, SESSION_M, turn),
            Value::Null,
            "{turn}: the main call's answer ends the turn and takes the row down: {:?}",
            h.stderr
        );
    }
}
