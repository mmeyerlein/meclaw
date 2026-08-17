//! Phase 12-D T24: integration tests for `/ui/dead_letters`.
//!
//! Pure read of the in-memory DL queue (`drain=false`). Rendered as an HTML list
//! with `error_code` + path + body preview (truncated).

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

fn app_from(test_h: &meclaw_testing::ColonyHandle) -> (Router, tempfile::TempDir) {
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        meclaw_api::router::SurfaceState::disabled(),
    );
    (app, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_dead_letters_renders_html_empty_hint() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/dead_letters")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.starts_with("text/html"));

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("Dead Letters"));
    assert!(body.contains("No dead letters."));
}

/// P1 (message browser): every dead letter links back to the message it came
/// from — message-exact when the persisted envelope carries an `id`, otherwise
/// trace-exact. Never an empty link, never a missing column.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_dead_letters_links_into_message_browser() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    // POST to an unresolvable target -> the message dead-letters.
    let body_json = serde_json::json!({
        "target": "/nowhere",
        "body": { "messages": [{ "origin": "user", "type": "text", "text": "hi" }] }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header("content-type", "application/json")
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // The DLQ write is async; poll the JSON endpoint until the entry lands.
    for attempt in 0..50 {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/colony/dead_letters")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if !j["dead_letters"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            break;
        }
        assert!(attempt < 49, "dead letter did not land in time");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/dead_letters")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();

    assert!(body.contains("Original"), "the origin column exists");
    // The persisted envelope carries `id`, so this must be the message-exact
    // link — the trace-level fallback is for legacy rows only.
    assert!(
        body.contains("/ui/message?id="),
        "dead letter links to the exact origin message, not just its trace"
    );
    assert!(!body.contains("href=\"\""), "no empty link");
}
