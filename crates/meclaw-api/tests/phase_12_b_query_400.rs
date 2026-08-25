//! Phase-12-B T13: 400 Bad Request mapping for malformed query parameters
//! (UUIDs, integers). Spec l.1660: 422 stays mutation-reject-write-only.
//! Read filter errors are 400.
//!
//! Three tests:
//! 1. `?trace_id=not-a-uuid` → 400 with the JSON error body `{error: "bad_query"}`
//!    (custom mapping in `handlers/trace.rs`, T8.6).
//! 2. `?correlation_id=also-not-a-uuid` → analogous.
//! 3. `?limit=not-a-number` → 400 (a free win from axum's `Query<T>` extractor;
//!    the body shape is axum's default text, not our JSON — acceptable for T13's
//!    purpose, documented in the phase-14 backlog).

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
        meclaw_api::router::SurfaceState::disabled(),
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
    // axum's `Query<T>` extractor returns 400 on an integer parse failure for
    // `limit=not-a-number`. The body format is axum's default (text), not our
    // JSON shape — for T13's purpose the 400 is the essential signal.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ledger_unknown_group_by_returns_400_bad_query() {
    // GH #267: the HTTP door must not be a wider door than the message door.
    // `group_by` accepts exactly "model"; anything else is refused before the
    // read runs, the same refusal the EDA endpoint takes at parse time —
    // translated into this surface's own 400 shape.
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/ledger?group_by=cell_type")
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
        json["detail"].as_str().unwrap().contains("group_by"),
        "detail names the field, got {json}"
    );
    // A refusal carries no answer: no counts, no echoed query.
    assert!(json.get("ledger").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ledger_overlong_cycle_id_returns_400_bad_query() {
    // A filtering value is never truncated — a shortened `cycle_id` would
    // answer a different question, so 65 characters are refused, not clamped.
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let long = "c".repeat(65);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/colony/ledger?cycle_id={long}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "bad_query");
    assert!(json["detail"].as_str().unwrap().contains("cycle_id"));
}
