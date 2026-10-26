//! GH #1239: the native Anthropic Messages translate (`provider: "anthropic"`).
//!
//! Two pure halves, no I/O: [`build_messages_request`] turns the UBF history
//! into a `POST /v1/messages` body, [`parse_messages_response_with`] turns the
//! Messages answer back into the same [`TranslatedResponse`] the chat wire
//! yields, so everything after the translate stays dialect-blind. GH #1241
//! adds what the cell's anthropic lane needs around them: image blocks,
//! effort/thinking, the cache marks and the thinking of an open tool round.

use crate::llm::params::{CacheMode, LlmParams};
use crate::llm::translate::{
    TranslateError, TranslatedResponse, cache_control, encode_base64, peer_content, takes_param,
    trailing_assistant_note, usage_cost, usage_tokens_cache_write, usage_tokens_cached,
};
use meclaw_core::serde_json::{Map, Value, json};

/// Build the `POST /v1/messages` body (T10-03).
///
/// - `system` is a top-level field; a UBF `system` turn in the history is
///   appended to it, because the Messages wire has no system role.
/// - Every wire message carries content blocks, and a turn whose role equals
///   the previous one's is appended to it: consecutive `tool_call` turns
///   become ONE assistant message of `tool_use` blocks, and the
///   `tool_result` turns of the round, with any user text after them, ONE
///   user message in history order -- the provider answers 400 to a round
///   split over several messages or to a result behind a text block, and
///   nothing is reordered here. Blank texts are dropped (`is_blank_text`).
/// - `image_blocks` ([`image_block`], T10-04) follow the text of the LAST
///   `user` turn (GH #847: never a `peer` turn), in the same user message;
///   without a `user` turn they close the history as user content.
/// - `max_tokens` is mandatory on this wire and always written; the sampling
///   and reasoning fields follow the model's list ([`sampling_fields`],
///   T10-05), the cache marks `cache_mode` ([`apply_cache_marks`], T10-06).
/// - `provider_extra` is the overlay at the root and wins on conflict.
pub(crate) fn build_messages_request(
    params: &LlmParams,
    system_string: &str,
    input_messages: &[Value],
    tools_extracted: &[Value],
    image_blocks: Vec<Value>,
) -> Result<Value, TranslateError> {
    // `translate::image_anchor` counts the same turns as an ordinal among
    // the role-`user` wire messages; this wire merges those, so the anchor
    // is the turn itself.
    let anchor = input_messages.iter().rposition(|t| {
        t.get("origin").and_then(Value::as_str) == Some("user")
            && t.get("type").and_then(Value::as_str) == Some("text")
    });
    let mut images = Some(image_blocks).filter(|b| !b.is_empty());
    let mut system = system_string.to_string();
    let mut messages: Vec<Value> = Vec::new();
    for (i, turn) in input_messages.iter().enumerate() {
        match map_turn(turn)? {
            Mapped::System(text) if text.trim().is_empty() => {}
            Mapped::System(text) => {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&text);
            }
            Mapped::Block(_, block) if is_blank_text(&block) => {}
            Mapped::Block(role, block) => push_block(&mut messages, role, block),
        }
        // Behind the anchor's text even when that text was blank and fell.
        if Some(i) == anchor {
            for block in images.take().into_iter().flatten() {
                push_block(&mut messages, "user", block);
            }
        }
    }
    for block in images.take().into_iter().flatten() {
        push_block(&mut messages, "user", block);
    }
    fold_trailing_assistant(&mut messages);
    let mut body = Map::new();
    body.insert("model".into(), json!(params.model));
    body.insert("max_tokens".into(), json!(params.effective_max_tokens()));
    place_sampling(&mut body, params);
    if !system.is_empty() {
        body.insert("system".into(), Value::String(system));
    }
    body.insert("messages".into(), Value::Array(messages));
    if !tools_extracted.is_empty() {
        let tools = tools_extracted.iter().map(to_messages_tool).collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    apply_cache_marks(&mut body, params);
    for (k, v) in &params.provider_extra {
        body.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(body))
}

/// GH #1260 (OR-WG-6): a `tool_scope` with `require` asks the provider for a
/// tool call at all on this one request (`tool_choice: {type: "any"}`). Written AFTER the
/// build, so it wins over a static `provider_extra.tool_choice` for this
/// call only; the cell calls it only while tools stay (`Scoped::require`).
///
/// Not beside thinking (fix round 1, review M-3): the Messages wire refuses a
/// forced tool call next to extended thinking with a 400, so a request whose
/// `thinking` is on keeps its tool choice as built and the call answers
/// `false` -- the cell says so in a `WARN` line. Only `thinking: {type:
/// "disabled"}` counts as off here: Sonnet 5.5's `between_tools` off
/// ([`thinking_is_off`]) is no way past it, because Sonnet 5.5 refuses a forced
/// tool choice in every thinking mode (GH #1300 review).
pub(crate) fn require_tool_call(request: &mut Value) -> bool {
    let Some(body) = request.as_object_mut() else {
        return false;
    };
    let thinking = body
        .get("thinking")
        .is_some_and(|t| t.get("type").and_then(Value::as_str) != Some("disabled"));
    if thinking {
        return false;
    }
    body.insert("tool_choice".into(), json!({"type": "any"}));
    true
}

/// GH #1282: the request never ends on an assistant message -- that is a
/// prefill, and current Claude models refuse it with 400 (R025: an escalated
/// tool round ending on the quick voice's bridge sentence, a consult window
/// ending on the previous answer). A trailing assistant message of text
/// blocks leaves the wire as a closing note in the user message before it
/// ([`trailing_assistant_note`]), or as the only user message of an empty
/// rest. Folded into the user turn rather than sent as a mid-conversation
/// `system` message: that one is model-gated (not on Haiku 5.5 or Sonnet 5)
/// and must follow a user message, which a trailing assistant turn is not.
/// A trailing message with a `tool_use` block is an open round and stays.
fn fold_trailing_assistant(messages: &mut Vec<Value>) {
    let Some(last) = messages.last().filter(|m| m["role"] == "assistant") else {
        return;
    };
    let Some(blocks) = last.get("content").and_then(Value::as_array) else {
        return;
    };
    if blocks.is_empty() || blocks.iter().any(|b| b["type"] != "text") {
        return;
    }
    let said: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
    let note = trailing_assistant_note(&said);
    messages.pop();
    push_block(messages, "user", json!({"type": "text", "text": note}));
}

/// Append a content block under `role`: to the last message when it has the
/// same role (the wire wants the roles to alternate), else as a new one.
fn push_block(messages: &mut Vec<Value>, role: &str, block: Value) {
    match messages.last_mut() {
        Some(last) if last["role"] == role => {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.push(block);
            }
        }
        _ => messages.push(json!({"role": role, "content": [block]})),
    }
}

