//! Phase-8 test helper — OpenAI Chat-Completions JSON wrapper around
//! `meclaw_testing::mock_http::start_mock_server_capturing`.
//!
//! T10: canned-response helpers (text-completion, tool-calls, error-status)
//! + `OpenAiRequestSnapshot` parser for captured request bodies.
//!
//! Used by T11 (call_openai unit tests), T20/T21 (LlmCell integration), T27-T31
//! (phase_8_demo). Future integration tests declare:
//! ```ignore
//!     #[path = "mock_openai.rs"]
//!     mod mock_openai;
//!     use mock_openai::{MockOpenAI, canned_chat_completion};
//! ```
//!
//! GH #1283: one validator (`provider_validator`) plays the provider rules of
//! every wire the mock serves (chat, responses, messages, decisions) and
//! answers a refused request with the provider's status and error body.
//!
//! Layering: provider-specific knowledge (OpenAI Chat-Completions JSON shape)
//! lives here in `meclaw-cells/tests/`; the generic TCP listener +
//! request-capture mechanics live in `meclaw-testing/src/mock_http.rs`.

#![allow(dead_code)]

use meclaw_testing::mock_http::{
    CapturedRequest, MockResponse, RequestValidator, start_mock_server_capturing_with_validator,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// MockOpenAI server handle. Lives for the duration of the test; the background
/// listener task is held via the `JoinHandle` field and aborts on drop only
/// when the tokio runtime tears down.
pub struct MockOpenAI {
    /// The bound socket address (`127.0.0.1:<port>`).
    pub addr: std::net::SocketAddr,
    /// Convenience: `format!("http://{addr}")` — usable as `LlmParams::base_url`.
    pub base_url: String,
    /// Captured requests in arrival order. Lock + iterate via `recorded_requests()`.
    pub captured: Arc<Mutex<Vec<CapturedRequest>>>,
    #[allow(dead_code)]
    join: JoinHandle<()>,
}

impl MockOpenAI {
    /// Start a mock provider with a sequence of canned responses (one per
    /// expected incoming request, in order).
    ///
    /// Every request first meets [`provider_validator`] (GH #1283): a request
    /// the real provider refuses is answered with the provider's status and
    /// error body, and a rejection does NOT consume the canned sequence.
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let (addr, join, captured) =
            start_mock_server_capturing_with_validator(responses, Some(provider_validator())).await;
        let base_url = format!("http://{}", addr);
        Self {
            addr,
            base_url,
            captured,
            join,
        }
    }

    /// Snapshot all captured Chat-Completions request bodies, parsed as JSON.
    /// Panics if a captured body is not valid JSON (test contract — the
    /// caller must only point real OpenAI-shaped traffic at this mock).
    pub async fn recorded_requests(&self) -> Vec<OpenAiRequestSnapshot> {
        let caps = self.captured.lock().await;
        caps.iter()
            .map(|c| {
                let body: serde_json::Value =
                    serde_json::from_slice(&c.body).expect("captured body is JSON");
                OpenAiRequestSnapshot {
                    method: c.method.clone(),
                    path: c.path.clone(),
                    headers: c.headers.clone(),
                    body,
                }
            })
            .collect()
    }
}

/// Parsed view of a captured OpenAI Chat-Completions request.
pub struct OpenAiRequestSnapshot {
    /// HTTP method (expected: `"POST"`).
    pub method: String,
    /// Request path (expected: `"/v1/chat/completions"` or similar).
    pub path: String,
    /// Request headers, names lowercased (e.g. `"authorization"`, `"content-type"`).
    pub headers: HashMap<String, String>,
    /// Parsed JSON body — the OpenAI Chat-Completions request payload.
    pub body: serde_json::Value,
}

impl OpenAiRequestSnapshot {
    /// `body.model` as `&str`, if present.
    pub fn model(&self) -> Option<&str> {
        self.body.get("model").and_then(|v| v.as_str())
    }
    /// `body.messages` as `&Vec<Value>`, if present.
    pub fn messages(&self) -> Option<&Vec<serde_json::Value>> {
        self.body.get("messages").and_then(|v| v.as_array())
    }
    /// `body.temperature` as `f64`, if present.
    pub fn temperature(&self) -> Option<f64> {
        self.body.get("temperature").and_then(|v| v.as_f64())
    }
    /// `body.tools` as `&Vec<Value>`, if present.
    pub fn tools(&self) -> Option<&Vec<serde_json::Value>> {
        self.body.get("tools").and_then(|v| v.as_array())
    }
}

// ───── The provider rules (GH #1283) ─────
//
// One validator for every wire the mock serves. Wire rules hold for every
// model; each answers with the status and the body the provider sends. The
// texts below are the measured refusals and the single source for every test
// that plays one (the held binding mock of GH #1298 included).

/// Measured refusal of the chat wire for an assistant `tool_calls` message
/// that is not followed by its `tool` answers (Run-4b, receipt 922d93f).
pub const TOOL_SEQUENCE_REFUSAL: &str = "An assistant message with 'tool_calls' must be \
followed by tool messages responding to each 'tool_call_id'.";

/// Measured refusal of the Messages wire for a request that ends on an
/// assistant turn (GH #1282, current Claude models).
pub const PREFILL_REFUSAL: &str = "This model does not support assistant message prefill. \
The conversation must end with a user message.";

