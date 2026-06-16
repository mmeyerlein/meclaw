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
