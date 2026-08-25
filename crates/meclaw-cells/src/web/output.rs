//! W8 (GH #380): the `web` cell's reply shapes.
//!
//! Byte-for-byte the store's conventions, and deliberately its own code. The
//! two cells answer the same *shape* — a `tool_result` turn per op, a `results[]`
//! slot for a bundle, `bundle_errors` stamped unconditionally — because a caller
//! that has learned to read one should not have to learn the other. They do not
//! share an implementation, because sharing one would couple two cell types
//! through their output format and make every future change to either a
//! negotiation.
//!
//! The one rule that is easy to get wrong, inherited from the store's GH #295:
//! per-op metadata goes in the body's `results[]` slot, **never** on the turn.
//! `$defs/TurnObject` in `crates/meclaw-core/schemas/ubf-body.json` is
//! `additionalProperties: false`, and the colony validates every emission
//! against it before routing — a turn carrying `operation` would dead-letter the
//! whole reply as `InvalidUbfBody`, and no caller would ever see it.

use meclaw_core::serde_json::{Map, Value, json};

/// What one op did.
#[derive(Debug, Clone)]
pub struct OpOutcome {
    /// The op name, as the caller wrote it (`object.create`, `query`, …).
    pub operation: String,
    /// How many rows the op changed. A read reports 0.
    pub rows_affected: i64,
    /// The op's answer, for a read. `null` for a write.
    pub payload: Value,
    /// Set when the op was refused. One of the closed codes below.
    pub error_code: Option<String>,
    /// The human-readable half of a refusal.
    pub error_text: Option<String>,
}

impl OpOutcome {
    /// A write that changed `rows` rows.
    pub fn wrote(operation: &str, rows: i64) -> Self {
        Self {
            operation: operation.to_string(),
            rows_affected: rows,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        }
    }

    /// A read that answers with `payload`.
    pub fn read(operation: &str, payload: Value) -> Self {
        Self {
            operation: operation.to_string(),
            rows_affected: 0,
            payload,
            error_code: None,
            error_text: None,
        }
    }

    /// A refusal. `code` must be one of the documented `error_code` strings —
    /// they are public contract surface, so the list is closed and lives in
    /// `docs/cell-types.md`.
    pub fn refused(operation: &str, code: &str, text: impl Into<String>) -> Self {
        Self {
            operation: operation.to_string(),
            rows_affected: 0,
            payload: Value::Null,
            error_code: Some(code.to_string()),
            error_text: Some(text.into()),
        }
    }

    /// Whether this outcome was a refusal.
    pub fn is_error(&self) -> bool {
        self.error_code.is_some()
    }
}

/// The reply to a message carrying exactly one `tool_call`.
///
/// The op's metadata rides on the header, because there is exactly one op to
/// describe.
pub fn build_tool_result(
    outcome: &OpOutcome,
    tool_call_id: String,
    duration_ms: i64,
) -> (Value, Map<String, Value>) {
    let text = match &outcome.error_text {
        Some(t) => t.clone(),
        None => meclaw_core::serde_json::to_string(&outcome.payload).unwrap_or_default(),
    };
    let body = json!({
        "messages": [{
            "origin": "tool",
            "type": "tool_result",
            "text": text,
            "id": tool_call_id,
        }]
    });
    let mut headers = Map::new();
    headers.insert("operation".into(), json!(outcome.operation));
    headers.insert("rows_affected".into(), json!(outcome.rows_affected));
    headers.insert("duration_ms".into(), json!(duration_ms));
    if let Some(code) = &outcome.error_code {
        headers.insert("error_code".into(), json!(code));
    }
    (body, headers)
}

/// One leg of a bundle: an op, its result turn and its own metadata.
#[derive(Debug, Clone)]
pub struct BundleLeg {
    id: String,
    operation: String,
    rows_affected: i64,
    duration_ms: i64,
    error_code: Option<String>,
    text: String,
}

impl BundleLeg {
    /// A leg from a completed op.
    pub fn from_outcome(outcome: &OpOutcome, id: String, duration_ms: i64) -> Self {
        let text = match &outcome.error_text {
            Some(t) => t.clone(),
            None => meclaw_core::serde_json::to_string(&outcome.payload).unwrap_or_default(),
        };
        Self {
            id,
            operation: outcome.operation.clone(),
            rows_affected: outcome.rows_affected,
            duration_ms,
            error_code: outcome.error_code.clone(),
            text,
        }
    }

    /// The schema-pure turn.
    fn to_turn(&self) -> Value {
        json!({
            "origin": "tool",
            "type": "tool_result",
            "text": self.text,
            "id": self.id,
        })
    }

    /// The metadata entry, correlated back by `tool_call_id`.
    fn to_result(&self) -> Value {
        let mut m = Map::new();
        m.insert("tool_call_id".into(), json!(self.id));
        m.insert("operation".into(), json!(self.operation));
        m.insert("rows_affected".into(), json!(self.rows_affected));
        m.insert("duration_ms".into(), json!(self.duration_ms));
        if let Some(code) = &self.error_code {
            m.insert("error_code".into(), json!(code));
        }
        Value::Object(m)
    }
}

