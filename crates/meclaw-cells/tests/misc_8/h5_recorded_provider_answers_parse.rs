//! GH #999 -- every provider answer the conformance tool recorded parses in
//! the real `llm` cell (replay, offline).
//!
//! The tool (K9) stores the bodies a real endpoint sent under
//! `fixtures/conformance/<slug>/responses/*.json`. Each one is served here by
//! the mock to a real cell and must arrive at the receiver as a SUCCESS: the
//! `finish_reason` mapped, `tokens_prompt` set where the provider reported
//! it, and no thinking trace inside a turn. A recording that ran out of
//! budget thinking (`finish_reason` `length`, no text, a non-empty trace --
//! whichever probe) is the one real answer that must arrive as
//! `reasoning_exhausted` instead.
//!
//! Hand-made answers of a broken provider live under a `fake__` slug and are
//! named in `EXPECTED_RED` with the `meta.error.kind` they must produce, so
//! every validation rule of the cell is replayed from a file as well. At least
//! one recording must exist, or the replay proves nothing.
//!
//! GH #1243: a recording of the native Messages wire (`"type": "message"`,
//! the `claude-haiku-5-5` row) replays through a cell born with
//! `provider: "anthropic"`; its `stop_reason`, `usage` and `thinking` blocks
//! stand where the chat wire has `finish_reason`, `prompt_tokens` and the
//! trace fields.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

// harness_fold: #[path = "h5_support/mod.rs"]
#[allow(unused_imports)]
use crate::h5; // harness_fold: mod h5;

use meclaw_core::serde_json::{self, Value};
use meclaw_testing::mock_http::MockResponse;
use std::path::PathBuf;

/// `<slug>/responses/<file>` of a fake answer -> the kind it must produce.
const EXPECTED_RED: &[(&str, &str)] = &[
    (
        "fake__cell-shapes/responses/tool_arguments_object.json",
        "malformed_tool_call",
    ),
    ("fake__cell-shapes/responses/empty.json", "empty_answer"),
    (
        "fake__cell-shapes/responses/reasoning_exhausted.json",
        "reasoning_exhausted",
    ),
];

/// Where the provider puts its thinking trace in `message`.
const TRACE_FIELDS: &[&str] = &["reasoning", "reasoning_content", "reasoning_details"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

fn recordings() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for slug in std::fs::read_dir(root()).into_iter().flatten().flatten() {
        let responses = slug.path().join("responses");
        for f in std::fs::read_dir(&responses)
            .into_iter()
            .flatten()
            .flatten()
        {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "json") {
                let rel = p
                    .strip_prefix(root())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    out
}

/// The labels of a `reasoning_details` item, not its trace. Measured on
/// 2026-10-05 (GH #1000): a hosted model's tool answer carries an encrypted
/// detail whose `id` IS the tool call's id (`call_…`), which rightly reaches
/// the turn -- read as a trace it made the replay red for a correct cell.
const TRACE_LABELS: &[&str] = &["id", "type", "format", "index"];

/// GH #1243: a native answer in the chat wire's message form -- its text
/// blocks as `content`, its `tool_use` blocks as `tool_calls`, its thinking
/// blocks under the first trace field -- so the same checks read it.
fn native_message(answer: &Value) -> Value {
    let blocks = |kind: &str| -> Vec<Value> {
        answer["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == kind)
            .cloned()
            .collect()
    };
    let text: String = blocks("text")
        .iter()
        .filter_map(|b| b["text"].as_str().map(str::to_string))
        .collect();
    let thinking: Vec<Value> = blocks("thinking")
        .iter()
        .map(|b| b["thinking"].clone())
        .collect();
    let mut message = serde_json::Map::new();
    message.insert("content".to_string(), Value::String(text));
    message.insert("tool_calls".to_string(), Value::Array(blocks("tool_use")));
    message.insert(TRACE_FIELDS[0].to_string(), Value::Array(thinking));
    Value::Object(message)
}

/// GH #1243: the native `stop_reason` in the chat wire's terms, as the cell
/// maps it (`translate_anthropic::parse_messages_response`).
fn native_finish(answer: &Value) -> String {
    match answer["stop_reason"].as_str().unwrap_or("") {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" | "model_context_window_exceeded" => "length",
        "tool_use" => "tool_calls",
        "refusal" => "content_filter",
        other => other,
    }
    .to_string()
}

/// Every non-empty string a thinking field holds (the trace text itself).
fn trace_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.trim().len() >= 12 => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| trace_texts(x, out)),
        Value::Object(o) => o
            .iter()
            .filter(|(k, _)| !TRACE_LABELS.contains(&k.as_str()))
            .for_each(|(_, x)| trace_texts(x, out)),
        _ => {}
    }
}

