//! Phase-7 shared tool-cell helpers.
//!
//! Provides:
//! - `parse_tool_call_args`: extracts the single `tool_call` turn from
//!   `msg.body.messages[]`, parses its `text` as JSON, returns args + id.
//! - `build_tool_result_body`: UBF body with one `tool_result` turn.
//! - `build_error_body`: UBF body with `finish_reason: "error"` + `error_code`.
//! - `with_external_timeout`: a `tokio::time::timeout` wrapper (unused in
//!   slice 1; provided for slice 3 / the web cells).
//! - the `ERR_*` constants: error_code strings (a shared taxonomy for all tool
//!   cells in `meclaw-cells`).
//!
//! Design decision 7.4 (phase-7 brainstorm review 2026-05-21):
//! NO `ToolError` enum, but shared error_code constants + functions taking a
//! `code: &str` param. Avoids maintaining an enum and a string in parallel and
//! stays consistent with the spec-wide `error_code` string convention (see
//! § Canonical dead-letter strings, overview l.570).

use meclaw_core::{JsonValue, Message, serde_json};

// ---- error_code constants (shared taxonomy) ----

/// error_code for invalid input arguments.
pub const ERR_INVALID_INPUT: &str = "invalid_input";
/// error_code for a path outside the security boundary.
pub const ERR_PATH_OUTSIDE_BOUNDARY: &str = "path_outside_boundary";
/// error_code for a resource that was not found.
pub const ERR_NOT_FOUND: &str = "not_found";
/// error_code for a path that is not a directory.
pub const ERR_NOT_A_DIRECTORY: &str = "not_a_directory";
/// error_code for a path that is not a file.
pub const ERR_NOT_A_FILE: &str = "not_a_file";
/// error_code for I/O errors.
pub const ERR_IO_ERROR: &str = "io_error";
/// Provided for slice 3 (the web cells). Not emitted in slice 1 — FileCell has
/// no `external_timeout` contract (disk I/O is not network-timed).
pub const ERR_TIMEOUT: &str = "timeout";
/// error_code for a search pattern that is not found (find_replace).
pub const ERR_PATTERN_NOT_FOUND: &str = "pattern_not_found";

// ---- tool_call parser ----

/// Extracts exactly one `tool_call` turn from `msg.body.messages[]`, parses its
/// `text` as a JSON object and returns `(args, id)`.
///
/// Failure modes (all as `Err(error_text)` with `ERR_INVALID_INPUT` semantics —
/// the cell turns them into an error message):
/// - no `messages[]` array in the body
/// - not a single `tool_call` turn present
/// - more than one `tool_call` turn
/// - `text` missing or not a string
/// - `text` is not a valid JSON object
/// - `id` missing or not a string
///
/// Returns `(JsonValue, Option<String>)` — `id: Option` only because the UBF
/// schema validation makes `id` formally mandatory for `tool_call`, while the
/// helper also wants to accept test probes without an id for the phase-7 tool
/// cells (defensively).
pub fn parse_tool_call_args(msg: &Message) -> Result<(JsonValue, Option<String>), String> {
    let body_value: JsonValue = match &msg.body {
        meclaw_core::Body::Inline(v) => v.clone(),
        // Blob resolution is phase 12; not expected in phase 7.
        _ => return Err("body is not inline".into()),
    };
    let messages = body_value
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "body.messages[] missing or not an array".to_string())?;
    let tool_calls: Vec<&JsonValue> = messages
        .iter()
        .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("tool_call"))
        .collect();
    match tool_calls.len() {
        0 => return Err("no tool_call turn in messages[]".into()),
        1 => {}
        n => return Err(format!("expected exactly one tool_call turn, found {n}")),
    }
    let tc = tool_calls[0];
    let text = tc
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tool_call.text missing or not a string".to_string())?;
    let args: JsonValue =
        serde_json::from_str(text).map_err(|e| format!("tool_call.text is not valid JSON: {e}"))?;
    if !args.is_object() {
        return Err("tool_call.text JSON must be an object".into());
    }
    let id = tc.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
    Ok((args, id))
}

// ---- body builders ----

