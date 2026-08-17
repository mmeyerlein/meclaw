//! Phase-12-A TDD anchor: build_router serves exactly /health, everything else
//! is an axum 404. Deliberately NO /colony/events 501 in 12-A (that arrives in 12-B).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use meclaw_api::router::build_router;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_returns_200_and_unknown_routes_404() {
    // build_router takes a ColonyHandle — for 12-A a minimal stub sender suffices,
    // since /health never touches colony. A real ColonyHandle arrives in 12-B.
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: tx,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app = build_router(
        colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        meclaw_api::router::SurfaceState::disabled(),
    );

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    // Phase 12-A asserted a 404 on /colony/registry. From phase 12-B T8.1 the
    // route is registered (200); the 404 probe pattern stays valuable though —
    // now against a genuinely unknown path.
    let unknown = app
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}
