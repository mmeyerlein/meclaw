//! GH #422 — `POST /colony/mutations` takes the manifest body form too.
//!
//! The HTTP door is the one an operator reaches with `curl`, and R5's promise
//! is that five `curl`s become one. This file measures that at the wire:
//!
//! * two valid entries → 200, `manifest.applied == 2`;
//! * a broken second entry → 422, `manifest.failed_at == 2`, and the first
//!   entry stays committed (no rollback);
//! * a single mutation POST is UNCHANGED — same two status codes, same keys as
//!   before this lane. That is the HTTP twin of
//!   `gh422_the_single_mutation_body_does_not_move`.
//!
//! The handler tells the two forms apart nowhere: it hands the body to the
//! colony verbatim as `ColonyMsg::MutationDoor` and renders the verdict with
//! `meclaw_colony::mutation_door_reply`. One rule, one renderer, two doors.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// A template directory the colony can instantiate from, loaded via
/// `RescanTemplates` — same pattern as `phase_12_b_mutations_post.rs`.
async fn setup_template(h: &meclaw_testing::ColonyHandle, name: &str, cell_type: &str) {
    let root = h.tempdir_path();
    let templates_root = root.join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"{cell_type}"}},"params":{{"emitted_target":"/unset"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
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
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// One `add_nodes` entry from the `echo` template.
fn entry(name: &str) -> serde_json::Value {
    serde_json::json!({
        "scope": "/",
        "diff": { "add_nodes": [{
            "name": name,
            "template": "echo",
            "override_params": { "emitted_target": "/sink" }
        }]}
    })
}

/// POST `body` at `/colony/mutations` and return `(status, json)`.
async fn post(
    h: &meclaw_testing::ColonyHandle,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: h.inbox_tx.clone(),
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, _blob_td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/colony/mutations")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_post_of_two_valid_entries_is_200() {
    let h = meclaw_testing::ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo").await;

    let (status, json) = post(
        &h,
        serde_json::json!({"manifest": [entry("a"), entry("b")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert_eq!(json["manifest"]["outcome"], "committed");
    assert_eq!(json["manifest"]["applied"], 2);
    assert_eq!(json["manifest"]["ids"].as_array().expect("ids").len(), 2);
    assert!(
        json.get("mutation").is_none(),
        "a manifest answers in its own slot: {json}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_post_whose_second_entry_refuses_is_422_and_says_where() {
    let h = meclaw_testing::ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo").await;

    let broken = serde_json::json!({
        "scope": "/",
        "diff": { "add_nodes": [{ "name": "b", "template": "definitely_not_a_real_template" }]}
    });
    let (status, json) = post(
        &h,
        serde_json::json!({"manifest": [entry("a"), broken, entry("c")]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    assert_eq!(json["manifest"]["outcome"], "rejected");
    assert_eq!(json["manifest"]["applied"], 1, "no rollback: {json}");
    assert_eq!(json["manifest"]["failed_at"], 2);
    assert_eq!(json["manifest"]["remaining"], 1);
    assert_eq!(json["manifest"]["error_code"], "template_missing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_post_that_cannot_be_read_is_422_schema() {
    let h = meclaw_testing::ColonyHandle::new_with_echo();
    let (status, json) = post(&h, serde_json::json!({"manifest": "not an array"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    assert_eq!(json["manifest"]["outcome"], "rejected");
    assert_eq!(json["manifest"]["applied"], 0);
    assert_eq!(
        json["manifest"]["error_code"], "schema",
        "no new error_code is minted for a broken body form: {json}"
    );
}

/// The HTTP twin of `gh422_the_single_mutation_body_does_not_move`.
///
/// The single form's POST answers with the same two codes and the same keys as
/// before this lane — the renderer changed, the document did not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_mutation_post_is_unchanged() {
    let h = meclaw_testing::ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo").await;

    let (status, json) = post(&h, entry("solo")).await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let keys: Vec<&str> = json["mutation"]
        .as_object()
        .expect("mutation slot")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["id", "outcome"], "committed keys: {json}");

    let (status, json) = post(
        &h,
        serde_json::json!({
            "scope": "/", "ctx": {},
            "diff": { "add_nodes": [{ "name": "x", "template": "definitely_not_a_real_template" }]}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {json}");
    let keys: Vec<&str> = json["mutation"]
        .as_object()
        .expect("mutation slot")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec!["details", "error_code", "id", "outcome"],
        "rejected keys — and `violations` is still not on this wire (GH #293): {json}"
    );
}
