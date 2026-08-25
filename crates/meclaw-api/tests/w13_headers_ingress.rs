//! W13 hardening: `POST /messages` rejects a non-object `headers` with 422
//! `invalid_headers` instead of silently degrading it to `{}`.
//!
//! The reference is the `ttl` field on the very same request: it has answered
//! 422 `invalid_ttl` for every non-conforming value since the TTL slice, while
//! `headers` swallowed a string, a number or an array and returned 202 for a
//! message that carried none of the caller's correlation data. Same request,
//! same class of mistake, two different answers — this pins the symmetric one.
//!
//! The colony is a RAW mpsc receiver: a 422 must leave the inbox untouched
//! (positive receipt that nothing was routed), and the accepted case must show
//! the headers arriving in the envelope's `context` compartment.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meclaw_colony::ColonyMsg;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

fn raw_colony_app() -> (
    axum::Router,
    tokio::sync::mpsc::Receiver<ColonyMsg>,
    tempfile::TempDir,
) {
    let (inbox_tx, inbox_rx) = tokio::sync::mpsc::channel::<ColonyMsg>(8);
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: inbox_tx,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, blob_td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(api_colony, blob_store, 9);
    (app, inbox_rx, blob_td)
}

fn ubf_request(headers: Option<serde_json::Value>) -> serde_json::Value {
    let mut req = serde_json::json!({
        "target": "/echo/cell",
        "body": {"messages": [{"origin": "user", "type": "text", "text": "ping"}]}
    });
    if let Some(h) = headers {
        req.as_object_mut().unwrap().insert("headers".into(), h);
    }
    req
}

async fn post_json(app: axum::Router, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Every non-object, non-null `headers` value is a 422 `invalid_headers`, and
/// nothing reaches the colony.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_non_object_headers_is_422() {
    for bad in [
        serde_json::json!("trace-id"),
        serde_json::json!(7),
        serde_json::json!(true),
        serde_json::json!([{"k": "v"}]),
    ] {
        let (app, mut inbox_rx, _td) = raw_colony_app();
        let (status, body) = post_json(app, &ubf_request(Some(bad.clone()))).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "headers={bad} must be rejected with 422, got {body}"
        );
        assert_eq!(
            body["error"], "invalid_headers",
            "headers={bad} must carry the invalid_headers token, got {body}"
        );
        assert!(
            inbox_rx.try_recv().is_err(),
            "headers={bad}: nothing may reach the colony inbox on a 422"
        );
    }
}

/// Absent and `null` stay accepted — a JSON client serialising an optional
/// field as null must not be punished (same rule as `ttl`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_absent_or_null_headers_is_accepted() {
    for req in [
        ubf_request(None),
        ubf_request(Some(serde_json::Value::Null)),
    ] {
        let (app, mut inbox_rx, _td) = raw_colony_app();
        let (status, _) = post_json(app, &req).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(
            inbox_rx.try_recv().is_ok(),
            "an accepted message must reach the colony inbox"
        );
    }
}

/// An object `headers` is accepted and lands in the envelope's `context`
/// compartment — the positive receipt that the gate did not eat the payload.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_object_headers_reaches_the_context_compartment() {
    let (app, mut inbox_rx, _td) = raw_colony_app();
    let (status, _) = post_json(app, &ubf_request(Some(serde_json::json!({"trace": "abc"})))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    match inbox_rx.recv().await.expect("handler must send Route") {
        ColonyMsg::Route { msg, .. } => {
            assert_eq!(
                msg.headers.context.get("trace").and_then(|v| v.as_str()),
                Some("abc"),
                "the caller's headers must survive the gate"
            );
        }
        _ => panic!("expected ColonyMsg::Route"),
    }
}
