//! The `code` cell's stdin document, for tests that run a shipped script
//! directly through `python3` instead of through a colony.
//!
//! The substrate hands a script a three-object document — `envelope`, `body`
//! and `params` — and there is exactly one place in the tree that builds it
//! (`meclaw_cells::code::wire::build_stdin_json`). A test that pipes its own
//! JSON into the script has to agree with that shape, or it pins a wire that
//! does not exist. This helper is that agreement, written once: hand it the
//! message the way a test naturally spells it (envelope fields and body slots
//! side by side, optionally a `params` object) and it returns the document the
//! substrate would have built.

use meclaw_core::serde_json::{Map, Value, json};

/// The envelope fields the substrate lifts out of the flat spelling.
const ENVELOPE_KEYS: &[&str] = &[
    "header",
    "target",
    "reply_to",
    "trace_id",
    "parent_message_id",
    "correlation_id",
    "ttl",
];

/// Turn a flatly spelled message into the `code` cell's stdin document.
///
/// Envelope fields move into `envelope`, a `params` key becomes the top-level
/// `params` object (`{}` when absent), and everything else is a body slot.
/// A non-object input yields an empty document rather than a panic, so a test
/// that pipes deliberate garbage still exercises the script's own guard.
#[must_use]
pub fn code_stdin(flat: &Value) -> Value {
    let mut envelope = Map::new();
    let mut body = Map::new();
    let mut params = json!({});
    if let Value::Object(o) = flat {
        for (k, v) in o {
            if k == "params" {
                params = v.clone();
            } else if ENVELOPE_KEYS.contains(&k.as_str()) {
                envelope.insert(k.clone(), v.clone());
            } else {
                body.insert(k.clone(), v.clone());
            }
        }
    }
    json!({"envelope": envelope, "body": body, "params": params})
}

/// The same document, ready to be written to a child process' stdin.
#[must_use]
pub fn code_stdin_bytes(flat: &Value) -> Vec<u8> {
    code_stdin(flat).to_string().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flat_spelling_is_split_into_the_three_objects() {
        let doc = code_stdin(&json!({
            "header": {"hop": {"route": "in_turn"}},
            "messages": [{"origin": "user", "type": "text", "text": "hi"}],
            "params": {"window_size": 7}
        }));
        assert_eq!(doc["envelope"]["header"]["hop"]["route"], "in_turn");
        assert_eq!(doc["body"]["messages"][0]["text"], "hi");
        assert_eq!(doc["params"]["window_size"], 7);
        assert_eq!(doc.as_object().map(Map::len), Some(3));
    }

    #[test]
    fn the_three_objects_are_there_even_for_an_empty_message() {
        let doc = code_stdin(&json!({}));
        assert_eq!(doc, json!({"envelope": {}, "body": {}, "params": {}}));
    }
}
