//! Phase-12-X T18: POST /messages with multipart/form-data uploads each file
//! field into the DiskBlobStore, builds a UBF body with the `attachments[]`
//! slot (BlobRef-serialized), and returns 202 + {message_id, attachments}.
//!
//! The JSON path stays untouched (no phase jumping: no Body::Blob auto-offload).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_multipart_uploads_to_blob_and_returns_attachments() {
    let blob_td = tempfile::TempDir::new().unwrap();
    let blob_root = blob_td.path().to_path_buf();

    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let blob_store = Arc::new(meclaw_colony::blob::DiskBlobStore::new(&blob_root).unwrap());
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        meclaw_api::router::SurfaceState::disabled(),
    );

    // Hand-rolled multipart body — small enough to keep this test self-contained
    // without pulling reqwest's multipart builder into the request side.
    let boundary = "----TestBoundary";
    let pdf_content = b"%PDF-1.4 fake content here";
    let prelude = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"target\"\r\n\r\n\
         /echo/cell\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"attachment\"; filename=\"report.pdf\"\r\n\
         Content-Type: application/pdf\r\n\r\n",
        boundary = boundary
    );
    let mut body_bytes = prelude.into_bytes();
    body_bytes.extend_from_slice(pdf_content);
    body_bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/messages")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "non-JSON response (status={status}, body={:?}, err={e})",
            String::from_utf8_lossy(&bytes)
        )
    });
    assert_eq!(status, StatusCode::ACCEPTED, "body: {json}");

    assert!(json["message_id"].is_string());
    let attachments = json["attachments"]
        .as_array()
        .expect("attachments array in response");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["mime_type"], "application/pdf");
    assert_eq!(attachments[0]["filename"], "report.pdf");
    assert!(attachments[0]["blob_id"].is_string());
    assert_eq!(attachments[0]["size_bytes"], pdf_content.len() as u64);

    // Verify the blob + sidecar landed on disk where DiskBlobStore promises.
    let blob_id = attachments[0]["blob_id"].as_str().unwrap();
    let blob_path = blob_root.join(format!("{blob_id}.pdf"));
    let sidecar_path = blob_root.join(format!("{blob_id}.pdf.meta.json"));
    assert!(blob_path.exists(), "missing blob file: {blob_path:?}");
    assert!(
        sidecar_path.exists(),
        "missing sidecar file: {sidecar_path:?}"
    );
    let on_disk = std::fs::read(&blob_path).unwrap();
    assert_eq!(on_disk, pdf_content);
}

/// Phase-12-X T18: the JSON path stays untouched. Verifies via a fresh build of
/// this test binary that the multipart branching logic does not disturb the JSON
/// path (exact reproducer from phase_12_b_messages_post.rs).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_messages_json_path_unchanged_by_multipart_branch() {
    let blob_td = tempfile::TempDir::new().unwrap();
    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let blob_store = Arc::new(meclaw_colony::blob::DiskBlobStore::new(blob_td.path()).unwrap());
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        meclaw_api::router::SurfaceState::disabled(),
    );

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
    assert!(json["message_id"].is_string());
    // JSON-path: no attachments slot in response.
    assert!(json.get("attachments").is_none());
}
