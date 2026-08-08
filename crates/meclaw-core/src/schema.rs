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

    #[test]
    fn tool_call_without_id_fails_validation() {
        let err = validate_ubf_body(&json!({
            "messages": [{"origin": "assistant", "type": "tool_call"}]
        }))
        .unwrap_err();
        assert!(!err.is_empty(), "tool_call without id must fail");
    }
}
