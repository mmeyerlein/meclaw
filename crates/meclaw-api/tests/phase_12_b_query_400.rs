//! Phase-12-B T13: 400 Bad Request mapping fuer malformed Query-Parameter
//! (UUIDs, Integers). Spec Z.1660: 422 bleibt Mutation-Reject-Write-only.
//! Read-Filter-Fehler sind 400.
//!
//! Drei Tests:
//! 1. `?trace_id=not-a-uuid` → 400 mit JSON-Error-Body `{error: "bad_query"}`
//!    (custom Mapping in `handlers/trace.rs`, T8.6).
//! 2. `?correlation_id=also-not-a-uuid` → analog.
//! 3. `?limit=not-a-number` → 400 (free win von axum's `Query<T>`-Extractor;
//!    Body-Shape ist axum-default Text, nicht unser JSON — fuer T13's Zweck
//!    akzeptabel, in Phase-14-Backlog dokumentiert).

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

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
async fn trace_invalid_uuid_returns_400_bad_query() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/trace?trace_id=not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "bad_query");
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("uuid")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_invalid_correlation_id_returns_400_bad_query() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/trace?correlation_id=also-not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "bad_query");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_invalid_limit_returns_400_bad_query() {
    // axum's `Query<T>`-Extractor returnt 400 bei integer-parse-fail fuer
    // `limit=not-a-number`. Body-Format ist axum-default (Text), nicht unser
    // JSON-Shape — fuer T13's Zweck ist die 400 das wesentliche Signal.
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/registry?limit=not-a-number")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
