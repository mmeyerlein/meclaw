//! P10 — the Responses wire dialect (second dialect beside chat-completions).
//!
//! Pinned against `github.com/openai/codex` @
//! `266c6920d9b82fe4d68959529565256b12a9be99` (read 2026-08-09):
//!
//! - request struct: `codex-api/src/common.rs:252-275`
//! - subscription field values (`store:false`, `stream:true`, `include`):
//!   `core/src/client.rs:926-942`
//! - `input[]` item shapes: `protocol/src/models.rs:716-734, 814-840`
//! - SSE event set + "the truth is `output_item.done`, not the deltas":
//!   `codex-api/src/sse/responses.rs:330-480`
//!
//! **Translate boundary discipline (cell-types.md § Provider-Translate):**
//! provider-native JSON never leaves this module. The cell core sees UBF only.
//!
//! **Streaming:** the wire is streamed because the subscription backend accepts
//! nothing else, but the cell stays atomically-emitting (cell-types.md Z.168) —
//! deltas are dropped and the full body is folded into ONE assistant turn.

use crate::llm::params::{AuthMode, LlmParams};
use crate::llm::translate::{TranslateError, TranslatedResponse};
use serde_json::Value;

/// Build a Responses-API request body from UBF input.
///
/// Field choices that are NOT free (reference `core/src/client.rs:926-942`):
/// `store` is always `false` (the subscription backend does not persist), and
/// `stream` is always `true` (there is no non-streaming path on that backend).
/// `include: ["reasoning.encrypted_content"]` is a consequence of `store:false`
/// and is sent only on the subscription lane — the metered endpoint rejects it
/// for models without reasoning.
pub(crate) fn build_responses_request(
    params: &LlmParams,
    system_string: &str,
    input_messages: &[Value],
    tools_extracted: &[Value],
) -> Result<Value, TranslateError> {
    use serde_json::{Map, json};

    let mut input: Vec<Value> = Vec::new();
    for turn in input_messages {
        input.push(map_turn(turn)?);
    }

    let mut body = Map::new();
    body.insert("model".into(), json!(params.model));
    if !system_string.is_empty() {
        body.insert("instructions".into(), json!(system_string));
    }
    body.insert("input".into(), Value::Array(input));
    body.insert("tool_choice".into(), json!("auto"));
    body.insert("parallel_tool_calls".into(), json!(false));
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    if params.auth == AuthMode::OauthSubscription {
        body.insert(
            "include".into(),
            Value::Array(vec![json!("reasoning.encrypted_content")]),
        );
    }
    // P14: the subscription backend rejects both fields outright
    // ("Unsupported parameter: temperature", verified live 2026-08-10). They
    // ARE valid on the official Responses API, so the cut belongs on `auth`,
    // not on `wire_dialect` — the metered lane keeps its sampling control.
    if params.auth != AuthMode::OauthSubscription {
        body.insert("temperature".into(), json!(params.temperature));
        body.insert("max_output_tokens".into(), json!(params.max_tokens));
    }
    if !tools_extracted.is_empty() {
        let tools: Vec<Value> = tools_extracted.iter().map(to_responses_tool).collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    // provider_extra overlay wins (cell-types.md:145), same rule as the
    // chat-completions dialect. It runs AFTER the inserts above, so it stays
    // the escape hatch for a caller who does need a sampling param.
    for (k, v) in &params.provider_extra {
        body.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(body))
}

/// Chat-completions carries the tool schema bare; Responses tags it inline
/// (`{"type":"function", name, description, parameters}` — flat, unlike the
/// nested chat-completions form).
fn to_responses_tool(tool: &Value) -> Value {
    let mut obj = tool.as_object().cloned().unwrap_or_default();
    obj.insert("type".into(), Value::String("function".into()));
    Value::Object(obj)
}

/// Build the Responses-dialect HTTP request headers (A4: the Translate
/// boundary owns every param→wire-destination decision).
///
/// Pinned against the reference client (§ 3.2 of the P10 plan):
///
/// | header | source |
/// |---|---|
/// | `Accept: text/event-stream` | mandatory whenever `stream:true` |
/// | `session-id` | fresh UUID per call (note the hyphen, not `session_id`) |
/// | `originator` | `params.oauth_originator`, default `codex_cli_rs` |
/// | `User-Agent` | `{originator}/{version} ({os}; {arch})` |
/// | `version` | subscription lane: `params.oauth_client_version`, default the Codex client version — the backend gates model availability on it (P14). Otherwise this crate's version. |
/// | `ChatGPT-Account-ID` | token store's `account_id`, subscription lane only |
///
/// `Authorization` is deliberately absent: it is not param-controllable and is
/// set last by the wire layer from the bearer (secret hygiene, cell-types
/// Z.185). The attribution headers are appended by the caller.
pub(crate) fn build_responses_headers(
    params: &LlmParams,
    account_id: Option<&str>,
) -> Vec<(String, String)> {
    let originator = params
        .oauth_originator
        .as_deref()
        .unwrap_or(crate::llm::auth::DEFAULT_OAUTH_ORIGINATOR)
        .to_string();
    // P14: on the subscription lane this is a gate, not a label — the backend
    // refuses models that require a newer client than the one reported. The
    // metered lane has no such gate and keeps reporting the honest value.
    let version = if params.auth == AuthMode::OauthSubscription {
        params
            .oauth_client_version
            .as_deref()
            .unwrap_or(crate::llm::auth::DEFAULT_OAUTH_CLIENT_VERSION)
    } else {
        env!("CARGO_PKG_VERSION")
    };
    let mut out = vec![
        ("Accept".to_string(), "text/event-stream".to_string()),
        (
            "session-id".to_string(),
            meclaw_core::Uuid::now_v7().to_string(),
        ),
        ("version".to_string(), version.to_string()),
        (
            "User-Agent".to_string(),
            format!(
                "{originator}/{version} ({}; {})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        ),
        ("originator".to_string(), originator),
    ];
    // Only the subscription lane carries an account id; the metered endpoint
    // has no use for it.
    if params.auth == AuthMode::OauthSubscription
        && let Some(id) = account_id
    {
        out.push(("ChatGPT-Account-ID".to_string(), id.to_string()));
    }
    out
}

/// Map one UBF turn to a typed Responses `input[]` item.
fn map_turn(turn: &Value) -> Result<Value, TranslateError> {
    use serde_json::json;
    let origin = turn.get("origin").and_then(|v| v.as_str()).unwrap_or("");
    let type_ = turn.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let text = turn.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let id = turn.get("id").and_then(|v| v.as_str()).unwrap_or("");
    match (origin, type_) {
        ("user", "text") => Ok(message_item("user", "input_text", text)),
        ("system", "text") => Ok(message_item("system", "input_text", text)),
        // Assistant text is echoed back as `output_text` — the reference
        // distinguishes model-produced from user-supplied content by that tag.
        ("assistant", "text") => Ok(message_item("assistant", "output_text", text)),
        ("assistant", "tool_call") => {
            let f: Value = serde_json::from_str(text)
                .map_err(|e| TranslateError::ToolCallParse(format!("id={id}: {e}")))?;
            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
            // Arguments travel as a JSON *string* on this wire.
            let arguments = match f.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "{}".to_string(),
            };
            Ok(json!({
                "type": "function_call",
                "call_id": id,
                "name": name,
                "arguments": arguments,
            }))
        }
        ("tool", "tool_result") => Ok(json!({
            "type": "function_call_output",
            "call_id": id,
            "output": text,
        })),
        (_, "image") | (_, "audio") => Err(TranslateError::TypeUnsupported(type_.to_string())),
        _ => Err(TranslateError::TypeUnsupported(format!(
            "origin={origin}, type={type_}"
        ))),
    }
}

fn message_item(role: &str, content_type: &str, text: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "role": role,
        "content": [{"type": content_type, "text": text}],
    })
}

