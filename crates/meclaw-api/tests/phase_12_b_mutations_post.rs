//! Phase-12-B T9: POST /colony/mutations with 200/422 status code mapping.
//! Full MutationOutcome::Rejected detail stays in the `mutation` reply slot
//! (spec Z.1660).
//!
//! Pattern wie phase_12_b_routes.rs: meclaw_testing::ColonyHandle treibt eine
//! echte Colony-Task, wir wrappen deren inbox in einen meclaw_api::ColonyHandle
//! und feuern HTTP-Requests gegen den vollen Router.
//!
//! Mutation-Payload-Schema (kanonisch, siehe handle_mutation in colony.rs):
//! `{ "scope": "/", "ctx": {}, "diff": { "add_nodes": [...] } }`.
//! Test 1 (200): valides add_nodes via "echo"-Template, das ColonyHandle::new_with_echo
//! plus eines on-disk Template-Verzeichnisses + RescanTemplates registriert.
//! Test 2 (422): unbekanntes Template → TemplateMissing-Reject → 422 mit
//! voller `mutation`-Detail (outcome/id/error_code/details).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Helper: legt ein Template-Verzeichnis fuer `name`/`cell_type` an und laedt es
/// via `RescanTemplates` in die Colony-Registry. Spiegelt das Pattern aus
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
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

    // `override_params.echo_to` ist Pflicht fuer EchoCell (sonst spawn-Reject).
    // Wir setzen einen plausiblen Pfad — die Cell wird nicht wirklich gepingt,
    // wir wollen nur Committed beobachten.
    let body_json = serde_json::json!({
        "scope": "/",
        "ctx": {},
        "diff": {
            "add_nodes": [
                {
                    "name": "echo1",
                    "template": "echo",
                    "override_params": { "echo_to": "/sink" }
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
    // Kein Template-Setup: das Template-Lookup in handle_mutation rejected
    // sofort mit TemplateMissing.
    let test_h = meclaw_testing::ColonyHandle::new();
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: test_h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);

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
