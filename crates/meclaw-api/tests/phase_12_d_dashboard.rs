//! Phase 12-D T21: Integration-Tests fuer `/ui/` Dashboard + Root-Redirect.
//!
//! Dashboard aggregiert 3 Reads (Registry + DeadLetters + Trace?error=true) als
//! server-rendered HTML — **explizit kein konsistenter Snapshot** (Spec Z.468):
//! die drei Reads sind sequentielle ColonyMsg::Read*-Calls, zwischen denen
//! der Colony-State weiterlaufen darf.

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
async fn ui_dashboard_returns_html_with_three_sections() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(Request::builder().uri("/ui/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("text/html"),
        "expected text/html, got {ct:?}"
    );

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("<!DOCTYPE html>") || body.contains("<!doctype html>"));
    assert!(body.contains("Cells"), "dashboard missing 'Cells' section");
    assert!(
        body.contains("Dead Letters"),
        "dashboard missing 'Dead Letters' section"
    );
    assert!(
        body.contains("Recent Errors"),
        "dashboard missing 'Recent Errors' section"
    );
    assert!(
        body.contains("kein konsistenter Snapshot"),
        "dashboard missing snapshot disclaimer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn root_redirects_to_ui_dashboard() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_redirection(),
        "expected redirection 3xx, got {status}"
    );
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(loc, "/ui/");
}
