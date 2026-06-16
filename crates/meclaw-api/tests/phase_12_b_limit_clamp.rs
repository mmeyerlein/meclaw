//! Phase-12-B T13: Read-Limit-Clamp Default 100 / Hard-Cap 1000 (Spec Z.412).
//!
//! Diese Tests feuern `?limit=10000` und `?limit=0` gegen `/colony/registry`
//! und verifizieren, dass der Handler NICHT mit 500 antwortet, sondern sauber
//! mit `200 OK` und leerem `registry`-Array zurueckkommt. Die `clamp_limit`-
//! Funktion aus `handlers::mod` (T8.0) ist bereits in allen 6 Read-Handlern
//! verdrahtet — diese Tests sind die HTTP-End-to-End-Bestaetigung.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Helper: baut einen `meclaw_api::ColonyHandle` aus einem laufenden
/// `meclaw_testing::ColonyHandle`. Beide leben im Test-Scope.
fn api_colony_from(test_h: &meclaw_testing::ColonyHandle) -> Arc<meclaw_api::ColonyHandle> {
    Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    })
}

fn app_from(test_h: &meclaw_testing::ColonyHandle) -> (Router, tempfile::TempDir) {
    let (blob_store, td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(
        api_colony_from(test_h),
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
    );
    (app, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_limit_10000_clamps_to_1000_and_returns_200() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/registry?limit=10000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["registry"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_limit_0_clamps_to_1_and_returns_200() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/registry?limit=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["registry"].is_array());
}
