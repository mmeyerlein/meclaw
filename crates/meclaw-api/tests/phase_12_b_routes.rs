//! Phase 12-B T8: Integration-Tests fuer die /colony/*-HTTP-Handler.
//!
//! Pattern pro Test:
//! 1. `meclaw_testing::ColonyHandle::new()` startet eine echte Colony-Task
//!    (multi_thread, worker_threads=4 — implizit via #[tokio::test(...)]).
//! 2. `meclaw_api::ColonyHandle` wrappt `test_h.inbox_tx.clone()` plus einen
//!    Stub-`templates_root` (PathBuf::new() — kein Rescan-Driver hier).
//! 3. `build_router(api_colony)` baut den vollen Router; der Test feuert ein
//!    `oneshot::Request` via `tower::ServiceExt::oneshot` und asserted Status
//!    + JSON-Slot-Name.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Helper: baut einen `meclaw_api::ColonyHandle` aus einem laufenden
/// `meclaw_testing::ColonyHandle`. Beide leben im Test-Scope; Drop schliesst
/// die Inbox sauber.
fn api_colony_from(test_h: &meclaw_testing::ColonyHandle) -> Arc<meclaw_api::ColonyHandle> {
    Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    })
}

/// Helper: kombiniert `api_colony_from` mit einem ephemeren `DiskBlobStore`
/// (Phase-12-X T17). Returnt zusätzlich den `TempDir`-Owner, den der Caller
/// in einem `let _td = ...;`-Binding halten muss.
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

    // ohne ?scope → default "/" (alles).
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
    // ColonyHandle::new() startet eine Colony in einem frischen TempDir;
    // wir reichen denselben Pfad als templates_root weiter, damit der
    // Rescan-Arm einen existierenden (leeren) Ordner findet — apply_scan_result
    // toleriert leere Roots (loggt nur Warn) und ack'd.
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
    // Leere Queue → leerer Drain.
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
