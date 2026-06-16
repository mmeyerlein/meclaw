//! Phase 12-D T22: Integration-Tests fuer `/ui/registry`.
//!
//! Server-rendered HTML-Tabelle der Cells + GET-Form fuer Filter
//! (path_prefix, type, limit). Verwendet dieselbe ColonyMsg::ReadRegistry
//! wie der JSON-Handler aus 12-B.

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
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
    (app, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_registry_returns_html_with_filter_form() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/registry")
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
    // Filter-Form mit allen drei Inputs.
    assert!(
        body.contains("<form") && body.contains("method=\"get\""),
        "expected GET-form"
    );
    assert!(body.contains("action=\"/ui/registry\""));
    assert!(body.contains("name=\"path_prefix\""));
    assert!(body.contains("name=\"type\""));
    assert!(body.contains("name=\"limit\""));
    // Leerer State -> Empty-Hint.
    assert!(body.contains("Keine Cells"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_registry_filter_values_round_trip_into_form() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/registry?path_prefix=/main&type=echo&limit=25")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    // Eingegebene Werte erscheinen als `value=`-Defaults im Form.
    assert!(body.contains("value=\"/main\""));
    assert!(body.contains("value=\"echo\""));
    assert!(body.contains("value=\"25\""));
}
