//! GH #1435 (D38 K7) -- `memory_to_brain` "0" is the policy of before.
//!
//! WHY this file exists: the knob ships "0" (the set switches it on), so a
//! `simple` turn whose stamp names `memory` must still take the quick way
//! with the knob off -- same call, same window without memory rows
//! (`QUICK_NO_KINDS`), same `route` mark with the three keys of before and no
//! `reason`. A mutant that reads the need without the knob turns this red.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_00000000000000k0";

fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(text),
                            tool_call(RECALL_ID, "memory_recall"),
                            tool_result(RECALL_ID, "WHAT WAS SAID\n  user: \"Anna lives in Hamburg.\"")],
               "system": mode("Be brief.")}),
    );
}

fn route_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'route' ORDER BY seq")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
        .collect()
}

#[test]
fn knob_off_is_byte_identical() {
    if !shipped() {
        return;
    }
    // The knob lives in the lead since GH #1447.
    let lead = cell_config("lead");
    assert_eq!(lead["params"]["memory_to_brain"], json!("0"), "ships off");
    assert_eq!(
        lead["contract"]["settings"]["memory_to_brain"]["default"],
        json!("0"),
        "the setting says the same"
    );

    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("lead", "memory_to_brain", json!("0")),
    ]);
    seam(&mut h, "t1", "simple", "memory", "How is my sister doing?");
    assert!(
        h.routed("brain").is_empty(),
        "off: the brain is not asked: {:?}",
        h.out
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "off: the quick way of before: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        !sj::to_string(&quick[0].messages())
            .expect("messages")
            .contains("WHAT WAS SAID"),
        "the quick window carries no memory rows: {:?}",
        quick[0].messages()
    );
    let marks = route_marks(&h);
    assert_eq!(marks.len(), 1, "{marks:?}");
    let mut keys: Vec<&str> = marks[0]
        .as_object()
        .expect("mark object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["call", "path", "way"],
        "the mark of before, no reason: {marks:?}"
    );
    assert_eq!(marks[0]["way"], json!("quick"), "{marks:?}");
}