/// The reply to a bundle of N > 1 ops.
///
/// `bundle_errors` is stamped unconditionally, including when it is zero: a `0`
/// says *checked and clean*, which a consumer has to be able to tell apart from
/// *nobody stamped it*. The header's own `error_code` is deliberately **not**
/// set — it keeps its hard meaning (the whole reply is a refusal and carries no
/// payload) and never signals a partial failure.
///
/// A bundle is explicitly **not a transaction**: a failed leg does not roll back
/// its siblings. That is the store's ruling and it holds for the same reason
/// here — a display patch that half-applied is visible, and pretending otherwise
/// would mean holding a write transaction open across a render.
pub fn build_bundle_result(
    legs: &[BundleLeg],
    total_duration_ms: i64,
) -> (Value, Map<String, Value>) {
    let rows_affected: i64 = legs.iter().map(|l| l.rows_affected).sum();
    let bundle_errors = legs.iter().filter(|l| l.error_code.is_some()).count() as i64;
    let body = json!({
        "messages": legs.iter().map(BundleLeg::to_turn).collect::<Vec<Value>>(),
        "results": legs.iter().map(BundleLeg::to_result).collect::<Vec<Value>>(),
    });
    let mut headers = Map::new();
    headers.insert("operation".into(), json!("bundle"));
    headers.insert("rows_affected".into(), json!(rows_affected));
    headers.insert("duration_ms".into(), json!(total_duration_ms));
    headers.insert("bundle_errors".into(), json!(bundle_errors));
    (body, headers)
}

/// A whole-reply refusal: the message could not be read as ops at all.
pub fn build_refusal(operation: &str, code: &str, text: String, duration_ms: i64) -> Value {
    json!({
        "header": {
            "finish_reason": "error",
            "error_code": code,
            "operation": operation,
            "duration_ms": duration_ms,
        },
        "messages": [{"origin": "tool", "type": "tool_result", "text": text, "id": ""}]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_reply_carries_its_metadata_on_the_header() {
        let (body, headers) =
            build_tool_result(&OpOutcome::wrote("object.create", 1), "c1".into(), 3);
        assert_eq!(headers["operation"], json!("object.create"));
        assert_eq!(headers["rows_affected"], json!(1));
        assert!(!headers.contains_key("bundle_errors"), "not a bundle");
        assert_eq!(body["messages"][0]["id"], json!("c1"));
    }

    #[test]
    fn a_bundle_puts_metadata_in_results_and_never_on_a_turn() {
        // The schema guard: a turn carrying `operation` dead-letters the reply.
        let legs = vec![
            BundleLeg::from_outcome(&OpOutcome::wrote("object.create", 1), "a".into(), 1),
            BundleLeg::from_outcome(&OpOutcome::wrote("object.update", 1), "b".into(), 1),
        ];
        let (body, headers) = build_bundle_result(&legs, 5);
        assert_eq!(headers["operation"], json!("bundle"));
        assert_eq!(headers["rows_affected"], json!(2));
        assert_eq!(headers["bundle_errors"], json!(0));

        for turn in body["messages"].as_array().unwrap() {
            let keys: Vec<&String> = turn.as_object().unwrap().keys().collect();
            assert_eq!(
                keys.len(),
                4,
                "a turn has exactly origin/type/text/id, got {keys:?}"
            );
        }
        assert_eq!(body["results"][1]["operation"], json!("object.update"));
        assert_eq!(body["results"][0]["tool_call_id"], json!("a"));
    }

    #[test]
    fn a_clean_bundle_still_stamps_zero() {
        let legs = vec![BundleLeg::from_outcome(
            &OpOutcome::wrote("object.create", 1),
            "a".into(),
            1,
        )];
        let (_, headers) = build_bundle_result(&legs, 1);
        assert_eq!(
            headers["bundle_errors"],
            json!(0),
            "0 means checked and clean, which is not the same as unstamped"
        );
    }

    #[test]
    fn a_failed_leg_is_counted_and_its_siblings_stand() {
        let legs = vec![
            BundleLeg::from_outcome(&OpOutcome::wrote("object.create", 1), "a".into(), 1),
            BundleLeg::from_outcome(
                &OpOutcome::refused("object.update", "unknown_object", "no object \"x\""),
                "b".into(),
                1,
            ),
        ];
        let (body, headers) = build_bundle_result(&legs, 2);
        assert_eq!(headers["bundle_errors"], json!(1));
        assert_eq!(
            headers["rows_affected"],
            json!(1),
            "the sibling's write still counted — a bundle is not a transaction"
        );
        assert!(
            !headers.contains_key("error_code"),
            "the reply is not a refusal"
        );
        assert_eq!(body["results"][1]["error_code"], json!("unknown_object"));
    }
}
