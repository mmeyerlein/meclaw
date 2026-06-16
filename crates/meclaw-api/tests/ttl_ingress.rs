//! TTL slice (2026-06-11): `POST /messages` TTL wiring — spec
//! `docs/meclaw-overview.md` § Message-Modell (TTL-Semantik) + § Envelope-Setter-
//! Authority. Hierarchy under test: explicit request `ttl` field beats
//! `colony.json::message_default_ttl` (the `build_router` param) beats the
//! constant seed (64).
//!
//! The colony is a RAW mpsc receiver here — the assertions inspect the
//! `ColonyMsg::Route` message the HTTP layer hands to the colony (positive
//! receipt: the envelope `ttl` itself), no routing involved.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meclaw_colony::ColonyMsg;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Router wired to a raw colony inbox; returns the receiver to inspect the
/// `ColonyMsg::Route` the handler sends.
fn raw_colony_app(
    message_default_ttl: u32,
) -> (
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
    let app = meclaw_api::router::build_router(api_colony, blob_store, message_default_ttl);
    (app, inbox_rx, blob_td)
}

/// Minimal valid UBF request body without a `ttl` field.
fn ubf_request(extra: Option<(&str, serde_json::Value)>) -> serde_json::Value {
    let mut req = serde_json::json!({
        "target": "/echo/cell",
        "body": {
            "messages": [{"origin": "user", "type": "text", "text": "ping"}]
        }
    });
    if let Some((k, v)) = extra {
        req.as_object_mut().unwrap().insert(k.into(), v);
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

/// No `ttl` field → the configured colony.json default (router param) is
/// stamped on the initial message — NOT the constant 64.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_without_ttl_uses_configured_default() {
    let (app, mut inbox_rx, _td) = raw_colony_app(9);
    let (status, _) = post_json(app, &ubf_request(None)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    match inbox_rx.recv().await.expect("handler must send Route") {
        ColonyMsg::Route { msg, .. } => assert_eq!(
            msg.ttl, 9,
            "initial message must carry the configured message_default_ttl"
        ),
        other => panic!("expected ColonyMsg::Route, got {}", msg_name(&other)),
    }
}

/// Explicit positive `ttl` field beats the configured default (hierarchy:
/// message field > colony.json > constant).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_with_ttl_overrides_configured_default() {
    let (app, mut inbox_rx, _td) = raw_colony_app(9);
    let (status, _) = post_json(app, &ubf_request(Some(("ttl", serde_json::json!(5))))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    match inbox_rx.recv().await.expect("handler must send Route") {
        ColonyMsg::Route { msg, .. } => assert_eq!(
            msg.ttl, 5,
            "explicit request ttl must beat the configured default"
        ),
        other => panic!("expected ColonyMsg::Route, got {}", msg_name(&other)),
    }
}

/// Invalid `ttl` values are rejected with 422 `invalid_ttl` BEFORE anything is
/// routed: zero, negative, non-integer, non-number, and > u32::MAX.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_invalid_ttl_is_422() {
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-3),
        serde_json::json!(3.5),
        serde_json::json!("abc"),
        serde_json::json!(4_294_967_296_u64), // u32::MAX + 1
    ] {
        let (app, mut inbox_rx, _td) = raw_colony_app(9);
        let (status, body) = post_json(app, &ubf_request(Some(("ttl", bad.clone())))).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "ttl={bad} must be rejected with 422"
        );
        assert_eq!(
            body["error"], "invalid_ttl",
            "ttl={bad} must carry the invalid_ttl error token, got {body}"
        );
        assert!(
            inbox_rx.try_recv().is_err(),
            "ttl={bad}: nothing may reach the colony inbox on a 422"
        );
    }
}

/// `ttl: null` is treated as absent (the configured default applies) — JSON
/// clients serialising optional fields as null must not be punished.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_null_ttl_uses_configured_default() {
    let (app, mut inbox_rx, _td) = raw_colony_app(9);
    let (status, _) = post_json(app, &ubf_request(Some(("ttl", serde_json::Value::Null)))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    match inbox_rx.recv().await.expect("handler must send Route") {
        ColonyMsg::Route { msg, .. } => assert_eq!(msg.ttl, 9),
        other => panic!("expected ColonyMsg::Route, got {}", msg_name(&other)),
    }
}

/// The multipart path stamps the configured default too (no `ttl` form field —
/// the JSON `ttl` request field is the only per-message override surface).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_multipart_uses_configured_default() {
    let (app, mut inbox_rx, _td) = raw_colony_app(9);
    let boundary = "X-MECLAW-TEST-BOUNDARY";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"target\"\r\n\r\n/echo/cell\r\n--{boundary}--\r\n"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    match inbox_rx.recv().await.expect("handler must send Route") {
        ColonyMsg::Route { msg, .. } => assert_eq!(
            msg.ttl, 9,
            "multipart initial message must carry the configured message_default_ttl"
        ),
        other => panic!("expected ColonyMsg::Route, got {}", msg_name(&other)),
    }
}

/// Debug helper: short variant name for panic messages (ColonyMsg is not Debug).
fn msg_name(m: &ColonyMsg) -> &'static str {
    match m {
        ColonyMsg::Route { .. } => "Route",
        _ => "non-Route",
    }
}
