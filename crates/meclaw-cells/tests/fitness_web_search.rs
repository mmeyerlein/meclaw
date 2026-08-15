//! Track T (#104) — fitness battery for the `web_search` cell.
//!
//! Against a local mock endpoint (`meclaw_testing::mock_http`), LLM-free.
//! The cell is a generic `{results: [...]}` wrapper; the battery pins:
//!
//! - a conforming response yields `result_count` = len(results) and the raw
//!   body in `text`;
//! - a NON-conforming response is graceful by contract: `result_count` 0 and
//!   the body still passed through — never a hard error;
//! - the operation timeout (rule 12) is typed; missing `query` is
//!   `invalid_input`;
//! - the query travels as `?q=` and the api_key as a Bearer header.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::WebSearchCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use meclaw_testing::mock_http::{MockResponse, start_mock_server, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use support::{ToolRig, assert_error, assert_normal_result, header_of, text_of};

fn rig_at(endpoint: String, extra: Option<(&str, meclaw_core::JsonValue)>) -> ToolRig {
    let mut params = json!({
        "endpoint": endpoint,
        "max_concurrency": 2,
        "external_timeout_ms": 10000
    });
    if let Some((k, v)) = extra {
        params.as_object_mut().unwrap().insert(k.into(), v);
    }
    ToolRig::spawn(
        Arc::new(WebSearchCellFactory) as Arc<dyn CellFactory>,
        "/search",
        params,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conforming_response_reports_the_result_count() {
    let body = json!({"results": [
        {"title": "One", "url": "http://a", "snippet": "s1"},
        {"title": "Two", "url": "http://b", "snippet": "s2"},
        {"title": "Three", "url": "http://c", "snippet": "s3"}
    ]})
    .to_string();
    let (addr, _srv) = start_mock_server(MockResponse::ok_json(body.as_bytes())).await;
    let mut r = rig_at(format!("http://{addr}/search"), None);

    let em = r.call(json!({"query": "meclaw fitness"}), "c1").await;

    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "operation"), "web_search");
    assert_eq!(header_of(&em, "result_count"), 3);
    assert_eq!(text_of(&em), body, "the raw provider body is the text");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_conforming_response_is_graceful_count_zero_body_passed_through() {
    // Contract (cell-types.md § web_search, phase-7 conventions): a
    // non-conforming response is graceful, never a hard error.
    let (addr, _srv) =
        start_mock_server(MockResponse::ok_json(br#"{"unexpected": "shape"}"#)).await;
    let mut r = rig_at(format!("http://{addr}/search"), None);

    let em = r.call(json!({"query": "anything"}), "c1").await;

    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "result_count"), 0);
    assert_eq!(
        text_of(&em),
        r#"{"unexpected": "shape"}"#,
        "the body is passed through even when the shape is foreign"
    );

    // Same for a body that is not JSON at all.
    let (addr2, _srv2) = start_mock_server(MockResponse::ok(b"<html>not json</html>")).await;
    let mut r2 = rig_at(format!("http://{addr2}/search"), None);
    let em = r2.call(json!({"query": "anything"}), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(header_of(&em, "result_count"), 0);
    assert_eq!(text_of(&em), "<html>not json</html>");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_query_travels_as_q_and_the_key_as_bearer() {
    let (addr, _srv, captured) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"results": []}"#)]).await;
    let mut r = rig_at(
        format!("http://{addr}/search"),
        Some(("api_key", json!("sekret-token"))),
    );

    let em = r.call(json!({"query": "rust testing"}), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "result_count"), 0, "empty result list is 0");

    let reqs = captured.lock().await;
    assert_eq!(reqs.len(), 1);
    assert!(
        reqs[0].path.contains("q=rust") && reqs[0].path.contains("testing"),
        "the query is URL-encoded into ?q=: {}",
        reqs[0].path
    );
    assert_eq!(
        reqs[0].headers.get("authorization").map(String::as_str),
        Some("Bearer sekret-token"),
        "the api_key travels as a Bearer header"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_provider_hits_the_typed_operation_timeout() {
    let (addr, _srv) = start_mock_server(
        MockResponse::ok_json(br#"{"results": []}"#).with_delay(Duration::from_secs(20)),
    )
    .await;
    let mut r = ToolRig::spawn(
        Arc::new(WebSearchCellFactory) as Arc<dyn CellFactory>,
        "/search",
        json!({
            "endpoint": format!("http://{addr}/search"),
            "max_concurrency": 1,
            "external_timeout_ms": 300
        }),
    );

    let started = std::time::Instant::now();
    let em = r.call(json!({"query": "late"}), "c1").await;
    assert_error(&em, "timeout");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_or_empty_query_is_invalid_input() {
    let (addr, _srv) = start_mock_server(MockResponse::ok_json(br#"{"results": []}"#)).await;
    let mut r = rig_at(format!("http://{addr}/search"), None);

    let em = r.call(json!({}), "c1").await;
    assert_error(&em, "invalid_input");

    let em = r.call(json!({"query": ""}), "c2").await;
    assert_error(&em, "invalid_input");
}