/// The upstream sentence the router carries in `metadata.raw` when it refuses
/// a prefill on the chat wire (GH #1282, R025).
pub const ROUTER_PREFILL_RAW: &str = "This model does not support assistant message prefill.";

/// Measured refusal of the Messages wire when a replayed thinking block was
/// made under another prefix (production, GH #1298).
pub const BINDING_REFUSAL: &str = "messages.1.content.0: Invalid `signature` in `thinking` block. \
The block is bound to a different conversation. Remove the block, or set \
`thinking.block_binding.prefix_mismatch_behavior` to \"drop_block\". That setting requires the \
`thinking-binding-controls-2026-08-01` value in the `anthropic-beta` header. The system prompt \
differs from the one this block was created with.";

/// The wire a request speaks, read from its path (`docs/cell-types.md` § `llm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// `POST …/chat/completions`.
    Chat,
    /// `POST …/responses`.
    Responses,
    /// `POST …/messages` (Anthropic Messages).
    Messages,
    /// `POST …/alpha/decisions`.
    Decisions,
}

impl Wire {
    /// The wire of a request path, `None` for a path no provider serves.
    pub fn of(path: &str) -> Option<Self> {
        let path = path.split('?').next().unwrap_or(path);
        if path.ends_with("/chat/completions") {
            Some(Self::Chat)
        } else if path.ends_with("/responses") {
            Some(Self::Responses)
        } else if path.ends_with("/messages") {
            Some(Self::Messages)
        } else if path.ends_with("/decisions") {
            Some(Self::Decisions)
        } else {
            None
        }
    }
}

/// The validator `MockOpenAI::start` and `MockResponses::start` install.
pub fn provider_validator() -> RequestValidator {
    Arc::new(provider_refusal)
}

/// The provider's answer to a request it refuses, `None` for a request it
/// takes. Decisions stay free: the decider is replayed, not mocked strictly
/// (D65 addendum).
pub fn provider_refusal(req: &CapturedRequest) -> Option<MockResponse> {
    let wire = Wire::of(&req.path)?;
    let body: serde_json::Value = serde_json::from_slice(&req.body).ok()?;
    match wire {
        Wire::Chat => chat_refusal(&body).or_else(|| model_refusal(&body, "chat")),
        Wire::Messages => version_refusal(req)
            .or_else(|| messages_refusal(&body))
            .or_else(|| model_refusal(&body, "messages")),
        Wire::Responses | Wire::Decisions => None,
    }
}

/// Chat wire: R11 (tool-call sequence), R2 (a Claude model's prefill through
/// the router).
fn chat_refusal(body: &serde_json::Value) -> Option<MockResponse> {
    if tool_call_sequence_violation(body).is_some() {
        return Some(openai_error(400, TOOL_SEQUENCE_REFUSAL));
    }
    if ends_on_assistant(body) && names_a_claude_model(body) {
        return Some(router_error(400, ROUTER_PREFILL_RAW));
    }
    None
}

/// R2 is measured for Claude models behind the router (R025); OpenAI's own
/// chat wire takes a trailing assistant turn, so the rule judges only a model
/// id that names a Claude model (`anthropic/…` or `…claude…`).
fn names_a_claude_model(body: &serde_json::Value) -> bool {
    body.get("model").and_then(|v| v.as_str()).is_some_and(|m| {
        let m = m.to_ascii_lowercase();
        m.starts_with("anthropic/") || m.contains("claude")
    })
}

/// Messages wire: R14 (unknown top-level field), R1 (prefill), R13 (text
/// before `tool_result`); R15 (the version header) is judged before, on the
/// headers.
fn messages_refusal(body: &serde_json::Value) -> Option<MockResponse> {
    if let Some(refusal) = unknown_field_refusal(body) {
        return Some(refusal);
    }
    if ends_on_assistant(body) {
        return Some(anthropic_error(400, PREFILL_REFUSAL));
    }
    tool_result_order_refusal(body)
}

/// Whether `messages[]` ends on an assistant turn (a prefill).
fn ends_on_assistant(body: &serde_json::Value) -> bool {
    body.get("messages")
        .and_then(|v| v.as_array())
        .and_then(|m| m.last())
        .and_then(|m| m.get("role"))
        .and_then(|v| v.as_str())
        == Some("assistant")
}

/// For each assistant message with a non-empty `tool_calls[]`, the
/// immediately following consecutive `role: "tool"` messages must answer
/// every `tool_call_id`. Returns a violation description, or `None` if the
/// message sequence is wire-valid (Run-4b: Form A → 400, Form B/C → 200).
fn tool_call_sequence_violation(body: &serde_json::Value) -> Option<String> {
    let msgs = body.get("messages")?.as_array()?;
    for (i, m) in msgs.iter().enumerate() {
        if m.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(calls) = m.get("tool_calls").and_then(|v| v.as_array()) else {
            continue;
        };
        let mut pending: std::collections::HashSet<&str> = calls
            .iter()
            .filter_map(|c| c.get("id").and_then(|v| v.as_str()))
            .collect();
        for follower in &msgs[i + 1..] {
            if pending.is_empty() || follower.get("role").and_then(|v| v.as_str()) != Some("tool") {
                break;
            }
            if let Some(id) = follower.get("tool_call_id").and_then(|v| v.as_str()) {
                pending.remove(id);
            }
        }
        if !pending.is_empty() {
            let mut ids: Vec<&str> = pending.into_iter().collect();
            ids.sort_unstable();
            return Some(format!("messages[{i}]: unanswered tool_call_id(s) {ids:?}"));
        }
    }
    None
}

