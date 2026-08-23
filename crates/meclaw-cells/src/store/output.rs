//! Output-Builder for `store`: tool_result-Turn + headers per
//! cell-types.md § store (operation/rows_affected/duration_ms/error_code).

use crate::store::ops::OpOutcome;
use meclaw_core::serde_json::{Map, Value, json};

/// Build the `tool_result`-Turn body + headers for a completed `store`
/// op. Caller wraps into a `CellOutput` and pushes via `sink.push(...)`.
///
/// Headers set: `operation`, `rows_affected`, `duration_ms`, optional
/// `error_code`. The `text` field of the tool_result-Turn contains either
/// the `error_text` (when `error_code` is set, per Brainstorm E5: SQL
/// errors are NORMAL tool_results with error_code, not finish_reason:error)
/// or the JSON-serialized `payload`.
pub fn build_tool_result(
    outcome: &OpOutcome,
    tool_call_id: String,
    duration_ms: i64,
) -> (Value, Map<String, Value>) {
    let text = if let Some(err_text) = &outcome.error_text {
        err_text.clone()
    } else {
        meclaw_core::serde_json::to_string(&outcome.payload).unwrap_or_default()
    };
    let body = json!({
        "messages": [
            {
                "origin": "tool",
                "type": "tool_result",
                "text": text,
                "id": tool_call_id,
            }
        ]
    });
    let mut headers = Map::new();
    headers.insert("operation".into(), Value::String(outcome.operation.into()));
    headers.insert("rows_affected".into(), Value::from(outcome.rows_affected));
    headers.insert("duration_ms".into(), Value::from(duration_ms));
    if let Some(code) = outcome.error_code {
        headers.insert("error_code".into(), Value::String(code.into()));
    }
    (body, headers)
}

/// GH #295 — one leg of a bundle reply: an op, its result turn and its own
/// metadata.
///
/// A single-op reply puts the op's metadata on the message header, because
/// there is exactly one op to describe. A bundle cannot: its header describes
/// the bundle as a whole (`operation: "bundle"`, the summed `rows_affected`,
/// the total `duration_ms`, `bundle_errors`).
///
/// The per-op metadata therefore lands in the body's `results[]` slot, NOT in
/// the turn. `$defs/TurnObject` in `crates/meclaw-core/schemas/ubf-body.json`
/// is `additionalProperties: false`, and the colony validates every emission
/// against that schema before it routes it — a turn carrying `operation` or
/// `rows_affected` would dead-letter the whole reply as `InvalidUbfBody` and no
/// caller would ever see it. The body's TOP level is open by design ("Cell-
/// specific top-level slots are allowed"), which is where a store-specific slot
/// belongs. Turn and metadata are correlated by `tool_call_id`.
pub struct BundleLeg {
    /// The `tool_call.id` this leg answers (empty when the call carried none).
    /// It is the correlation key between the turn and its `results[]` entry.
    id: String,
    /// The op this leg ran, or `"error"` when the args named none.
    operation: String,
    /// Rows this op affected — `0` for every failure.
    rows_affected: i64,
    /// How long THIS op took, not the bundle.
    duration_ms: i64,
    /// The code when this op failed, `None` when it did not. Counting these is
    /// what the header's `bundle_errors` reports.
    error_code: Option<String>,
    /// Error text on failure, JSON-serialised payload otherwise — the same
    /// rule [`build_tool_result`] applies to a single-op reply.
    text: String,
}

impl BundleLeg {
    /// The leg a completed op produces. SQL errors arrive here too: they are
    /// normal outcomes carrying an `error_code` (brainstorm E5), and inside a
    /// bundle that code lands on the leg.
    pub fn from_outcome(outcome: &OpOutcome, id: String, duration_ms: i64) -> Self {
        let text = if let Some(err_text) = &outcome.error_text {
            err_text.clone()
        } else {
            meclaw_core::serde_json::to_string(&outcome.payload).unwrap_or_default()
        };
        Self {
            id,
            operation: outcome.operation.to_string(),
            rows_affected: outcome.rows_affected,
            duration_ms,
            error_code: outcome.error_code.map(|c| c.to_string()),
            text,
        }
    }

