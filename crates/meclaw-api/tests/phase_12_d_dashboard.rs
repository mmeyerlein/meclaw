//! Phase 12-D T21: integration tests for the `/ui/` dashboard + root redirect.
//!
//! The dashboard aggregates 3 reads (registry + dead letters + trace?error=true)
//! as server-rendered HTML — **explicitly not a consistent snapshot** (spec
//! l.468): the three reads are sequential ColonyMsg::Read* calls, and colony
//! state may move on between them.

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
        std::sync::Arc::new(meclaw_colony::SurfaceRegistry::new()),
    );
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
        body.contains("not a consistent snapshot"),
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

/// Where `GET /` sends a client that came through a path proxy.
async fn location_of(app: Router, prefix: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().uri("/");
    if let Some(p) = prefix {
        req = req.header("x-forwarded-prefix", p);
    }
    let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    (status, loc)
}

/// GH #685: the root redirect is the one URL the shell emits that a path
/// proxy has to be able to reach, so it moves with `X-Forwarded-Prefix` the
/// way the web shell's `<base href>` already does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_root_redirect_moves_with_a_forwarded_prefix() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);
    let (status, loc) = location_of(app, Some("/c/abc123")).await;
    assert!(status.is_redirection(), "expected 3xx, got {status}");
    assert_eq!(loc, "/c/abc123/ui/");
}

/// The same grammar the web cell reads the header with: a path, no trailing
/// slash, no protocol-relative `//`, no `..`, plain characters. Anything else
/// is ignored, not repaired -- the value came from whoever spoke HTTP to the
/// listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prefix_the_grammar_refuses_is_ignored() {
    let test_h = meclaw_testing::ColonyHandle::new();
    for bad in [
        "http://elsewhere",
        "c/abc",
        "/x/",
        "//evil.example",
        "/a/../b",
        "/sp ace",
    ] {
        let (app, _blob_td) = app_from(&test_h);
        let (status, loc) = location_of(app, Some(bad)).await;
        assert!(
            status.is_redirection(),
            "{bad:?}: expected 3xx, got {status}"
        );
        assert_eq!(loc, "/ui/", "{bad:?} was not ignored");
    }
}