/// Builds a UBF body with exactly one `tool_result` turn.
/// The `header` map is hung under `body.header`.
pub fn build_tool_result_body(
    text: String,
    id: Option<String>,
    header: serde_json::Map<String, JsonValue>,
) -> JsonValue {
    let mut turn = serde_json::Map::new();
    turn.insert("origin".into(), JsonValue::String("tool".into()));
    turn.insert("type".into(), JsonValue::String("tool_result".into()));
    turn.insert("text".into(), JsonValue::String(text));
    if let Some(i) = id {
        turn.insert("id".into(), JsonValue::String(i));
    }
    let mut body = serde_json::Map::new();
    if !header.is_empty() {
        body.insert("header".into(), JsonValue::Object(header));
    }
    body.insert(
        "messages".into(),
        JsonValue::Array(vec![JsonValue::Object(turn)]),
    );
    JsonValue::Object(body)
}

/// Builds a UBF body with `finish_reason: "error"` + `error_code: <code>` in the
/// header and one `tool_result` turn carrying the human-readable description in
/// `text`.
pub fn build_error_body(
    code: &str,
    text: String,
    id: Option<String>,
    mut header_extras: serde_json::Map<String, JsonValue>,
) -> JsonValue {
    header_extras.insert("finish_reason".into(), JsonValue::String("error".into()));
    header_extras.insert("error_code".into(), JsonValue::String(code.to_string()));
    build_tool_result_body(text, id, header_extras)
}

// ---- external_timeout wrapper (for slice 3) ----

/// `tokio::time::timeout` wrapper with a string error on Elapsed.
/// NOT called in slice 1 — FileCell has no `external_timeout` contract (disk I/O
/// is the operator's responsibility, not cell-timed).
/// Provided for slice 3 (`web_fetch`/`web_search`).
pub async fn with_external_timeout<F>(
    duration: std::time::Duration,
    fut: F,
) -> Result<F::Output, String>
where
    F: std::future::Future,
{
    match tokio::time::timeout(duration, fut).await {
        Ok(v) => Ok(v),
        Err(_) => Err(format!("external timeout after {duration:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{Body, MessageBuilder, Path, serde_json::json, validate_ubf_body};

    fn make_tool_call_msg(text: &str, id: &str) -> meclaw_core::Message {
        MessageBuilder::new(Path::new("/file"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant",
                    "type": "tool_call",
                    "text": text,
                    "id": id,
                }]
            })))
            .build()
    }

    #[test]
    fn parse_tool_call_args_happy_path() {
        let msg = make_tool_call_msg(r#"{"op":"read","path":"a.txt"}"#, "call-1");
        let (args, id) = parse_tool_call_args(&msg).unwrap();
        assert_eq!(args["op"], "read");
        assert_eq!(args["path"], "a.txt");
        assert_eq!(id.as_deref(), Some("call-1"));
    }

    #[test]
    fn parse_tool_call_args_rejects_no_messages() {
        let msg = MessageBuilder::new(Path::new("/file"))
            .body(Body::Inline(json!({})))
            .build();
        assert!(parse_tool_call_args(&msg).is_err());
    }

    #[test]
    fn parse_tool_call_args_rejects_non_json_text() {
        let msg = make_tool_call_msg("not-json", "call-2");
        assert!(parse_tool_call_args(&msg).is_err());
    }

    #[test]
    fn build_tool_result_body_produces_valid_ubf() {
        let mut header = serde_json::Map::new();
        header.insert("operation".into(), json!("read"));
        header.insert("bytes".into(), json!(5));
        let body = build_tool_result_body("hello".to_string(), Some("call-1".to_string()), header);
        validate_ubf_body(&body).expect("must be valid UBF");
        assert_eq!(body["messages"][0]["origin"], "tool");
        assert_eq!(body["messages"][0]["type"], "tool_result");
        assert_eq!(body["messages"][0]["text"], "hello");
        assert_eq!(body["messages"][0]["id"], "call-1");
        assert_eq!(body["header"]["operation"], "read");
        assert_eq!(body["header"]["bytes"], 5);
    }

    #[test]
    fn build_error_body_sets_finish_reason_and_code() {
        let body = build_error_body(
            ERR_NOT_FOUND,
            "path does not exist".to_string(),
            Some("call-1".to_string()),
            serde_json::Map::new(),
        );
        validate_ubf_body(&body).expect("must be valid UBF");
        assert_eq!(body["header"]["finish_reason"], "error");
        assert_eq!(body["header"]["error_code"], "not_found");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_external_timeout_returns_ok_for_fast_future() {
        let r = with_external_timeout(std::time::Duration::from_secs(1), async { 42 }).await;
        assert_eq!(r.unwrap(), 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_external_timeout_errors_on_slow_future() {
        let r = with_external_timeout(std::time::Duration::from_millis(20), async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            42
        })
        .await;
        assert!(r.is_err());
    }
}
