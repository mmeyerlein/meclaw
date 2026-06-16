//! Phase-12-B T10: POST /messages fire-and-forget — spec Z.1656/Z.1658.
//! Body: {target, body, headers?}. Response: 202 + {message_id}.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_returns_202_with_message_id() {
    // Use a fresh colony — POST /messages is fire-and-forget, the routing
    // will fail (no /echo/cell), the resulting DeadLetter is fine for the test.
    // We only assert the HTTP layer's 202+id behaviour.
    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    // Minimal UBF body — target is required, body has at least messages[].
    let body_json = serde_json::json!({
        "target": "/echo/cell",
        "body": {
            "messages": [{"origin": "user", "type": "text", "text": "ping"}]
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("message_id").is_some());
    assert!(!json["message_id"].as_str().unwrap().is_empty());
}
