//! GH #1532 (d58-3 raten, point 4) -- the memory ask carries the image cost
//! of the row whose window it carries.
//!
//! WHY this file exists: `./push` hands the memory hive the window of the
//! answering model (`input_soft`) and, since GH #1526, its rate
//! (`chars_per_token`) on every recall ask; the memory hive counts an inline
//! image at the request's `tokens_per_image` and, without one, at the core's
//! fixed 1024 tokens (raten's memory pins). The image cost of the same row
//! never left the curator, so the memory hive counted every image at 1024
//! whatever model the window belonged to. The rule: the row's
//! `tokens_per_image` (a whole number above zero) rides beside the window and
//! the rate, out of the same state reads; none known, no key -- the reader
//! counts at the core's cost, as before.
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
// harness_fold: #[path = "support/row_stamp.rs"]
#[allow(unused_imports)]
use crate::row_stamp; // harness_fold: mod row_stamp;

use curator_hive::*;
use meclaw_core::serde_json::{Map, Value, json};

/// One round of the model, answered on the tap with `usage`.
fn answer_round(h: &mut Hive, turn: &str, usage: Value) {
    let call = h.curate("s1", turn, 0, json!([user("Who is my son?")]), mode(""));
    h.tap(&call, "stop", usage, json!([said("noted")]));
}

/// The header of the one `recall` the collector's ask of `turn` sends out
/// (hop first, then context: the map that carries the window).
fn recall_head(h: &mut Hive, turn: &str) -> Map<String, Value> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": "s1", "iter": "0",
               "recall_query": "And what does he do?", "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user("And what does he do?")]}),
    );
    let asks = h.routed("recall");
    assert_eq!(asks.len(), 1, "{:?} {:?}", h.out, h.stderr);
    for map in [&asks[0].hop, &asks[0].context] {
        if map.contains_key("recall_input_soft") {
            return map.clone();
        }
    }
    panic!(
        "the ask carries the window: {:?} {:?}",
        asks[0].hop, asks[0].context
    )
}

fn head_after(usage: Value) -> Map<String, Value> {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    answer_round(&mut h, "t1", usage);
    recall_head(&mut h, "t2")
}

/// The usage is the header of a real answer of an `llm` cell born on a row
/// with the image cost 1600 (review raten I1: no stamp built by hand).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d583_raten_push_the_ask_carries_the_image_cost_of_the_row() {
    if !shipped() {
        return;
    }
    let stamp = row_stamp::real_stamp().await;
    let head = tokio::task::spawn_blocking(move || head_after(Value::Object(stamp)))
        .await
        .expect("the ask is built");
    assert_eq!(head.get("input_soft"), Some(&json!(250000)), "{head:?}");
    assert_eq!(
        head.get("tokens_per_image"),
        Some(&json!(1600)),
        "the ask carries the row's image cost beside its window: {head:?}"
    );
}

#[test]
fn gh_d583_raten_push_no_image_cost_named_leaves_no_key() {
    if !shipped() {
        return;
    }
    for usage in [
        json!({"input_soft": 250000, "input_hard": 1000000, "chars_per_token": 4}),
        json!({"input_soft": 250000, "input_hard": 1000000, "chars_per_token": 4,
               "tokens_per_image": 0}),
    ] {
        let head = head_after(usage.clone());
        assert_eq!(head.get("input_soft"), Some(&json!(250000)), "{head:?}");
        assert!(
            !head.contains_key("tokens_per_image"),
            "no cost named, no key -- the reader counts at the core's: {usage} {head:?}"
        );
    }
}
