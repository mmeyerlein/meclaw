//! D58 judge (GH #1480, contract points 1, 3 and 7; R-58-14 Nr. 3) -- a
//! second composition orders a view under another name.
//!
//! WHY this file exists: the judge is not talky's alone; the code hive and
//! others will order views too, without a change in the curator. The curator
//! holds no list of targets: a target is the name the orderer gives. Each
//! target gets the family of ITS contributions and nothing of another's; a
//! target nobody contributed to gets the moment and the conversation, and an
//! empty family -- never another target's leaves, never a refusal.
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
fn gh_d58_judge_a_second_composition_orders_a_view_under_another_name() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let _ = part(
        &h,
        &params,
        "review_gate",
        "repository",
        json!({"branch": "main", "open_changes": 3}),
        "",
    );

    let talky = the_view(&view_ask(&h, &params, "decide", "s-now", "t2"));
    let code = the_view(&view_ask(&h, &params, "review_gate", "s-now", "t2"));
    assert_eq!(
        leaf_value(&leaf(&code, "review_gate", "repository")),
        json!({"branch": "main", "open_changes": 3}),
        "the second orderer's target holds its own leaf: {:?}",
        code.body["system"]
    );
    assert!(
        code.body["system"].get("decide").is_none(),
        "never another target's family: {:?}",
        code.body["system"]
    );
    assert!(
        talky.body["system"].get("review_gate").is_none()
            && !leaf(&talky, "decide", "presenter").is_null(),
        "{:?}",
        talky.body["system"]
    );
    assert_eq!(said_in(&code), said_in(&talky), "the same conversation");

    // A name nobody ever contributed to: the moment and the turns, no leaf.
    let fresh = the_view(&view_ask(&h, &params, "night_planner", "s-now", "t2"));
    assert!(
        fresh.body["system"]["frame"]["text"].is_string(),
        "{:?}",
        fresh.body
    );
    assert!(
        fresh.body["system"]
            .get("night_planner")
            .is_none_or(|f| f.as_object().is_some_and(|o| o.is_empty())),
        "{:?}",
        fresh.body["system"]
    );
    assert_eq!(said_in(&fresh).len(), 2, "{:?}", fresh.body);
}
