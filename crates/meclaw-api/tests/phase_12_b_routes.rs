//! Phase 12-B T8: integration tests for the /colony/* HTTP handlers.
//!
//! Pattern per test:
//! 1. `meclaw_testing::ColonyHandle::new()` starts a real colony task
//!    (multi_thread, worker_threads=4 — implicit via #[tokio::test(...)]).
//! 2. `meclaw_api::ColonyHandle` wraps `test_h.inbox_tx.clone()` plus a stub
//!    `templates_root` (PathBuf::new() — no rescan driver here).
//! 3. `build_router(api_colony, blob_store, ttl)` builds the full router; the test fires a
//!    `oneshot::Request` via `tower::ServiceExt::oneshot` and asserts the status
//!    plus the JSON slot name.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Helper: builds a `meclaw_api::ColonyHandle` from a running
/// `meclaw_testing::ColonyHandle`. Both live in the test scope; drop closes the
/// inbox cleanly.
fn api_colony_from(test_h: &meclaw_testing::ColonyHandle) -> Arc<meclaw_api::ColonyHandle> {
    Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    })
}

/// Helper: combines `api_colony_from` with an ephemeral `DiskBlobStore`
/// (phase-12-X T17). Also returns the `TempDir` owner, which the caller must
/// hold in a `let _td = ...;` binding.
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
async fn get_mutations_returns_empty_with_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/mutations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json
        .get("mutations")
        .expect("response has 'mutations' slot");
    assert_eq!(slot.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_graph_default_scope_returns_full_schema() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    // without ?scope → default "/" (everything).
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/graph")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json.get("graph").expect("response has 'graph' slot");
    assert_eq!(slot["scope"].as_str().unwrap(), "/");
    assert_eq!(slot["graph_version"].as_u64().unwrap(), 0);
    assert!(slot["nodes"].is_array());
    assert!(slot["edges"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_graph_with_scope_echoes_scope() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/graph?scope=/main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["graph"]["scope"].as_str().unwrap(), "/main");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_trace_returns_empty_with_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/trace")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json.get("trace").expect("response has 'trace' slot");
    assert_eq!(slot.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_trace_bad_trace_id_returns_400() {
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
    assert_eq!(json["error"].as_str().unwrap(), "bad_query");
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .contains("trace_id is not a valid UUID")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_trace_bad_correlation_id_returns_400() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/trace?correlation_id=ZZZ")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"].as_str().unwrap(), "bad_query");
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .contains("correlation_id is not a valid UUID")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_templates_rescan_returns_ok_slot() {
    // ColonyHandle::new() starts a colony in a fresh TempDir; we pass the same
    // path on as templates_root so the rescan arm finds an existing (empty)
    // directory — apply_scan_result tolerates empty roots (it only logs a warn)
    // and acks.
    let test_h = meclaw_testing::ColonyHandle::new();
    let templates_root = test_h.tempdir_path().to_path_buf();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root,
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/colony/templates/rescan")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json.get("rescan").expect("response has 'rescan' slot");
    assert_eq!(slot["status"].as_str().unwrap(), "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_templates_returns_empty_with_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/templates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json
        .get("templates")
        .expect("response has 'templates' slot");
    assert_eq!(slot.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_dead_letters_returns_drained_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/colony/dead_letters")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json
        .get("dead_letters")
        .expect("response has 'dead_letters' slot");
    // Empty queue → empty drain.
    assert_eq!(slot.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_dead_letters_returns_empty_with_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/dead_letters")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json
        .get("dead_letters")
        .expect("response has 'dead_letters' slot");
    assert_eq!(slot.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_registry_returns_empty_with_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/registry")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json.get("registry").expect("response has 'registry' slot");
    assert_eq!(slot.as_array().unwrap().len(), 0);
}

/// GH #267 (ruling Q14): the HTTP twin of the `/colony/ledger` message
/// endpoint. Both doors have to exist — an endpoint with only one of them is a
/// spec violation, not a smaller surface (`docs/meclaw-overview.md` § `/colony`,
/// "Symmetrie interne API ↔ externe API").
///
/// The window `?since=0` covers everything the freshly booted colony has, which
/// is nothing: the point is that `messages.total` is **present** (a read that
/// happened and counted zero), not that it is non-zero. A read that could not
/// happen has no `messages` slot at all — that is the `unavailable` shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ledger_endpoint_answers_over_http() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/ledger?since=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let slot = json.get("ledger").expect("response has 'ledger' slot");
    // The aggregate slot holds an OBJECT, not a list (unlike its four siblings).
    assert!(slot.as_array().is_none(), "ledger slot is not a list");
    assert!(
        slot["messages"]["total"].is_u64(),
        "ledger carries messages.total, got {slot}"
    );
    // The echoed query shows which question was answered, not which was asked.
    assert_eq!(slot["query"]["since"].as_i64(), Some(0));
    assert_eq!(slot["scan_truncated"].as_bool(), Some(false));
    assert!(slot.get("unavailable").is_none(), "the read happened");
}
