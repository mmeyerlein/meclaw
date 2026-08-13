//! Phase-8 LlmCell Translate-functions (UBF <-> OpenAI). Pure, no I/O.
//!
//! T4: `concat_system_prompt` — system-tree -> joined prompt string.
//! T6: `build_openai_request` — UBF -> OpenAI request JSON.
//! T7: `parse_openai_response` — OpenAI response JSON -> UBF turn(s) + meta.

use meclaw_core::serde_json::Value;

/// Phase-8 Translate-error enum. T8 will add WireError mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranslateError {
    /// Unsupported UBF turn `type` (e.g. `image`, `audio`, unknown combo).
    TypeUnsupported(String),
    /// A `tool_call`-turn's `text` could not be parsed as the OpenAI
    /// function-call JSON (`{"name": ..., "arguments": "..."}`).
    ToolCallParse(String),
    /// Response carried a `finish_reason` that is null, missing or unknown —
    /// NEVER silently treated as `stop` (clarification 4).
    UnknownFinishReason(String),
    /// Response JSON shape mismatch (missing `choices[0]`, `model`, `id`, …).
    ResponseShape(String),
}

/// Walk a UBF system-tree and concatenate leaves' `text` values into a single
/// system-prompt string, joined by `"\n\n"`.
///
/// Order: sub-slots from `system_order` first (in that order), then remaining
/// top-level keys alphabetically. Inside each sub-tree, DFS in alphabetical key
/// order. The `tools` sub-slot is skipped entirely (extracted separately by T6
/// `build_openai_request`).
///
/// Infallible since GH #86. It used to reject a `{text_id}` leaf with
/// `BlobUnsupported`, because nothing resolved that pointer class; the substrate
/// now expands it at the delivery boundary, so every leaf that arrives here is
/// an inline `{"text": …}` container and there is no failure left to report.
pub(crate) fn concat_system_prompt(tree: &Value, system_order: &[String]) -> String {
    let Some(obj) = tree.as_object() else {
        return String::new();
    };
    let mut top_keys: Vec<String> = obj
        .keys()
        .filter(|k| k.as_str() != "tools")
        .cloned()
        .collect();
    // Order: system_order first (in given order), then remaining alphabetically.
    let mut ordered: Vec<String> = Vec::new();
    for k in system_order {
        if let Some(pos) = top_keys.iter().position(|x| x == k) {
            ordered.push(top_keys.remove(pos));
        }
    }
    top_keys.sort();
    ordered.extend(top_keys);

    let mut parts: Vec<String> = Vec::new();
    for k in &ordered {
        let subtree = &obj[k];
        walk_collect(subtree, &mut parts);
    }
    parts.join("\n\n")
}

fn walk_collect(node: &Value, out: &mut Vec<String>) {
    let Some(obj) = node.as_object() else {
        return;
    };
    if obj.contains_key("text") {
        if let Some(t) = obj["text"].as_str() {
            out.push(t.to_string());
        }
        return;
    }
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for k in keys {
        walk_collect(&obj[k], out);
    }
}

