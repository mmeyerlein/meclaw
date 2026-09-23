//! GH #728, lock 10 — two consults in one bundle are two rounds of the core (OR-T13).
//!
//! Since #724 the core's collector ADOPTS the id it is handed (`hop.turn_id or
//! context.turn_id`), and the assistant's consult edges handed it the SURFACE's round
//! id in `context.turn_id`. Two `consult_cogny` calls in one bundle therefore opened
//! two core rounds under ONE key, and the second met the first one's unfired
//! `leg-window` guard (the warning the adoption comment names). The core's rounds are
//! its own: the consult edges drop `turn_id` from the context, the core mints, and the
//! return is correlated by `consult_id` alone.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

fn consult_edges() -> Vec<serde_json::Value> {
    config_of(ASSISTANT)["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| {
            e["to"] == "./cogny"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("consult_cogny"))
        })
        .cloned()
        .collect()
}

#[test]
fn both_consult_edges_drop_the_surfaces_round_id() {
    let edges = consult_edges();
    assert_eq!(edges.len(), 2, "talky and talky-chat: {edges:?}");
    for e in edges {
        let deleted = e["modifier"]["delete_context"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            deleted.contains(&json!("turn_id")),
            "{} -> ./cogny still hands the core the surface's round id: {e}",
            e["from"]
        );
        assert_eq!(
            e["modifier"]["set_context"]["consult_id"], "hop.consult_id",
            "the correlation still travels: {e}"
        );
    }
}

/// What the core's collector receives over such an edge: the errand, the correlation
/// in context, and no `turn_id` on either compartment.
fn errand(consult: &str) -> serde_json::Value {
    lane(
        "in_turn",
        json!({"tool_name": "consult_cogny", "consult_id": consult}),
        json!({"consult_id": consult, "consult_class": "consult", "col_phase": ""}),
        json!([{"origin": "assistant", "type": "tool_call", "id": consult,
                "text": "{\"question\": \"x\"}"}]),
    )
}

#[test]
fn two_errands_of_one_bundle_open_two_rounds_in_the_core() {
    let a = hop_str(
        in_phase(&assemble(&[], errand("call-1")), "turn-open"),
        "turn_id",
    );
    let b = hop_str(
        in_phase(&assemble(&[], errand("call-2")), "turn-open"),
        "turn_id",
    );
    assert!(!a.is_empty() && !b.is_empty());
    assert_ne!(a, b, "two consults, two rounds of the core");
}