/// An answer that spent its budget thinking: `length`, no text, no tool call
/// and a NON-EMPTY trace -- the rule of the cell's `has_thinking_trace`.
/// Read from the answer's shape, not the file name: a thinking model can
/// exhaust even the basic probe's 16 tokens (K1-green, README § 4).
fn is_exhausted_reasoning(message: &Value, finish: &str) -> bool {
    let no_text = message["content"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty());
    let no_call = message["tool_calls"]
        .as_array()
        .is_none_or(|a| a.is_empty());
    let trace = TRACE_FIELDS.iter().any(|f| match &message[*f] {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    });
    finish == "length" && no_text && no_call && trace
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_recorded_provider_answers_parse() {
    let files = recordings();
    assert!(
        !files.is_empty(),
        "no recorded answer under {} -- the replay would prove nothing",
        root().display()
    );
    for (rel, _) in EXPECTED_RED {
        assert!(
            files.iter().any(|(r, _)| r == rel),
            "EXPECTED_RED names a missing file: {rel}"
        );
    }
    for (rel, path) in files {
        let raw = std::fs::read(&path).unwrap();
        let answer: Value = serde_json::from_slice(&raw).expect("a json recording");
        let native = answer["type"] == "message";
        let (message, finish, extra) = if native {
            (
                native_message(&answer),
                native_finish(&answer),
                serde_json::json!({"provider": "anthropic"}),
            )
        } else {
            let finish = answer["choices"][0]["finish_reason"].as_str().unwrap_or("");
            (
                answer["choices"][0]["message"].clone(),
                finish.to_string(),
                serde_json::json!({}),
            )
        };
        let message = &message;
        let t = h5::one_turn(MockResponse::ok_json(&raw), extra).await;

        let expected_red = EXPECTED_RED
            .iter()
            .find(|(r, _)| *r == rel)
            .map(|(_, k)| *k)
            .or_else(|| is_exhausted_reasoning(message, &finish).then_some("reasoning_exhausted"));
        if let Some(kind) = expected_red {
            assert_eq!(t.hop["finish_reason"], "error", "{rel}: {:?}", t.body);
            assert_eq!(t.kind(), Some(kind), "{rel}: {}", t.body["meta"]);
            continue;
        }

        assert_ne!(t.hop["finish_reason"], "error", "{rel}: {}", t.body["meta"]);
        let mapped = if finish == "function_call" {
            "tool_calls"
        } else {
            finish.as_str()
        };
        assert_eq!(t.hop["finish_reason"], mapped, "{rel}");
        let prompt = if native {
            [
                "input_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
            ]
            .iter()
            .map(|k| answer["usage"][*k].as_u64())
            .sum::<Option<u64>>()
        } else {
            answer["usage"]["prompt_tokens"].as_u64()
        };
        if let Some(n) = prompt {
            assert_eq!(t.hop["tokens_prompt"], n, "{rel}: {:?}", t.hop);
        }
        let mut traces = Vec::new();
        for f in TRACE_FIELDS {
            trace_texts(&message[*f], &mut traces);
        }
        let turns = t.body["messages"].to_string();
        for trace in traces {
            assert!(
                !turns.contains(&trace),
                "{rel}: a thinking trace reached a turn: {trace}"
            );
        }
    }
}

#[test]
fn h5_exhaustion_is_read_from_the_answer_not_the_file_name() {
    // A thinking model may spend even the 16 tokens of the basic probe on its
    // trace (`length`, no text, a trace -- K1-green, README § 4). The cell
    // names that `reasoning_exhausted` whatever file it came from, so the
    // replay must expect it from the answer's shape alone.
    let thought = serde_json::json!({"content": null, "reasoning": "Let me think about this."});
    assert!(is_exhausted_reasoning(&thought, "length"));
    // Same rule as the cell's `has_thinking_trace`: an empty trace is no
    // trace, so such an answer is not an exhausted reasoning.
    for empty in [
        serde_json::json!({"content": "", "reasoning": ""}),
        serde_json::json!({"content": "", "reasoning_details": []}),
        serde_json::json!({"content": "", "reasoning_content": {}}),
    ] {
        assert!(!is_exhausted_reasoning(&empty, "length"), "{empty}");
    }
}

#[test]
fn h5_a_detail_label_is_no_trace() {
    // The shape of the measured tool answer (GH #1000): the encrypted data is
    // the trace, the call id beside it is a label the turn rightly carries.
    let details = serde_json::json!([{
        "type": "reasoning.encrypted",
        "data": "AY89a1/t5Eo6gnNCI7uB5Qd4",
        "format": "google-gemini-v1",
        "id": "call_2615879",
        "index": 0
    }]);
    let mut out = Vec::new();
    trace_texts(&details, &mut out);
    assert_eq!(out, vec!["AY89a1/t5Eo6gnNCI7uB5Qd4".to_string()]);
}