/// Parse a Responses SSE body into UBF turns + meta.
///
/// Follows the reference's rule: the deltas
/// (`response.output_text.delta`, …) are display-only and are ignored; the
/// canonical content is `response.output_item.done`, closed by
/// `response.completed`.
pub(crate) fn parse_responses_sse(body: &str) -> Result<TranslatedResponse, TranslateError> {
    let mut items: Vec<Value> = Vec::new();
    let mut model = String::new();
    let mut response_id = String::new();
    let mut tokens_prompt = None;
    let mut tokens_completion = None;
    let mut completed = false;
    let mut incomplete_reason: Option<String> = None;

    for line in body.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(payload) else {
            // A non-JSON data line is not fatal on this wire.
            continue;
        };
        match event.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "response.created" | "response.completed" | "response.incomplete" => {
                let resp = event.get("response").unwrap_or(&Value::Null);
                if let Some(m) = resp.get("model").and_then(|v| v.as_str()) {
                    model = m.to_string();
                }
                if let Some(i) = resp.get("id").and_then(|v| v.as_str()) {
                    response_id = i.to_string();
                }
                if let Some(u) = resp.get("usage") {
                    tokens_prompt = u.get("input_tokens").and_then(|v| v.as_u64());
                    tokens_completion = u.get("output_tokens").and_then(|v| v.as_u64());
                }
                if event.get("type").and_then(|v| v.as_str()) == Some("response.completed") {
                    completed = true;
                }
                if let Some(r) = resp
                    .get("incomplete_details")
                    .and_then(|d| d.get("reason"))
                    .and_then(|v| v.as_str())
                {
                    incomplete_reason = Some(r.to_string());
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    items.push(item.clone());
                }
            }
            "response.failed" => {
                let msg = event
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("code").or_else(|| e.get("message")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                return Err(TranslateError::ResponseShape(format!(
                    "provider reported response.failed: {msg}"
                )));
            }
            _ => {}
        }
    }

    if !completed && incomplete_reason.is_none() {
        return Err(TranslateError::ResponseShape(
            "stream ended without response.completed".to_string(),
        ));
    }
    build_translated(
        &items,
        model,
        response_id,
        tokens_prompt,
        tokens_completion,
        incomplete_reason,
    )
}

