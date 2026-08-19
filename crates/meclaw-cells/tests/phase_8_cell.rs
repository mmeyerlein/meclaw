//! Phase-8 LlmCell integration tests (wire, state, LlmCell::handle).
//!
//! T11: call_openai HTTP/error/timeout/auth-redact. T12-T15: state.rs DB-IO
//! against TestRoot. T18-T22: LlmCell handle() orchestration. T23 onwards
//! lives in tests/phase_8_demo.rs after the T23-T26 rewrite.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::wire::{WireError, call_openai, redact_authorization};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_error_status, canned_tool_calls};
use std::time::Duration;

// ───── T11: call_openai happy path ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_happy_returns_response_json() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}]
    });
    let resp = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(
        resp.get("choices").is_some(),
        "response has choices: {resp:?}"
    );
    assert_eq!(resp["choices"][0]["message"]["content"], "hi");
}

// ───── T11: WireError mappings ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_rate_limit_429() {
    let mock = MockOpenAI::start(vec![canned_error_status(429)]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let result = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(result, Err(WireError::RateLimited)),
        "got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_unauthorized_401() {
    let mock = MockOpenAI::start(vec![canned_error_status(401)]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let result = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(result, Err(WireError::Unauthorized)),
        "got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_model_not_found_404() {
    let mock = MockOpenAI::start(vec![canned_error_status(404)]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let result = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(result, Err(WireError::ModelNotFound)),
        "got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_http_status_500() {
    let mock = MockOpenAI::start(vec![canned_error_status(500)]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let result = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(result, Err(WireError::HttpStatus(500))),
        "got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_body_parse_fail_on_non_json() {
    // 200 OK with a non-JSON body (text/plain). reqwest's `.json::<Value>()`
    // will fail to parse → BodyParse.
    let mock = MockOpenAI::start(vec![MockResponse::ok(b"not json at all")]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let result = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(result, Err(WireError::BodyParse(_))),
        "got {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_network_error_on_closed_port() {
    // Connect to an unbound port → connection refused → Network.
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let result = call_openai(
        &client,
        "http://127.0.0.1:1/foo",
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(result, Err(WireError::Network(_))),
        "got {result:?}"
    );
    // Paranoia: the error message MUST NOT contain the api_key.
    if let Err(WireError::Network(msg)) = &result {
        assert!(
            !msg.contains("test-key"),
            "api_key leaked in Network msg: {msg}"
        );
    }
}

// ───── T11: A-Timeout via MockResponse::with_delay ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_timeout_fires_a_timeout() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("x", "stop").with_delay(Duration::from_secs(5)),
    ])
    .await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let started = std::time::Instant::now();
    let result = call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_millis(200),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(matches!(result, Err(WireError::Timeout)), "got {result:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "must fire ~200ms, took {elapsed:?}"
    );
}

// ───── T11: Authorization header + redact ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_openai_sends_authorization_bearer_header() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let _ = call_openai(
        &client,
        &url,
        Some("test-key-XYZ"),
        &[],
        &serde_json::json!({}),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    assert_eq!(
        snaps[0].headers.get("authorization").map(|s| s.as_str()),
        Some("Bearer test-key-XYZ")
    );
}

#[test]
fn redact_authorization_replaces_value() {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("authorization", "Bearer sk-secret-12345".parse().unwrap());
    h.insert("content-type", "application/json".parse().unwrap());
    let r = redact_authorization(&h);
    assert_eq!(r.get("authorization"), Some(&"<redacted>".to_string()));
    assert_eq!(r.get("content-type"), Some(&"application/json".to_string()));
}

