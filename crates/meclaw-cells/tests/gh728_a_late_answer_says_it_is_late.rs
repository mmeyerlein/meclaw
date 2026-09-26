//! GH #728, lock 4 — a late answer says it is late.
//!
//! Ruling point 2: "a deadline decides between a leg and a straggler". The deadline is
//! stamped at DEPARTURE (`now + late_after_ms` on the depart row) and read when the
//! ANSWER leaves (OR-T11: the consumer judges what reaches it, so "late" is measured at
//! the answer, not at the return). A straggler keeps the member's turn as its label —
//! it is late, not anonymous — and says so in `hop.late`.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

const T: &str = "chat#0f1e2d3c4b5a6978";

#[test]
fn past_the_deadline_the_answer_is_late_and_keeps_its_label() {
    let (key, _) = open_advice("k-7", json!([{"turn_id": T, "deadline_ms": 1_000}]));
    let out = assemble(&[], answer_in(&key, "It is 21C."));
    let ans = on_route(&out, "answer");
    assert_eq!(hop_str(ans, "late"), "1", "{ans}");
    assert_eq!(hop_str(ans, "turn_id"), T, "late, never re-identified");
}

#[test]
fn inside_the_deadline_it_is_a_leg() {
    let (key, _) = open_advice(
        "k-7",
        json!([{"turn_id": T, "deadline_ms": now_ms() + 600_000}]),
    );
    let out = assemble(&[], answer_in(&key, "It is 21C."));
    assert_eq!(hop_str(on_route(&out, "answer"), "late"), "0");
}

#[test]
fn an_ordinary_turns_answer_carries_the_keys_empty() {
    // Keys always present, empty where they mean nothing: a CEL modifier that reads an
    // absent hop key fails and skips its edge.
    let out = assemble(&[], answer_in(T, "hello"));
    let ans = on_route(&out, "answer");
    assert_eq!(hop_str(ans, "turn_id"), T);
    assert_eq!(hop_str(ans, "round_id"), T);
    assert_eq!(hop_str(ans, "late"), "");
}

#[test]
fn only_the_answer_route_hands_out_the_label() {
    let (key, _) = open_advice("k-7", json!([{"turn_id": T, "deadline_ms": 1_000}]));
    let out = assemble(&[], answer_in(&key, "It is 21C."));
    let store = in_phase(&out, "ans-w");
    assert_eq!(
        hop_str(store, "turn_id"),
        key,
        "the store write stays on the key"
    );
    assert!(store["header"].get("late").is_none(), "{store}");
}

#[test]
fn the_contract_declares_the_two_new_hop_keys() {
    let c = &config_of(ASSEMBLE)["contract"];
    // 2.2.0 since GH #834 (the brief leg), 2.3.0 since GH #843 (`finish_reason`
    // and `truncated` on every answer) and GH #845/#847 in the same release; the
    // two keys of GH #728 stand.
    assert_eq!(c["version"], "2.3.0");
    for k in ["late", "round_id"] {
        assert_eq!(c["emits"]["hop"][k]["type"], "string", "emits.hop.{k}");
    }
}