fn json_response(status: u16, body: serde_json::Value) -> MockResponse {
    MockResponse {
        status,
        body: body.to_string().into_bytes(),
        content_type: "application/json".into(),
        delay: None,
    }
}

/// An error in the OpenAI form (`{"error": {message, type, param, code}}`).
pub fn openai_error(status: u16, message: &str) -> MockResponse {
    let kind = if status >= 500 {
        "server_error"
    } else {
        "invalid_request_error"
    };
    json_response(
        status,
        serde_json::json!({"error": {
            "message": message, "type": kind, "param": null, "code": null}}),
    )
}

/// An error in the router form: the router's sentence, the status as
/// `code`, the upstream's own sentence in `metadata.raw`.
pub fn router_error(status: u16, raw: &str) -> MockResponse {
    json_response(
        status,
        serde_json::json!({"error": {
            "message": "Provider returned error", "code": status,
            "metadata": {"raw": raw}}}),
    )
}

/// An error in the Messages form (`{"type": "error", "error": {type, message}}`).
pub fn anthropic_error(status: u16, message: &str) -> MockResponse {
    let kind = match status {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        s if s >= 500 => "api_error",
        _ => "invalid_request_error",
    };
    json_response(
        status,
        serde_json::json!({"type": "error", "error": {"type": kind, "message": message}}),
    )
}

// ───── Recorded refusals (GH #1283 part B, GH #1609) ─────
//
// The conformance recordings (`tests/fixtures/conformance/<slug>/measured.json`,
// field `refusals`) hold each refusal of a model as the provider sent it:
// rule, form (`param`), wire, status and body verbatim. They are the only
// rule table: a model rule (R3-R10) judges a request only when its model id
// is a recorded `model_id` with a refusal of that form on that wire, and the
// wire rules R13-R15 take their sentence from the first recording that has
// one. Neutral test models (`test-model`, `claude-test-b`, ...) carry the wire
// rules only.

/// One refusal of a conformance recording.
#[derive(Debug, Clone)]
pub struct RecordedRefusal {
    /// The rule the recording files the refusal under (`R3` ... `R15`).
    pub rule: String,
    /// The refused form, as the conformance tool names it (`temperature`,
    /// `reasoning_off`, `unknown_field`, ...).
    pub param: String,
    /// `chat` or `messages`.
    pub wire: String,
    /// The HTTP status the provider answered.
    pub status: u16,
    /// The response body, verbatim.
    pub body: String,
}

impl RecordedRefusal {
    /// The provider's sentence: `error.message` of the body.
    pub fn message(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// The recorded answer, status and body byte for byte.
    pub fn response(&self) -> MockResponse {
        MockResponse {
            status: self.status,
            body: self.body.clone().into_bytes(),
            content_type: "application/json".into(),
            delay: None,
        }
    }
}

/// The recordings folder; valid from `meclaw-cells` and from a crate that
/// mounts this file by `#[path]` (`meclaw-cli`).
fn recordings_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../meclaw-cells/tests/fixtures/conformance")
}

/// `model_id` -> its recorded refusals, read once per test binary, in folder
/// order. A recording without `refusals` adds an empty list.
pub fn recorded_refusals() -> &'static Vec<(String, Vec<RecordedRefusal>)> {
    static RECORDED: std::sync::OnceLock<Vec<(String, Vec<RecordedRefusal>)>> =
        std::sync::OnceLock::new();
    RECORDED.get_or_init(|| {
        let mut dirs: Vec<_> = std::fs::read_dir(recordings_dir())
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        dirs.sort();
        dirs.iter()
            .filter_map(|d| std::fs::read_to_string(d.join("measured.json")).ok())
            .filter_map(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .filter_map(|v| {
                let model = v["model_id"].as_str()?.to_string();
                let refusals = v["refusals"]
                    .as_array()
                    .map(|a| a.iter().filter_map(recorded_refusal).collect())
                    .unwrap_or_default();
                Some((model, refusals))
            })
            .collect()
    })
}

fn recorded_refusal(v: &serde_json::Value) -> Option<RecordedRefusal> {
    Some(RecordedRefusal {
        rule: v["rule"].as_str()?.to_string(),
        param: v["param"].as_str()?.to_string(),
        wire: v["wire"].as_str()?.to_string(),
        status: u16::try_from(v["status"].as_u64()?).ok()?,
        body: v["body"].as_str()?.to_string(),
    })
}

/// The first recorded refusal of `rule` (any model): the sentence of a wire
/// rule. `None` when no recording has one -- the rule is then not played.
pub fn wire_rule_refusal(rule: &str) -> Option<&'static RecordedRefusal> {
    recorded_refusals()
        .iter()
        .flat_map(|(_, r)| r.iter())
        .find(|r| r.rule == rule)
}

/// The refusals recorded for `model` on `wire`; empty for a model without a
/// recording (every neutral test model).
pub fn model_refusals(model: &str, wire: &str) -> Vec<&'static RecordedRefusal> {
    recorded_refusals()
        .iter()
        .filter(|(m, _)| m == model)
        .flat_map(|(_, r)| r.iter())
        .filter(|r| r.wire == wire)
        .collect()
}

