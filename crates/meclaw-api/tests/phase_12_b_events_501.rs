//! Phase-12-B T11: /colony/events returns 501 (Phase-14-Defer),
//! NOT axum-404 — the endpoint is known but deferred.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_colony_events_returns_501_not_implemented() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "deferred");
    assert_eq!(json["phase"], 14);
    assert!(json["spec"].as_str().unwrap().contains("Phase 14"));
}