/// Parse a non-streamed Responses body (`stream:false`). Kept because the
/// metered endpoint and OpenAI-compatible proxies may answer plain JSON.
pub(crate) fn parse_responses_response(json: &Value) -> Result<TranslatedResponse, TranslateError> {
    let items: Vec<Value> = json
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let model = json
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let response_id = json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let usage = json.get("usage");
    build_translated(
        &items,
        model,
        response_id,
        usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_u64()),
        usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_u64()),
        json.get("incomplete_details")
            .and_then(|d| d.get("reason"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    )
}

/// Fold collected `output[]`/`output_item.done` items into UBF turns.
///
/// Order mirrors the chat-completions dialect: tool-call turns first, then the
/// text turn, so downstream topologies see one shape regardless of dialect.
fn build_translated(
    items: &[Value],
    model: String,
    response_id: String,
    tokens_prompt: Option<u64>,
    tokens_completion: Option<u64>,
    incomplete_reason: Option<String>,
) -> Result<TranslatedResponse, TranslateError> {
    use serde_json::json;

    let mut tool_turns: Vec<Value> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();

    for item in items {
        match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "message" => {
                if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                    for part in content {
                        if part.get("type").and_then(|v| v.as_str()) == Some("output_text")
                            && let Some(t) = part.get("text").and_then(|v| v.as_str())
                        {
                            text_parts.push(t.to_string());
                        }
                    }
                }
            }
            "function_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                // UBF tool_call text is the stringified function object — the
                // same shape the chat-completions dialect produces.
                let text = json!({"name": name, "arguments": arguments}).to_string();
                tool_turns.push(json!({
                    "origin": "assistant", "type": "tool_call",
                    "id": call_id, "text": text,
                }));
            }
            // `reasoning` items carry no UBF representation today; the encrypted
            // blob is only useful when replaying a chain, which an atomically
            // emitting cell never does.
            _ => {}
        }
    }

    if model.is_empty() {
        return Err(TranslateError::ResponseShape(
            "response carried no model".to_string(),
        ));
    }

    let mut assistant_turn = tool_turns.clone();
    if !text_parts.is_empty() {
        assistant_turn.push(json!({
            "origin": "assistant", "type": "text", "text": text_parts.join(""),
        }));
    }

    let finish_reason = match incomplete_reason.as_deref() {
        Some("max_output_tokens") => "length".to_string(),
        Some("content_filter") => "content_filter".to_string(),
        Some(other) => {
            return Err(TranslateError::UnknownFinishReason(other.to_string()));
        }
        None if !tool_turns.is_empty() => "tool_calls".to_string(),
        None => "stop".to_string(),
    };

    Ok(TranslatedResponse {
        assistant_turn,
        finish_reason,
        tokens_prompt,
        tokens_completion,
        model,
        response_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(auth_oauth: bool) -> LlmParams {
        let raw = if auth_oauth {
            json!({"provider":"openai","model":"gpt-5","auth":"oauth_subscription",
                   "auth_ref":"/tmp/a.json","temperature":0.3,"max_tokens":512})
        } else {
            json!({"provider":"openai","model":"gpt-5","api_key":"k",
                   "wire_dialect":"responses","temperature":0.3,"max_tokens":512})
        };
        LlmParams::parse(&raw).unwrap()
    }

    // ───── C1: body shape ─────

    #[test]
    fn responses_request_matches_pinned_shape() {
        let b = build_responses_request(&params(true), "be terse", &[], &[]).unwrap();
        assert_eq!(b["model"], "gpt-5");
        assert_eq!(b["instructions"], "be terse");
        assert_eq!(b["store"], false, "subscription backend never persists");
        assert_eq!(b["stream"], true, "there is no non-streaming path");
        assert_eq!(b["tool_choice"], "auto");
        assert_eq!(b["parallel_tool_calls"], false);
        assert_eq!(b["include"][0], "reasoning.encrypted_content");
        // P14: the sampling assertions moved to
        // `metered_responses_lane_keeps_its_sampling_params` — the ChatGPT
        // backend rejects both fields, so pinning them here pinned a shape that
        // cannot be sent.
        assert!(
            b.get("max_tokens").is_none(),
            "chat-completions field name leaked"
        );
        assert!(b.get("messages").is_none(), "chat-completions shape leaked");
    }

    // ───── P14: the sampling params the subscription backend rejects ─────

    /// Live 2026-08-10: `{"detail":"Unsupported parameter: temperature"}`, and
    /// the same for `max_output_tokens`. Both are valid on the official
    /// Responses API — hence the cut is on `auth`, not on `wire_dialect`.
    #[test]
    fn subscription_lane_omits_the_sampling_params_the_backend_rejects() {
        let b = build_responses_request(&params(true), "be terse", &[], &[]).unwrap();
        assert!(
            b.get("temperature").is_none(),
            "ChatGPT backend answers 'Unsupported parameter: temperature'"
        );
        assert!(
            b.get("max_output_tokens").is_none(),
            "ChatGPT backend answers 'Unsupported parameter: max_output_tokens'"
        );
    }

    #[test]
    fn metered_responses_lane_keeps_its_sampling_params() {
        let b = build_responses_request(&params(false), "be terse", &[], &[]).unwrap();
        assert_eq!(b["temperature"], 0.3, "the official Responses API takes it");
        assert_eq!(b["max_output_tokens"], 512);
    }

    /// Regression guard for the R3 escape hatch, green before AND after the
    /// cut: the `provider_extra` overlay is applied after the body inserts.
    #[test]
    fn provider_extra_can_still_force_a_sampling_param_on_the_subscription_lane() {
        let raw = json!({"provider":"openai","model":"gpt-5.6-luna",
                         "auth":"oauth_subscription","auth_ref":"/tmp/a.json",
                         "provider_extra":{"temperature":0.9}});
        let p = LlmParams::parse(&raw).unwrap();
        let b = build_responses_request(&p, "", &[], &[]).unwrap();
        assert_eq!(
            b["temperature"], 0.9,
            "the provider_extra overlay still wins"
        );
    }

    // ───── P14: the `version` header is a gate, not a label ─────

    fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
        h.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// Live 2026-08-10: reporting this crate's version made the backend answer
    /// `"The 'gpt-5.6-luna' model requires a newer version of Codex."`
    #[test]
    fn subscription_headers_report_the_codex_client_version_not_meclaws() {
        let h = build_responses_headers(&params(true), Some("acct"));
        assert_eq!(
            header(&h, "version"),
            Some(crate::llm::auth::DEFAULT_OAUTH_CLIENT_VERSION)
        );
        assert_ne!(
            header(&h, "version"),
            Some(env!("CARGO_PKG_VERSION")),
            "meclaw's own version makes recent models unreachable"
        );
        assert!(
            header(&h, "User-Agent")
                .unwrap()
                .contains(crate::llm::auth::DEFAULT_OAUTH_CLIENT_VERSION),
            "User-Agent is derived from the same version"
        );
    }

    #[test]
    fn oauth_client_version_param_overrides_the_default() {
        let raw = json!({"provider":"openai","model":"gpt-5.6-luna",
                         "auth":"oauth_subscription","auth_ref":"/tmp/a.json",
                         "oauth_client_version":"9.9.9"});
        let p = LlmParams::parse(&raw).unwrap();
        let h = build_responses_headers(&p, Some("acct"));
        assert_eq!(header(&h, "version"), Some("9.9.9"));
    }

    #[test]
    fn metered_responses_lane_still_reports_meclaws_own_version() {
        let h = build_responses_headers(&params(false), None);
        assert_eq!(
            header(&h, "version"),
            Some(env!("CARGO_PKG_VERSION")),
            "no gate there — keep the honest value"
        );
    }

    #[test]
    fn empty_system_string_omits_instructions() {
        let b = build_responses_request(&params(true), "", &[], &[]).unwrap();
        assert!(b.get("instructions").is_none());
    }

    #[test]
    fn include_is_subscription_only() {
        // The metered endpoint rejects reasoning includes for plain models.
        let b = build_responses_request(&params(false), "", &[], &[]).unwrap();
        assert!(b.get("include").is_none());
        assert_eq!(b["store"], false);
    }

    #[test]
    fn provider_extra_overlays_the_body() {
        let raw = json!({"provider":"openai","model":"gpt-5","auth":"oauth_subscription",
                         "auth_ref":"/tmp/a.json","provider_extra":{"service_tier":"flex"}});
        let p = LlmParams::parse(&raw).unwrap();
        let b = build_responses_request(&p, "", &[], &[]).unwrap();
        assert_eq!(b["service_tier"], "flex");
    }

    // ───── C2: turn mapping ─────

    #[test]
    fn maps_ubf_turns_to_typed_items() {
        let turns = vec![
            json!({"origin":"user","type":"text","text":"hi"}),
            json!({"origin":"assistant","type":"text","text":"hello"}),
            json!({"origin":"system","type":"text","text":"ctx"}),
        ];
        let b = build_responses_request(&params(true), "", &turns, &[]).unwrap();
        let input = b["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "hi");
        // assistant content is tagged output_text, not input_text
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2]["role"], "system");
    }

    #[test]
    fn rejects_unsupported_turn_types() {
        let turns = vec![json!({"origin":"user","type":"image","text":"x"})];
        let err = build_responses_request(&params(true), "", &turns, &[]).unwrap_err();
        assert!(matches!(err, TranslateError::TypeUnsupported(_)), "{err:?}");
    }

    // ───── C3: tools ─────

    #[test]
    fn maps_tool_turns_and_tool_schema() {
        let turns = vec![
            json!({"origin":"assistant","type":"tool_call","id":"call_1",
                   "text":"{\"name\":\"get_weather\",\"arguments\":\"{}\"}"}),
            json!({"origin":"tool","type":"tool_result","id":"call_1","text":"sunny"}),
        ];
        let tools = vec![json!({"name":"get_weather","description":"w","parameters":{}})];
        let b = build_responses_request(&params(true), "", &turns, &tools).unwrap();
        let input = b["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_1");
        assert_eq!(input[0]["name"], "get_weather");
        assert_eq!(
            input[0]["arguments"], "{}",
            "arguments travel as a JSON string"
        );
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["output"], "sunny");
        // tool schema is flat + inline-tagged on this wire
        assert_eq!(b["tools"][0]["type"], "function");
        assert_eq!(b["tools"][0]["name"], "get_weather");
        assert!(
            b["tools"][0].get("function").is_none(),
            "nested form leaked"
        );
    }

    #[test]
    fn malformed_tool_call_text_is_reported() {
        let turns = vec![json!({"origin":"assistant","type":"tool_call","id":"c","text":"{{"})];
        let err = build_responses_request(&params(true), "", &turns, &[]).unwrap_err();
        assert!(matches!(err, TranslateError::ToolCallParse(_)), "{err:?}");
    }

    // ───── C4: non-streamed parse ─────

    #[test]
    fn parses_plain_json_response() {
        let body = json!({
            "id": "resp_1", "model": "gpt-5-2026",
            "output": [{"type":"message","role":"assistant",
                        "content":[{"type":"output_text","text":"hello"}]}],
            "usage": {"input_tokens": 11, "output_tokens": 3}
        });
        let t = parse_responses_response(&body).unwrap();
        assert_eq!(t.model, "gpt-5-2026");
        assert_eq!(t.response_id, "resp_1");
        assert_eq!(t.finish_reason, "stop");
        assert_eq!(t.tokens_prompt, Some(11));
        assert_eq!(t.tokens_completion, Some(3));
        assert_eq!(t.assistant_turn.len(), 1);
        assert_eq!(t.assistant_turn[0]["text"], "hello");
    }

    // ───── C5/C6: SSE parse ─────

    /// Faithful SSE framing: an `event:` line precedes every `data:` line, as
    /// the real endpoint sends it (verified 2026-08-09). The parser must skip
    /// the `event:` lines and read only `data:`.
    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|e| {
                let name = e.get("type").and_then(|v| v.as_str()).unwrap_or("message");
                format!("event: {name}\ndata: {e}\n\n")
            })
            .collect::<String>()
    }

    #[test]
    fn parses_sse_stream_from_output_item_done() {
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"resp_9","model":"gpt-5-sse"}}),
            // deltas are display-only and must not contribute to the result
            json!({"type":"response.output_text.delta","delta":"hel"}),
            json!({"type":"response.output_text.delta","delta":"lo"}),
            json!({"type":"response.output_item.done","item":{
                "type":"message","role":"assistant",
                "content":[{"type":"output_text","text":"hello"}]}}),
            json!({"type":"response.completed","response":{
                "id":"resp_9","model":"gpt-5-sse",
                "usage":{"input_tokens":7,"output_tokens":2}}}),
        ]);
        let t = parse_responses_sse(&body).unwrap();
        assert_eq!(t.assistant_turn.len(), 1);
        assert_eq!(
            t.assistant_turn[0]["text"], "hello",
            "text must come from output_item.done, and deltas must not double it"
        );
        assert_eq!(t.finish_reason, "stop");
        assert_eq!(t.tokens_prompt, Some(7));
    }

    #[test]
    fn sse_surfaces_response_model() {
        // response.model receipt rule: the model the provider actually used.
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"r","model":"gpt-5-actual"}}),
            json!({"type":"response.output_item.done","item":{
                "type":"message","content":[{"type":"output_text","text":"x"}]}}),
            json!({"type":"response.completed","response":{"id":"r","model":"gpt-5-actual"}}),
        ]);
        let t = parse_responses_sse(&body).unwrap();
        assert_eq!(t.model, "gpt-5-actual");
        assert_eq!(t.response_id, "r");
    }

    #[test]
    fn sse_tool_calls_become_ubf_turns() {
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"r","model":"m"}}),
            json!({"type":"response.output_item.done","item":{
                "type":"function_call","call_id":"call_7","name":"lookup",
                "arguments":"{\"q\":1}"}}),
            json!({"type":"response.completed","response":{"id":"r","model":"m"}}),
        ]);
        let t = parse_responses_sse(&body).unwrap();
        assert_eq!(t.finish_reason, "tool_calls");
        assert_eq!(t.assistant_turn[0]["type"], "tool_call");
        assert_eq!(t.assistant_turn[0]["id"], "call_7");
        let inner: Value =
            serde_json::from_str(t.assistant_turn[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(inner["name"], "lookup");
        assert_eq!(inner["arguments"], "{\"q\":1}");
    }

    #[test]
    fn sse_reasoning_items_are_dropped_not_leaked() {
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"r","model":"m"}}),
            json!({"type":"response.output_item.done","item":{
                "type":"reasoning","summary":[],"encrypted_content":"OPAQUE-BLOB"}}),
            json!({"type":"response.output_item.done","item":{
                "type":"message","content":[{"type":"output_text","text":"answer"}]}}),
            json!({"type":"response.completed","response":{"id":"r","model":"m"}}),
        ]);
        let t = parse_responses_sse(&body).unwrap();
        assert_eq!(t.assistant_turn.len(), 1);
        let dump = serde_json::to_string(&t.assistant_turn).unwrap();
        assert!(
            !dump.contains("OPAQUE-BLOB"),
            "encrypted blob leaked: {dump}"
        );
    }

    #[test]
    fn sse_failed_event_is_an_error() {
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"r","model":"m"}}),
            json!({"type":"response.failed","response":{
                "error":{"code":"context_length_exceeded","message":"too long"}}}),
        ]);
        let err = parse_responses_sse(&body).unwrap_err();
        match err {
            TranslateError::ResponseShape(d) => {
                assert!(d.contains("context_length_exceeded"), "{d}")
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn sse_truncated_stream_is_an_error_not_a_silent_stop() {
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"r","model":"m"}}),
            json!({"type":"response.output_item.done","item":{
                "type":"message","content":[{"type":"output_text","text":"partial"}]}}),
        ]);
        let err = parse_responses_sse(&body).unwrap_err();
        assert!(matches!(err, TranslateError::ResponseShape(_)), "{err:?}");
    }

    #[test]
    fn sse_incomplete_maps_to_length() {
        let body = sse(&[
            json!({"type":"response.created","response":{"id":"r","model":"m"}}),
            json!({"type":"response.output_item.done","item":{
                "type":"message","content":[{"type":"output_text","text":"cut"}]}}),
            json!({"type":"response.incomplete","response":{"id":"r","model":"m",
                "incomplete_details":{"reason":"max_output_tokens"}}}),
        ]);
        let t = parse_responses_sse(&body).unwrap();
        assert_eq!(t.finish_reason, "length");
    }
}
