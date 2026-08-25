//! P1 (message browser): `GET /colony/messages` — the JSON read surface over
//! `colony.db::message_log`.
//!
//! Read-only by construction: the tests below pin the happy path, the 400 on a
//! malformed UUID, and — as a standing receipt — that no write method is bound
//! on any route this package added.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;
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
async fn colony_messages_returns_messages_slot() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/colony/messages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json["messages"].is_array(),
        "slot name per spec convention: messages"
    );
    assert_eq!(json["scan_truncated"], serde_json::json!(false));
    assert_eq!(
        json["scan_budget"],
        serde_json::json!(5000),
        "default scan budget is disclosed to the client"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn colony_messages_rejects_invalid_uuid_with_400() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    for field in [
        "id",
        "trace_id",
        "parent_message_id",
        "correlation_id",
        "before_id",
    ] {
        // before_id only takes effect together with before_created_at.
        let uri = format!("/colony/messages?before_created_at=1&{field}=not-a-uuid");
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{field} must be rejected"
        );
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"], serde_json::json!("bad_query"));
        let detail = json["detail"].as_str().expect("detail present");
        assert!(detail.contains(field), "detail names the field: {detail}");
        assert!(
            !detail.contains("not-a-uuid"),
            "the rejected value must never be echoed back: {detail}"
        );
    }
}

/// Read-only receipt: the message browser adds no side-effecting path. Any write
/// method on a P1 route must be unbound (405), not silently accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_browser_routes_reject_write_methods() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    for (method, uri) in [
        ("POST", "/colony/messages"),
        ("DELETE", "/colony/messages"),
        ("PUT", "/colony/messages"),
        ("PATCH", "/colony/messages"),
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri} must not be bound"
        );
    }
}

/// Blob-kind bodies are NOT fetched unless the caller asks. The colony's
/// auto-offload (`route_with_log` → `offload_oversized`) produces a real
/// `body_kind="blob"` row: the API's blob store is rooted at the same
/// `<root>/blobs` directory the colony offloads into.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn colony_messages_resolves_blob_only_when_asked() {
    let td = tempfile::TempDir::new().expect("tempdir");
    // Tiny inline threshold so a modest body is offloaded into the blob store.
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_inline_max_bytes": 64}"#,
    )
    .expect("write colony.json");

    let test_h = meclaw_testing::ColonyHandle::new_with_blobs_at(&td, vec![]);
    assert_eq!(
        test_h.blob_inline_max_bytes(),
        64,
        "custom offload threshold applied"
    );
    test_h
        .spawn(meclaw_core::Path::new("/echo"), || {
            meclaw_testing::mocks::EchoMockCell::new(meclaw_core::Path::new("/echo"))
        })
        .await;

    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    // Same directory the colony offloads into — this is what makes the lazy
    // resolution resolve anything at all.
    let blob_store = Arc::new(
        meclaw_colony::blob::DiskBlobStore::new(td.path().join("blobs")).expect("blob store"),
    );
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    let long_text = "x".repeat(4096);
    let body_json = serde_json::json!({
        "target": "/echo",
        "body": { "messages": [{ "origin": "user", "type": "text", "text": long_text }] }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Routing + persistence are async fire-and-forget: poll for the blob row.
    let mut blob_row = None;
    for _ in 0..50 {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/colony/messages?body_kind=blob")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(first) = j["messages"].as_array().and_then(|a| a.first()) {
            blob_row = Some(first.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let row = blob_row.expect("colony offloaded the oversized body into a blob row");
    assert_eq!(row["body_kind"], serde_json::json!("blob"));
    assert!(
        row["blob_body"].is_null(),
        "no store access without ?resolve_blob=true"
    );
    let blob_id = row["body_payload"].as_str().expect("blob id in payload");

    // Now with resolution.
    let uri = "/colony/messages?body_kind=blob&resolve_blob=true";
    let r = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = to_bytes(r.into_body(), 1 << 22).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let resolved = &j["messages"][0];
    assert_eq!(resolved["body_payload"].as_str(), Some(blob_id));
    assert!(
        resolved["blob_error"].is_null(),
        "blob must resolve cleanly: {:?}",
        resolved["blob_error"]
    );
    let body = &resolved["blob_body"];
    assert!(!body.is_null(), "resolved blob body present");
    assert_eq!(
        body["messages"][0]["text"].as_str().map(|s| s.len()),
        Some(4096),
        "the resolved body is the real payload, not a stub"
    );
}
