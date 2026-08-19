//! W4 (Audit-Ruling A4): provider-attribution params → HTTP request headers.
//!
//! The attribution params (`http_referer`, `x_title`) are regular params; the
//! Translate boundary maps them to wire HTTP headers (not the request body).
//! These tests pin: (a) set attribution → headers on the wire; (b) unset →
//! no headers; (e) Authorization stays the single auth header and params can
//! NOT override it; plus the full-cell path (params → wire headers) with the
//! body-params left untouched (d).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::wire::call_openai;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::time::Duration;

// ───── (a) set attribution → headers on the wire ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_request_carries_attribution_headers() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    let extra = vec![
        (
            "HTTP-Referer".to_string(),
            "https://example.com".to_string(),
        ),
        ("X-Title".to_string(), "Example App".to_string()),
    ];
    call_openai(
        &client,
        &url,
        Some("test-key"),
        &extra,
        &body,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    // Headers are lowercased by the capturing listener.
    assert_eq!(
        snaps[0].headers.get("http-referer").map(|s| s.as_str()),
        Some("https://example.com")
    );
    assert_eq!(
        snaps[0].headers.get("x-title").map(|s| s.as_str()),
        Some("Example App")
    );
}

// ───── (b) unset attribution → no header noise ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_request_no_attribution_headers_when_empty() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    call_openai(
        &client,
        &url,
        Some("test-key"),
        &[],
        &body,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    assert!(
        !snaps[0].headers.contains_key("http-referer"),
        "no http-referer header when attribution unset"
    );
    assert!(
        !snaps[0].headers.contains_key("x-title"),
        "no x-title header when attribution unset"
    );
}

// ───── (e) Authorization is the single auth header; params can't override ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_authorization_cannot_be_overridden_by_extra_header() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi", "stop")]).await;
    let url = format!("{}/v1/chat/completions", mock.base_url);
    let client = reqwest::Client::builder().build().unwrap();
    let body = serde_json::json!({"model": "gpt-4o"});
    // An extra header trying to clobber Authorization must be ignored.
    let extra = vec![("Authorization".to_string(), "Bearer evil".to_string())];
    call_openai(
        &client,
        &url,
        Some("real-key"),
        &extra,
        &body,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    assert_eq!(
        snaps[0].headers.get("authorization").map(|s| s.as_str()),
        Some("Bearer real-key"),
        "Authorization must remain the api_key Bearer, never the injected value"
    );
}

// ───── full cell: params → wire headers, body-params untouched (a + d + e) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_handle_passes_attribution_params_to_wire_and_keeps_body_clean() {
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
        "api_key": "test-key-W4",
        "base_url": format!("{}/v1", mock.base_url),
        "http_referer": "https://example.com",
        "x_title": "Example App",
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let mut cell = LlmCell::new(params, http);

    let td = TempDir::new().unwrap();
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(raw_conn, None);
    let (tx, _rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
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
            "messages": [{"origin":"user","type":"text","text":"Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    // (a) attribution params reached the wire as headers.
    assert_eq!(
        snaps[0].headers.get("http-referer").map(|s| s.as_str()),
        Some("https://example.com")
    );
    assert_eq!(
        snaps[0].headers.get("x-title").map(|s| s.as_str()),
        Some("Example App")
    );
    // (e) Authorization is the single auth header, equal to the api_key Bearer.
    assert_eq!(
        snaps[0].headers.get("authorization").map(|s| s.as_str()),
        Some("Bearer test-key-W4")
    );
    // (d) body-params unchanged AND attribution did NOT leak into the body.
    assert_eq!(snaps[0].model(), Some("gpt-4o"));
    assert_eq!(snaps[0].temperature(), Some(0.7));
    assert_eq!(
        snaps[0].body.get("max_tokens").and_then(|v| v.as_u64()),
        Some(4096)
    );
    assert!(
        snaps[0].body.get("http_referer").is_none(),
        "attribution param must NOT appear in the request body"
    );
    assert!(
        snaps[0].body.get("x_title").is_none(),
        "attribution param must NOT appear in the request body"
    );
}
