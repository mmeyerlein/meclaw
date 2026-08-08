//! Pre-14 Pass-2 rejection-test B5 — Befund-2 ingress UBF validation (422).
//!
//! The HTTP ingress is a trust boundary: every source body is validated against
//! the UBF schema BEFORE it enters the routing path, rejected with 422 +
//! `error = "invalid_ubf_body"` rather than letting a malformed body reach a cell
//! (`handlers/messages.rs`, both content-type paths).
//!
//! Covered here:
//!   * (a) JSON path, malformed UBF (no `system`/`messages`/`attachments` slot,
//!     violating the schema `anyOf`) → 422 `invalid_ubf_body`. Real reject.
//!   * (c) POSITIVE control: an attachments-only JSON body (a legitimate `anyOf`
//!     branch) → 202, NOT 422.
//!
//! ## (b) Multipart path — NOT a triggerable 422 (Pass-2 STOP finding)
//! The task asked for a multipart request with a schema-violating body → 422.
//! That cannot be constructed: `post_messages_multipart` does NOT accept a
//! client-supplied body — it SYNTHESIZES `{"attachments": [<BlobRef…>]}` from the
//! uploaded files. A `BlobRef` always serializes to a schema-valid `Attachment`
//! (`blob_id`/`mime_type`/`size_bytes`), and an upload with no file fields yields
//! `{"attachments": []}`, which still satisfies the `attachments` anyOf branch.
//! So no real multipart request reaches the 422 branch (messages.rs ~l.283) — it
//! is defensive-but-unreachable. The test below proves the positive direction
//! (a target-only multipart is accepted, 202), documenting the finding instead of
//! faking a 422 that the code cannot emit. See the Pass-2 report.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Build a router over a fresh test colony + blob store. Returns the router, the
/// live `ColonyHandle` (the caller MUST keep it alive — dropping it aborts the
/// colony task, which would turn the accepted-path sends into 503s), and the blob
/// `TempDir`.
fn build_test_app() -> (
    axum::Router,
    meclaw_testing::ColonyHandle,
    tempfile::TempDir,
) {
    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, blob_td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
    (app, test_h, blob_td)
}

// ── (a) JSON path, malformed UBF → 422 invalid_ubf_body ──────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn json_malformed_ubf_body_rejected_422_before_routing() {
    let (app, _test_h, _blob_td) = build_test_app();

    // Body has NONE of system/messages/attachments → violates the UBF anyOf.
    let body_json = serde_json::json!({
        "target": "/echo/cell",
        "body": {"not_a_ubf_slot": "bar"}
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

    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "malformed UBF body must be rejected with 422"
    );
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json["error"], "invalid_ubf_body",
        "422 must carry error=invalid_ubf_body, got: {json}"
    );
}

// ── (c) POSITIVE control: attachments-only JSON body → 202 (valid anyOf) ──────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn json_attachments_only_body_accepted_202() {
    let (app, _test_h, _blob_td) = build_test_app();

    // attachments-only is a legitimate UBF shape (file-upload message).
    let body_json = serde_json::json!({
        "target": "/sink",
        "body": {
            "attachments": [{
                "blob_id": meclaw_core::Uuid::now_v7().to_string(),
                "mime_type": "application/pdf",
                "filename": "report.pdf",
                "size_bytes": 4096
            }]
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

    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "an attachments-only body is a valid anyOf branch and must NOT 422"
    );
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json["message_id"].is_string(),
        "202 must carry a message_id"
    );
}

// ── (b-documentation) multipart synthesizes an always-valid body → never 422 ──
//
// Empirically pins the STOP finding: a target-only multipart upload (no files)
// produces the synthesized `{"attachments": []}` body, which is schema-valid →
// 202, never 422. There is no multipart input that drives the 422 branch.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multipart_synthesized_body_is_always_valid_never_422() {
    let (app, _test_h, _blob_td) = build_test_app();

    let boundary = "----Pre14B5Boundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"target\"\r\n\r\n\
         /sink\r\n\
         --{boundary}--\r\n"
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
                .body(Body::from(body.into_bytes()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "multipart synthesizes a valid attachments body → 202 (the 422 branch is \
         structurally unreachable; see module doc + Pass-2 report)"
    );
}