// ───── T20/T21: LlmCell::handle steps 5..8 (build-translate + HTTP call +
//       Parse-Response + Emit-Assistant-Turn) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_with_messages_hits_mock_openai() {
    // T20 + T21 integration: full happy path through steps 1..8 — parse,
    // persist, read_system_tree, build_openai_request, call_openai →
    // MockOpenAI, parse_openai_response, emit_assistant_turn. Asserts:
    // (1) MockOpenAI received exactly 1 request with leading-system + user.
    // (2) Cell emitted UBF assistant-turn to reply_to with full header+meta.
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_chat_completion("hi back", "stop")]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-T20",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"identity": {"soul": {"text": "P"}}},
            "messages": [{"origin":"user","type":"text","text":"Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    // (1) Mock-server received-request verification.
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        1,
        "MockOpenAI must have received exactly 1 request"
    );
    assert_eq!(snaps[0].path, "/v1/chat/completions");
    assert_eq!(snaps[0].method, "POST");
    assert_eq!(
        snaps[0].headers.get("authorization").map(|s| s.as_str()),
        Some("Bearer test-key-T20")
    );
    let messages = snaps[0].messages().expect("body has messages[]");
    assert_eq!(messages.len(), 2, "leading system + user turn expected");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "P");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Hi");
    assert_eq!(snaps[0].model(), Some("gpt-4o"));

    // (2) Cell-emit verification: assistant-turn UBF on reply_to=/observer.
    let em = rx.recv().await.expect("cell must emit assistant-turn");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(
        em.content["messages"],
        json!([{"origin":"assistant","type":"text","text":"hi back"}])
    );
    assert_eq!(em.content["header"]["finish_reason"], "stop");
    assert_eq!(em.content["header"]["tokens_prompt"], 10);
    assert_eq!(em.content["header"]["tokens_completion"], 5);
    assert_eq!(em.content["header"]["model"], "gpt-4o-mock");
    assert_eq!(em.content["meta"]["provider"], "openai");
    assert_eq!(em.content["meta"]["model"], "gpt-4o-mock");
    assert_eq!(em.content["meta"]["response_id"], "chatcmpl-test-001");
    // latency_ms is whole-millisecond wall time around the provider call. A warm
    // local mock in a release build answers in well under 1ms, so 0 is a correct
    // value (the old `> 0` assertion assumed debug-speed timing and flaked in
    // release). Assert presence + valid u64 instead.
    assert!(
        em.content["meta"]["latency_ms"].as_u64().is_some(),
        "latency_ms must be a present u64 (0 is valid for a sub-ms call): {}",
        em.content["meta"]["latency_ms"]
    );
}

