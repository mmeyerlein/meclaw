//! P1 (message browser): `GET /ui/message?id=…` — the detail view.
//!
//! The inspection value of the browser lives here: both header compartments
//! rendered apart, the full payload, blob bodies on demand only, and every
//! pivot (trace, parent chain, children, correlation, registry) as a link.

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

async fn body_of(resp: axum::response::Response) -> String {
    let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// POST a message to `/echo` and return the first logged row as JSON.
async fn route_one_message(app: &Router) -> serde_json::Value {
    let body_json = serde_json::json!({
        "target": "/echo",
        "body": { "messages": [{ "origin": "user", "type": "text", "text": "hallo welt" }] }
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

    for attempt in 0..50 {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/colony/messages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(first) = j["messages"].as_array().and_then(|a| a.first()) {
            return first.clone();
        }
        assert!(attempt < 49, "message did not reach message_log in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    unreachable!()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_message_detail_shows_headers_payload_and_all_pivots() {
    let test_h = meclaw_testing::ColonyHandle::new();
    test_h
        .spawn(meclaw_core::Path::new("/echo"), || {
            meclaw_testing::mocks::EchoMockCell::new(meclaw_core::Path::new("/echo"))
        })
        .await;
    let (app, _blob_td) = app_from(&test_h);

    let row = route_one_message(&app).await;
    let id = row["id"].as_str().expect("row id").to_string();
    let trace_id = row["trace_id"].as_str().expect("trace id").to_string();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ui/message?id={id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;

    // Both header compartments, visibly apart.
    assert!(body.contains("context"), "context compartment rendered");
    assert!(body.contains("hop"), "hop compartment rendered");

    // Full payload, not the truncated preview.
    assert!(body.contains("hallo welt"), "payload rendered");

    // Pivots.
    assert!(
        body.contains(&format!("/ui/trace?trace_id={trace_id}")),
        "trace pivot"
    );
    assert!(
        body.contains(&format!("/ui/messages?parent_message_id={id}")),
        "children pivot"
    );
    assert!(body.contains("/ui/registry?path_prefix="), "registry pivot");
    assert!(body.contains("/ui/messages"), "back to the list");

    // No empty links anywhere on the page.
    assert!(!body.contains("href=\"\""), "no empty pivot link");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_message_unknown_id_renders_not_found_hint() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let uri = "/ui/message?id=01900000-0000-7000-8000-000000000000";
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;
    assert!(body.contains("Keine Message gefunden"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_message_invalid_id_returns_400() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/message?id=nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Read-only receipt for the UI routes this package added.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_browser_ui_routes_reject_write_methods() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    for (method, uri) in [
        ("POST", "/ui/messages"),
        ("DELETE", "/ui/messages"),
        ("POST", "/ui/message"),
        ("DELETE", "/ui/message"),
        ("PUT", "/ui/message"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri} must not be bound"
        );
    }
}

/// Blob bodies are a click, not a side effect of opening the page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_message_offers_blob_as_an_explicit_link() {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_inline_max_bytes": 64}"#,
    )
    .expect("write colony.json");

    let test_h = meclaw_testing::ColonyHandle::new_with_blobs_at(&td, vec![]);
    test_h
        .spawn(meclaw_core::Path::new("/echo"), || {
            meclaw_testing::mocks::EchoMockCell::new(meclaw_core::Path::new("/echo"))
        })
        .await;
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let blob_store = Arc::new(
        meclaw_colony::blob::DiskBlobStore::new(td.path().join("blobs")).expect("blob store"),
    );
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    let marker = "q".repeat(4096);
    let body_json = serde_json::json!({
        "target": "/echo",
        "body": { "messages": [{ "origin": "user", "type": "text", "text": marker }] }
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

    let mut id = None;
    for _ in 0..50 {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/colony/messages?body_kind=blob")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(first) = j["messages"].as_array().and_then(|a| a.first()) {
            id = first["id"].as_str().map(|s| s.to_string());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let id = id.expect("blob row present");

    // Without ?blob=1: link offered, content not loaded.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ui/message?id={id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_of(resp).await;
    // maud escapes `&` in attributes — the correct serialization is `&amp;`.
    assert!(
        body.contains(&format!("/ui/message?id={id}&amp;blob=1")),
        "blob resolution is offered as an explicit link"
    );
    assert!(
        !body.contains(&"q".repeat(200)),
        "blob content must NOT be loaded without the click"
    );

    // With ?blob=1: content resolved.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ui/message?id={id}&blob=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_of(resp).await;
    assert!(
        body.contains(&"q".repeat(200)),
        "resolved blob body is rendered on request"
    );
}