    /// The leg an op that never reached an outcome produces — a write the
    /// write surface refused, args `dispatch` could not use, a query the
    /// timeout interrupted.
    ///
    /// On a single-op reply each of those is a WHOLE-message refusal
    /// (`finish_reason: "error"` plus a header `error_code`, no payload). A
    /// bundle cannot refuse wholesale on behalf of one leg — its siblings ran
    /// and have rows to hand back — so the refusal travels as a leg like any
    /// other outcome, and the header's `bundle_errors` counts it.
    pub fn refusal(
        operation: &str,
        id: String,
        duration_ms: i64,
        error_code: &str,
        text: String,
    ) -> Self {
        Self {
            id,
            operation: operation.to_string(),
            rows_affected: 0,
            duration_ms,
            error_code: Some(error_code.to_string()),
            text,
        }
    }

    /// This leg's `tool_result` turn — the four keys a tool_result needs.
    /// Beyond them `$defs/TurnObject` allows only `happened_at` (GH #135) and
    /// nothing else at all (`additionalProperties: false`), which a metadata
    /// key would fall outside of. Identical in shape to what
    /// [`build_tool_result`] puts in a single-op reply.
    fn to_turn(&self) -> Value {
        let mut turn = Map::new();
        turn.insert("origin".into(), Value::String("tool".into()));
        turn.insert("type".into(), Value::String("tool_result".into()));
        turn.insert("text".into(), Value::String(self.text.clone()));
        turn.insert("id".into(), Value::String(self.id.clone()));
        Value::Object(turn)
    }

    /// This leg's entry in the body's `results[]` slot: everything the header
    /// carries for a single op, plus the `tool_call_id` that ties it to its
    /// turn.
    fn to_result(&self) -> Value {
        let mut entry = Map::new();
        entry.insert("tool_call_id".into(), Value::String(self.id.clone()));
        entry.insert("operation".into(), Value::String(self.operation.clone()));
        entry.insert("rows_affected".into(), Value::from(self.rows_affected));
        entry.insert("duration_ms".into(), Value::from(self.duration_ms));
        if let Some(code) = &self.error_code {
            entry.insert("error_code".into(), Value::String(code.clone()));
        }
        Value::Object(entry)
    }
}

