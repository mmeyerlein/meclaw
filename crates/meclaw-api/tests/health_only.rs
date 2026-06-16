//! Phase-12-A TDD-Anker: build_router liefert genau /health, alles andere axum-404.
//! Bewusst KEIN /colony/events-501 in 12-A (das kommt in 12-B).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use meclaw_api::router::build_router;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_returns_200_and_unknown_routes_404() {
    // build_router nimmt einen ColonyHandle — für 12-A reicht ein minimaler
    // Stub-Sender, weil /health Colony nicht anfasst. Echter ColonyHandle in 12-B.
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: tx,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app = build_router(colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

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

    // Phase 12-A asserted 404 auf /colony/registry. Ab Phase 12-B T8.1 ist die
    // Route registriert (200); das 404-Probe-Pattern bleibt aber wertvoll —
    // jetzt gegen einen wirklich unbekannten Pfad.
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
