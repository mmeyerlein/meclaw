//! GH #1248 (T4-10) -- after a tool result the curator names the speaker.
//!
//! WHY this file exists: a quick turn that called a tool comes back on the seam
//! with the result, and the owner's line (R-WP-1, R-WM-1) is that the curator
//! decides who speaks it: the quick voice while the result is speakable as it
//! is -- every call of the turn answered, the newest result neither empty nor
//! an error, at most `quick_max_iter` (2) iterations -- else the brain, with
//! one `escalate` mark that says why. A turn once escalated stays with the
//! brain for the rest of its round.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn seam(h: &mut Hive, turn_id: &str, iter: u32, messages: Value) {
    h.out.clear();
    let it = iter.to_string();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": it, "phase": "",
                     "path": "simple", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": it,
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

fn escalations(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate' ORDER BY seq")
        .iter()
        .map(|r| {
            meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("{}"))
                .expect("the mark is JSON")
        })
        .collect()
}

#[test]
fn the_curator_names_the_speaker_after_a_tool_result() {
    if !shipped() {
        return;
    }
    let (max, _) = policy_scope(json!({}), "QUICK_MAX_ITER", Value::Null);
    assert_eq!(
        max,
        json!(2),
        "the quick voice speaks at most two iterations"
    );
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);

    // A speakable result: the quick voice speaks it.
    seam(
        &mut h,
        "t1",
        0,
        json!([user("what is the weather in Berlin?")]),
    );
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    // T4-09: the quick voice says one sentence beside every tool call.
    let first = &h.routed("quick")[0];
    assert!(
        first.body["system"]["instructions"]["quick"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("beside the call")),
        "the quick call carries `instructions.quick`: {:?}",
        first.body["system"]
    );
    seam(
        &mut h,
        "t1",
        1,
        json!([
            tool_call("c1", "web_search"),
            tool_result("c1", "Sunny, 21 degrees.")
        ]),
    );
    assert_eq!(
        h.routed("quick").len(),
        1,
        "a speakable result stays with the quick voice: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(escalations(&h).is_empty(), "nothing escalated yet");

    // An error result: the brain speaks, one mark says why.
    seam(&mut h, "t2", 0, json!([user("and in Hamburg?")]));
    seam(
        &mut h,
        "t2",
        1,
        json!([
            tool_call("c2", "web_search"),
            tool_result("c2", r#"{"error": "timeout"}"#)
        ]),
    );
    assert!(h.routed("quick").is_empty(), "{:?}", h.out);
    assert_eq!(
        h.routed("brain").len(),
        1,
        "an error escalates to the brain: {:?} {:?}",
        h.out,
        h.stderr
    );
    let marks = escalations(&h);
    assert_eq!(marks.len(), 1, "one escalate mark: {marks:?}");
    assert_eq!(marks[0]["reason"], json!("unspeakable"), "{marks:?}");
    assert_eq!(marks[0]["from"], json!("quick"));
    assert_eq!(marks[0]["to"], json!("brain"));

    // The escalated turn stays with the brain, without a second mark.
    seam(
        &mut h,
        "t2",
        2,
        json!([
            tool_call("c3", "web_search"),
            tool_result("c3", "Rain, 14 degrees.")
        ]),
    );
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(escalations(&h).len(), 1, "the turn was escalated once");

    // Past the iteration bound: the brain, reason `iterations`.
    seam(
        &mut h,
        "t3",
        3,
        json!([
            tool_call("c4", "web_search"),
            tool_result("c4", "Fog, 9 degrees.")
        ]),
    );
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    let marks = escalations(&h);
    assert_eq!(marks.len(), 2, "{marks:?}");
    assert_eq!(marks[1]["reason"], json!("iterations"), "{marks:?}");
}
