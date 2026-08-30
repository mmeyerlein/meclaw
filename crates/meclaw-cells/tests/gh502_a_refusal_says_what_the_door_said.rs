//! GH #502 — a refused manifest keeps the one sentence that says why.
//!
//! The mutation door computes a precise reason for every refusal and answers
//! with it in `details` (`colony_dispatch::build_manifest_reply`). The submit
//! hive promotes the whole reply into `context.sub_carry`, and the gate's
//! `render()` turns that carry into the receipt. Until this test, `render()`
//! read `outcome`, `applied`, `error_code`, `failed_at` and `remaining` out of
//! it and stopped there — so the requester, and the composer's repair round
//! behind it, were told the class of the refusal and never the key.
//!
//! Measured on a live colony: one wish, three repair rounds, three different
//! digests, and the identical sentence back every time.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, shipped_script};

const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);

/// The pop round: the colony has answered, the gate has read its flight row
/// back, and the receipt is rendered out of the carry. `sub_carry` is what the
/// hive's own edge promoted — the door's reply, verbatim.
fn pop(carry: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": "/os/submit/store",
            "header": {
                "hop": { "phase": "pop" },
                "context": {
                    "sub_origin": "gate",
                    "sub_phase": "pop",
                    "sub_carry": carry.to_string(),
                },
            },
            "ttl": 64,
            "messages": [{
                "origin": "tool", "type": "tool_result", "id": "s1",
                "text": json!([{
                    "id": "row-1",
                    "tool_call_id": "c9",
                    "manifest_sha256": "f5a9b8dc31a6",
                }]).to_string(),
            }],
            "params": {}
        }),
    )
}

/// The receipt is the emission on `route: receipt`; the delete beside it is the
/// store's business.
fn receipt_of(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "receipt")
        .cloned()
        .expect("the pop round renders a receipt")
}

/// The door's own words, for the refusal this test was written against.
const DETAILS: &str = "post_state_addresses/schema feed/clock: \
                       override_params['interval_ms'] names no param of timer \
                       in template 'clock'. Its params are: 'query_timeout_ms', \
                       'schedules'";

#[test]
fn a_refusal_carries_the_doors_own_reason_into_the_receipt() {
    let out = pop(json!({
        "outcome": "rejected",
        "applied": 0,
        "ids": [],
        "failed_at": 1,
        "id": "01a04bd0-0fd4-71e1-97b0-3ddedb9c2fe1",
        "error_code": "schema",
        "details": DETAILS,
        "remaining": 0,
    }));
    let r = receipt_of(&out);
    let text = r["messages"][0]["text"].as_str().expect("a receipt text");

    // The sentence that was already there stays in front of it, unchanged: a
    // caller that greps for the position and the class keeps working.
    assert!(
        text.contains("manifest refused at position 1: schema"),
        "the existing sentence is unchanged: {text}"
    );
    // And the door's reason rides behind it, verbatim. Naming the key is the
    // whole point — a repair round that is told `schema` and no more repairs
    // blind.
    assert!(
        text.contains("override_params['interval_ms'] names no param of timer"),
        "the door's own reason reaches the receipt: {text}"
    );
    // The header is untouched: no new key, no renamed code.
    assert_eq!(r["header"]["error_code"], "schema");
    assert_eq!(r["header"]["failed_at"], 1);
    assert_eq!(r["header"]["applied"], 0);
    assert_eq!(r["header"]["tool_call_id"], "c9");
}

#[test]
fn a_refusal_without_details_reads_exactly_as_it_did() {
    let out = pop(json!({
        "outcome": "rejected",
        "applied": 0,
        "ids": [],
        "failed_at": 2,
        "error_code": "edge_schema",
        "remaining": 3,
    }));
    let text = receipt_of(&out)["messages"][0]["text"]
        .as_str()
        .expect("a receipt text")
        .to_string();
    assert_eq!(
        text,
        "manifest refused at position 2: edge_schema (0 applied, 3 untouched)"
    );
}

#[test]
fn a_committed_manifest_says_nothing_new() {
    let out = pop(json!({
        "outcome": "committed",
        "applied": 2,
        "ids": ["m-1", "m-2"],
        // A door that answered `committed` has no reason to give, but a carry
        // that carried one anyway must not turn a success into a complaint.
        "details": DETAILS,
    }));
    let r = receipt_of(&out);
    assert_eq!(
        r["messages"][0]["text"].as_str().expect("a receipt text"),
        "manifest applied: 2 declaration(s)"
    );
    assert!(r["header"].get("error_code").is_none());
}
