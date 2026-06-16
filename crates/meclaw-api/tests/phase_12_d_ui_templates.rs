//! Phase 12-D T26: Integration-Tests fuer `/ui/templates`.
//!
//! Server-rendered Templates-Tabelle + Filter-Form. Wrappt
//! `ColonyMsg::ReadTemplates` analog zum 12-B-JSON-Handler.

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
async fn ui_templates_renders_html_with_filter_form() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/templates")
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
    assert!(body.contains("<form") && body.contains("action=\"/ui/templates\""));
    assert!(body.contains("name=\"name\""));
    assert!(body.contains("name=\"type\""));
    assert!(body.contains("Templates"));
    // Leerer Registry-State.
    assert!(body.contains("Keine Templates"));
}
