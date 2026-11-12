//! D58 judge (GH #1480, contract point 3) -- parts in a curate cost no
//! message of their own.
//!
//! WHY this file exists: the person's how-questions travel today in the body
//! of the curate (`hints`, `hints_of`, the collector's knob `brief_hints`) and
//! `./intake` keeps them under `judge:hints:<person>` with a store message of
//! their own beside the curate (intake.py, lane `in_curate`, the list
//! `also`). The field `parts` replaces both: a list of contributions
//! `{target, source, value, subject}` that rides the curate the collector
//! sends anyway, so the hints cost no message -- the curate with parts takes
//! exactly as many store messages and leaves exactly as many messages as the
//! same curate without them, and the next view of the target holds the leaf.
//! The rule of the round stays as it was for the hints (`subject` the round's
//! one person, kept only on the turn's first curate, iteration 0).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

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
use meclaw_core::serde_json::{Value, json};

/// One curate of turn `t2` at iteration 0, with `parts` when given; the
/// store messages and the messages it left.
fn curate(parts: Option<Value>) -> (Hive, usize, usize) {
    let mut h = Hive::new();
    said_turn(&mut h, 1, "s-now", "t1", "the choir", "the choir");
    let steps = h.ledger_steps.len();
    let hop = json!({"session_id": "s-now", "turn_id": "t2", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s-now", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    let mut body = json!({"messages": [user("the choir, search please")]});
    if let Some(p) = parts {
        body["parts"] = p;
    }
    h.out.clear();
    h.lane("in_curate", ctx, hop, body);
    let store = h.ledger_steps.len() - steps;
    let left = h.out.len();
    (h, store, left)
}

#[test]
fn gh_d58_judge_parts_in_a_curate_cost_no_message_of_their_own() {
    let (_, store_without, left_without) = curate(None);
    let parts = json!([{"target": "decide", "source": "affinity", "value": hints_value(),
                        "subject": "member:e"}]);
    let (h, store_with, left_with) = curate(Some(parts));
    assert!(
        store_without > 0 && left_without > 0,
        "check of the setup: the curate ran ({store_without} store messages, {left_without} out)"
    );
    assert_eq!(
        (store_with, left_with),
        (store_without, left_without),
        "parts ride the curate: no store message and no message of their own"
    );

    let view = the_view(&view_ask(&h, &view_params(), "decide", "s-now", "t3"));
    assert_eq!(
        leaf_value(&leaf(&view, "decide", "affinity")),
        hints_value(),
        "the next view of the target holds the leaf the curate brought: {:?}",
        view.body["system"]
    );
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'judge:%'")
            .is_empty(),
        "no state key `judge:*` any more"
    );
}
