//! GH #728, lock 5 — a delegation answer carries the caller's turn.
//!
//! The delegation lane (#784) was the second copy of the advice mint: "fresh
//! `turn_id`". Its label needs no lookup (OR-T12): the voice cell stamps the turn that
//! was open when the model delegated (`open_turn_id`, `"<session>#<n>"`) on the HOP of
//! the `delegation` emission, and the member's edge carries it through. The label is
//! read from the hop and from nowhere else — the context of a lane that re-enters the
//! collector may still hold the id of the round before this one.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

fn delegation(hop_turn: Option<&str>, ctx_turn: Option<&str>) -> serde_json::Value {
    let mut hop = json!({});
    if let Some(t) = hop_turn {
        hop["turn_id"] = json!(t);
    }
    let mut ctx = json!({"delegation_id": "dlg-1", "engine": "duplex"});
    if let Some(t) = ctx_turn {
        ctx["turn_id"] = json!(t);
    }
    lane(
        "in_delegation",
        hop,
        ctx,
        json!([{"origin": "user", "type": "text", "text": "when is my appointment?"}]),
    )
}

fn round_key(doc: serde_json::Value) -> String {
    hop_str(in_phase(&assemble(&[], doc), "turn-open"), "turn_id")
}

#[test]
fn the_label_comes_off_the_hop() {
    let key = round_key(delegation(Some("s1#3"), None));
    assert!(key.starts_with("s1#3~"), "{key}");
    let out = assemble(&[], answer_in(&key, "Thursday."));
    let ans = on_route(&out, "answer");
    assert_eq!(hop_str(ans, "turn_id"), "s1#3");
    assert_eq!(hop_str(ans, "round_id"), key);
    assert_eq!(hop_str(ans, "late"), "0");
}

#[test]
fn a_context_id_alone_is_no_label() {
    let key = round_key(delegation(None, Some("s1#3")));
    assert!(!key.contains("s1#3"), "the context is not the hop: {key}");
    let out = assemble(&[], answer_in(&key, "Thursday."));
    let ans = on_route(&out, "answer");
    assert_eq!(
        hop_str(ans, "turn_id"),
        key,
        "no label, the key is all there is"
    );
    assert_eq!(hop_str(ans, "late"), "");
}

#[test]
fn a_hop_label_is_disarmed_like_every_adopted_id() {
    let key = round_key(delegation(Some("a|b~c"), None));
    assert!(key.starts_with("a_b_c~"), "{key}");
    let key = round_key(delegation(Some("close-s1"), None));
    assert!(
        !key.starts_with("close-"),
        "the reserved shape is refused: {key}"
    );
}

#[test]
fn the_row_keeps_its_role_and_correlation() {
    let out = assemble(&[], delegation(Some("s1#3"), None));
    let open = in_phase(&out, "turn-open");
    let row = call(open, "c-open-turn");
    assert_eq!(row["row"]["role"], "delegation");
    assert_eq!(row["row"]["consult_id"], "dlg-1");
    assert_eq!(row["row"]["turn_id"], hop_str(open, "turn_id").as_str());
}
