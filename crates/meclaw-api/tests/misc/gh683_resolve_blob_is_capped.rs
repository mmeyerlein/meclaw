//! GH #683 — one page of `GET /colony/messages?resolve_blob=true` never
//! fetches an unbounded number of blob bodies.
//!
//! Measured on a live colony (2026-09-13): one page of a busy screen was 100
//! blobs at ~484 KB each — 48 MB of JSON parsed and re-serialised on one worker
//! of a four-worker runtime, with no yield inside the parse of a body and none
//! inside the final serialisation of the page. The page now
//! resolves at most a fixed number of bodies, says so once with
//! `blob_resolution_truncated`, and leaves the rows beyond the cap with their
//! `body_payload` uuid so a caller that wants the rest pages for it.
//!
//! The rows come from the production write path — `meclaw-api` has no
//! `rusqlite` in its dev-dependencies, and the colony's own offload is what
//! produces a `body_kind="blob"` row anyway.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// Mirrors `message_log::RESOLVE_BLOB_MAX` — the constant is `pub(crate)`, and
/// the number is a documented part of the endpoint's contract.
const RESOLVE_BLOB_MAX: usize = 25;
/// Oversized posts; each one offloads at least its own body, so the log holds
/// comfortably more blob rows than the cap.
const POSTS: usize = 40;

async fn post_oversized(app: &axum::Router, i: usize) {
    let long_text = format!("{i:04}-{}", "x".repeat(4096));
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
}

async fn get_json(app: &axum::Router, uri: &str) -> serde_json::Value {
    let r = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "{uri}");
    let bytes = to_bytes(r.into_body(), 1 << 24).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Poll until the log holds at least `want` blob rows — the write path is
/// fire-and-forget, so the count is waited for, never assumed.
async fn wait_for_blob_rows(app: &axum::Router, want: usize) {
    let mut last = serde_json::Value::Null;
    for _ in 0..200 {
        last = get_json(app, "/colony/messages?body_kind=blob&limit=1000").await;
        let n = last["messages"].as_array().map(|a| a.len()).unwrap_or(0);
        if n >= want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the log never reached {want} blob rows; last page was {last}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_resolves_at_most_the_cap_and_says_so() {
    let td = tempfile::TempDir::new().expect("tempdir");
    // Tiny inline threshold so a modest body is offloaded into the blob store.
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_inline_max_bytes": 64}"#,
    )
    .expect("write colony.json");

    let test_h = meclaw_testing::ColonyHandle::new_with_blobs_at(&td, vec![]);
    assert_eq!(test_h.blob_inline_max_bytes(), 64);
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
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        Arc::new(meclaw_colony::SurfaceRegistry::new()),
    );

    for i in 0..POSTS {
        post_oversized(&app, i).await;
    }
    wait_for_blob_rows(&app, RESOLVE_BLOB_MAX + 10).await;

    // A page wider than the cap: exactly the cap is resolved, the page says so
    // once, and every row beyond keeps its uuid and carries neither a body nor
    // an error.
    let j = get_json(
        &app,
        "/colony/messages?body_kind=blob&resolve_blob=true&limit=1000",
    )
    .await;
    let messages = j["messages"].as_array().expect("messages slot");
    // The echo replies keep landing while this runs, so the page is at least as
    // wide as the one waited for — never assumed equal to it.
    let blob_rows = messages.len();
    assert!(
        blob_rows >= RESOLVE_BLOB_MAX + 10,
        "a page wider than the cap"
    );
    let resolved = messages
        .iter()
        .filter(|m| !m["blob_body"].is_null())
        .count();
    assert_eq!(
        resolved, RESOLVE_BLOB_MAX,
        "exactly the cap is resolved on a page of {blob_rows} blob rows"
    );
    for (i, m) in messages.iter().enumerate() {
        assert_eq!(m["body_kind"], serde_json::json!("blob"));
        assert!(
            m["body_payload"]
                .as_str()
                .map(|s| meclaw_core::Uuid::parse_str(s).is_ok())
                .unwrap_or(false),
            "row {i} keeps its blob id: {m}"
        );
        assert!(
            m["blob_error"].is_null(),
            "row {i}: the cap is not an error, it is a stop: {m}"
        );
        if i < RESOLVE_BLOB_MAX {
            assert!(!m["blob_body"].is_null(), "row {i} is within the cap: {m}");
        } else {
            assert!(m["blob_body"].is_null(), "row {i} is beyond the cap: {m}");
        }
    }
    assert_eq!(
        j["blob_resolution_truncated"],
        serde_json::json!(true),
        "the page says it stopped short: {j}"
    );

    // A page within the cap: everything resolved, and the key is still there,
    // saying false — a reader never has to guess whether the field exists.
    let j = get_json(
        &app,
        "/colony/messages?body_kind=blob&resolve_blob=true&limit=10",
    )
    .await;
    let messages = j["messages"].as_array().expect("messages slot");
    assert_eq!(messages.len(), 10);
    assert!(
        messages.iter().all(|m| !m["blob_body"].is_null()),
        "every one of the ten is resolved: {j}"
    );
    assert_eq!(
        j["blob_resolution_truncated"],
        serde_json::json!(false),
        "nothing was cut: {j}"
    );
}
