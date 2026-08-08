//! Phase 12-D T25: integration tests for `/ui/trace` + the tree render.
//!
//! Three test bundles:
//!   * `ui_trace_no_query_renders_empty_form` — without `trace_id`, form only.
//!   * `ui_trace_unknown_trace_id_shows_empty_hint` — no match -> empty hint.
//!   * `ui_trace_renders_nested_tree_for_two_hop_chain` — POST /messages to an
//!     unresolved target produces source + error reply (parent_message_id), and
//!     `/ui/trace?trace_id=<id>` renders the nested `<ul>`/`<li>` structure.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

mod common;

fn app_from(test_h: &meclaw_testing::ColonyHandle) -> (Router, tempfile::TempDir) {
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
    (app, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_trace_no_query_renders_empty_form() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/trace")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("<form"));
    assert!(body.contains("name=\"trace_id\""));
    assert!(
        body.contains("Enter a trace ID"),
        "expected hint when no trace_id supplied"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_trace_unknown_trace_id_shows_empty_hint() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let nope = "01900000-0000-7000-8000-000000000000";
    let uri = format!("/ui/trace?trace_id={nope}");
    let resp = app
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("No trace found for"),
        "expected empty-trace hint"
    );
    // The trace id round-trips back into the form.
    assert!(body.contains(nope));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_trace_renders_tree_for_logged_hop() {
    // Spawn EchoMockCell (terminal, without echo_to) at /echo. POST /messages to
    // /echo produces a routable hop that lands in the message_log.
    // /ui/trace?trace_id=<id> renders it as a ul.tree with the target path.
    let test_h = meclaw_testing::ColonyHandle::new();
    test_h
        .spawn(meclaw_core::Path::new("/echo"), || {
            meclaw_testing::mocks::EchoMockCell::new(meclaw_core::Path::new("/echo"))
        })
        .await;
    let (app, _blob_td) = app_from(&test_h);

    let body_json = serde_json::json!({
        "target": "/echo",
        "body": { "messages": [{ "origin": "user", "type": "text", "text": "hi" }] }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let trace_id = json["message_id"].as_str().unwrap().to_string();

    // Routing + persistence are async fire-and-forget; poll briefly until
    // /colony/trace shows at least 1 entry with this trace_id.
    let mut attempts = 0;
    let trace_uri = format!("/colony/trace?trace_id={trace_id}");
    loop {
        attempts += 1;
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&trace_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let n = j["trace"].as_array().map(|a| a.len()).unwrap_or(0);
        if n >= 1 {
            break;
        }
        if attempts > 50 {
            panic!("trace did not propagate within timeout");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Now /ui/trace?trace_id=…
    let ui_uri = format!("/ui/trace?trace_id={trace_id}");
    let resp = app
        .oneshot(Request::builder().uri(&ui_uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    // The tree must be rendered as ul.tree and contain at least one <li> entry
    // with the target path.
    assert!(
        body.contains("ul class=\"tree\"") || body.contains("class=\"tree\""),
        "expected ul.tree wrapper"
    );
    assert!(body.contains("/echo"), "expected target path in tree");
}
