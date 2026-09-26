//! UBF-Body validation against the JSON-Schema in `schemas/ubf-body.json`.
//!
//! Spec § "Schema validation: timing and scope": validation runs on
//! cell output in Colony, before routing, only there. Compiled once via
//! `OnceLock`. The `jsonschema` dependency is encapsulated — only this
//! module's `validate_ubf_body` function is exported from the crate.

use serde_json::Value;
use std::sync::OnceLock;

const UBF_SCHEMA_JSON: &str = include_str!("../schemas/ubf-body.json");

static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();

fn validator() -> &'static jsonschema::Validator {
    VALIDATOR.get_or_init(|| {
        let schema: Value =
            serde_json::from_str(UBF_SCHEMA_JSON).expect("UBF schema JSON malformed");
        jsonschema::validator_for(&schema).expect("UBF schema is not valid Draft 2020-12")
    })
}

/// Eagerly initialize the UBF validator at startup. Call once from the
/// host (e.g. `colony_task`) so the first cell-output validation in the
/// hot path does not pay the jsonschema-compile cost (~100–150 ms in
/// debug builds). Subsequent calls are no-ops (OnceLock semantics).
pub fn init_validator() {
    let _ = validator();
}

/// Validate a body candidate against the Universal-Body-Format schema.
/// Returns `Ok(())` if valid, or `Err(String)` with a semicolon-joined
/// description of all validation errors.
pub fn validate_ubf_body(body: &Value) -> Result<(), String> {
    let v = validator();
    let errors: Vec<String> = v.iter_errors(body).map(|e| e.to_string()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_object_fails_anyof() {
        let err = validate_ubf_body(&json!({})).unwrap_err();
        assert!(!err.is_empty(), "empty body must fail anyOf");
    }

    #[test]
    fn minimal_messages_empty_array_is_valid() {
        validate_ubf_body(&json!({"messages": []})).expect("empty messages[] is valid");
    }

    #[test]
    fn turn_object_user_text_is_valid() {
        validate_ubf_body(&json!({
            "messages": [{"origin": "user", "type": "text", "text": "hi"}]
        }))
        .unwrap();
    }

    #[test]
    fn invalid_origin_fails_validation() {
        let err = validate_ubf_body(&json!({
            "messages": [{"origin": "WRONG", "type": "text"}]
        }))
        .unwrap_err();
        assert!(!err.is_empty());
    }

    /// GH #847: `peer` is the role of the other side's words — a turn from
    /// another colony or another speaker in a room, never the agent's own person.
    #[test]
    fn peer_origin_is_valid() {
        validate_ubf_body(&json!({
            "messages": [{"origin": "peer", "type": "text", "text": "hello from the far side"}]
        }))
        .expect("origin peer is a member of the closed set");
    }

    /// GH #847 / OR-SN-32: `speaker` and `speaker_ref` name who a peer turn is from.
    #[test]
    fn speaker_fields_on_a_peer_turn_are_valid() {
        for speaker_ref in ["3a47fe3e", "3a47fe3e0b1c"] {
            validate_ubf_body(&json!({
                "messages": [{"origin": "peer", "type": "text", "text": "hi",
                    "speaker": "Jonas", "speaker_ref": speaker_ref}]
            }))
            .unwrap_or_else(|e| panic!("speaker_ref {speaker_ref} must be valid: {e}"));
        }
    }

    /// GH #847: the speaker fields exist only on `peer` — on the agent's own
    /// person, on its own answer or anywhere else they are a claim nobody checked.
    #[test]
    fn speaker_fields_on_a_non_peer_turn_are_invalid() {
        for origin in ["user", "assistant", "tool", "system"] {
            for (field, value) in [("speaker", "Jonas"), ("speaker_ref", "3a47fe3e")] {
                let mut turn = json!({"origin": origin, "type": "text", "text": "hi"});
                turn[field] = json!(value);
                assert!(
                    validate_ubf_body(&json!({"messages": [turn]})).is_err(),
                    "{field} on origin {origin} must be refused"
                );
            }
        }
    }

    /// GH #847: the reference has one form (8 or 12 lowercase hex digits), so it
    /// cannot carry a path, a bracket or another name.
    #[test]
    fn a_malformed_speaker_ref_is_invalid() {
        for bad in [
            "3A47",
            "3A47FE3E",
            "../x",
            "3a47fe3",
            "3a47fe3e0",
            "",
            "3a47fe3e]",
        ] {
            assert!(
                validate_ubf_body(&json!({
                    "messages": [{"origin": "peer", "type": "text", "speaker_ref": bad}]
                }))
                .is_err(),
                "speaker_ref {bad:?} must be refused"
            );
        }
    }

    /// GH #847: a speaker name is short; 120 characters is the cap.
    #[test]
    fn an_overlong_speaker_is_invalid() {
        let long = "x".repeat(121);
        assert!(
            validate_ubf_body(&json!({
                "messages": [{"origin": "peer", "type": "text", "speaker": long}]
            }))
            .is_err()
        );
        validate_ubf_body(&json!({
            "messages": [{"origin": "peer", "type": "text", "speaker": "x".repeat(120)}]
        }))
        .expect("120 characters is still a name");
    }

    #[test]
    fn tool_call_without_id_fails_validation() {
        let err = validate_ubf_body(&json!({
            "messages": [{"origin": "assistant", "type": "tool_call"}]
        }))
        .unwrap_err();
        assert!(!err.is_empty(), "tool_call without id must fail");
    }
}
