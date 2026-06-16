//! Phase 12-D T27: Error-Pages 400/500 für `/ui/*` — KEIN 422 bei Reads.
//!
//! Spec-Klarstellung: `422 Unprocessable Entity` ist POST-Mutation-only.
//! UI-Reads, die invalide Query-Werte bekommen (z.B. UUID-Parse-Fail),
//! antworten mit 400-HTML — niemals 422.
//!
//! 500 ist die generische Fallback-Page, wenn die Colony-Inbox down
//! ist oder der ack-Receiver droppen.

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
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
    (app, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_trace_invalid_uuid_returns_400_html() {
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/trace?trace_id=not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("text/html"),
        "expected HTML response, got {ct:?}"
    );
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("400 Bad Request"));
    assert!(body.contains("trace_id"));
    // Stelle sicher, dass es KEIN JSON ist (kein leading `{`).
    assert!(
        !body.trim_start().starts_with('{'),
        "must be HTML, not JSON"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_trace_invalid_uuid_is_400_not_422() {
    // Z.412 / Spec: 422 ist POST-Mutation-only. Reads geben 400, nie 422.
    let test_h = meclaw_testing::ColonyHandle::new();
    let (app, _blob_td) = app_from(&test_h);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/trace?trace_id=zzz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_ne!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_trace_500_when_colony_inbox_closed() {
    // Drop the receiver-side: build a ColonyHandle whose inbox-Sender has no
    // receiver (channel created locally, rx immediately dropped). Send will
    // fail → handler must respond 500-HTML, not 503-JSON, not panic.
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(rx);
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: tx,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    let trace_id = "01900000-0000-7000-8000-000000000000";
    let uri = format!("/ui/trace?trace_id={trace_id}");
    let resp = app
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.starts_with("text/html"));
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("500 Internal Server Error"));
}