/// Whether `body` carries the form a recording names. Forms of the wire rules
/// (R13-R15) are judged by their own checks, never here.
fn carries_form(body: &serde_json::Value, param: &str) -> bool {
    match param {
        "reasoning_off" => body["reasoning"]["enabled"] == serde_json::json!(false),
        "temperature" | "top_p" => body.get(param).is_some_and(|v| !v.is_null()),
        "tool_choice_named" => body["tool_choice"].get("function").is_some(),
        "tool_choice_required" => body["tool_choice"] == "required",
        "tool_choice_any" => body["tool_choice"]["type"] == "any",
        "thinking_enabled" => body["thinking"]["type"] == "enabled",
        "thinking_disabled" => body["thinking"]["type"] == "disabled",
        _ => false,
    }
}

/// Model rules (R3-R10): the recorded answer of the first recorded form the
/// request carries.
fn model_refusal(body: &serde_json::Value, wire: &str) -> Option<MockResponse> {
    let model = body.get("model").and_then(|v| v.as_str())?;
    model_refusals(model, wire)
        .into_iter()
        .find(|r| carries_form(body, &r.param))
        .map(RecordedRefusal::response)
}

/// The tool-use id the conformance tool sends for R13; the recorded sentence
/// names it, the mock names the ids of the request instead.
const RECORDED_TOOL_USE_ID: &str = "toolu_meclaw_1";

/// The unknown field the conformance tool sends for R14; the mock names the
/// field of the request instead.
const RECORDED_UNKNOWN_FIELD: &str = "x_meclaw_unknown";

/// The top-level fields of the Messages API (its published request schema);
/// any other is refused (R14).
const MESSAGES_FIELDS: &[&str] = &[
    "model",
    "messages",
    "max_tokens",
    "system",
    "metadata",
    "stop_sequences",
    "stream",
    "temperature",
    "top_p",
    "top_k",
    "tools",
    "tool_choice",
    "thinking",
    "output_config",
    "service_tier",
    "container",
    "mcp_servers",
    "context_management",
    "cache_control",
];

/// R15: a Messages request without `anthropic-version`.
fn version_refusal(req: &CapturedRequest) -> Option<MockResponse> {
    if req.headers.contains_key("anthropic-version") {
        return None;
    }
    let rec = wire_rule_refusal("R15")?;
    Some(anthropic_error(rec.status, &rec.message()))
}

/// R14: the first top-level field the Messages API does not know.
fn unknown_field_refusal(body: &serde_json::Value) -> Option<MockResponse> {
    let field = body
        .as_object()?
        .keys()
        .find(|k| !MESSAGES_FIELDS.contains(&k.as_str()))?;
    let rec = wire_rule_refusal("R14")?;
    let text = rec.message().replace(RECORDED_UNKNOWN_FIELD, field);
    Some(anthropic_error(rec.status, &text))
}

