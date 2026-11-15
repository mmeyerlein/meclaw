//! d58-3 namen (GH #1525, contract point 5, N9b) -- the pending note names
//! no product.
//!
//! WHY this file exists: while an answer of another model runs, the policy
//! told the speaking model "A consult is pending ... Do not consult again"
//! -- the word of one composition (its errand family `consult`) in a text
//! every composition's model reads, and the fields the policy and the lead
//! hand each other were named after it (`consult_id`, `consult_class`). The
//! text is the param `pending_note` now, shipped with a neutral wording; a
//! composition sets its own. The fields are named for what they carry, an
//! ask (`ask_id`, `ask_class`), at both ends.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`); no
//! model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

/// The neutral wording the policy ships (contract point 5).
const NEUTRAL: &str = "An answer from another model is pending; do not ask for it again.";

/// `literal` stands in `script` as a quoted string of the code.
fn quoted(script: &str, literal: &str) -> bool {
    script.contains(&format!("\"{literal}\"")) || script.contains(&format!("'{literal}'"))
}

#[test]
fn gh_d583_namen_the_pending_note_names_no_product() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"]["pending_note"],
        json!(NEUTRAL),
        "the policy ships the neutral pending note"
    );
    assert!(
        policy["contract"]["settings"].get("pending_note").is_some(),
        "a composition can set `pending_note`"
    );
    let ask = json!({"ask_id": "a1", "at": "t0"});
    let (got, err) = policy_scope(json!({}), "pending_text(ARGS)", ask.clone());
    let text = got
        .as_str()
        .unwrap_or_else(|| panic!("a text: {got} {err}"));
    assert!(
        text.contains(NEUTRAL),
        "the note says the shipped wording: {text}"
    );
    assert!(
        !text.to_lowercase().contains("consult"),
        "the shipped note names no errand family: {text}"
    );
    let (got, err) = policy_scope(
        json!({"pending_note": "PROBE NOTE"}),
        "pending_text(ARGS)",
        ask,
    );
    assert!(
        got.as_str().is_some_and(|t| t.contains("PROBE NOTE")),
        "the composition's `pending_note` is what the model reads: {got} {err}"
    );
    // The fields at both ends.
    for (cell, script) in [
        ("curator/policy", script_of("policy")),
        ("lead/lead", lead_script()),
    ] {
        for gone in ["consult_id", "consult_class"] {
            assert!(
                !quoted(&script, gone),
                "{cell} still names the field `{gone}`"
            );
        }
        for kept in ["ask_id", "ask_class"] {
            assert!(quoted(&script, kept), "{cell} names the field `{kept}`");
        }
    }
    let policy_script = script_of("policy");
    for gone in ["A consult is pending", "Do not consult again"] {
        assert!(
            !policy_script.contains(gone),
            "the policy's code still says {gone:?}"
        );
    }
}

/// Fix round (review I1): the intake reads the follow-up id of the errand
/// tool under the field its family names (`<errand_slot>_id`), as the policy
/// does -- never under a product's literal. With the slot "probe" the
/// argument `probe_id` names the ask; with no slot the call's own id does.
#[test]
fn gh_d583_namen_the_intake_names_the_follow_up_id_by_its_family() {
    if !shipped() {
        return;
    }
    let intake = script_of("intake");
    assert!(
        !quoted(&intake, "consult_id"),
        "curator/intake still names the field `consult_id`"
    );
    let call = |args: meclaw_core::serde_json::Value| {
        json!([{"origin": "assistant", "type": "tool_call", "id": "call-1",
                "text": json!({"name": "probe_tool", "arguments": args}).to_string()}])
    };
    let (got, err) = cell_scope(
        "intake",
        json!({"errand_tool": "probe_tool", "errand_slot": "probe"}),
        "errand_ids(ARGS)",
        call(json!({"probe_id": "k-9", "consult_id": "k-0"})),
    );
    assert_eq!(
        got,
        json!(["k-9"]),
        "the family's field names the ask: {err}"
    );
    let (got, err) = cell_scope(
        "intake",
        json!({"errand_tool": "probe_tool", "errand_slot": "probe"}),
        "errand_ids(ARGS)",
        call(json!({"consult_id": "k-0"})),
    );
    assert_eq!(
        got,
        json!(["call-1"]),
        "another family's field is none: {err}"
    );
    let (got, err) = cell_scope(
        "intake",
        json!({"errand_tool": "probe_tool", "errand_slot": ""}),
        "errand_ids(ARGS)",
        call(json!({"probe_id": "k-9"})),
    );
    assert_eq!(
        got,
        json!(["call-1"]),
        "no slot, no field: the call's id: {err}"
    );
}
