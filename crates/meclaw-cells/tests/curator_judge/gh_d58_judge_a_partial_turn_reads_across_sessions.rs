//! D58 judge (GH #1480, contract point 6) -- a partial turn reads across
//! sessions.
//!
//! WHY this file exists: a view for a partial turn of a running voice
//! session (`context.delta` "1") carries the call's id as its session, while
//! the conversation stands under the generation's session (live-2 K3); the
//! call often has no generation yet. Today's `turns_ask` reads the round's
//! newest rows whatever their session for a delta (history.py 2222-2246,
//! 2387) and ages the topic against the call's opening (GH #1436). The view
//! keeps both: a partial's view holds the conversation of the other session,
//! and a topic named before the call opened frames none of its partials --
//! while the same view with the aging off still carries it (the control).
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
use meclaw_core::serde_json::{Map, Value, json};

fn with(aging: &str) -> (String, Map<String, Value>) {
    history_params(
        &[
            ("place", json!("Berlin")),
            ("timezone", json!("Europe/Berlin")),
            ("topic_aging", json!(aging)),
            ("topic_age_turns", json!(4)),
        ],
        &[],
    )
}

/// The view of the partial `turn` of `call`.
fn partial_view(h: &Hive, p: &(String, Map<String, Value>), call: &str, turn: &str) -> Msg {
    the_view(&drive(
        h,
        p,
        "in_view_ask",
        // `live`: the handle of the running session (strand `live`), by which
        // the opening is read; master reads the opening by the session id.
        json!({"session_id": call, "live": call, "turn_id": turn, "delta": "1"}),
        json!({}),
        view_ask_body("decide", 4, 1500),
    ))
}

#[test]
fn gh_d58_judge_a_partial_turn_reads_across_sessions() {
    let mut h = Hive::new();
    // The text session before the call: a crisis named there.
    said_turn(&mut h, 1, "tg-gen", "t1", "I feel awful", "the crisis");
    said_turn(&mut h, 3, "tg-gen", "t2", "the choir tonight", "");
    // The call opens (its first seed writes the opening, aging on).
    let aging = with("1");
    let _ = only(&seed_ask(&h, &aging, "call-9"), "seed");

    let view = partial_view(&h, &aging, "call-9", "call-9#1");
    let said: Vec<String> = said_in(&view).into_iter().map(|(_, t)| t).collect();
    assert!(
        said.contains(&"I feel awful".to_string())
            && said.contains(&"the choir tonight".to_string()),
        "the partial reads the round's turns of the other session: {said:?}"
    );
    let system = view.body["system"].to_string();
    assert!(
        !system.contains("the crisis"),
        "a topic named before the call opened frames none of its partials: {system}"
    );

    // The control: the aging off, the same partial's view carries the topic.
    let carried = partial_view(&h, &with("0"), "call-9", "call-9#1");
    assert!(
        carried.body["system"].to_string().contains("the crisis"),
        "with the aging off the topic stands in the view: {:?}",
        carried.body["system"]
    );
}
