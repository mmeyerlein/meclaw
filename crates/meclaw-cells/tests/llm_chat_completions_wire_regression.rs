//! P10 step A0 — regression pin on the EXISTING `api_key` / chat-completions path.
//!
//! P10 adds a second auth dimension and a second wire dialect to the `llm` cell.
//! The existing path must stay byte-semantically unchanged. This test is the
//! tripwire: it drives one deliberately broad request (system prompt + tools +
//! every supported turn type + attribution params + `provider_extra` overlay)
//! through the full cell and pins BOTH the serialized request body and the set
//! of request header names against a committed fixture.
//!
//! If a P10 change alters the legacy wire in any way, this fails.
//!
//! Fixture: `tests/fixtures/expected_chat_completions_body.json` — deliberately
//! beside the test, not under `plans/`: `plans/` is private and never exported,
//! so a fixture living there would make this test unrunnable in the public repo.
//! Regenerate ONLY on an explicit sanctioned change to the legacy wire.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Path of the committed body fixture, relative to the crate root (which is
/// cargo's working directory for integration tests).
const BODY_FIXTURE: &str = "tests/fixtures/expected_chat_completions_body.json";

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

/// One deliberately broad legacy request: system subtree with a tool schema,
/// all supported turn types, attribution params, `provider_extra` overlay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_chat_completions_body_and_headers_match_frozen_fixture() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();

    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-a0",
        "base_url": format!("{}/v1", mock.base_url),
        "temperature": 0.3,
        "max_tokens": 512,
        "system_order": ["identity", "instructions"],
        "provider_extra": {"seed": 42},
        "http_referer": "https://example.test",
        "x_title": "Example App",
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(raw_conn, None);
    let (sink, _rx) = mk_sink();

    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {
                "identity": {"text": "You are a fixture."},
                "instructions": {"text": "Be terse."},
                "tools": {
                    "get_weather": {"text": "{\"name\":\"get_weather\",\"description\":\"w\",\"parameters\":{\"type\":\"object\",\"properties\":{}}}"}
                }
            },
            "messages": [
                {"origin": "user", "type": "text", "text": "first"},
                {"origin": "assistant", "type": "tool_call", "id": "call_1",
                 "text": "{\"name\":\"get_weather\",\"arguments\":\"{}\"}"},
                {"origin": "tool", "type": "tool_result", "id": "call_1", "text": "sunny"},
                {"origin": "assistant", "type": "text", "text": "it is sunny"},
                {"origin": "user", "type": "text", "text": "thanks"}
            ]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");

    // ─── (1) body pin ───
    let actual = &snaps[0].body;
    let expected: meclaw_core::serde_json::Value = {
        let text = std::fs::read_to_string(BODY_FIXTURE).unwrap_or_else(|e| {
            panic!(
                "missing legacy-wire fixture {BODY_FIXTURE}: {e}\n\
                 actual body was:\n{}",
                meclaw_core::serde_json::to_string_pretty(actual).unwrap()
            )
        });
        meclaw_core::serde_json::from_str(&text).expect("fixture is valid JSON")
    };
    assert_eq!(
        actual,
        &expected,
        "legacy chat-completions body drifted from the frozen fixture.\nactual:\n{}",
        meclaw_core::serde_json::to_string_pretty(actual).unwrap()
    );

    // ─── (2) path pin ───
    assert_eq!(snaps[0].path, "/v1/chat/completions");
    assert_eq!(snaps[0].method, "POST");

    // ─── (3) header-name pin: exactly the legacy set, nothing added ───
    let mut names: Vec<&str> = snaps[0]
        .headers
        .keys()
        .map(|k| k.as_str())
        .filter(|k| !matches!(*k, "host" | "content-length" | "accept" | "user-agent"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["authorization", "content-type", "http-referer", "x-title"],
        "legacy header set drifted"
    );
    assert_eq!(
        snaps[0].headers.get("authorization").map(String::as_str),
        Some("Bearer test-key-a0"),
        "legacy Authorization form drifted"
    );
}
