//! P1 (message browser): `GET /ui/messages` — the list view.
//!
//! Server-rendered, no JS, read-only. Filter form round-trips its values, the
//! table links every row into the detail view, and long payloads are truncated
//! here (full render belongs to the detail page).

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_messages_renders_filter_form_and_empty_hint() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/messages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;

    assert!(body.contains("<form"));
    for field in [
        "to_path_prefix",
        "from_path_prefix",
        "trace_id",
        "correlation_id",
        "body_kind",
        "since",
        "until",
        "limit",
    ] {
        assert!(
            body.contains(&format!("name=\"{field}\"")),
            "filter field {field} missing"
        );
    }
    assert!(
        body.contains("No messages for this filter."),
        "empty hint expected"
    );
    assert!(
        body.contains("href=\"/ui/messages\""),
        "nav must link the new page"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_messages_round_trips_filter_values_into_the_form() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/messages?to_path_prefix=/mem&body_kind=inline&limit=25")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;
    assert!(body.contains("value=\"/mem\""), "to_path_prefix preserved");
    assert!(body.contains("value=\"inline\""), "body_kind preserved");
    assert!(body.contains("value=\"25\""), "limit preserved");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_messages_rejects_invalid_uuid_with_400() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/messages?trace_id=nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_messages_lists_routed_message_with_detail_link_and_truncated_payload() {
    let test_h = meclaw_testing::ColonyHandle::new();
    test_h
        .spawn(meclaw_core::Path::new("/echo"), || {
            meclaw_testing::mocks::EchoMockCell::new(meclaw_core::Path::new("/echo"))
        })
        .await;
    let (app, _blob_td) = app_from(&test_h);

    // A payload well beyond the list-view truncation limit.
    let long_text = "z".repeat(500);
    let body_json = serde_json::json!({
        "target": "/echo",
        "body": { "messages": [{ "origin": "user", "type": "text", "text": long_text }] }
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

    // Routing + persistence are async: poll the JSON endpoint until the row lands.
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
        if !j["messages"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            break;
        }
        assert!(attempt < 49, "message did not reach message_log in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/messages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;

    assert!(body.contains("/echo"), "target path rendered");
    assert!(
        body.contains("/ui/message?id="),
        "every row links into the detail view"
    );
    assert!(
        !body.contains(&"z".repeat(200)),
        "list view must truncate long payloads"
    );
    assert!(body.contains('…'), "truncation marker present");
}