/// T10-04: one image attachment as a Messages content block -- the bytes
/// inline as base64 under the sidecar's MIME type, like the data URL of the
/// chat wire, so no blob ever leaves the colony as a link.
pub(crate) fn image_block(mime_type: &str, bytes: &[u8]) -> Value {
    json!({
        "type": "image",
        "source": {"type": "base64", "media_type": mime_type, "data": encode_base64(bytes)}
    })
}

/// T10-05: the sampling fields of a Messages request before the model's
/// `supported_params` is laid over them, under their sampling names
/// (`translate::SAMPLING_PARAMS`), so `hop.dropped`/`hop.unverified` read the
/// same list as the request. `temperature` only with a list (OR-programm-tm.
/// AN2.6): a model that refuses it answers 400 to every call, so it goes out
/// only where the list names it. `reasoning` comes last and wins (AN2.7).
pub(crate) fn sampling_fields(params: &LlmParams) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    if let Some(effort) = &params.reasoning_effort {
        out.push(("reasoning_effort", json!(effort)));
    }
    if let Some(budget) = params.thinking_budget {
        out.push(("thinking_token_budget", json!(budget)));
    }
    if params.supported_params.is_some() {
        out.push(("temperature", json!(params.temperature)));
    }
    if let Some(reasoning) = &params.reasoning {
        out.push(("reasoning", reasoning.clone()));
    }
    out
}

/// T10-05: write each field the model takes in its Messages form --
/// `reasoning_effort` as `output_config.effort` (`none` = thinking
/// disabled), `thinking_budget` as the `budget_tokens` of an older model,
/// `reasoning` as the `thinking` object verbatim. GH #1300: "off" takes the
/// form the model accepts ([`thinking_off`]), and a chat-wire `reasoning`
/// object without a `type` (`{"enabled": false}`, `{"effort": …}`) is read
/// as the off or effort it means.
fn place_sampling(body: &mut Map<String, Value>, params: &LlmParams) {
    for (name, value) in sampling_fields(params) {
        if !takes_param(params, name) {
            continue;
        }
        if name == "reasoning"
            && value.get("type").is_none()
            && value.get("enabled") != Some(&Value::Bool(false))
            && value.get("effort").is_none()
        {
            // GH #1300 review: a chat-wire object that is neither an off nor
            // an effort (`{"enabled": true}`, `{"exclude": true}`, `{"max_tokens":
            // N}`) has no Messages form; adaptive thinking is the field left out.
            continue;
        }
        let (key, value) = match name {
            "reasoning_effort" if value == "none" => thinking_off(&params.model),
            "reasoning_effort" => ("output_config", json!({"effort": value})),
            "thinking_token_budget" => (
                "thinking",
                json!({"type": "enabled", "budget_tokens": value}),
            ),
            "reasoning" if value.get("type").is_none() => {
                if value.get("enabled") == Some(&Value::Bool(false)) {
                    thinking_off(&params.model)
                } else {
                    ("output_config", json!({"effort": value["effort"]}))
                }
            }
            "reasoning" => ("thinking", value),
            other => (other, value),
        };
        body.insert(key.into(), value);
    }
}

/// GH #1300: thinking switched off, in the form the model takes. Opus 5.5,
/// Fable 5 / 5.1 and Mythos 5.1 think always and refuse any explicit
/// `disabled` with a 400, so the nearest they have is the lowest effort;
/// Sonnet 5.5 refuses `disabled` and turns thinking off with `between_tools`;
/// the older models take `disabled`.
fn thinking_off(model: &str) -> (&'static str, Value) {
    if model.contains("opus-5-5") || model.contains("fable") || model.contains("mythos") {
        ("output_config", json!({"effort": "low"}))
    } else if model.contains("sonnet-5-5") {
        ("thinking", json!({"type": "between_tools"}))
    } else {
        ("thinking", json!({"type": "disabled"}))
    }
}

/// GH #1300: whether a built `thinking` object switches thinking off --
/// `disabled`, or Sonnet 5.5's `between_tools` ([`thinking_off`]). A provider
/// switch reads it to keep an instance's off across the switch.
pub(crate) fn thinking_is_off(thinking: &Value) -> bool {
    matches!(
        thinking.get("type").and_then(Value::as_str),
        Some("disabled" | "between_tools")
    )
}

/// GH #1273 (R-AW-3): an open tool round without its signed thinking came
/// from another model, and the Messages wire refuses thinking beside it, so a
/// `thinking` that is on leaves the request; one that is off
/// ([`thinking_is_off`], GH #1300) stays, or Sonnet 5.5 would run this call
/// adaptive at its own default although the instance set `none`. `true` when
/// it removed one.
pub(crate) fn skip_thinking_after_switch(request: &mut Value) -> bool {
    let on = request.get("thinking").is_some_and(|t| !thinking_is_off(t));
    if on && let Some(obj) = request.as_object_mut() {
        obj.remove("thinking");
    }
    on
}

/// T10-06 (GH #890 on this wire): `implicit` is the top-level mark, the
/// provider places it itself; `breakpoints` marks the end of `system` and the
/// end of the stable history -- the block right before the youngest user
/// text, in wire order, a `tool_result` of the same wire message included
/// (OR-programm-tm.AN2.5) and never a thinking block, which takes no mark.
/// Without history before that text only `system` is marked; `off` writes
/// nothing. `cache_ttl_s` >= 3600 asks for the one-hour mark.
fn apply_cache_marks(body: &mut Map<String, Value>, params: &LlmParams) {
    let mark = cache_control(params.cache_ttl_s);
    match params.cache_mode {
        CacheMode::Off => {}
        CacheMode::Implicit => {
            body.entry("cache_control").or_insert(mark);
        }
        CacheMode::Breakpoints => {
            let system = body
                .get("system")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(text) = system {
                let block = json!([{"type": "text", "text": text, "cache_control": mark.clone()}]);
                body.insert("system".into(), block);
            }
            if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
                mark_stable_history(messages, mark);
            }
        }
    }
}

