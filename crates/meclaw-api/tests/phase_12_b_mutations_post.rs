//! Phase-12-B T9: POST /colony/mutations with 200/422 status code mapping.
//! Full MutationOutcome::Rejected detail stays in the `mutation` reply slot
//! (spec l.1660).
//!
//! Same pattern as phase_12_b_routes.rs: meclaw_testing::ColonyHandle drives a
//! real colony task, we wrap its inbox in a meclaw_api::ColonyHandle and fire
//! HTTP requests at the full router.
//!
//! Mutation payload schema (canonical, see handle_mutation in colony.rs):
//! `{ "scope": "/", "ctx": {}, "diff": { "add_nodes": [...] } }`.
//! Test 1 (200): a valid add_nodes via the "echo" template, registered by
//! ColonyHandle::new_with_echo plus an on-disk template directory + RescanTemplates.
//! Test 2 (422): unknown template → TemplateMissing reject → 422 with the full
//! `mutation` detail (outcome/id/error_code/details).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Helper: creates a template directory for `name`/`cell_type` and loads it into
/// the colony registry via `RescanTemplates`. Mirrors the pattern from
/// `crates/meclaw-colony/tests/phase_6_demo.rs`.
async fn setup_template(h: &meclaw_testing::ColonyHandle, name: &str, cell_type: &str) {
    let root = h.tempdir_path();
    let templates_root = root.join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"{cell_type}"}},"params":{{}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_valid_mutation_returns_200_with_committed_slot() {
    let test_h = meclaw_testing::ColonyHandle::new_with_echo();
    setup_template(&test_h, "echo", "echo").await;

    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        meclaw_api::router::SurfaceState::disabled(),
    );

    // `override_params.emitted_target` is mandatory for EchoCell (otherwise spawn reject).
    // We set a plausible path — the cell is never actually pinged, we only want
    // to observe Committed.
    let body_json = serde_json::json!({
        "scope": "/",
        "ctx": {},
        "diff": {
            "add_nodes": [
                {
                    "name": "echo1",
                    "template": "echo",
                    "override_params": { "emitted_target": "/sink" }
                }
            ]
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/colony/mutations")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert_eq!(json["mutation"]["outcome"], "committed");
    assert!(json["mutation"]["id"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_invalid_mutation_returns_422_with_rejected_detail() {
    // No template setup: the template lookup in handle_mutation rejects
    // immediately with TemplateMissing.
    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        meclaw_api::router::SurfaceState::disabled(),
    );

    let body_json = serde_json::json!({
        "scope": "/",
        "ctx": {},
        "diff": {
            "add_nodes": [
                { "name": "unknown_cell", "template": "definitely_not_a_real_template" }
            ]
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/colony/mutations")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["mutation"]["outcome"], "rejected");
    assert!(json["mutation"]["error_code"].is_string());
    assert!(json["mutation"]["details"].is_string());
}
