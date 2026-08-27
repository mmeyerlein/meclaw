//! A tool name the builder does not wire must come back as an ordinary
//! tool_result under the id that asked, never as silence: an unanswered
//! tool_call_id parks the fan-in until the TTL runs out, and TTL expiry emits
//! nothing at all towards the surface.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const UNKNOWN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/unknown/config.json"
);

fn run_unknown(hop: Value) -> Value {
    emit_one(
        &shipped_script(UNKNOWN),
        &json!({"header": {"hop": hop}, "messages": []}),
    )
}

#[test]
fn an_unwired_tool_name_answers_under_the_id_that_asked() {
    let out = run_unknown(json!({"tool_name": "bash", "tool_call_id": "c-9"}));
    assert_eq!(out["header"]["error_code"], "unknown_tool");
    assert_eq!(out["header"]["tool_call_id"], "c-9");
    assert_eq!(out["messages"][0]["type"], "tool_result");
    assert_eq!(out["messages"][0]["id"], "c-9");
    assert!(
        out["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("bash"),
        "name the tool that was asked for -- a refusal it cannot name is one \
         the model cannot explain"
    );
}

#[test]
fn a_call_with_no_name_at_all_still_answers() {
    let out = run_unknown(json!({"tool_call_id": "c-10"}));
    assert_eq!(out["header"]["error_code"], "unknown_tool");
    assert_eq!(out["messages"][0]["id"], "c-10");
}
