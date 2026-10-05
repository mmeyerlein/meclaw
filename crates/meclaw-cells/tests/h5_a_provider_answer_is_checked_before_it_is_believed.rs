//! GH #999 -- the `llm` cell checks a provider answer before it believes it.
//!
//! Before this, `parse_openai_response` checked `finish_reason`, `model` and
//! `id` and handed everything else on as it came: a tool call without a name
//! or with an object for `arguments` reached the tool caller, which failed
//! later with no word about the provider; an answer with neither text nor a
//! tool call became a SUCCESS with no turn (`content: null`) or an empty one
//! (`content: ""`); and a server that streamed on the chat wire left a parse
//! complaint with no kind. Each case now arrives as an error answer with its
//! own `meta.error.kind` (`error_code` stays `provider_error`, the closed
//! enum), measured at the sink:
//!
//! - F1 `malformed_tool_call`: `id` and `function.name` not empty,
//!   `function.arguments` a STRING holding a JSON object -- the empty string
//!   is the empty object (common provider practice) and passes as `"{}"`;
//! - F2 `empty_answer`, or `reasoning_exhausted` when `finish_reason` is
//!   `length` and the message carries a thinking trace -- the budget went
//!   into thinking, which a caller fixes differently from an empty reply;
//! - F3 (a lock, green from the start): no thinking trace ever reaches a turn;
//! - F4 `unexpected_stream`: `text/event-stream` on the chat wire.
//!
//! Free of a paid call by construction: the provider is the in-process mock.

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "h5_support/mod.rs"]
mod h5;

use h5::{Turn, completion, one_turn};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::mock_http::MockResponse;

/// The answer is an error of the closed `provider_error` code, from the parse
/// stage, with `kind` -- and the conversation went back unchanged.
fn assert_refused(t: &Turn, kind: &str, case: &str) {
    assert_eq!(t.hop["finish_reason"], "error", "{case}: {:?}", t.body);
    assert_eq!(t.hop["error_code"], "provider_error", "{case}: {:?}", t.hop);
    assert_eq!(t.kind(), Some(kind), "{case}: {}", t.body["meta"]);
    assert_eq!(
        t.body["messages"],
        json!([{"origin": "user", "type": "text", "text": "hi"}]),
        "{case}: Gate-1 pass-through"
    );
}

fn tool_call(call: Value) -> MockResponse {
    completion(
        json!({"role": "assistant", "content": null, "tool_calls": [call]}),
        "tool_calls",
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_tool_call_arguments_must_be_a_json_string() {
    for (case, arguments) in [
        ("an object", json!({"text": "ping"})),
        ("not json", json!("text=ping")),
        ("an array", json!("[1]")),
    ] {
        let t = one_turn(
            tool_call(json!({"id": "call_1", "type": "function",
                             "function": {"name": "echo", "arguments": arguments}})),
            json!({}),
        )
        .await;
        assert_refused(&t, "malformed_tool_call", case);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_tool_call_without_name_is_refused() {
    for (case, call) in [
        (
            "no name",
            json!({"id": "call_1", "type": "function", "function": {"arguments": "{}"}}),
        ),
        (
            "an empty name",
            json!({"id": "call_1", "type": "function",
                   "function": {"name": "", "arguments": "{}"}}),
        ),
        (
            "no id",
            json!({"type": "function", "function": {"name": "echo", "arguments": "{}"}}),
        ),
        ("no function", json!({"id": "call_1", "type": "function"})),
    ] {
        let t = one_turn(tool_call(call), json!({})).await;
        assert_refused(&t, "malformed_tool_call", case);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_empty_arguments_string_is_an_empty_object() {
    let t = one_turn(
        tool_call(json!({"id": "call_1", "type": "function",
                         "function": {"name": "echo", "arguments": ""}})),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "tool_calls", "{:?}", t.body);
    let turn = &t.body["messages"][0];
    assert_eq!(turn["type"], "tool_call", "{turn}");
    assert_eq!(turn["id"], "call_1", "{turn}");
    let function: Value =
        meclaw_core::serde_json::from_str(turn["text"].as_str().expect("text")).unwrap();
    assert_eq!(function["name"], "echo", "{function}");
    assert_eq!(function["arguments"], "{}", "{function}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_an_empty_answer_is_named() {
    for (case, message, finish) in [
        (
            "null content",
            json!({"role": "assistant", "content": null}),
            "stop",
        ),
        (
            "empty content",
            json!({"role": "assistant", "content": ""}),
            "stop",
        ),
        (
            "blank content",
            json!({"role": "assistant", "content": " \n "}),
            "stop",
        ),
        (
            "length without a thinking trace",
            json!({"role": "assistant", "content": ""}),
            "length",
        ),
    ] {
        let t = one_turn(completion(message, finish), json!({})).await;
        assert_refused(&t, "empty_answer", case);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_reasoning_that_ate_the_budget_is_named() {
    for (case, message) in [
        (
            "reasoning",
            json!({"role": "assistant", "content": "",
                   "reasoning": "Let me think about the single word to answer with"}),
        ),
        (
            "reasoning_content",
            json!({"role": "assistant", "content": null,
                   "reasoning_content": "Let me think about the single word to answer with"}),
        ),
        (
            "reasoning_details",
            json!({"role": "assistant", "content": null,
                   "reasoning_details": [{"type": "reasoning.text", "text": "Let me think"}]}),
        ),
    ] {
        let t = one_turn(completion(message, "length"), json!({})).await;
        assert_refused(&t, "reasoning_exhausted", case);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_the_thinking_never_reaches_the_turn() {
    // A lock, green from the start: the parser never read a thinking field.
    let t = one_turn(
        completion(
            json!({"role": "assistant", "content": "OK",
                   "reasoning": "secret-trace-one",
                   "reasoning_content": "secret-trace-two",
                   "reasoning_details": [{"type": "reasoning.text", "text": "secret-trace-three"}]}),
            "stop",
        ),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "stop", "{:?}", t.body);
    assert_eq!(
        t.body["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "OK"}])
    );
    let whole = t.body.to_string();
    assert!(!whole.contains("secret-trace"), "{whole}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_a_stream_on_the_chat_wire_is_refused() {
    let sse = "data: {\"id\":\"c1\",\"model\":\"model-a\",\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
    let t = one_turn(
        MockResponse {
            status: 200,
            body: sse.as_bytes().to_vec(),
            content_type: "text/event-stream".into(),
            delay: None,
        },
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "error", "{:?}", t.body);
    assert_eq!(t.hop["error_code"], "provider_error", "{:?}", t.hop);
    assert_eq!(t.kind(), Some("unexpected_stream"), "{}", t.body["meta"]);
    assert_eq!(t.body["meta"]["error"]["source"], "wire");
}