fn mark_stable_history(messages: &mut [Value], mark: Value) {
    let block_type = |m: &Value, b: usize| {
        m.get("content")
            .and_then(|c| c.get(b))
            .and_then(|block| block.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (m, message) in messages.iter().enumerate() {
        let len = message
            .get("content")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        positions.extend((0..len).map(|b| (m, b)));
    }
    let Some(youngest) = positions.iter().rposition(|&(m, b)| {
        messages[m]["role"] == "user" && block_type(&messages[m], b).as_deref() == Some("text")
    }) else {
        return;
    };
    let target = positions[..youngest].iter().rev().find(|&&(m, b)| {
        !matches!(
            block_type(&messages[m], b).as_deref(),
            Some("thinking" | "redacted_thinking")
        )
    });
    if let Some(&(m, b)) = target
        && let Some(block) = messages
            .get_mut(m)
            .and_then(|msg| msg.get_mut("content"))
            .and_then(|c| c.get_mut(b))
            .and_then(Value::as_object_mut)
    {
        block.insert("cache_control".into(), mark);
    }
}

/// T10-10: the `tool_use` ids of the open tool round -- from the end of the
/// history, past the `tool_result` turns, the assistant turns of the answer
/// that asked for them (its tool calls and the text beside them,
/// OR-programm-tm.AN2.9). Empty when the history does not end in a round.
pub(crate) fn open_round_ids(input_messages: &[Value]) -> Vec<String> {
    let field = |t: &Value, k: &str| t.get(k).and_then(Value::as_str).map(str::to_string);
    let mut ids: Vec<String> = input_messages
        .iter()
        .rev()
        .skip_while(|t| field(t, "type").as_deref() == Some("tool_result"))
        .take_while(|t| field(t, "origin").as_deref() == Some("assistant"))
        .filter(|t| field(t, "type").as_deref() == Some("tool_call"))
        .filter_map(|t| field(t, "id"))
        .collect();
    ids.reverse();
    ids
}

/// T10-10: what a tool turn's answer must carry back -- its `thinking` and
/// `redacted_thinking` blocks verbatim, signature included, keyed by its
/// `tool_use` ids. The provider checks the signature and refuses a tool
/// round whose thinking is missing or altered; the UBF turn has no room for
/// it, so the cell keeps it. `None` for any other answer.
pub(crate) fn thinking_of_tool_turn(json: &Value) -> Option<(Vec<String>, Value)> {
    if json.get("stop_reason").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }
    let blocks = json.get("content").and_then(Value::as_array)?;
    let of_type = |b: &Value, t: &[&str]| {
        b.get("type")
            .and_then(Value::as_str)
            .is_some_and(|x| t.contains(&x))
    };
    let thinking: Vec<Value> = blocks
        .iter()
        .filter(|b| of_type(b, &["thinking", "redacted_thinking"]))
        .cloned()
        .collect();
    let ids: Vec<String> = blocks
        .iter()
        .filter(|b| of_type(b, &["tool_use"]))
        .filter_map(|b| b.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    (!thinking.is_empty() && !ids.is_empty()).then_some((ids, Value::Array(thinking)))
}

/// T10-10: put the open round's thinking back in front of its `tool_use`
/// blocks -- index 0 of the LAST assistant message, the one the round's
/// results answer. A closed round carries none.
pub(crate) fn replay_thinking(request: &mut Value, blocks: Value) {
    let Value::Array(blocks) = blocks else {
        return;
    };
    let last = request
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .and_then(|m| m.iter_mut().rev().find(|msg| msg["role"] == "assistant"))
        .and_then(|msg| msg.get_mut("content"))
        .and_then(Value::as_array_mut);
    if let Some(content) = last {
        let rest = std::mem::take(content);
        content.extend(blocks);
        content.extend(rest);
    }
}

/// GH #1298: the `anthropic-beta` value that admits `thinking.block_binding`.
pub(crate) const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// GH #1298: a replayed tool round asks the provider to DROP a thinking block
/// whose prefix changed instead of refusing the request. The provider binds
/// a signed thinking block to the `system`, the `tools` and every message
/// before it; a replay under a changed prefix was answered 400 "Invalid
/// signature in thinking block ... The system prompt differs" (production,
/// talky under a native brain), and the turn died after its interim sentence.
/// Set only beside adaptive thinking (an absent `thinking` is adaptive on the
/// models that sign blocks; on the 5.5 models `enabled`, `disabled` and
/// `between_tools` refuse the field), and the call then sends [`THINKING_BINDING_BETA`]
/// ([`binds_thinking`]). Returns whether the field was set.
pub(crate) fn bind_replayed_thinking(request: &mut Value) -> bool {
    let Some(body) = request.as_object_mut() else {
        return false;
    };
    let thinking = body
        .entry("thinking")
        .or_insert_with(|| json!({"type": "adaptive"}));
    if thinking.get("type").and_then(Value::as_str) != Some("adaptive") {
        return false;
    }
    let Some(obj) = thinking.as_object_mut() else {
        return false;
    };
    obj.insert(
        "block_binding".into(),
        json!({"prefix_mismatch_behavior": "drop_block"}),
    );
    true
}

/// GH #1298: whether the request carries `thinking.block_binding`, which the
/// provider admits only under [`THINKING_BINDING_BETA`].
pub(crate) fn binds_thinking(request: &Value) -> bool {
    request
        .get("thinking")
        .and_then(|t| t.get("block_binding"))
        .is_some()
}

/// The Messages wire refuses an empty text block anywhere in `messages[]`
/// with 400 "text content blocks must be non-empty", old turns included, and
/// the chat parse keeps `content: ""` next to `tool_calls` as a text turn
/// (re-check I1 of GH #1239), so a history moving over from a chat model
/// carries one. Such a block is dropped: an assistant turn of `tool_use`
/// blocks keeps those, any other turn disappears, and the neighbours of the
/// same role merge like any consecutive turns, so the roles still alternate.
fn is_blank_text(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|t| t.trim().is_empty())
}

/// One UBF turn on the Messages wire: a content block under a role, or text
/// for the top-level `system`.
enum Mapped {
    System(String),
    Block(&'static str, Value),
}

fn map_turn(turn: &Value) -> Result<Mapped, TranslateError> {
    let field = |k: &str| turn.get(k).and_then(Value::as_str).unwrap_or("");
    let (origin, type_, text, id) = (field("origin"), field("type"), field("text"), field("id"));
    let text_block = |t: &str| json!({"type": "text", "text": t});
    Ok(match (origin, type_) {
        ("user", "text") => Mapped::Block("user", text_block(text)),
        ("peer", "text") => Mapped::Block("user", text_block(&peer_content(turn))),
        ("assistant", "text") => Mapped::Block("assistant", text_block(text)),
        ("system", "text") => Mapped::System(text.to_string()),
        ("assistant", "tool_call") => Mapped::Block("assistant", tool_use_block(id, text)?),
        ("tool", "tool_result") => Mapped::Block(
            "user",
            json!({"type": "tool_result", "tool_use_id": id, "content": text}),
        ),
        (_, "image") | (_, "audio") => {
            return Err(TranslateError::TypeUnsupported(type_.to_string()));
        }
        _ => {
            return Err(TranslateError::TypeUnsupported(format!(
                "origin={origin}, type={type_}"
            )));
        }
    })
}

/// A UBF `tool_call` text is the chat wire's function object `{"name",
/// "arguments": "<JSON string>"}`; the Messages wire wants the arguments as
/// the object `input`. Empty arguments are `{}`, as on the chat wire's way in
/// (GH #999 F1).
fn tool_use_block(id: &str, text: &str) -> Result<Value, TranslateError> {
    let parse = |e: String| TranslateError::ToolCallParse(format!("id={id}: {e}"));
    let function: Value =
        meclaw_core::serde_json::from_str(text).map_err(|e| parse(e.to_string()))?;
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let input = match function.get("arguments") {
        None => json!({}),
        Some(Value::String(a)) if a.trim().is_empty() => json!({}),
        Some(Value::String(a)) => {
            meclaw_core::serde_json::from_str(a).map_err(|e| parse(e.to_string()))?
        }
        Some(_) => return Err(parse("arguments is not a JSON string".into())),
    };
    Ok(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
}

/// A `system.tools` leaf is the OpenAI tool object (`{"type": "function",
/// "function": {name, description, parameters}}`, or the flat form); the
/// Messages wire wants `{name, description, input_schema}`.
fn to_messages_tool(tool: &Value) -> Value {
    let f = tool.get("function").unwrap_or(tool);
    let mut out = Map::new();
    for (from, to) in [
        ("name", "name"),
        ("description", "description"),
        ("parameters", "input_schema"),
    ] {
        if let Some(v) = f.get(from) {
            out.insert(to.into(), v.clone());
        }
    }
    Value::Object(out)
}

/// Translate a Messages answer into the cell's response shape (T10-07).
///
/// `stop_reason`: `end_turn`/`stop_sequence` → `stop`,
/// `max_tokens`/`model_context_window_exceeded` → `length`, `tool_use` →
/// `tool_calls`, `refusal` → `content_filter`; anything else (`pause_turn`
/// only comes with server tools, which the cell never sends) or none is
/// `UnknownFinishReason`, never a silent `stop`. Tool calls come first, then
/// the text blocks joined into one turn, as on the chat wire; thinking blocks
/// are not read into a turn. `tokens_prompt` is the whole prompt:
/// `input_tokens` counts only what follows the last cache breakpoint, so the
/// cache writes and reads are added to it.
///
/// `silence_ok` is the caller's allowed silence (`hop.silence_ok == "1"`,
/// GH #1320 K-13): an answer with neither text nor `tool_use` is then one
/// empty text turn instead of `EmptyAnswer`, as on the chat wire
/// ([`crate::llm::translate::parse_openai_response_with`]). A budget spent on
/// thinking stays `ReasoningExhausted`; without the knob nothing changes
/// (#999 F2).
pub(crate) fn parse_messages_response_with(
    json: &Value,
    silence_ok: bool,
) -> Result<TranslatedResponse, TranslateError> {
    let finish_reason = match json.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn" | "stop_sequence") => "stop",
        Some("max_tokens" | "model_context_window_exceeded") => "length",
        Some("tool_use") => "tool_calls",
        Some("refusal") => "content_filter",
        Some(other) => return Err(TranslateError::UnknownFinishReason(other.to_string())),
        None => return Err(TranslateError::UnknownFinishReason("null".to_string())),
    }
    .to_string();
    let blocks = json
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| TranslateError::ResponseShape("missing content[]".to_string()))?;
    let mut assistant_turn: Vec<Value> = Vec::new();
    let mut text = String::new();
    let mut thinking = false;
    for (i, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(block.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("tool_use") => assistant_turn.push(tool_call_turn(i, block)?),
            Some("thinking" | "redacted_thinking") => thinking = true,
            _ => {}
        }
    }
    let silent = assistant_turn.is_empty() && text.trim().is_empty();
    if silent {
        let exhausted = finish_reason == "length" && thinking;
        if exhausted || !silence_ok {
            return Err(if exhausted {
                TranslateError::ReasoningExhausted
            } else {
                TranslateError::EmptyAnswer
            });
        }
        // GH #1320 K-13: allowed silence is one empty text turn.
        assistant_turn.push(json!({"origin": "assistant", "type": "text", "text": ""}));
    }
    // A blank text next to `tool_use` stays out of the history: the next
    // request would carry it as an empty text block (re-check I1).
    if !text.trim().is_empty() {
        assistant_turn.push(json!({"origin": "assistant", "type": "text", "text": text}));
    }
    let required = |k: &str| {
        json.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| TranslateError::ResponseShape(format!("missing response.{k}")))
    };
    let (model, response_id) = (required("model")?, required("id")?);
    let usage = json.get("usage");
    let count = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_u64);
    let tokens_prompt = [
        count("input_tokens"),
        count("cache_creation_input_tokens"),
        count("cache_read_input_tokens"),
    ]
    .into_iter()
    .flatten()
    .reduce(u64::saturating_add);
    Ok(TranslatedResponse {
        assistant_turn,
        finish_reason,
        tokens_prompt,
        tokens_completion: count("output_tokens"),
        tokens_cached: usage_tokens_cached(usage),
        tokens_cache_write: usage_tokens_cache_write(usage),
        cost: usage_cost(usage),
        model,
        response_id,
        thinking_dropped: thinking_dropped_of(json),
    })
}

