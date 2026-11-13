//! D58 judge (GH #1480, contract point 2; close of wave d58-2, review M4 of
//! the judge strand) -- a view for a target named like one of the view's own
//! leaves is refused.
//!
//! WHY this file exists: the system tree of a view holds the leaves `frame`
//! and `topic` and, beside them, the target's family under the target's name
//! (`system[<target>]`). A target called `frame` or `topic` put its family in
//! the place of that leaf: the moment or the topic was gone without a word.
//! The view now refuses such a target at the door, `hop.refused`
//! `bad_arguments` with an empty system tree and no ledger read, like every
//! other refusal at the door.
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
fn gh_d58_judge_a_target_named_like_a_leaf_is_refused() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    h.row(at(5), "s-now", "t2", "user", &user("and tomorrow?"), 1);
    for name in ["frame", "topic"] {
        let _ = part(&h, &params, name, "presenter", screen_value(), "");
        let run = view_ask(&h, &params, name, "s-now", "t2");
        let view = the_view(&run);
        assert_eq!(
            view.hop.get("refused"),
            Some(&json!("bad_arguments")),
            "a target named `{name}` would replace the view's own leaf: {:?}",
            view.hop
        );
        assert_eq!(
            view.body.get("system"),
            Some(&json!({})),
            "the refusal at the door carries an empty system tree: {:?}",
            view.body
        );
        assert!(
            run.store.is_empty(),
            "refused at the door, before any ledger read: {:?}",
            run.store
        );
    }
    // Any other name is answered as before, its family beside the leaves.
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");
    let view = the_view(&view_ask(&h, &params, "decide", "s-now", "t2"));
    assert!(view.hop.get("refused").is_none(), "{:?}", view.hop);
    assert!(
        view.body["system"]["frame"]["text"].is_string(),
        "{:?}",
        view.body
    );
    assert_eq!(
        leaf_value(&leaf(&view, "decide", "presenter")),
        screen_value()
    );
}