/// Build the OpenAI Chat-Completions request body JSON from `LlmParams`,
/// a pre-concatenated `system_string` (T4 output), the UBF `messages[]`-turns
/// and the already-extracted `tools_extracted` array.
///
/// Shape:
/// ```json
/// {
///   "model": params.model,
///   "messages": [{"role":"system","content": system_string}?, ...mapped_turns],
///   "temperature": params.temperature,
///   "max_tokens": params.max_tokens,
///   "tools": tools_extracted    // omitted when empty
/// }
/// ```
/// then `params.provider_extra` is overlaid at root (overlay wins per
/// cell-types.md:145).
///
/// - `system_string` empty -> no leading system-message inserted.
/// - `tools_extracted` empty -> no `tools` key in body.
///
/// Returns `Err(TranslateError::TypeUnsupported)` for `image`/`audio`/unknown
/// turn-types, and `Err(TranslateError::ToolCallParse)` if an
/// `assistant`/`tool_call`-turn's `text` is not valid JSON.
pub(crate) fn build_openai_request(
    params: &crate::llm::params::LlmParams,
    system_string: &str,
    input_messages: &[Value],
    tools_extracted: &[Value],
) -> Result<Value, TranslateError> {
    use meclaw_core::serde_json::{Map, json};

    let mut messages: Vec<Value> = Vec::new();
    if !system_string.is_empty() {
        messages.push(json!({"role": "system", "content": system_string}));
    }
    // Merge rule (cell-types § Provider-Translate): CONSECUTIVE UBF tool_call
    // turns collapse into ONE wire assistant message with tool_calls[] — the
    // provider 400s per-call assistant messages whose results follow later
    // (Run-4b wire finding, receipt 922d93f). UBF stays one turn = one call.
    let mut prev_was_tool_call = false;
    for turn in input_messages {
        let is_tool_call = turn.get("origin").and_then(|v| v.as_str()) == Some("assistant")
            && turn.get("type").and_then(|v| v.as_str()) == Some("tool_call");
        if is_tool_call && prev_was_tool_call {
            let entry = tool_call_entry(turn)?;
            if let Some(tcs) = messages
                .last_mut()
                .and_then(|m| m.get_mut("tool_calls"))
                .and_then(|v| v.as_array_mut())
            {
                tcs.push(entry);
                continue;
            }
        }
        messages.push(map_turn(turn)?);
        prev_was_tool_call = is_tool_call;
    }

    let mut body = Map::new();
    body.insert("model".into(), json!(params.model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("temperature".into(), json!(params.temperature));
    body.insert("max_tokens".into(), json!(params.max_tokens));
    if !tools_extracted.is_empty() {
        body.insert("tools".into(), Value::Array(tools_extracted.to_vec()));
    }
    // provider_extra overlay (wins on conflict per cell-types.md:145).
    for (k, v) in &params.provider_extra {
        body.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(body))
}

/// Standard base64 alphabet (RFC 4648 §4), the encoder side of the in-tree
/// `store::query::hamming::decode_base64`. Hand-rolled for the same reason the
/// decoder is: the tech stack is a closed allow-list and this is 20 lines.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes as standard base64 with `=` padding.
pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        let sextets = [
            (triple >> 18) & 0x3F,
            (triple >> 12) & 0x3F,
            (triple >> 6) & 0x3F,
            triple & 0x3F,
        ];
        for (i, s) in sextets.iter().enumerate() {
            // 1 input byte carries 2 sextets, 2 bytes carry 3; the rest is padding.
            if i <= chunk.len() {
                out.push(BASE64_ALPHABET[*s as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Build one OpenAI-compatible image content part from an attachment's bytes
/// and its (sidecar-authoritative) MIME type — GH #87.
///
/// Shape per the Chat-Completions vision contract: a `content` array entry
/// `{"type": "image_url", "image_url": {"url": "data:<mime>;base64,<data>"}}`.
/// The data URL is self-contained, so no blob ever leaves the colony as a
/// dereferenceable link.
pub(crate) fn image_content_part(mime_type: &str, bytes: &[u8]) -> Value {
    use meclaw_core::serde_json::json;
    json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{mime_type};base64,{}", encode_base64(bytes))}
    })
}

/// Fold resolved image parts into a built Chat-Completions request — GH #87.
///
/// The attachments hang off the message body, not off a turn, so they join the
/// conversation where a vision model expects them: on the **last user
/// message**, whose plain string `content` becomes a content array of
/// `{"type":"text"}` plus the image parts (an existing array is extended).
/// Without a user message the parts become one appended user message of their
/// own — an attachment always has to reach the model as user input.
///
/// **Empty `image_parts` is a no-op**: a cell that declares no attachment
/// consumption produces the pre-GH-#87 request byte for byte.
pub(crate) fn attach_image_parts(request: &mut Value, image_parts: Vec<Value>) {
    use meclaw_core::serde_json::json;
    if image_parts.is_empty() {
        return;
    }
    let Some(messages) = request.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };
    let last_user = messages
        .iter_mut()
        .rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .map(|idx| &mut messages[idx]);
    match last_user {
        Some(msg) => {
            let content = msg.get("content").cloned().unwrap_or(Value::Null);
            let mut parts: Vec<Value> = match content {
                Value::Array(existing) => existing,
                Value::String(text) => vec![json!({"type": "text", "text": text})],
                _ => Vec::new(),
            };
            parts.extend(image_parts);
            msg["content"] = Value::Array(parts);
        }
        None => messages.push(json!({"role": "user", "content": image_parts})),
    }
}

/// Map provider-attribution `params` to their HTTP request headers (A4).
///
/// **Translate is where provider knowledge lives** (A4 ruling): the Translate
/// boundary decides each param's wire destination. The explicit mapping table:
///
/// | param           | wire destination       |
/// |-----------------|------------------------|
/// | `model`         | request body `model`   |
/// | `temperature`   | request body `temperature` |
/// | `max_tokens`    | request body `max_tokens` |
/// | `provider_extra`| request body (overlay) |
/// | `http_referer`  | HTTP header `HTTP-Referer` |
/// | `x_title`       | HTTP header `X-Title`  |
///
/// This function is the header side of that table. `build_openai_request` is
/// the body side. Only `Some` attribution params produce a header — an unset
/// param produces nothing (no empty-header noise). The table is a closed
/// allowlist: `Authorization` is NOT a param-controllable header — it is the
/// `api_key` Bearer set by the wire layer (secret hygiene).
pub(crate) fn build_attribution_headers(
    params: &crate::llm::params::LlmParams,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(v) = &params.http_referer {
        out.push(("HTTP-Referer".to_string(), v.clone()));
    }
    if let Some(v) = &params.x_title {
        out.push(("X-Title".to_string(), v.clone()));
    }
    out
}

fn map_turn(turn: &Value) -> Result<Value, TranslateError> {
    use meclaw_core::serde_json::json;
    let origin = turn.get("origin").and_then(|v| v.as_str()).unwrap_or("");
    let type_ = turn.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let text = turn.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let id = turn.get("id").and_then(|v| v.as_str()).unwrap_or("");
    match (origin, type_) {
        ("user", "text") => Ok(json!({"role": "user", "content": text})),
        ("assistant", "text") => Ok(json!({"role": "assistant", "content": text})),
        ("system", "text") => Ok(json!({"role": "system", "content": text})),
        ("assistant", "tool_call") => Ok(json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": [tool_call_entry(turn)?],
        })),
        ("tool", "tool_result") => Ok(json!({
            "role": "tool",
            "tool_call_id": id,
            "content": text,
        })),
        (_, "image") | (_, "audio") => Err(TranslateError::TypeUnsupported(type_.to_string())),
        _ => Err(TranslateError::TypeUnsupported(format!(
            "origin={origin}, type={type_}"
        ))),
    }
}

/// Build the OpenAI `tool_calls[]`-entry (`{id, type, function}`) for a UBF
/// `tool_call`-turn. Used by `map_turn` (fresh assistant message) and by the
/// consecutive-turn merge in `build_openai_request` (append to last message).
fn tool_call_entry(turn: &Value) -> Result<Value, TranslateError> {
    use meclaw_core::serde_json::json;
    let text = turn.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let id = turn.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let function: Value = meclaw_core::serde_json::from_str(text)
        .map_err(|e| TranslateError::ToolCallParse(format!("id={id}: {e}")))?;
    Ok(json!({"id": id, "type": "function", "function": function}))
}

/// Map a `TranslateError` to the UBF `error_code` (cell-types Z.112).
///
/// All variants map to `"provider_error"` (Q6 catch-all, see Phase-8-Plan
/// § 2 Q6). cell-types Z.112 does not have a translate-specific code, so
/// `provider_error` is the catch-all bucket for Translate-/Mapping-/
/// `UnknownFinishReason` error.
///
/// The signature accepts `&TranslateError` for symmetry with
/// `wire_error_to_code` and so future variants can be mapped differently
/// without breaking callers.
pub(crate) fn translate_error_to_code(_err: &TranslateError) -> &'static str {
    "provider_error"
}

