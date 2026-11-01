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

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

// harness_fold: #[path = "h5_support/mod.rs"]
#[allow(unused_imports)]
use crate::h5; // harness_fold: mod h5;

use h5::{Turn, base_params, completion, one_turn, turn_with};
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

/// GH #1360: `arguments` as a JSON string whose content is itself a JSON
/// string of the object -- a small model's slip on the chat wire.
fn double_encoded(object: &Value) -> Value {
    Value::String(meclaw_core::serde_json::to_string(&object.to_string()).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_tool_args_double_encoded_decoded_once() {
    let args = json!({"question": "Please keep this: we married in 1985."});
    let t = one_turn(
        tool_call(json!({"id": "call_1", "type": "function",
                         "function": {"name": "ask_keeper",
                                      "arguments": double_encoded(&args)}})),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "tool_calls", "{:?}", t.body);
    let turn = &t.body["messages"][0];
    let function: Value =
        meclaw_core::serde_json::from_str(turn["text"].as_str().expect("text")).unwrap();
    assert_eq!(function["name"], "ask_keeper", "{function}");
    let decoded: Value =
        meclaw_core::serde_json::from_str(function["arguments"].as_str().expect("a string"))
            .expect("the arguments are one JSON object string now");
    assert_eq!(decoded, args, "decoded exactly once: {function}");
}

/// GH #1279: the repair is said, not only done. The one WARN
/// line names `args_double_decoded` and the call it repaired; the call id is
/// this test's own, so a parallel test of the binary cannot answer for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_tool_args_double_decoded_is_logged() {
    let log = meclaw_testing::log_recorder::global();
    let args = json!({"question": "Where did we meet?"});
    let t = one_turn(
        tool_call(json!({"id": "call_fu2_pin", "type": "function",
                         "function": {"name": "ask_keeper",
                                      "arguments": double_encoded(&args)}})),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "tool_calls", "{:?}", t.body);
    let lines = log.lines(
        tracing::Level::WARN,
        &["args_double_decoded", "tool_call_id=call_fu2_pin"],
    );
    assert_eq!(lines.len(), 1, "one line names the repair: {lines:?}");

    // A call whose arguments are the object string itself is no repair.
    let t = one_turn(
        tool_call(json!({"id": "call_fu2_plain", "type": "function",
                         "function": {"name": "ask_keeper",
                                      "arguments": args.to_string()}})),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "tool_calls", "{:?}", t.body);
    assert!(
        log.lines(tracing::Level::WARN, &["tool_call_id=call_fu2_plain"])
            .is_empty(),
        "nothing to repair, nothing said"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_tool_args_triple_encoded_is_error() {
    let twice = double_encoded(&json!({"question": "x"}));
    let thrice = Value::String(meclaw_core::serde_json::to_string(&twice).unwrap());
    let t = one_turn(
        tool_call(json!({"id": "call_1", "type": "function",
                         "function": {"name": "ask_keeper", "arguments": thrice}})),
        json!({}),
    )
    .await;
    assert_refused(&t, "malformed_tool_call", "three levels");
}

/// GH #1373 (BAT R1 C1): the provider returned `web_search"}` -- a rest of the
/// arguments in the name -- for the declared `web_search`. Dispatched as it
/// came, it cost a round against `unknown_tool` and a second search. The name
/// is repaired once to the declared tool, and the WARN line says so, with this
/// test's own call id (a parallel test of the binary cannot answer for it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_a_tool_name_with_an_argument_rest_is_repaired_and_logged() {
    let log = meclaw_testing::log_recorder::global();
    let declared = json!({"type": "function",
                          "function": {"name": "web_search", "parameters": {}}});
    let t = turn_with(
        tool_call(json!({"id": "call_fxname_pin", "type": "function",
                         "function": {"name": "web_search\"}",
                                      "arguments": "{\"query\":\"weather\"}"}})),
        base_params,
        json!({"system": {"tools": {"web_search": {"text": declared.to_string()}}},
               "messages": [{"origin": "user", "type": "text", "text": "hi"}]}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "tool_calls", "{:?}", t.body);
    let function: Value =
        meclaw_core::serde_json::from_str(t.body["messages"][0]["text"].as_str().expect("text"))
            .unwrap();
    assert_eq!(function["name"], "web_search", "{function}");
    let lines = log.lines(
        tracing::Level::WARN,
        &["tool_name_repaired", "tool_call_id=call_fxname_pin"],
    );
    assert_eq!(lines.len(), 1, "one line names the repair: {lines:?}");
}

/// GH #1373 (review m2): a name that already is the declared tool is no
/// repair -- it stays as it came and no `tool_name_repaired` line names its
/// call id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_an_exactly_declared_tool_name_is_no_repair() {
    let log = meclaw_testing::log_recorder::global();
    let declared = json!({"type": "function",
                          "function": {"name": "web_search", "parameters": {}}});
    let t = turn_with(
        tool_call(json!({"id": "call_fxname_exact", "type": "function",
                         "function": {"name": "web_search",
                                      "arguments": "{\"query\":\"weather\"}"}})),
        base_params,
        json!({"system": {"tools": {"web_search": {"text": declared.to_string()}}},
               "messages": [{"origin": "user", "type": "text", "text": "hi"}]}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "tool_calls", "{:?}", t.body);
    let function: Value =
        meclaw_core::serde_json::from_str(t.body["messages"][0]["text"].as_str().expect("text"))
            .unwrap();
    assert_eq!(function["name"], "web_search", "{function}");
    let lines = log.lines(
        tracing::Level::WARN,
        &["tool_name_repaired", "tool_call_id=call_fxname_exact"],
    );
    assert!(lines.is_empty(), "a declared name is no repair: {lines:?}");
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