// ───── T21: tool_call response → UBF tool_call turn ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_emits_tool_call_turn() {
    // T21 tool-call branch: MockOpenAI returns a tool_calls response →
    // cell emits a single UBF turn with origin=assistant, type=tool_call,
    // id=pass-through, text=stringified function-JSON, finish_reason=
    // "tool_calls".
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-1",
        "calc",
        r#"{"x":2}"#,
    )])])
    .await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-T21",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    // system.tools.calc gives the cell a tool-schema to send (forces the
    // request body to carry a `tools` array). Leaf must be a `{text:"<json>"}`-
    // wrapper per `extract_tools` contract.
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {
                "tools": {
                    "calc": {"text": r#"{"type":"function","function":{"name":"calc"}}"#}
                }
            },
            "messages": [{"origin":"user","type":"text","text":"calc 2"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let em = rx.recv().await.expect("cell must emit tool_call turn");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "tool_calls");
    let turns = em.content["messages"].as_array().unwrap();
    assert_eq!(turns.len(), 1, "expected single tool_call turn");
    assert_eq!(turns[0]["origin"], "assistant");
    assert_eq!(turns[0]["type"], "tool_call");
    assert_eq!(turns[0]["id"], "call-1");
    // text is the stringified `function` sub-object from the response.
    let fn_text: serde_json::Value =
        serde_json::from_str(turns[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(fn_text["name"], "calc");
    assert_eq!(fn_text["arguments"], r#"{"x":2}"#);
}

// ───── Translate-Merge e2e: 14b re-entry thread against the sharpened mock ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_merges_consecutive_tool_call_turns_on_wire() {
    // Run-4b masked case, now pinned e2e: a re-entry thread with THREE
    // consecutive UBF tool_call turns (one id each) + their results must hit
    // the wire as ONE assistant message with tool_calls[] (Form B). The
    // sharpened MockOpenAI 400s the unmerged Form A — so a non-error
    // emission here proves the merge end-to-end (cell → translate → wire).
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_chat_completion("Berlin 15, HH 17", "stop")]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-merge",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let call = |id: &str| {
        json!({"origin": "assistant", "type": "tool_call", "id": id,
               "text": r#"{"name":"search","arguments":"{}"}"#})
    };
    let result =
        |id: &str, t: &str| json!({"origin": "tool", "type": "tool_result", "id": id, "text": t});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [
                {"origin": "user", "type": "text", "text": "weather in 3 cities?"},
                call("c1"), call("c2"), call("c3"),
                result("c1", "15"), result("c2", "17"), result("c3", "10"),
            ]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    // Non-error emission ⟺ the sharpened mock accepted the wire form.
    let em = rx.recv().await.expect("cell must emit assistant turn");
    assert_eq!(
        em.content["header"]["finish_reason"], "stop",
        "merge must survive the sharpened mock, got {:#?}",
        em.content
    );

    // Wire receipt: exactly ONE assistant message carrying all three calls.
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1);
    let msgs = reqs[0].messages().unwrap();
    let assistant_msgs: Vec<_> = msgs.iter().filter(|m| m["role"] == "assistant").collect();
    assert_eq!(assistant_msgs.len(), 1, "form B: one assistant message");
    assert_eq!(assistant_msgs[0]["tool_calls"].as_array().unwrap().len(), 3);
}

// ───── T21: response-parse-fail → provider_error with Gate-1 pass-through ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_response_parse_fail_emits_provider_error_pass_through() {
    // MockOpenAI returns a 200 with malformed JSON (no `choices` array).
    // parse_openai_response → ResponseShape → cell emits provider_error
    // with source="parse", input_messages pass-through (Gate-1).
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let bad = MockResponse {
        status: 200,
        body: br#"{"id":"x-id","model":"y-model"}"#.to_vec(),
        content_type: "application/json".into(),
        delay: None,
    };
    let mock = MockOpenAI::start(vec![bad]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-T21-parse",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let user_turns = json!([{"origin":"user","type":"text","text":"Hi"}]);
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": user_turns.clone()})))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let em = rx.recv().await.expect("cell must emit parse-fail error");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "provider_error");
    assert_eq!(em.content["meta"]["error"]["source"], "parse");
    // Gate-1: input messages pass through unchanged.
    assert_eq!(em.content["messages"], user_turns);
    // Defensive model/response_id surfaced from the (partial) response.
    assert_eq!(em.content["meta"]["model"], "y-model");
    assert_eq!(em.content["meta"]["response_id"], "x-id");
}

// ───── T22: LlmCell::handle error-matrix (WireError → error_code + Gate-1) ─────
//
// Five end-to-end integration tests verifying every WireError variant maps
// to the correct UBF `error_code` AND that `messages` passes through
// unchanged (Gate-1 spec § 9). T11 already covers the wire-layer mapping;
// these tests prove the cell-level orchestration honors it for handle()
// Step 6's wire-error branch.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_rate_limit_emits_rate_limit_passthrough() {
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_error_status(429)]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": [user_turn.clone()]})))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let em = rx.recv().await.expect("cell must emit rate_limit error");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "rate_limit");
    assert_eq!(em.content["meta"]["error"]["source"], "wire");
    // Gate-1: messages pass-through unchanged.
    assert_eq!(em.content["messages"], json!([user_turn]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_unauthorized_emits_auth_passthrough() {
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_error_status(401)]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": [user_turn.clone()]})))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let em = rx.recv().await.expect("cell must emit auth error");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "auth");
    assert_eq!(em.content["meta"]["error"]["source"], "wire");
    assert_eq!(em.content["messages"], json!([user_turn]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_model_not_found_emits_model_not_found_passthrough() {
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_error_status(404)]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": [user_turn.clone()]})))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let em = rx
        .recv()
        .await
        .expect("cell must emit model_not_found error");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "model_not_found");
    assert_eq!(em.content["meta"]["error"]["source"], "wire");
    assert_eq!(em.content["messages"], json!([user_turn]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_http_500_emits_provider_error_passthrough() {
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_error_status(500)]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": [user_turn.clone()]})))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let em = rx.recv().await.expect("cell must emit provider_error");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "provider_error");
    assert_eq!(em.content["meta"]["error"]["source"], "wire");
    assert_eq!(em.content["messages"], json!([user_turn]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_handle_a_timeout_emits_timeout_passthrough_wallclock_fast() {
    // A-Timeout: mock delays response by 5s, cell's external_timeout_ms=200
    // fires first → WireError::Timeout → emit_error(timeout, source="wire").
    // Wallclock asserts the A-timeout actually fires fast (proves the cell
    // passes external_timeout_ms through to wire::call_openai).
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let slow = canned_chat_completion("late", "stop").with_delay(Duration::from_secs(5));
    let mock = MockOpenAI::start(vec![slow]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "k",
        "base_url": format!("{}/v1", mock.base_url),
        "external_timeout_ms": 200u64,
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, mut rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );

    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({"messages": [user_turn.clone()]})))
        .build();

    let started = std::time::Instant::now();
    cell.handle(msg, &sink, &mut conn).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "A-timeout must fire ~200ms, took {elapsed:?}"
    );

    let em = rx.recv().await.expect("cell must emit timeout error");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "timeout");
    assert_eq!(em.content["meta"]["error"]["source"], "wire");
    // Gate-1: messages pass-through unchanged.
    assert_eq!(em.content["messages"], json!([user_turn]));
}

// ───── T24: LlmCellFactory::spawn_cell (cell_dir-substrate + RespawnFn) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_factory_spawn_cell_opens_cell_db_at_cell_dir() {
    // Phase-13-K-2: LlmCellFactory now returns `Dormant`. Initial spawn opens
    // cell.db for the schema_version check (init-only), then closes it. The
    // WakeFn re-opens cell.db on first message → after `wake(receiver)` the
    // cell.db file must exist at cell_dir/cell.db, and the cell processes a
    // message end-to-end against MockOpenAI.
    use meclaw_cells::LlmCellFactory;
    use meclaw_colony::CellFactory;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, Path};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("llm");
    std::fs::create_dir_all(&cell_dir).unwrap();

    let raw_params = json!({
        "provider":"openai","model":"gpt-4o","api_key":"test-key",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let (out_tx, mut out_rx) = mpsc::channel(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = factory
        .spawn_cell(
            Path::new("/llm"),
            raw_params,
            out_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();

    // cell.db opened eagerly during the init schema-version check.
    assert!(
        cell_dir.join("cell.db").exists(),
        "cell.db must exist after spawn_cell (init schema_version check)"
    );

    let (sender, receiver, wake) = match spawned {
        meclaw_colony::SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            ..
        } => (sender, receiver, wake),
        meclaw_colony::SpawnedCellKind::Active { .. } => {
            unreachable!("Phase-13-K-2: LlmCellFactory returns Dormant")
        }
    };
    // Drive the Wake directly (no Colony available in this unit test).
    wake(receiver);

    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin":"user","type":"text","text":"hi"}]
        })))
        .build();
    sender.send(msg).await.unwrap();

    let em = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("emit timed out")
        .expect("channel closed");
    assert_eq!(em.target, Path::new("/observer"));
    assert_eq!(em.content["messages"][0]["text"], "ok");

    drop(sender);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_cell_factory_respawn_returns_working_cell() {
    // T24 RespawnFn smoke-test: after draining the initial cell, calling the
    // RespawnFn closure must yield a fresh (Sender, JoinHandle) pair whose
    // cell processes messages correctly. Mock serves two canned responses,
    // one per spawn.
    use meclaw_cells::LlmCellFactory;
    use meclaw_colony::CellFactory;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, Path};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let mock = MockOpenAI::start(vec![
        canned_chat_completion("first", "stop"),
        canned_chat_completion("second", "stop"),
    ])
    .await;
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("llm");
    std::fs::create_dir_all(&cell_dir).unwrap();

    let raw_params = json!({
        "provider":"openai","model":"gpt-4o","api_key":"test-key",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let (out_tx, mut out_rx) = mpsc::channel(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = factory
        .spawn_cell(
            Path::new("/llm"),
            raw_params,
            out_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();

    let (sender1, receiver1, wake1, respawn) = match spawned {
        meclaw_colony::SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            respawn,
            ..
        } => (sender, receiver, wake, respawn),
        meclaw_colony::SpawnedCellKind::Active { .. } => {
            unreachable!("Phase-13-K-2: LlmCellFactory returns Dormant")
        }
    };

    // Initial wake + send — verify it works.
    wake1(receiver1);
    let msg1 = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin":"user","type":"text","text":"hi"}]
        })))
        .build();
    sender1.send(msg1).await.unwrap();
    let em1 = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("first emit timed out")
        .expect("first channel closed");
    assert_eq!(em1.content["messages"][0]["text"], "first");

    // Close initial sender → cell task ends.
    drop(sender1);

    // Now respawn — fresh Sender + JoinHandle + peace_rx from the RespawnFn closure.
    let (s2, j2, _peace_rx2, _backstop_rx2) = (respawn)();
    let msg2 = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin":"user","type":"text","text":"again"}]
        })))
        .build();
    s2.send(msg2).await.unwrap();
    let em2 = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
        .await
        .expect("second emit timed out")
        .expect("second channel closed");
    assert_eq!(em2.content["messages"][0]["text"], "second");

    drop(s2);
    j2.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llm_cell_factory_spawn_cell_rejects_schema_version_mismatch() {
    // T24 init-only schema_version-check: pre-populate cell.db with a bumped
    // schema_version (simulate a future-Phase cell.db), then assert spawn_cell
    // errors out BEFORE any HTTP traffic. We deliberately pass no base_url so
    // any real call would attempt openai.com — failing earlier proves the
    // check fired first.
    use meclaw_cells::LlmCellFactory;
    use meclaw_colony::CellFactory;
    use meclaw_colony::persist::open_or_create_cell_db;
    use meclaw_core::Path;
    use meclaw_core::serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("llm");
    std::fs::create_dir_all(&cell_dir).unwrap();
    // Pre-populate cell.db with a bumped schema_version.
    {
        let conn = open_or_create_cell_db(&cell_dir.join("cell.db")).unwrap();
        conn.execute("UPDATE meta SET value='99' WHERE key='schema_version'", [])
            .unwrap();
    }
    let raw_params = json!({
        "provider":"openai","model":"gpt-4o","api_key":"k",
    });
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let (out_tx, _out_rx) = mpsc::channel(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let r = factory.spawn_cell(
        Path::new("/llm"),
        raw_params,
        out_tx,
        cell_dir,
        meclaw_colony::ContractView::default(),
        inbox_tx,
        None,
        0,
        None,
        None,
        1000,
    );
    let err = match r {
        Ok(_) => panic!("spawn_cell must reject mismatched schema_version"),
        Err(e) => e,
    };
    assert!(
        err.contains("schema") || err.contains("version"),
        "error must mention schema/version: {err}"
    );
}
