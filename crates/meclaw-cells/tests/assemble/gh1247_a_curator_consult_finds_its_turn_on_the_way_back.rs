//! GH #1247 (T4-11) -- the answer to a curator's consult comes back to the
//! turn that asked.
//!
//! WHY this file exists: talky's collector ties an `in_advice` to the member's
//! turn through the `depart` row it writes when a `consult_cogny` call of the
//! voice model leaves (GH #728) -- the advisor answers under its own turn id,
//! so the departure is the last place that knows the member's. A complex turn
//! now leaves through the curator on `consult` (R-WP-5), with no tool call the
//! collector ever sees: without a departure the answer opened a round under a
//! provisional key and the member's turn was never answered. talky hands the
//! consult to the collector as well (`in_depart`), and the collector writes the departure
//! under the curator's `consult_id`; the answer then finds the turn.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const T: &str = "chat#5a6b7c8d9e0f1a2b";

#[test]
fn a_curator_consult_finds_its_turn_on_the_way_back() {
    let talky = config_of("templates/talky/config.json");
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        edges.iter().any(|e| e["from"] == "./curator"
            && e["to"] == "./collector"
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'consult'"))),
        "talky hands the curator's consult to its collector"
    );

    let out = assemble(
        &[],
        lane(
            "in_depart",
            json!({"consult_id": "k-call-1", "consult_class": "consult", "turn_id": T}),
            json!({"turn_id": T}),
            json!([{"origin": "user", "type": "text", "text": "plan my week around the move"}]),
        ),
    );
    let departs: Vec<Value> = out
        .iter()
        .flat_map(calls_of)
        .map(|(_, c)| c)
        .filter(|c| c["table"] == "round" && c["row"]["role"] == "depart")
        .collect();
    assert_eq!(departs.len(), 1, "one departure for the consult: {out:?}");
    let row = &departs[0]["row"];
    assert_eq!(
        row["correlation"], "k-call-1",
        "under the curator's consult id"
    );
    assert_eq!(row["turn_id"], T, "naming the member's turn");
    assert_eq!(row["session_id"], SESSION);
    let deadline = row["deadline_ms"].as_i64().unwrap_or(0);
    assert!(deadline > now_ms(), "a deadline ahead: {row:?}");

    // cogny's answer comes back under its own turn id and finds the turn.
    let (key, _) = open_advice("k-call-1", json!([{"turn_id": T, "deadline_ms": deadline}]));
    assert!(
        key.starts_with(&format!("{T}~")),
        "the answer's round carries the member's turn: {key}"
    );
}
