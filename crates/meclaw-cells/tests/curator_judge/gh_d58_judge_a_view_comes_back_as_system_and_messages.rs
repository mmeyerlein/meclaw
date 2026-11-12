//! D58 judge (GH #1480, contract points 1 and 2) -- a view comes back as
//! system and messages.
//!
//! WHY this file exists: the judge is a model like any other (R-58-14) and
//! gets its window from the curator in the form every model gets one. A view
//! for a named target is `{messages, system}`: `messages` the newest turns of
//! both sides without the running one, oldest first; `system` a tree of
//! leaves -- the moment (`frame`, the leaf of strand frame: time and place)
//! and the family of the target with the contributions of the domains. The
//! package `judge` and the switch `judge: true` are gone. A view WITHOUT a
//! target is the plain read of before (`turns_ask` without `judge`): only
//! `messages`, no system part.
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
fn gh_d58_judge_a_view_comes_back_as_system_and_messages() {
    let mut h = Hive::new();
    let params = view_params();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    said_turn(&mut h, 3, "s-now", "t2", "its concerts", "");
    // The running turn's words already stand on the wall: never in a view.
    h.row(at(5), "s-now", "t3", "user", &user("and tomorrow?"), 1);
    let _ = part(&h, &params, "decide", "presenter", screen_value(), "");

    let view = the_view(&view_ask(&h, &params, "decide", "s-now", "t3"));
    assert!(
        view.body.get("judge").is_none() && view.body.get("turns").is_none(),
        "the package `judge` and the list `turns` are gone: {:?}",
        view.body
    );
    assert_eq!(
        said_in(&view),
        vec![
            ("user".to_string(), "the choir".to_string()),
            ("assistant".to_string(), "about the choir".to_string()),
            ("user".to_string(), "its concerts".to_string()),
            ("assistant".to_string(), "about its concerts".to_string()),
        ],
        "the newest turns of both sides, oldest first, without the running turn"
    );
    let system = view.body["system"].clone();
    assert!(system.is_object(), "a view carries a system tree: {system}");
    let frame = system["frame"]["text"].as_str().unwrap_or("");
    assert!(
        frame.contains("place: Berlin") && frame.contains("Europe/Berlin"),
        "the moment is the leaf `frame` (time and place): {system}"
    );
    assert_eq!(
        leaf_value(&leaf(&view, "decide", "presenter")),
        screen_value(),
        "the family of the target holds the domain's leaf: {system}"
    );

    // Without a target: the person's words alone, as `turns_ask` without
    // `judge` gave them (the route's `context_block` reads them, and a set
    // with `context_turns` above 0 and the judge off rides on this).
    let plain = drive(
        &h,
        &params,
        "in_view_ask",
        json!({"session_id": "s-now", "turn_id": "t3"}),
        json!({}),
        view_ask_body("", 4, 1500),
    );
    let plain = the_view(&plain);
    assert_eq!(
        said_in(&plain),
        vec![
            ("user".to_string(), "the choir".to_string()),
            ("user".to_string(), "its concerts".to_string()),
        ],
        "{:?}",
        plain.body
    );
    assert!(
        plain
            .body
            .get("system")
            .is_none_or(|s| s.as_object().is_some_and(|o| o.is_empty())),
        "a view without a target carries no system part: {:?}",
        plain.body
    );
}