/// GH #1298 (review m4 of F2): the replayed thinking blocks the provider
/// dropped, out of the top-level `input_transformations` of an answer under
/// the binding beta (`[{"type": "thinking_dropped", "path", "reason"}]`,
/// reason `prefix_binding_mismatch` or `model_binding_mismatch`). Entries of
/// any other type are not drops and are passed over, as the provider asks of
/// types a client does not know. Empty without the field.
pub(crate) fn thinking_dropped_of(json: &Value) -> Vec<Value> {
    json.get("input_transformations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("thinking_dropped"))
        .map(|e| {
            json!({
                "path": e.get("path").cloned().unwrap_or(Value::Null),
                "reason": e.get("reason").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

/// A `tool_use` block as the UBF `tool_call` turn the chat wire yields: its
/// text is `{"name", "arguments": "<JSON string>"}`. Checked before it is
/// believed (GH #999 F1): id and name non-empty, `input` an object.
fn tool_call_turn(i: usize, block: &Value) -> Result<Value, TranslateError> {
    let shape = |what: &str| TranslateError::ToolCallShape(format!("content[{i}]: {what}"));
    let non_empty = |k: &str| {
        block
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    };
    let id = non_empty("id").ok_or_else(|| shape("tool_use without id"))?;
    let name = non_empty("name").ok_or_else(|| shape("tool_use without name"))?;
    let input = block
        .get("input")
        .filter(|v| v.is_object())
        .ok_or_else(|| shape("tool_use input is not an object"))?;
    let function = json!({"name": name, "arguments": input.to_string()});
    Ok(json!({
        "origin": "assistant",
        "type": "tool_call",
        "id": id,
        "text": function.to_string(),
    }))
}

/// The face without the knob, for the unit tests of this file. The cell
/// reads `hop.silence_ok` and calls [`parse_messages_response_with`] on every
/// turn (`run_messages_lane`), so outside the tests this face had no caller
/// any more (clippy `dead_code`, gate c3dd2cb, GH #1320).
#[cfg(test)]
pub(crate) fn parse_messages_response(json: &Value) -> Result<TranslatedResponse, TranslateError> {
    parse_messages_response_with(json, false)
}

#[cfg(test)]
mod tests {
    use super::{
        bind_replayed_thinking, binds_thinking, build_messages_request, parse_messages_response,
        require_tool_call, skip_thinking_after_switch, thinking_is_off,
    };
    use crate::llm::params::LlmParams;
    use crate::llm::translate::{TranslateError, parse_openai_response};
    use meclaw_core::serde_json::{Value, json};

    fn tool_call(id: &str, name: &str, arguments: &str) -> Value {
        let text = json!({"name": name, "arguments": arguments}).to_string();
        json!({"origin": "assistant", "type": "tool_call", "id": id, "text": text})
    }

    /// GH #1260 (OR-WG-6): `require` on the Messages wire is `{type: any}`,
    /// over a static overlay.
    #[test]
    fn a_required_call_says_any_and_beats_provider_extra() {
        let params = LlmParams::parse(&json!({
            "provider": "anthropic", "model": "claude-haiku-5-5", "api_key": "k",
            "provider_extra": {"tool_choice": {"type": "auto"}}
        }))
        .expect("params");
        let tools = [json!({"type": "function", "function": {
            "name": "web", "description": "d", "parameters": {"type": "object"}}})];
        let history = [json!({"origin": "user", "type": "text", "text": "hi"})];
        let mut body =
            build_messages_request(&params, "", &history, &tools, Vec::new()).expect("request");
        assert_eq!(body["tool_choice"], json!({"type": "auto"}));
        assert!(require_tool_call(&mut body));
        assert_eq!(body["tool_choice"], json!({"type": "any"}));
    }

    /// GH #1298: the drop request rides beside adaptive thinking only -- an
    /// absent `thinking` becomes adaptive, `enabled`/`disabled` stay as they
    /// are and carry no binding (the provider refuses the field there).
    #[test]
    fn the_drop_request_rides_beside_adaptive_thinking_only() {
        let bound = json!({"type": "adaptive",
                           "block_binding": {"prefix_mismatch_behavior": "drop_block"}});
        let mut body = json!({"model": "m"});
        assert!(bind_replayed_thinking(&mut body) && binds_thinking(&body));
        assert_eq!(body["thinking"], bound);
        let mut body = json!({"thinking": {"type": "adaptive", "display": "summarized"}});
        assert!(bind_replayed_thinking(&mut body));
        assert_eq!(body["thinking"]["display"], "summarized");
        for other in [
            json!({"type": "disabled"}),
            json!({"type": "enabled", "budget_tokens": 1024}),
        ] {
            let mut body = json!({"thinking": other.clone()});
            assert!(!bind_replayed_thinking(&mut body) && !binds_thinking(&body));
            assert_eq!(body["thinking"], other);
        }
    }

    /// Fix round 1 (review M-3): beside thinking the requirement is not
    /// written -- the provider refuses both together -- and the call says so.
    #[test]
    fn require_stays_off_beside_thinking() {
        let mut body = json!({"thinking": {"type": "enabled", "budget_tokens": 1024},
                              "tool_choice": {"type": "auto"}});
        assert!(!require_tool_call(&mut body));
        assert_eq!(body["tool_choice"], json!({"type": "auto"}));
        let mut body = json!({"thinking": {"type": "adaptive"}});
        assert!(!require_tool_call(&mut body));
        assert!(body.get("tool_choice").is_none());
        let mut body = json!({"thinking": {"type": "disabled"}});
        assert!(require_tool_call(&mut body));
        assert_eq!(body["tool_choice"], json!({"type": "any"}));
    }

    #[test]
    fn the_request_lifts_system_merges_tool_rounds_and_maps_tools() {
        let params = LlmParams::parse(&json!({
            "provider": "anthropic", "model": "claude-haiku-5-5", "api_key": "k",
            "max_tokens": 512, "provider_extra": {"metadata": {"user_id": "u"}}
        }))
        .expect("params");
        let history = [
            json!({"origin": "user", "type": "text", "text": "hi"}),
            tool_call("c1", "get", "{\"q\":1}"),
            tool_call("c2", "put", ""),
            json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "r1"}),
            json!({"origin": "tool", "type": "tool_result", "id": "c2", "text": "r2"}),
            json!({"origin": "user", "type": "text", "text": "next"}),
        ];
        let tools = [json!({"type": "function", "function": {
            "name": "get", "description": "d", "parameters": {"type": "object"}}})];
        let body =
            build_messages_request(&params, "S", &history, &tools, Vec::new()).expect("request");
        assert_eq!(
            body,
            json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 512,
                "system": "S",
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                    {"role": "assistant", "content": [
                        {"type": "tool_use", "id": "c1", "name": "get", "input": {"q": 1}},
                        {"type": "tool_use", "id": "c2", "name": "put", "input": {}}
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "c1", "content": "r1"},
                        {"type": "tool_result", "tool_use_id": "c2", "content": "r2"},
                        {"type": "text", "text": "next"}
                    ]}
                ],
                "tools": [{"name": "get", "description": "d", "input_schema": {"type": "object"}}],
                "metadata": {"user_id": "u"}
            })
        );
    }

    /// Re-check I1: the Messages wire refuses an empty text block anywhere in
    /// `messages[]` with 400 ("text content blocks must be non-empty"), old
    /// turns included. A chat answer with `content: ""` next to `tool_calls`
    /// (OpenRouter, vLLM) leaves an empty text turn in the history; a
    /// conversation moving to an anthropic cell carries it, and a blank text
    /// next to `tool_use` in this wire's own answer must not leave one either.
    #[test]
    fn blank_text_never_reaches_the_messages_wire() {
        let params = LlmParams::parse(&json!({
            "provider": "anthropic", "model": "claude-haiku-5-5", "api_key": "k",
            "max_tokens": 512
        }))
        .expect("params");
        let chat = json!({
            "id": "r", "model": "m",
            "choices": [{"finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": "",
                "tool_calls": [{"id": "c1", "type": "function",
                                "function": {"name": "get", "arguments": "{}"}}]}}]
        });
        let chat_turn = parse_openai_response(&chat)
            .expect("chat answer")
            .assistant_turn;
        assert_eq!(
            chat_turn.len(),
            2,
            "the chat parse keeps the empty text: {chat_turn:?}"
        );
        let mut history = vec![
            json!({"origin": "system", "type": "text", "text": "  "}),
            json!({"origin": "user", "type": "text", "text": "q"}),
        ];
        history.extend(chat_turn);
        history.extend([
            json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "r1"}),
            json!({"origin": "assistant", "type": "text", "text": "  "}),
            json!({"origin": "user", "type": "text", "text": "next"}),
            json!({"origin": "assistant", "type": "text", "text": ""}),
        ]);
        let body = build_messages_request(&params, "", &history, &[], Vec::new()).expect("request");
        assert_eq!(
            body,
            json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 512,
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "q"}]},
                    {"role": "assistant", "content": [
                        {"type": "tool_use", "id": "c1", "name": "get", "input": {}}
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "c1", "content": "r1"},
                        {"type": "text", "text": "next"}
                    ]}
                ]
            })
        );

        let answer = json!({
            "id": "m", "model": "c", "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "\n\n"},
                {"type": "tool_use", "id": "tu1", "name": "get", "input": {}}
            ]
        });
        let r = parse_messages_response(&answer).expect("answer");
        assert_eq!(r.assistant_turn.len(), 1, "{:?}", r.assistant_turn);
        assert_eq!(r.assistant_turn[0]["type"], "tool_call");
    }

    fn anthropic(extra: Value) -> LlmParams {
        let mut raw = json!({
            "provider": "anthropic", "model": "claude-haiku-5-5", "api_key": "k",
            "max_tokens": 512
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            raw[k] = v;
        }
        LlmParams::parse(&raw).expect("params")
    }

    /// GH #1300: "off" in the form each 5.5 model accepts. Opus 5.5 cannot
    /// switch thinking off (`disabled` is a 400 at every effort), so its off is
    /// the lowest effort; Sonnet 5.5 refuses `disabled` and takes
    /// `between_tools`; every other model keeps `disabled`. A chat-wire
    /// `reasoning` object a registry push leaves on the cell (`{"enabled":
    /// false}`, `{"effort": …}`) is read in the same terms instead of being
    /// sent as a `thinking` object the Messages wire answers with a 400.
    #[test]
    fn off_and_chat_reasoning_objects_take_the_form_each_model_accepts() {
        let q = [json!({"origin": "user", "type": "text", "text": "q"})];
        let body = |extra: Value| {
            build_messages_request(&anthropic(extra), "", &q, &[], Vec::new()).expect("request")
        };
        for off in [
            json!({"reasoning_effort": "none"}),
            json!({"reasoning": {"enabled": false}}),
        ] {
            let mut sonnet = off.clone();
            sonnet["model"] = json!("claude-sonnet-5-5");
            let b = body(sonnet);
            assert_eq!(b["thinking"], json!({"type": "between_tools"}), "{b}");
            assert!(b.get("output_config").is_none(), "{b}");

            let mut opus = off.clone();
            opus["model"] = json!("claude-opus-5-5");
            let b = body(opus);
            assert_eq!(b["output_config"], json!({"effort": "low"}), "{b}");
            assert!(b.get("thinking").is_none(), "{b}");

            for always_on in ["claude-fable-5", "claude-fable-5-1", "claude-mythos-5-1"] {
                let mut m = off.clone();
                m["model"] = json!(always_on);
                let b = body(m);
                assert_eq!(
                    b["output_config"],
                    json!({"effort": "low"}),
                    "{always_on}: {b}"
                );
                assert!(b.get("thinking").is_none(), "{always_on}: {b}");
            }

            let b = body(off);
            assert_eq!(b["thinking"], json!({"type": "disabled"}), "{b}");
        }
        for neither in [
            json!({"enabled": true}),
            json!({"exclude": true}),
            json!({"max_tokens": 2048}),
        ] {
            let b = body(json!({"reasoning": neither, "model": "claude-sonnet-5-5"}));
            assert!(
                b.get("thinking").is_none() && b.get("output_config").is_none(),
                "{neither} has no Messages form, so it is left out: {b}"
            );
        }
        let b = body(json!({"reasoning": {"effort": "high", "max_tokens": 2048}}));
        assert_eq!(b["output_config"], json!({"effort": "high"}), "{b}");
        assert!(b.get("thinking").is_none(), "{b}");
        for (t, off) in [
            ("disabled", true),
            ("between_tools", true),
            ("adaptive", false),
            ("enabled", false),
        ] {
            assert_eq!(thinking_is_off(&json!({"type": t})), off, "{t}");
            let mut r = json!({"thinking": {"type": t}});
            assert_eq!(skip_thinking_after_switch(&mut r), !off, "{t}");
            assert_eq!(r.get("thinking").is_some(), off, "after a switch, {t}: {r}");
        }
        let b = body(json!({"reasoning_effort": "none", "model": "claude-sonnet-5-5"}));
        let mut r = b.clone();
        assert!(
            !skip_thinking_after_switch(&mut r)
                && r["thinking"] == json!({"type": "between_tools"}),
            "an instance's `none` on Sonnet 5.5 survives a provider switch: {r}"
        );
        let b = body(json!({"reasoning": {"effort": "medium"}}));
        assert_eq!(b["output_config"], json!({"effort": "medium"}), "{b}");
        assert!(b.get("thinking").is_none(), "{b}");
        let b = body(json!({"reasoning_effort": "medium", "model": "claude-sonnet-5-5"}));
        assert_eq!(b["output_config"], json!({"effort": "medium"}), "{b}");
        assert!(b.get("thinking").is_none(), "{b}");
    }

    /// T10-05 (GH #1241): `reasoning_effort` becomes `output_config.effort`
    /// (`none` = thinking disabled), `thinking_budget` the `budget_tokens` of
    /// an older model, `reasoning` the `thinking` object verbatim (and wins);
    /// `temperature` goes out only when the model's list names it, and the
    /// list gates every field by its sampling name.
    #[test]
    fn effort_thinking_and_sampling_follow_the_supported_list() {
        let q = [json!({"origin": "user", "type": "text", "text": "q"})];
        let body = |extra: Value| {
            build_messages_request(&anthropic(extra), "", &q, &[], Vec::new()).expect("request")
        };
        let b = body(json!({"reasoning_effort": "max"}));
        assert_eq!(b["output_config"], json!({"effort": "max"}), "{b}");
        assert!(
            b.get("thinking").is_none() && b.get("temperature").is_none(),
            "{b}"
        );
        let b = body(json!({"reasoning_effort": "none"}));
        assert_eq!(b["thinking"], json!({"type": "disabled"}), "{b}");
        assert!(b.get("output_config").is_none(), "{b}");
        let b = body(json!({"thinking_budget": 2048}));
        assert_eq!(
            b["thinking"],
            json!({"type": "enabled", "budget_tokens": 2048}),
            "{b}"
        );
        let b = body(json!({"reasoning": {"type": "adaptive"}, "thinking_budget": 2048}));
        assert_eq!(b["thinking"], json!({"type": "adaptive"}), "{b}");

        let listed = json!({"temperature": 0.3, "reasoning_effort": "low",
                            "supported_params": ["temperature"]});
        let b = body(listed.clone());
        assert_eq!(b["temperature"], json!(0.3), "{b}");
        assert!(b.get("output_config").is_none(), "{b}");
        assert_eq!(
            crate::llm::translate::dropped_params(&anthropic(listed)),
            vec!["reasoning_effort".to_string()]
        );
        let b = body(json!({"temperature": 0.3, "reasoning_effort": "low",
                            "supported_params": ["reasoning_effort"]}));
        assert!(b.get("temperature").is_none(), "{b}");
        assert_eq!(b["output_config"], json!({"effort": "low"}), "{b}");
    }

    /// T10-06 (GH #1241): `breakpoints` marks the end of `system` and the end
    /// of the stable history -- the block right before the youngest user
    /// turn, a tool result of the same wire message included; `cache_ttl_s`
    /// 3600 asks for the one-hour mark. `implicit` is the top-level mark,
    /// `off` the request of before.
    #[test]
    fn cache_marks_close_the_system_and_the_stable_history() {
        let history = [
            json!({"origin": "user", "type": "text", "text": "old"}),
            tool_call("c1", "get", ""),
            json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "r1"}),
            json!({"origin": "user", "type": "text", "text": "new"}),
        ];
        let mode = |m: &str, ttl: u64| anthropic(json!({"cache_mode": m, "cache_ttl_s": ttl}));
        let hour = json!({"type": "ephemeral", "ttl": "1h"});
        let b = build_messages_request(&mode("breakpoints", 3600), "S", &history, &[], Vec::new())
            .expect("request");
        assert_eq!(
            b["system"],
            json!([{"type": "text", "text": "S", "cache_control": hour}])
        );
        assert_eq!(
            b["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "old"}]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "c1", "name": "get", "input": {}}]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "c1", "content": "r1",
                     "cache_control": hour},
                    {"type": "text", "text": "new"}
                ]}
            ])
        );
        assert!(b.get("cache_control").is_none(), "{b}");

        let b = build_messages_request(
            &mode("breakpoints", 300),
            "S",
            &history[3..],
            &[],
            Vec::new(),
        )
        .expect("request");
        assert_eq!(
            b["system"],
            json!([{"type": "text", "text": "S", "cache_control": {"type": "ephemeral"}}])
        );
        assert_eq!(
            b["messages"],
            json!([{"role": "user", "content": [{"type": "text", "text": "new"}]}]),
            "no history before the youngest user turn, no second mark"
        );

        let b = build_messages_request(&mode("implicit", 300), "S", &history, &[], Vec::new())
            .expect("request");
        assert_eq!(b["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(b["system"], "S");
        let b = build_messages_request(&mode("off", 0), "S", &history, &[], Vec::new())
            .expect("request");
        assert!(
            !b.to_string().contains("cache_control"),
            "off sends no mark: {b}"
        );
    }

    #[test]
    fn the_answer_becomes_ubf_turns_with_reason_and_cache_usage() {
        let answer = json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": [
                {"type": "thinking", "thinking": "hm", "signature": "s"},
                {"type": "text", "text": "Let me "},
                {"type": "text", "text": "look."},
                {"type": "tool_use", "id": "tu1", "name": "get", "input": {"q": 1}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "cache_creation_input_tokens": 20,
                      "cache_read_input_tokens": 30, "output_tokens": 5}
        });
        let r = parse_messages_response(&answer).expect("answer");
        assert_eq!(r.finish_reason, "tool_calls");
        assert_eq!(r.assistant_turn.len(), 2, "{:?}", r.assistant_turn);
        let call = &r.assistant_turn[0];
        assert_eq!(call["type"], "tool_call");
        assert_eq!(call["id"], "tu1");
        let function: Value =
            meclaw_core::serde_json::from_str(call["text"].as_str().expect("text")).expect("json");
        assert_eq!(function, json!({"name": "get", "arguments": "{\"q\":1}"}));
        assert_eq!(
            r.assistant_turn[1],
            json!({"origin": "assistant", "type": "text", "text": "Let me look."})
        );
        assert_eq!(r.tokens_prompt, Some(60));
        assert_eq!(r.tokens_completion, Some(5));
        assert_eq!(r.tokens_cached, Some(30));
        assert_eq!(r.tokens_cache_write, Some(20));
        assert_eq!(
            (r.model.as_str(), r.response_id.as_str()),
            ("claude-haiku-5-5", "msg_1")
        );

        for (stop, mapped) in [
            ("end_turn", "stop"),
            ("stop_sequence", "stop"),
            ("max_tokens", "length"),
            ("model_context_window_exceeded", "length"),
            ("refusal", "content_filter"),
        ] {
            let a = json!({"id": "m", "model": "c", "stop_reason": stop,
                           "content": [{"type": "text", "text": "x"}]});
            assert_eq!(
                parse_messages_response(&a).expect(stop).finish_reason,
                mapped
            );
        }
        let a = json!({"id": "m", "model": "c", "stop_reason": "pause_turn",
                       "content": [{"type": "text", "text": "x"}]});
        assert!(matches!(
            parse_messages_response(&a),
            Err(TranslateError::UnknownFinishReason(_))
        ));
    }

    /// GH #1320 K-13: `silence_ok` turns an empty answer into one empty text
    /// turn on both wires; without it, and for a budget spent on thinking,
    /// the #999 F2 errors stand.
    #[test]
    fn gh1320_silence_ok_lets_an_empty_answer_through_on_both_wires() {
        use crate::llm::translate::parse_openai_response_with;
        let silent = json!([{"origin": "assistant", "type": "text", "text": ""}]);
        let empty = json!({"id": "m", "model": "c", "stop_reason": "end_turn",
                           "content": [{"type": "text", "text": " "}]});
        assert_eq!(
            parse_messages_response(&empty).err(),
            Some(TranslateError::EmptyAnswer)
        );
        let r = super::parse_messages_response_with(&empty, true).expect("silence");
        assert_eq!(Value::Array(r.assistant_turn), silent);
        assert_eq!(r.finish_reason, "stop");
        let cut = json!({"id": "m", "model": "c", "stop_reason": "max_tokens",
                         "content": [{"type": "thinking", "thinking": "hm"}]});
        assert_eq!(
            super::parse_messages_response_with(&cut, true).err(),
            Some(TranslateError::ReasoningExhausted)
        );
        for content in [json!(null), json!(" ")] {
            let chat = json!({"id": "r", "model": "c", "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": content}}]});
            assert_eq!(
                parse_openai_response(&chat).err(),
                Some(TranslateError::EmptyAnswer)
            );
            let r = parse_openai_response_with(&chat, true).expect("silence");
            assert_eq!(Value::Array(r.assistant_turn), silent, "{chat}");
        }
        // A cut without a thinking trace is no `ReasoningExhausted`: with the
        // knob it is the empty turn, but it keeps `length`, so the cell's
        // length continuation takes it and not the dispatcher's silence
        // (which marks `stop` only). Held on both wires (re-check m2).
        let chat_cut = json!({"id": "r", "model": "c", "choices": [{
            "finish_reason": "length",
            "message": {"role": "assistant", "content": " "}}]});
        let r = parse_openai_response_with(&chat_cut, true).expect("an empty cut");
        assert_eq!(Value::Array(r.assistant_turn), silent);
        assert_eq!(r.finish_reason, "length");
        let messages_cut = json!({"id": "m", "model": "c", "stop_reason": "max_tokens",
                                  "content": [{"type": "text", "text": ""}]});
        let r = super::parse_messages_response_with(&messages_cut, true).expect("an empty cut");
        assert_eq!(Value::Array(r.assistant_turn), silent);
        assert_eq!(r.finish_reason, "length");
    }

    /// The closing context note a trailing assistant text travels in (GH #1282).
    const NOTE: &str = "[Context, not a new request -- your last message in this conversation, already delivered to the person:]";

    fn ends_on_user(body: &Value) -> bool {
        body["messages"]
            .as_array()
            .and_then(|m| m.last())
            .is_some_and(|m| m["role"] == "user")
    }

    /// GH #1282 (R025 finding 1): an escalated tool round reaches the brain
    /// with the quick voice's bridge sentence as the LAST turn. Sent as is it
    /// is a prefill, and current Claude models answer 400 ("This model does
    /// not support assistant message prefill"). The sentence travels as a
    /// closing note in the last user message instead -- its text verbatim,
    /// labelled as the assistant's own, already said.
    #[test]
    fn an_escalated_round_never_ends_on_the_bridge_sentence() {
        let history = [
            json!({"origin": "user", "type": "text", "text": "hello"}),
            json!({"origin": "assistant", "type": "text", "text": "Hi there."}),
            json!({"origin": "user", "type": "text", "text": "Quick question: weather in Athens?"}),
            json!({"origin": "assistant", "type": "text", "text": "One moment, I am looking that up."}),
        ];
        let tools = [json!({"type": "function", "function": {
            "name": "web", "description": "d", "parameters": {"type": "object"}}})];
        let body = build_messages_request(&anthropic(json!({})), "S", &history, &tools, Vec::new())
            .expect("request");
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "hello"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi there."}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "Quick question: weather in Athens?"},
                    {"type": "text", "text": format!("{NOTE}\nOne moment, I am looking that up.")}
                ]}
            ])
        );
    }

    /// GH #1282 (R025 finding 2): the consult window of the reasoning core
    /// ends on the conversation model's previous answer behind the order.
    /// Same rule: never a prefill, the answer reaches the model as context.
    #[test]
    fn a_consult_window_never_ends_on_the_old_answer() {
        let history = [
            json!({"origin": "system", "type": "text", "text": "consult brief"}),
            json!({"origin": "user", "type": "text", "text": "Plan me a thorough 3-day trip to Athens."}),
            json!({"origin": "assistant", "type": "text", "text": "Season 2 arrives in April."}),
        ];
        let body = build_messages_request(&anthropic(json!({})), "S", &history, &[], Vec::new())
            .expect("request");
        assert_eq!(body["system"], "S\n\nconsult brief");
        assert_eq!(
            body["messages"],
            json!([{"role": "user", "content": [
                {"type": "text", "text": "Plan me a thorough 3-day trip to Athens."},
                {"type": "text", "text": format!("{NOTE}\nSeason 2 arrives in April.")}
            ]}])
        );
    }

    /// GH #1282 lock: whatever the window ends with, the request ends on a
    /// user message -- several trailing texts join into one note, and a
    /// window of nothing but an assistant text becomes one user message.
    #[test]
    fn a_messages_request_never_ends_on_assistant() {
        let a = |t: &str| json!({"origin": "assistant", "type": "text", "text": t});
        let u = |t: &str| json!({"origin": "user", "type": "text", "text": t});
        let windows: Vec<Vec<Value>> = vec![
            vec![u("q"), a("one"), a("two")],
            vec![a("only")],
            vec![
                u("q"),
                tool_call("c1", "get", ""),
                json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "r"}),
                a("Found it."),
            ],
            vec![u("q"), a("x"), a("  ")],
        ];
        for w in &windows {
            let body = build_messages_request(&anthropic(json!({})), "", w, &[], Vec::new())
                .expect("request");
            assert!(ends_on_user(&body), "ends on assistant: {body}");
        }
        let body = build_messages_request(&anthropic(json!({})), "", &windows[0], &[], Vec::new())
            .expect("request");
        assert_eq!(
            body["messages"],
            json!([{"role": "user", "content": [
                {"type": "text", "text": "q"},
                {"type": "text", "text": format!("{NOTE}\none\n\ntwo")}
            ]}])
        );
        let body = build_messages_request(&anthropic(json!({})), "", &windows[1], &[], Vec::new())
            .expect("request");
        assert_eq!(
            body["messages"],
            json!([{"role": "user", "content": [
                {"type": "text", "text": format!("{NOTE}\nonly")}
            ]}])
        );
    }

    /// Review PF M-3: in the folded form the history cache mark stays on the
    /// order text before the note -- the stable prefix of the next turn --
    /// and the note itself carries none.
    #[test]
    fn the_cache_mark_stays_on_the_order_before_the_note() {
        let params = anthropic(json!({"cache_mode": "breakpoints", "cache_ttl_s": 300}));
        let mark = json!({"type": "ephemeral"});
        let history = [
            json!({"origin": "user", "type": "text", "text": "hello"}),
            json!({"origin": "assistant", "type": "text", "text": "Hi there."}),
            json!({"origin": "user", "type": "text", "text": "Weather in Athens?"}),
            json!({"origin": "assistant", "type": "text", "text": "One moment."}),
        ];
        let body =
            build_messages_request(&params, "S", &history, &[], Vec::new()).expect("request");
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "hello"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hi there."}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "Weather in Athens?", "cache_control": mark},
                    {"type": "text", "text": format!("{NOTE}\nOne moment.")}
                ]}
            ])
        );
    }

    /// GH #1282: a window that ends on a user turn or a tool result is the
    /// request of before, byte for byte, cache marks included.
    #[test]
    fn a_window_ending_on_user_or_tool_result_stays_byte_identical() {
        let params = anthropic(json!({"cache_mode": "breakpoints", "cache_ttl_s": 300}));
        let mark = json!({"type": "ephemeral"});
        let on_result = [
            json!({"origin": "user", "type": "text", "text": "q"}),
            tool_call("c1", "get", ""),
            json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "r1"}),
        ];
        let body =
            build_messages_request(&params, "S", &on_result, &[], Vec::new()).expect("request");
        assert_eq!(
            body.to_string(),
            json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 512,
                "system": [{"type": "text", "text": "S", "cache_control": mark}],
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "q"}]},
                    {"role": "assistant", "content": [
                        {"type": "tool_use", "id": "c1", "name": "get", "input": {}}]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "c1", "content": "r1"}]}
                ]
            })
            .to_string()
        );
        let on_user = [
            json!({"origin": "user", "type": "text", "text": "a"}),
            json!({"origin": "assistant", "type": "text", "text": "b"}),
            json!({"origin": "user", "type": "text", "text": "c"}),
        ];
        let body =
            build_messages_request(&params, "S", &on_user, &[], Vec::new()).expect("request");
        assert_eq!(
            body.to_string(),
            json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 512,
                "system": [{"type": "text", "text": "S", "cache_control": mark}],
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "a"}]},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "b", "cache_control": mark}]},
                    {"role": "user", "content": [{"type": "text", "text": "c"}]}
                ]
            })
            .to_string()
        );
    }
}