/// Translated OpenAI Chat-Completions response — assistant UBF-turn(s) plus
/// meta-fields the LlmCell will surface as UBF-headers (model, response_id,
/// finish_reason, token-usage).
///
/// `assistant_turn` may contain 0, 1 or many turns: tool-calls produce one
/// turn per OpenAI `tool_call`, optionally followed by a text-turn if the
/// response carries both `content` and `tool_calls`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslatedResponse {
    /// UBF-turns to emit (1+ for normal responses; multiple for tool-calls).
    pub(crate) assistant_turn: Vec<Value>,
    /// Mapped finish-reason: `stop` / `length` / `tool_calls` / `content_filter`.
    /// Legacy `function_call` is mapped to `tool_calls`; null/unknown raises
    /// `UnknownFinishReason`.
    pub(crate) finish_reason: String,
    /// Prompt-side token-count from `usage.prompt_tokens` (optional).
    pub(crate) tokens_prompt: Option<u64>,
    /// Completion-side token-count from `usage.completion_tokens` (optional).
    pub(crate) tokens_completion: Option<u64>,
    /// Actual model from `response.model` (may differ from request).
    pub(crate) model: String,
    /// Provider response-id from `response.id`.
    pub(crate) response_id: String,
}

/// Parse an OpenAI Chat-Completions response body into UBF assistant-turn(s)
/// + meta. Phase 8 assumes `n=1` implicit, so only `choices[0]` is consulted.
///
/// Steps:
/// 1. Extract `choices[0]`. Missing → `ResponseShape`.
/// 2. Map `finish_reason`: `stop`/`length`/`tool_calls`/`content_filter`
///    pass-through; legacy `function_call` → `tool_calls`; null/missing/unknown
///    → `UnknownFinishReason` (NEVER silently coerced to `stop`,
///    the spec owner's clarification 4).
/// 3. Build `assistant_turn`: each `message.tool_calls[i]` → one UBF
///    `tool_call`-turn (id pass-through, text = serde-stringified
///    `function`). If `message.content` is a non-null string, append a
///    `text`-turn. Order: tool_calls first, then text.
/// 4. Extract meta: `model` and `id` are required (else `ResponseShape`);
///    `usage.prompt_tokens` / `usage.completion_tokens` are optional.
pub(crate) fn parse_openai_response(json: &Value) -> Result<TranslatedResponse, TranslateError> {
    use meclaw_core::serde_json::json;

    let choice = json
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| TranslateError::ResponseShape("missing choices[0]".to_string()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| TranslateError::ResponseShape("missing choices[0].message".to_string()))?;

    let raw_reason = choice.get("finish_reason");
    let finish_reason = match raw_reason.and_then(|v| v.as_str()) {
        Some("stop") => "stop".to_string(),
        Some("length") => "length".to_string(),
        Some("tool_calls") => "tool_calls".to_string(),
        Some("content_filter") => "content_filter".to_string(),
        // Legacy single-call format from older Chat-Completions; map to current.
        Some("function_call") => "tool_calls".to_string(),
        Some(other) => return Err(TranslateError::UnknownFinishReason(other.to_string())),
        None => return Err(TranslateError::UnknownFinishReason("null".to_string())),
    };

    let mut assistant_turn: Vec<Value> = Vec::new();
    // Tool calls first.
    if let Some(tcs) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let function = tc.get("function").cloned().unwrap_or(Value::Null);
            let text = meclaw_core::serde_json::to_string(&function).map_err(|e| {
                TranslateError::ResponseShape(format!("tool_call.function serialize: {e}"))
            })?;
            assistant_turn.push(json!({
                "origin": "assistant",
                "type": "tool_call",
                "id": id,
                "text": text,
            }));
        }
    }
    // Then text content (if present and non-null).
    if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
        assistant_turn.push(json!({
            "origin": "assistant",
            "type": "text",
            "text": content,
        }));
    }

    let model = json
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TranslateError::ResponseShape("missing response.model".to_string()))?
        .to_string();
    let response_id = json
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TranslateError::ResponseShape("missing response.id".to_string()))?
        .to_string();
    let tokens_prompt = json
        .get("usage")
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let tokens_completion = json
        .get("usage")
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64());

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
    use super::{
        TranslateError, attach_image_parts, build_openai_request, concat_system_prompt,
        encode_base64, image_content_part, parse_openai_response, translate_error_to_code,
    };
    use crate::llm::params::LlmParams;
    use meclaw_core::serde_json::{Value, json};

    // ---------- T4 tests (kept) ----------

    #[test]
    fn empty_tree_returns_empty_string() {
        let tree = json!({});
        let out = concat_system_prompt(&tree, &[]);
        assert_eq!(out, "");
    }

    #[test]
    fn single_leaf_returns_its_text() {
        let tree = json!({"identity": {"soul": {"text": "S"}}});
        let out = concat_system_prompt(&tree, &[]);
        assert_eq!(out, "S");
    }

    #[test]
    fn respects_system_order_then_alphabetic() {
        let tree = json!({
            "identity": {"soul": {"text": "S"}},
            "facts":    {"x":    {"text": "F"}},
        });
        let out = concat_system_prompt(&tree, &["identity".to_string(), "facts".to_string()]);
        assert_eq!(out, "S\n\nF");
    }

    #[test]
    fn tools_subtree_excluded() {
        let tree = json!({
            "identity": {"soul": {"text": "S"}},
            "tools":    {"calc": {"text": "{\"name\":\"calc\"}"}}
        });
        let out = concat_system_prompt(&tree, &[]);
        assert_eq!(out, "S");
    }

    /// GH #86: a leaf that arrived as a `{text_id}` pointer reaches this
    /// function already resolved — the substrate expands it at the delivery
    /// boundary, into exactly the `{"text": …}` container an inline leaf uses.
    /// There is nothing left here to tell the two apart, which is the point:
    /// the old `BlobUnsupported` rejection guarded a case that can no longer
    /// arrive.
    #[test]
    fn a_leaf_that_arrived_resolved_joins_the_prompt_like_any_other() {
        let tree = json!({
            "identity": {
                "soul": {"text": "inline"},
                "body": {"text": "the long persona"},
            }
        });
        let out = concat_system_prompt(&tree, &[]);
        assert_eq!(
            out, "the long persona\n\ninline",
            "both leaves join the prompt; alphabetical DFS puts body before soul"
        );
    }

    // ---------- T6 tests ----------

    /// Build minimal LlmParams via `parse()` for tests.
    fn p() -> LlmParams {
        LlmParams::parse(&json!({
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "x",
        }))
        .unwrap()
    }

    #[test]
    fn build_minimal_user_turn() {
        let params = p();
        let messages = [json!({"origin": "user", "type": "text", "text": "Hi"})];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "Hi"}],
                "temperature": 0.7,
                "max_tokens": 4096,
            })
        );
        // No `tools`-key when empty.
        assert!(body.get("tools").is_none(), "expected no tools key");
    }

    #[test]
    fn build_with_leading_system_string() {
        let params = p();
        let messages = [json!({"origin": "user", "type": "text", "text": "Hi"})];
        let body = build_openai_request(&params, "You are X.", &messages, &[]).unwrap();
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], json!({"role": "system", "content": "You are X."}));
        assert_eq!(arr[1], json!({"role": "user", "content": "Hi"}));
    }

    #[test]
    fn build_tool_call_turn() {
        let params = p();
        let messages = [json!({
            "origin": "assistant",
            "type": "tool_call",
            "id": "id-1",
            "text": "{\"name\":\"calc\",\"arguments\":\"{}\"}",
        })];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0],
            json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": "id-1",
                    "type": "function",
                    "function": {"name": "calc", "arguments": "{}"},
                }],
            })
        );
    }

    /// Run-4b offline proof (research-assistant LIFT.md, receipt 922d93f):
    /// Form A — one wire assistant message PER call before the tool results —
    /// gets 400 from the provider ("must be followed by tool messages
    /// responding to each tool_call_id"). The wire contract is Form B: ONE
    /// assistant message carrying all consecutive calls in tool_calls[].
    /// UBF stays unchanged (one turn = one call = one id) — the merge is
    /// pure wire-format translation.
    #[test]
    fn build_merges_consecutive_tool_call_turns_into_one_message() {
        let params = p();
        let messages = [
            json!({"origin": "user", "type": "text", "text": "weather?"}),
            json!({"origin": "assistant", "type": "tool_call", "id": "c1",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
            json!({"origin": "assistant", "type": "tool_call", "id": "c2",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
            json!({"origin": "assistant", "type": "tool_call", "id": "c3",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
            json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "15"}),
            json!({"origin": "tool", "type": "tool_result", "id": "c2", "text": "17"}),
            json!({"origin": "tool", "type": "tool_result", "id": "c3", "text": "10"}),
        ];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        let arr = body["messages"].as_array().unwrap();
        // user + ONE merged assistant + 3 tool results = 5 wire messages.
        assert_eq!(arr.len(), 5, "expected merged form B, got {arr:#?}");
        assert_eq!(arr[1]["role"], "assistant");
        assert_eq!(arr[1]["content"], Value::Null);
        let tcs = arr[1]["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 3);
        assert_eq!(tcs[0]["id"], "c1");
        assert_eq!(tcs[1]["id"], "c2");
        assert_eq!(tcs[2]["id"], "c3");
        assert_eq!(arr[2]["role"], "tool");
        assert_eq!(arr[2]["tool_call_id"], "c1");
    }

    /// Form C (interleaved: call, its result, next call, …) is wire-valid
    /// per the Run-4b offline proof — turns separated by a tool_result must
    /// NOT be merged.
    #[test]
    fn build_keeps_interleaved_tool_calls_unmerged() {
        let params = p();
        let messages = [
            json!({"origin": "assistant", "type": "tool_call", "id": "c1",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
            json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "15"}),
            json!({"origin": "assistant", "type": "tool_call", "id": "c2",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
            json!({"origin": "tool", "type": "tool_result", "id": "c2", "text": "17"}),
        ];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(
            arr.len(),
            4,
            "interleaved form C must stay 1:1, got {arr:#?}"
        );
        assert_eq!(arr[0]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(arr[2]["tool_calls"].as_array().unwrap().len(), 1);
    }

    /// An assistant text-turn between two tool_call-turns breaks consecutiveness
    /// — only directly adjacent tool_call turns merge.
    #[test]
    fn build_text_turn_breaks_tool_call_merge_chain() {
        let params = p();
        let messages = [
            json!({"origin": "assistant", "type": "tool_call", "id": "c1",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
            json!({"origin": "assistant", "type": "text", "text": "thinking..."}),
            json!({"origin": "assistant", "type": "tool_call", "id": "c2",
                   "text": "{\"name\":\"search\",\"arguments\":\"{}\"}"}),
        ];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(
            arr.len(),
            3,
            "text turn must break the merge chain, got {arr:#?}"
        );
        assert_eq!(arr[0]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(arr[1]["content"], "thinking...");
        assert_eq!(arr[2]["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_tool_result_turn() {
        let params = p();
        let messages = [json!({
            "origin": "tool",
            "type": "tool_result",
            "id": "id-1",
            "text": "42",
        })];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0],
            json!({"role": "tool", "tool_call_id": "id-1", "content": "42"})
        );
    }

    #[test]
    fn build_with_tools_array() {
        let params = p();
        let messages = [json!({"origin": "user", "type": "text", "text": "Hi"})];
        let tools = [json!({
            "type": "function",
            "function": {"name": "calc", "parameters": {"type": "object"}}
        })];
        let body = build_openai_request(&params, "", &messages, &tools).unwrap();
        let t = body.get("tools").expect("tools key must be present");
        assert_eq!(t, &Value::Array(tools.to_vec()));
    }

    #[test]
    fn build_provider_extra_overlay() {
        let mut params = p();
        params.provider_extra.insert("seed".to_string(), json!(42));
        let messages = [json!({"origin": "user", "type": "text", "text": "Hi"})];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        assert_eq!(body["seed"], json!(42));
    }

    #[test]
    fn build_provider_extra_overrides_temperature() {
        let mut params = p();
        // Sanity: base default is 0.7.
        assert_eq!(params.temperature, 0.7);
        params
            .provider_extra
            .insert("temperature".to_string(), json!(0.0));
        let messages = [json!({"origin": "user", "type": "text", "text": "Hi"})];
        let body = build_openai_request(&params, "", &messages, &[]).unwrap();
        assert_eq!(
            body["temperature"],
            json!(0.0),
            "provider_extra.temperature must overlay-win over params.temperature"
        );
    }

    // ───── GH #87: attachments[] → image content parts ─────

    #[test]
    fn base64_encodes_every_residue_class() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(&[0, 0, 0]), "AAAA");
        assert_eq!(encode_base64(&[0xFF]), "/w==");
        assert_eq!(encode_base64(&[0xFF, 0xFF]), "//8=");
        assert_eq!(encode_base64(&[0, 0, 0, 0xFF]), "AAAA/w==");
        assert_eq!(encode_base64(b"Man"), "TWFu");
        assert_eq!(encode_base64(b"hello world"), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn base64_round_trips_through_the_in_tree_decoder() {
        // The store's hand-rolled decoder is the counterpart; a round trip over
        // a byte range that exercises the full alphabet pins both.
        let bytes: Vec<u8> = (0u8..=255).collect();
        let encoded = encode_base64(&bytes);
        let decoded = crate::store::query::hamming::decode_base64(&encoded).unwrap();
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn image_content_part_carries_mime_and_base64_data_url() {
        let part = image_content_part("image/png", &[0xFF, 0xFF]);
        assert_eq!(
            part,
            json!({"type": "image_url",
                   "image_url": {"url": "data:image/png;base64,//8="}})
        );
    }

    #[test]
    fn attach_image_parts_merges_into_the_last_user_message() {
        let params = p();
        let messages = [
            json!({"origin": "user", "type": "text", "text": "first"}),
            json!({"origin": "assistant", "type": "text", "text": "reply"}),
            json!({"origin": "user", "type": "text", "text": "look at this"}),
        ];
        let mut body = build_openai_request(&params, "", &messages, &[]).unwrap();
        attach_image_parts(&mut body, vec![image_content_part("image/png", b"\xff")]);
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 3, "no message is added when a user turn exists");
        // The earlier user turn keeps its plain string content.
        assert_eq!(arr[0]["content"], json!("first"));
        // The last one becomes a content array: text first, then the image.
        assert_eq!(
            arr[2],
            json!({"role": "user", "content": [
                {"type": "text", "text": "look at this"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,/w=="}}
            ]})
        );
    }

    #[test]
    fn attach_image_parts_appends_a_user_message_when_there_is_none() {
        let params = p();
        let messages = [json!({"origin": "assistant", "type": "text", "text": "hi"})];
        let mut body = build_openai_request(&params, "", &messages, &[]).unwrap();
        attach_image_parts(&mut body, vec![image_content_part("image/jpeg", b"\xff")]);
        let arr = body["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr[1],
            json!({"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,/w=="}}
            ]})
        );
    }

    #[test]
    fn attach_image_parts_without_parts_is_byte_identical() {
        let params = p();
        let messages = [json!({"origin": "user", "type": "text", "text": "Hi"})];
        let before = build_openai_request(&params, "sys", &messages, &[]).unwrap();
        let mut after = before.clone();
        attach_image_parts(&mut after, vec![]);
        assert_eq!(
            meclaw_core::serde_json::to_string(&after).unwrap(),
            meclaw_core::serde_json::to_string(&before).unwrap(),
            "a cell without attachments must produce the pre-GH-#87 request byte for byte"
        );
    }

    #[test]
    fn attach_image_parts_extends_an_existing_content_array() {
        let mut body = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "a"}]}
        ]});
        attach_image_parts(&mut body, vec![image_content_part("image/png", b"\xff")]);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0], json!({"type": "text", "text": "a"}));
        assert_eq!(content[1]["type"], "image_url");
    }

    #[test]
    fn build_rejects_image_type() {
        let params = p();
        let messages = [json!({"origin": "user", "type": "image", "text": "<bytes>"})];
        let err = build_openai_request(&params, "", &messages, &[]).unwrap_err();
        assert_eq!(err, TranslateError::TypeUnsupported("image".to_string()));
    }

    // ---------- W4 (A4): attribution-header mapping ----------

    fn p_attr(http_referer: Option<&str>, x_title: Option<&str>) -> LlmParams {
        let mut params = p();
        params.http_referer = http_referer.map(|s| s.to_string());
        params.x_title = x_title.map(|s| s.to_string());
        params
    }

    #[test]
    fn attribution_headers_set_map_to_wire_names() {
        let params = p_attr(Some("https://example.com"), Some("Example App"));
        let headers = super::build_attribution_headers(&params);
        assert_eq!(
            headers,
            vec![
                (
                    "HTTP-Referer".to_string(),
                    "https://example.com".to_string()
                ),
                ("X-Title".to_string(), "Example App".to_string()),
            ]
        );
    }

    #[test]
    fn attribution_headers_unset_yields_empty() {
        let params = p_attr(None, None);
        assert!(
            super::build_attribution_headers(&params).is_empty(),
            "unset attribution must produce no headers (no empty-header noise)"
        );
    }

    #[test]
    fn attribution_headers_partial_only_set_field() {
        let params = p_attr(Some("https://example.com"), None);
        let headers = super::build_attribution_headers(&params);
        assert_eq!(
            headers,
            vec![(
                "HTTP-Referer".to_string(),
                "https://example.com".to_string()
            )]
        );
    }

    #[test]
    fn attribution_headers_never_carry_authorization() {
        // Secret-hygiene (A4): the attribution mapping is a closed allowlist
        // (HTTP-Referer, X-Title). There is no params field that maps to
        // Authorization — by construction it can never appear here.
        let params = p_attr(Some("x"), Some("y"));
        let headers = super::build_attribution_headers(&params);
        assert!(
            !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("authorization")),
            "attribution headers must never include Authorization"
        );
    }

    #[test]
    fn attribution_env_substitution_chain_to_header() {
        // ${VAR} chain (.env → config → param → header). The .env→config step
        // is the generic colony substitution (walks every string param value);
        // this pins that an attribution param goes through it like any other
        // and the resolved value reaches the wire header verbatim.
        use std::collections::HashMap;
        let mut env = HashMap::new();
        env.insert(
            "OPENROUTER_HTTP_REFERER".to_string(),
            "https://example.com".to_string(),
        );
        env.insert("OPENROUTER_X_TITLE".to_string(), "Example App".to_string());
        let raw = json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x",
            "http_referer": "${OPENROUTER_HTTP_REFERER}",
            "x_title": "${OPENROUTER_X_TITLE}",
        });
        let substituted =
            meclaw_colony::mutation::substitute::substitute_env_only(&raw, &env).unwrap();
        let params = LlmParams::parse(&substituted).unwrap();
        let headers = super::build_attribution_headers(&params);
        assert_eq!(
            headers,
            vec![
                (
                    "HTTP-Referer".to_string(),
                    "https://example.com".to_string()
                ),
                ("X-Title".to_string(), "Example App".to_string()),
            ]
        );
    }

    // ---------- T7 tests ----------

    #[test]
    fn parse_minimal_text_response() {
        let resp = json!({
            "id": "chatcmpl-abc",
            "model": "gpt-4o-2026-01-01",
            "choices": [{
                "message": {"role": "assistant", "content": "Hi"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        });
        let t = parse_openai_response(&resp).unwrap();
        assert_eq!(
            t.assistant_turn,
            vec![json!({"origin": "assistant", "type": "text", "text": "Hi"})]
        );
        assert_eq!(t.finish_reason, "stop");
        assert_eq!(t.tokens_prompt, Some(5));
        assert_eq!(t.tokens_completion, Some(2));
        assert_eq!(t.model, "gpt-4o-2026-01-01");
        assert_eq!(t.response_id, "chatcmpl-abc");
    }

    #[test]
    fn parse_finish_reason_length_passthrough() {
        let resp = json!({
            "id": "chatcmpl-abc",
            "model": "gpt-4o-2026-01-01",
            "choices": [{
                "message": {"role": "assistant", "content": "Hi"},
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        });
        let t = parse_openai_response(&resp).unwrap();
        assert_eq!(t.finish_reason, "length");
    }

    #[test]
    fn parse_tool_calls_response() {
        let resp = json!({
            "id": "chatcmpl-abc",
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "calc", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3}
        });
        let t = parse_openai_response(&resp).unwrap();
        assert_eq!(t.finish_reason, "tool_calls");
        assert_eq!(t.assistant_turn.len(), 1);
        assert_eq!(t.assistant_turn[0]["origin"], "assistant");
        assert_eq!(t.assistant_turn[0]["type"], "tool_call");
        assert_eq!(t.assistant_turn[0]["id"], "call-1");
        let fn_text: Value =
            meclaw_core::serde_json::from_str(t.assistant_turn[0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(fn_text["name"], "calc");
        assert_eq!(fn_text["arguments"], "{}");
    }

    #[test]
    fn parse_function_call_legacy_mapped_to_tool_calls() {
        let resp = json!({
            "id": "x",
            "model": "y",
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "function_call"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        });
        let t = parse_openai_response(&resp).unwrap();
        assert_eq!(
            t.finish_reason, "tool_calls",
            "legacy function_call must map to tool_calls"
        );
    }

    #[test]
    fn parse_null_finish_reason_returns_unknown() {
        let resp = json!({
            "id": "x",
            "model": "y",
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": null
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        });
        let err = parse_openai_response(&resp).unwrap_err();
        assert!(
            matches!(err, TranslateError::UnknownFinishReason(_)),
            "expected UnknownFinishReason, got {err:?}"
        );
    }

    #[test]
    fn parse_missing_choices_returns_response_shape() {
        let resp = json!({"id": "x", "model": "y"});
        let err = parse_openai_response(&resp).unwrap_err();
        assert!(
            matches!(err, TranslateError::ResponseShape(_)),
            "expected ResponseShape, got {err:?}"
        );
    }

    // ---------- T8 tests (TranslateError -> error_code) ----------

    #[test]
    fn translate_error_all_variants_map_to_provider_error() {
        assert_eq!(
            translate_error_to_code(&TranslateError::TypeUnsupported("image".into())),
            "provider_error"
        );
        assert_eq!(
            translate_error_to_code(&TranslateError::ToolCallParse("x".into())),
            "provider_error"
        );
        assert_eq!(
            translate_error_to_code(&TranslateError::UnknownFinishReason("null".into())),
            "provider_error"
        );
        assert_eq!(
            translate_error_to_code(&TranslateError::ResponseShape("missing".into())),
            "provider_error"
        );
    }
}
