//! GH #728, lock 3 — two advices under one turn never share a fan-in.
//!
//! The uniqueness question the issue named as "the real work": one member turn may
//! produce several advisor answers, and `collector/assemble` keys its fan-in guard on
//! `where {"turn_id": …, "role": "leg-window", "fired": 0}`. Had the answer's round been
//! keyed on the member's turn, the second advice would meet the first one's unfired
//! guard and consume its fan-in. Ruling point 3: "the round keeps its own key;
//! `turn_id` is a label on it".

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

const T: &str = "chat#0f1e2d3c4b5a6978";

#[test]
fn two_advices_of_one_turn_open_two_rounds_with_one_label() {
    let departs = json!([{"turn_id": T, "deadline_ms": now_ms() + 600_000}]);
    let (a, _) = open_advice("k-7", departs.clone());
    let (b, _) = open_advice("k-7", departs);
    assert_ne!(a, b, "two rounds, two keys");
    for key in [&a, &b] {
        let out = assemble(&[], answer_in(key, "x"));
        assert_eq!(hop_str(on_route(&out, "answer"), "turn_id"), T);
    }
}

#[test]
fn each_rounds_guard_is_its_own() {
    let departs = json!([{"turn_id": T, "deadline_ms": now_ms() + 600_000}]);
    let (a, _) = open_advice("k-7", departs.clone());
    let (b, _) = open_advice("k-8", departs);
    // Round A completes its fan-in: the window leg is there and unfired. The closing
    // mark it sends is scoped to round A's key — round B's guard is untouched.
    let leg = json!({"turn_id": a, "iter": 0, "role": "leg-window", "fired": 0,
                     "turn": json!({"turns": [], "bytes": 0, "dropped": 0, "capped": 0})
                         .to_string()});
    let out = assemble(
        &[],
        bundle_reply("collect", &a, &[("c-collect-read", json!([leg]))]),
    );
    let done = in_phase(&out, "collect-done");
    let mark: serde_json::Value =
        serde_json::from_str(done["messages"][0]["text"].as_str().expect("op")).expect("op json");
    assert_eq!(
        mark["where"],
        json!({"turn_id": a, "role": "leg-window", "fired": 0}),
        "the guard of round A closes on round A's key"
    );
    assert_ne!(mark["where"]["turn_id"], json!(b));
    assert_ne!(
        mark["where"]["turn_id"],
        json!(T),
        "never on the shared label"
    );
}