/// R13: every `tool_use` of an assistant message needs its `tool_result`
/// blocks at the very start of the next message; a text in front of them (or
/// a missing result) is refused with the recorded sentence, its index and
/// ids those of the request.
fn tool_result_order_refusal(body: &serde_json::Value) -> Option<MockResponse> {
    let msgs = body.get("messages")?.as_array()?;
    for (i, m) in msgs.iter().enumerate() {
        if m["role"] != "assistant" {
            continue;
        }
        let ids: Vec<&str> = m["content"]
            .as_array()
            .map(|b| {
                b.iter()
                    .filter(|b| b["type"] == "tool_use")
                    .filter_map(|b| b["id"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let Some(next) = msgs.get(i + 1) else {
            continue;
        };
        if ids.is_empty() {
            continue;
        }
        let leading: Vec<&str> = next["content"]
            .as_array()
            .map(|b| {
                b.iter()
                    .take_while(|b| b["type"] == "tool_result")
                    .filter_map(|b| b["tool_use_id"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let missing: Vec<&str> = ids.into_iter().filter(|id| !leading.contains(id)).collect();
        if missing.is_empty() {
            continue;
        }
        let rec = wire_rule_refusal("R13")?;
        let message = rec.message();
        let rest = message
            .split_once(": ")
            .map_or(message.as_str(), |(_, r)| r);
        let text = format!(
            "messages.{}: {}",
            i + 1,
            rest.replace(RECORDED_TOOL_USE_ID, &missing.join(", "))
        );
        return Some(anthropic_error(rec.status, &text));
    }
    None
}

// ───── Canned-response builders ─────

/// Build a canned OpenAI Chat-Completions success response with the given
/// assistant text `content` and `finish_reason` (`"stop"` / `"length"` / etc.).
pub fn canned_chat_completion(content: &str, finish_reason: &str) -> MockResponse {
    let json = serde_json::json!({
        "id": "chatcmpl-test-001",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {"role": "assistant", "content": content},
            "finish_reason": finish_reason
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Build a canned OpenAI Chat-Completions tool-calls response.
/// `tool_calls` is a Vec of `(id, function_name, arguments_json)` tuples.
pub fn canned_tool_calls(tool_calls: Vec<(&str, &str, &str)>) -> MockResponse {
    let calls: Vec<serde_json::Value> = tool_calls
        .into_iter()
        .map(|(id, name, args)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": args}
            })
        })
        .collect();
    let json = serde_json::json!({
        "id": "chatcmpl-test-tc-001",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": serde_json::Value::Null,
                "tool_calls": calls
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Build a canned response that carries `content` **and** `tool_calls` in the
/// SAME choice -- the shape a model produces when it says "one moment" and asks
/// for a tool in one breath (GH #28). The `llm` cell maps it to one `tool_call`
/// turn per call, followed by the text turn.
pub fn canned_content_and_tool_calls(
    content: &str,
    tool_calls: Vec<(&str, &str, &str)>,
) -> MockResponse {
    let calls: Vec<serde_json::Value> = tool_calls
        .into_iter()
        .map(|(id, name, args)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": args}
            })
        })
        .collect();
    let json = serde_json::json!({
        "id": "chatcmpl-test-tc-002",
        "model": "gpt-4o-mock",
        "choices": [{
            "message": {"role": "assistant", "content": content, "tool_calls": calls},
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    });
    MockResponse::ok_json(json.to_string().as_bytes())
}

/// Build a canned chat-wire error: the HTTP status with the error body the
/// provider sends for it (OpenAI form, its public sentence and `code`).
pub fn canned_error_status(status: u16) -> MockResponse {
    let (message, code) = match status {
        400 => (
            "We could not parse the JSON body of your request.",
            serde_json::Value::Null,
        ),
        401 => (
            "Incorrect API key provided. You can find your API key at \
             https://platform.openai.com/account/api-keys.",
            "invalid_api_key".into(),
        ),
        404 => (
            "The model `gpt-4o-mock` does not exist or you do not have access to it.",
            "model_not_found".into(),
        ),
        429 => (
            "Rate limit reached for gpt-4o-mock on requests per min (RPM): \
             Limit 3, Used 3, Requested 1. Please try again in 20s.",
            "rate_limit_exceeded".into(),
        ),
        503 => (
            "The engine is currently overloaded, please try again later.",
            serde_json::Value::Null,
        ),
        s if s >= 500 => (
            "The server had an error while processing your request. Sorry about that!",
            serde_json::Value::Null,
        ),
        _ => ("The request was refused.", serde_json::Value::Null),
    };
    let mut resp = openai_error(status, message);
    let mut body: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("openai_error writes JSON");
    body["error"]["code"] = code;
    if status == 429 {
        body["error"]["type"] = "requests".into();
    }
    resp.body = body.to_string().into_bytes();
    resp
}

// ───── Self-tests ─────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream as StdTcpStream};

    /// Mirror of `meclaw_testing::mock_http::tests::raw_post_sync` — send a
    /// raw HTTP/1.1 POST with body, return the full response bytes. Stays
    /// dep-free (no reqwest in the test path).
    fn raw_post_sync(
        addr: SocketAddr,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Vec<u8> {
        let mut stream = StdTcpStream::connect(addr).unwrap();
        let mut req = format!("POST {path} HTTP/1.1\r\nHost: x\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        buf
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_openai_start_returns_base_url() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        assert!(mock.base_url.starts_with("http://"));
        assert!(mock.base_url.contains(&mock.addr.to_string()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recorded_requests_parses_body_as_json() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let addr = mock.addr;
        let req_body =
            r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"temperature":0.7}"#;
        let _resp = tokio::task::spawn_blocking(move || {
            raw_post_sync(
                addr,
                "/v1/chat/completions",
                &[
                    ("authorization", "Bearer test-key"),
                    ("content-type", "application/json"),
                ],
                req_body.as_bytes(),
            )
        })
        .await
        .unwrap();

        let snaps = mock.recorded_requests().await;
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].method, "POST");
        assert_eq!(snaps[0].path, "/v1/chat/completions");
        assert_eq!(snaps[0].model(), Some("gpt-4o"));
        assert_eq!(snaps[0].temperature(), Some(0.7));
        let msgs = snaps[0].messages().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hi");
        // Headers are lowercased by the capturing listener.
        assert_eq!(
            snaps[0].headers.get("authorization").map(|s| s.as_str()),
            Some("Bearer test-key")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canned_tool_calls_builds_expected_shape() {
        let resp = canned_tool_calls(vec![("call_abc", "web_fetch", r#"{"url":"https://x"}"#)]);
        // Decode the response body as JSON and check the shape.
        let parsed: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        let tc = &parsed["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "web_fetch");
        assert_eq!(tc["function"]["arguments"], r#"{"url":"https://x"}"#);
        assert_eq!(parsed["choices"][0]["finish_reason"], "tool_calls");
        assert!(parsed["choices"][0]["message"]["content"].is_null());
    }

    // ───── Wire-contract sharpening (Run-4b offline proof, receipt 922d93f) ─────
    //
    // The real provider 400s Form A ("An assistant message with 'tool_calls'
    // must be followed by tool messages responding to each 'tool_call_id'");
    // the 14b mock accepted it — mock leniency hid the wire constraint.
    // These tests pin the sharpened behavior: A → 400, B/C → 200.

    fn tc(id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "type": "function",
                           "function": {"name": "search", "arguments": "{}"}})
    }

    fn chat_body(messages: serde_json::Value) -> Vec<u8> {
        serde_json::json!({"model": "gpt-4o", "messages": messages})
            .to_string()
            .into_bytes()
    }

    async fn post_status(mock: &MockOpenAI, body: Vec<u8>) -> String {
        post_to(mock, "/v1/chat/completions", body).await.0
    }

    /// POST `body` to `path`; returns the status line and the parsed body.
    async fn post_to(
        mock: &MockOpenAI,
        path: &'static str,
        body: Vec<u8>,
    ) -> (String, serde_json::Value) {
        let addr = mock.addr;
        let resp = tokio::task::spawn_blocking(move || {
            let mut headers = vec![("content-type", "application/json")];
            if path.ends_with("/messages") {
                headers.push(("anthropic-version", "2023-06-01"));
            }
            raw_post_sync(addr, path, &headers, &body)
        })
        .await
        .unwrap();
        let text = String::from_utf8_lossy(&resp).to_string();
        let status = text.lines().next().unwrap_or("").to_string();
        let body = text
            .split_once("\r\n\r\n")
            .and_then(|(_, b)| serde_json::from_str(b).ok())
            .unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn form_a_consecutive_single_call_assistant_messages_rejected_400() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = chat_body(serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c1")]},
            {"role": "assistant", "content": null, "tool_calls": [tc("c2")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"},
        ]));
        let (status, err) = post_to(&mock, "/v1/chat/completions", body).await;
        assert!(
            status.starts_with("HTTP/1.1 400"),
            "form A must 400, got {status}"
        );
        assert_eq!(err["error"]["message"], TOOL_SEQUENCE_REFUSAL, "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn form_b_one_assistant_with_all_calls_accepted_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = chat_body(serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c1"), tc("c2")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"},
        ]));
        let status = post_status(&mock, body).await;
        assert!(
            status.starts_with("HTTP/1.1 200"),
            "form B must 200, got {status}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn form_c_interleaved_call_result_pairs_accepted_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = chat_body(serde_json::json!([
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c1")]},
            {"role": "tool", "tool_call_id": "c1", "content": "r1"},
            {"role": "assistant", "content": null, "tool_calls": [tc("c2")]},
            {"role": "tool", "tool_call_id": "c2", "content": "r2"},
        ]));
        let status = post_status(&mock, body).await;
        assert!(
            status.starts_with("HTTP/1.1 200"),
            "form C must 200, got {status}"
        );
    }

    // ───── GH #1283: wire rules for every model ─────

    fn body_of(model: &str, messages: serde_json::Value) -> Vec<u8> {
        serde_json::json!({"model": model, "messages": messages})
            .to_string()
            .into_bytes()
    }

    /// R2: the chat wire through the router refuses a request that ends on an
    /// assistant turn, in the router's form with the upstream sentence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r2_a_chat_prefill_is_refused_400_in_the_router_form() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = body_of(
            "anthropic/claude-test-b",
            serde_json::json!([{"role": "user", "content": "q"},
                               {"role": "assistant", "content": "One moment"}]),
        );
        let (status, err) = post_to(&mock, "/v1/chat/completions", body).await;
        assert!(status.starts_with("HTTP/1.1 400"), "{status}");
        assert_eq!(err["error"]["message"], "Provider returned error", "{err}");
        assert_eq!(err["error"]["code"], 400, "{err}");
        assert_eq!(err["error"]["metadata"]["raw"], ROUTER_PREFILL_RAW, "{err}");
    }

    /// R2 judges no other model: OpenAI's chat wire takes a trailing
    /// assistant turn, and the cell keeps it for those models (GH #1282).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r2_a_non_claude_chat_prefill_is_taken_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = body_of(
            "test-model",
            serde_json::json!([{"role": "user", "content": "q"},
                               {"role": "assistant", "content": "One moment"}]),
        );
        let (status, _) = post_to(&mock, "/v1/chat/completions", body).await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r2_a_chat_request_ending_on_the_user_is_taken_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = body_of(
            "anthropic/claude-test-b",
            serde_json::json!([{"role": "user", "content": "q"},
                               {"role": "assistant", "content": "a"},
                               {"role": "user", "content": "q2"}]),
        );
        let (status, _) = post_to(&mock, "/v1/chat/completions", body).await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    }

    /// R1: the Messages wire refuses a prefill with the measured sentence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r1_a_messages_prefill_is_refused_400_with_the_measured_sentence() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = body_of(
            "claude-test-b",
            serde_json::json!([{"role": "user", "content": "q"},
                               {"role": "assistant", "content": "One moment"}]),
        );
        let (status, err) = post_to(&mock, "/v1/messages", body).await;
        assert!(status.starts_with("HTTP/1.1 400"), "{status}");
        assert_eq!(err["type"], "error", "{err}");
        assert_eq!(err["error"]["type"], "invalid_request_error", "{err}");
        assert_eq!(err["error"]["message"], PREFILL_REFUSAL, "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r1_a_messages_request_ending_on_the_user_is_taken_200() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = body_of(
            "claude-test-b",
            serde_json::json!([{"role": "user", "content": "q"}]),
        );
        let (status, _) = post_to(&mock, "/v1/messages", body).await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    }

    /// The decisions wire stays free (D65 addendum): the same prefill shape
    /// that the other wires refuse is taken there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_decisions_wire_is_not_judged() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = body_of(
            "test-model",
            serde_json::json!([{"role": "user", "content": "q"},
                               {"role": "assistant", "content": "a"}]),
        );
        let (status, _) = post_to(&mock, "/alpha/decisions", body).await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    }

    #[test]
    fn the_wire_is_read_from_the_path() {
        assert_eq!(Wire::of("/v1/chat/completions"), Some(Wire::Chat));
        assert_eq!(Wire::of("/v1/responses"), Some(Wire::Responses));
        assert_eq!(Wire::of("/messages"), Some(Wire::Messages));
        assert_eq!(Wire::of("/alpha/decisions"), Some(Wire::Decisions));
        assert_eq!(Wire::of("/v1/models"), None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canned_error_status_sets_status_and_body() {
        let resp = canned_error_status(429);
        assert_eq!(resp.status, 429);
        assert_eq!(resp.content_type, "application/json");
        let parsed: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(parsed["error"]["code"], "rate_limit_exceeded");
        assert_eq!(parsed["error"]["type"], "requests");
        assert!(
            parsed["error"]["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("Rate limit reached")),
            "{parsed}"
        );
    }
    // ───── GH #1283 part B: the recorded rules ─────

    /// POST a JSON body to `path` with the given headers; status line + body.
    async fn post_with(
        mock: &MockOpenAI,
        path: &'static str,
        headers: Vec<(&'static str, &'static str)>,
        body: serde_json::Value,
    ) -> (String, serde_json::Value) {
        let addr = mock.addr;
        let bytes = body.to_string().into_bytes();
        let resp = tokio::task::spawn_blocking(move || {
            let mut all = vec![("content-type", "application/json")];
            all.extend(headers);
            raw_post_sync(addr, path, &all, &bytes)
        })
        .await
        .unwrap();
        let text = String::from_utf8_lossy(&resp).to_string();
        let status = text.lines().next().unwrap_or("").to_string();
        let body = text
            .split_once("\r\n\r\n")
            .and_then(|(_, b)| serde_json::from_str(b).ok())
            .unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    const VERSION: (&str, &str) = ("anthropic-version", "2023-06-01");

    /// One request of `model` with `extra` laid over a plain user turn.
    async fn judge(
        path: &'static str,
        model: &str,
        extra: serde_json::Value,
    ) -> (String, serde_json::Value) {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let mut body = serde_json::json!({"model": model, "max_tokens": 16,
            "messages": [{"role": "user", "content": "q"}]});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            body[k] = v;
        }
        post_with(&mock, path, vec![VERSION], body).await
    }

    /// The recording of `model` for `param` on `wire` (the fixture must have it).
    fn recorded(model: &str, wire: &str, param: &str) -> &'static RecordedRefusal {
        model_refusals(model, wire)
            .into_iter()
            .find(|r| r.param == param)
            .unwrap_or_else(|| panic!("no recorded {param} refusal for {model} on {wire}"))
    }

    /// A refused form gets the recorded status and the recorded body, byte for byte.
    fn assert_played(status: &str, body: &serde_json::Value, rec: &RecordedRefusal) {
        assert!(
            status.starts_with(&format!("HTTP/1.1 {}", rec.status)),
            "{status} (want {})",
            rec.status
        );
        let want: serde_json::Value = serde_json::from_str(&rec.body).unwrap();
        assert_eq!(body, &want);
    }

    fn assert_taken(status: &str, body: &serde_json::Value) {
        assert!(status.starts_with("HTTP/1.1 200"), "{status} {body}");
    }

    const CHAT: &str = "/v1/chat/completions";
    const MESSAGES: &str = "/v1/messages";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r3_reasoning_off_on_a_model_that_must_reason_is_refused_as_recorded() {
        let model = "anthropic/claude-sonnet-5.5";
        let rec = recorded(model, "chat", "reasoning_off");
        assert_eq!(rec.rule, "R3");
        assert_eq!(
            rec.message(),
            "Reasoning is mandatory for this endpoint and cannot be disabled."
        );
        let (s, b) = judge(
            CHAT,
            model,
            serde_json::json!({"reasoning": {"enabled": false}}),
        )
        .await;
        assert_played(&s, &b, rec);
        let (s, b) = judge(
            CHAT,
            model,
            serde_json::json!({"reasoning": {"effort": "low"}}),
        )
        .await;
        assert_taken(&s, &b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r4_temperature_on_a_router_model_without_it_is_refused_as_recorded() {
        let model = "openai/gpt-6-luna";
        let rec = recorded(model, "chat", "temperature");
        assert_eq!((rec.rule.as_str(), rec.status), ("R4", 404));
        assert!(
            rec.message()
                .starts_with("No endpoints found that can handle"),
            "{}",
            rec.message()
        );
        let (s, b) = judge(CHAT, model, serde_json::json!({"temperature": 0.3})).await;
        assert_played(&s, &b, rec);
        let (s, b) = judge(CHAT, model, serde_json::json!({})).await;
        assert_taken(&s, &b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r5_temperature_on_the_messages_wire_is_refused_as_recorded() {
        let model = "claude-haiku-5-5";
        let rec = recorded(model, "messages", "temperature");
        assert_eq!(rec.rule, "R5");
        assert_eq!(rec.message(), "`temperature` is deprecated for this model.");
        let (s, b) = judge(MESSAGES, model, serde_json::json!({"temperature": 0.3})).await;
        assert_played(&s, &b, rec);
        let (s, b) = judge(MESSAGES, model, serde_json::json!({})).await;
        assert_taken(&s, &b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r6_budget_thinking_is_refused_as_recorded() {
        let model = "claude-haiku-5-5";
        let rec = recorded(model, "messages", "thinking_enabled");
        assert_eq!(rec.rule, "R6");
        assert!(
            rec.message()
                .starts_with("\"thinking.type.enabled\" is not supported"),
            "{}",
            rec.message()
        );
        let on = serde_json::json!({"thinking": {"type": "enabled", "budget_tokens": 1024}});
        let (s, b) = judge(MESSAGES, model, on).await;
        assert_played(&s, &b, rec);
        let adaptive = serde_json::json!({"thinking": {"type": "adaptive"}});
        let (s, b) = judge(MESSAGES, model, adaptive).await;
        assert_taken(&s, &b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r8_a_named_tool_choice_is_refused_as_recorded() {
        let model = "qwen/qwen3.8-flash";
        let rec = recorded(model, "chat", "tool_choice_named");
        assert_eq!((rec.rule.as_str(), rec.status), ("R8", 404));
        assert!(
            rec.message()
                .starts_with("No endpoints found that support the provided 'tool_choice' value")
        );
        let tools = serde_json::json!([{"type": "function", "function": {"name": "echo", "parameters": {}}}]);
        let named = serde_json::json!({"tools": tools,
            "tool_choice": {"type": "function", "function": {"name": "echo"}}});
        let (s, b) = judge(CHAT, model, named).await;
        assert_played(&s, &b, rec);
        let (s, b) = judge(
            CHAT,
            model,
            serde_json::json!({"tools": tools, "tool_choice": "auto"}),
        )
        .await;
        assert_taken(&s, &b);
    }

    /// A neutral test model has no recording: the model rules leave it alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_model_without_a_recording_carries_only_the_wire_rules() {
        let every_form = serde_json::json!({"temperature": 0.3, "top_p": 0.9,
            "reasoning": {"enabled": false}});
        let (s, b) = judge(CHAT, "test-model", every_form).await;
        assert_taken(&s, &b);
        let (s, b) = judge(
            MESSAGES,
            "claude-test-b",
            serde_json::json!({"temperature": 0.3,
            "thinking": {"type": "enabled", "budget_tokens": 1024}}),
        )
        .await;
        assert_taken(&s, &b);
    }

    fn tool_round(user_content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"messages": [
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_a", "name": "echo", "input": {}}]},
            {"role": "user", "content": user_content},
        ]})
    }

    /// R13 holds for every model: a text in front of the `tool_result` is
    /// refused with the recorded sentence, naming the request's index and id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r13_a_text_before_the_tool_result_is_refused_400_with_the_recorded_sentence() {
        let rec = wire_rule_refusal("R13").expect("R13 recorded");
        let text_first = tool_round(serde_json::json!([
            {"type": "text", "text": "Here is the result."},
            {"type": "tool_result", "tool_use_id": "toolu_a", "content": "r"}]));
        let (s, b) = judge(MESSAGES, "claude-test-b", text_first).await;
        assert!(s.starts_with("HTTP/1.1 400"), "{s}");
        assert_eq!(b["error"]["type"], "invalid_request_error", "{b}");
        assert_eq!(
            b["error"]["message"],
            "messages.2: `tool_use` ids were found without `tool_result` blocks immediately \
             after: toolu_a. Each `tool_use` block must have a corresponding `tool_result` block \
             in the next message.",
            "{b} (recorded: {})",
            rec.message()
        );
        let result_first = tool_round(serde_json::json!([
            {"type": "tool_result", "tool_use_id": "toolu_a", "content": "r"},
            {"type": "text", "text": "Here is the result."}]));
        let (s, b) = judge(MESSAGES, "claude-test-b", result_first).await;
        assert_taken(&s, &b);
    }

    /// R14 holds for every model: an unknown top-level field is refused with
    /// the recorded sentence, naming the field sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r14_an_unknown_top_level_field_is_refused_400_with_the_recorded_sentence() {
        let (s, b) = judge(MESSAGES, "claude-test-b", serde_json::json!({"x_hint": 1})).await;
        assert!(s.starts_with("HTTP/1.1 400"), "{s}");
        assert_eq!(
            b["error"]["message"], "x_hint: Extra inputs are not permitted",
            "{b}"
        );
        let (s, b) = judge(
            MESSAGES,
            "claude-test-b",
            serde_json::json!({"metadata": {}}),
        )
        .await;
        assert_taken(&s, &b);
    }

    /// R14 knows the top-level `cache_control` the cell writes itself under
    /// `cache_mode: "implicit"` (GH #890): the provider takes it, so the mock does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r14_the_implicit_cache_mark_is_a_known_top_level_field() {
        let (s, b) = judge(
            MESSAGES,
            "claude-test-b",
            serde_json::json!({"cache_control": {"type": "ephemeral"}}),
        )
        .await;
        assert_taken(&s, &b);
    }

    /// R15 holds for every model: a Messages request without the version
    /// header is refused with the recorded sentence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r15_a_missing_version_header_is_refused_400_with_the_recorded_sentence() {
        let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
        let body = serde_json::json!({"model": "claude-test-b", "max_tokens": 16,
            "messages": [{"role": "user", "content": "q"}]});
        let (s, b) = post_with(&mock, MESSAGES, vec![], body.clone()).await;
        assert!(s.starts_with("HTTP/1.1 400"), "{s}");
        assert_eq!(b["type"], "error", "{b}");
        assert_eq!(
            b["error"]["message"], "anthropic-version: header is required",
            "{b}"
        );
        let (s, b) = post_with(&mock, MESSAGES, vec![VERSION], body).await;
        assert_taken(&s, &b);
    }
}