/// GH #295 — the reply body + headers for a bundle of N > 1 ops.
///
/// Body: `messages[]` with one schema-pure `tool_result` turn per leg, and the
/// store-specific top-level slot `results[]` with one metadata entry per leg,
/// both in call order and correlated by `tool_call_id`.
///
/// Headers set: `operation: "bundle"`, `rows_affected` = the sum over the
/// legs, `duration_ms` = the total the caller measured, and `bundle_errors` =
/// the number of legs carrying an `error_code`.
///
/// `bundle_errors` is stamped unconditionally (project ruling 2026-08-22,
/// option C): a `0` says *checked and clean*, which a consumer must be able to
/// tell apart from *nobody stamped it*. The key exists only here — a single-op
/// reply never carries it, so [`build_tool_result`]'s shape does not move a
/// byte. The header's own `error_code` is NOT set here: it keeps its hard
/// meaning (the whole reply is a refusal and carries no payload) and never
/// signals a partial failure.
pub fn build_bundle_result(
    legs: &[BundleLeg],
    total_duration_ms: i64,
) -> (Value, Map<String, Value>) {
    let rows_affected: i64 = legs.iter().map(|l| l.rows_affected).sum();
    let bundle_errors = legs.iter().filter(|l| l.error_code.is_some()).count() as i64;
    let body = json!({
        "messages": legs.iter().map(BundleLeg::to_turn).collect::<Vec<Value>>(),
        "results": legs.iter().map(BundleLeg::to_result).collect::<Vec<Value>>()
    });
    let mut headers = Map::new();
    headers.insert("operation".into(), Value::String("bundle".into()));
    headers.insert("rows_affected".into(), Value::from(rows_affected));
    headers.insert("duration_ms".into(), Value::from(total_duration_ms));
    headers.insert("bundle_errors".into(), Value::from(bundle_errors));
    (body, headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ops::OpOutcome;
    use meclaw_core::serde_json::Value;

    #[test]
    fn builds_tool_result_with_operation_header() {
        let outcome = OpOutcome {
            operation: "insert",
            rows_affected: 1,
            payload: Value::Null,
            error_code: None,
            error_text: None,
        };
        let (body, headers) = build_tool_result(&outcome, "call_id_1".to_string(), 42);
        let h_op = headers.get("operation").unwrap();
        assert_eq!(h_op, &Value::String("insert".into()));
        let h_rows = headers.get("rows_affected").unwrap();
        assert_eq!(h_rows.as_i64().unwrap(), 1);
        assert_eq!(headers.get("duration_ms").unwrap().as_i64().unwrap(), 42);
        assert!(headers.get("error_code").is_none());
        let turn = body["messages"].as_array().unwrap();
        assert_eq!(turn.len(), 1);
        assert_eq!(turn[0]["type"], "tool_result");
        assert_eq!(turn[0]["id"], "call_id_1");
    }

    #[test]
    fn bundle_headers_sum_the_rows_and_count_the_failures() {
        let ok = OpOutcome {
            operation: "select",
            rows_affected: 3,
            payload: Value::Array(vec![]),
            error_code: None,
            error_text: None,
        };
        let legs = vec![
            BundleLeg::from_outcome(&ok, "a".into(), 1),
            BundleLeg::refusal("insert", "b".into(), 2, "write_denied", "no".into()),
            BundleLeg::from_outcome(&ok, "c".into(), 3),
        ];
        let (body, headers) = build_bundle_result(&legs, 9);
        assert_eq!(headers.get("operation").unwrap(), "bundle");
        assert_eq!(headers.get("rows_affected").unwrap().as_i64().unwrap(), 6);
        assert_eq!(headers.get("duration_ms").unwrap().as_i64().unwrap(), 9);
        assert_eq!(headers.get("bundle_errors").unwrap().as_i64().unwrap(), 1);
        assert!(headers.get("error_code").is_none());
        let rs = body["results"].as_array().unwrap();
        assert_eq!(rs.len(), 3);
        assert_eq!(rs[1]["error_code"], "write_denied");
        assert_eq!(rs[1]["operation"], "insert");
        assert_eq!(rs[1]["duration_ms"], 2);
        assert_eq!(rs[1]["tool_call_id"], "b");
        assert!(rs[0].get("error_code").is_none());
    }

    /// The turns of a bundle carry exactly what `$defs/TurnObject` allows — the
    /// colony validates every emission against it, so an extra key here is a
    /// dead-lettered reply, not a richer one.
    #[test]
    fn bundle_turns_carry_no_keys_beyond_the_ubf_turn_schema() {
        let ok = OpOutcome {
            operation: "select",
            rows_affected: 1,
            payload: Value::Array(vec![]),
            error_code: None,
            error_text: None,
        };
        let (body, _headers) = build_bundle_result(
            &[
                BundleLeg::from_outcome(&ok, "a".into(), 1),
                BundleLeg::refusal("insert", "b".into(), 2, "write_denied", "no".into()),
            ],
            3,
        );
        for turn in body["messages"].as_array().unwrap() {
            let keys: Vec<&str> = turn.as_object().unwrap().keys().map(|k| &**k).collect();
            assert_eq!(
                keys,
                ["id", "origin", "text", "type"],
                "a bundle turn must stay schema-pure: {turn}"
            );
        }
        meclaw_core::validate_ubf_body(&body).expect("a bundle body must be valid UBF");
    }

    #[test]
    fn a_clean_bundle_still_stamps_bundle_errors_zero() {
        let ok = OpOutcome {
            operation: "select",
            rows_affected: 0,
            payload: Value::Array(vec![]),
            error_code: None,
            error_text: None,
        };
        let (_body, headers) = build_bundle_result(
            &[
                BundleLeg::from_outcome(&ok, "a".into(), 0),
                BundleLeg::from_outcome(&ok, "b".into(), 0),
            ],
            0,
        );
        assert_eq!(headers.get("bundle_errors").unwrap().as_i64().unwrap(), 0);
    }

    #[test]
    fn includes_error_code_header_on_sql_error() {
        let outcome = OpOutcome {
            operation: "insert",
            rows_affected: 0,
            payload: Value::Null,
            error_code: Some("constraint_violation"),
            error_text: Some("UNIQUE constraint failed".into()),
        };
        let (_body, headers) = build_tool_result(&outcome, "id".into(), 0);
        assert_eq!(
            headers.get("error_code").unwrap(),
            &Value::String("constraint_violation".into())
        );
    }
}
