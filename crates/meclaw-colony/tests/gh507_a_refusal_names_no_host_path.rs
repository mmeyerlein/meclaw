//! GH #507 — a refusal must not carry the host path of the staging directory.
//!
//! Since GH #502 the submitter's receipt carries the mutation door's `details`
//! verbatim, so whatever the staging refusals say is read twice: once by the
//! requester, once by the composing cell whose next prompt the receipt becomes.
//! One refusal class rendered an absolute host path into that sentence:
//!
//! ```text
//! manifest refused at position 2: invalid_params (1 applied, 1 untouched)
//!   -- InvalidParams("<colony root>/.staging/<mutation id>/feed-clock-a/config.json:
//!      params.schedules[0]: emit_to: required")
//! ```
//!
//! The staging path is addressable by neither reader. The directory is gone by
//! the time the sentence is read — staging is pre-destructive, the refusal is
//! what discards it — and everything that carries meaning is already in the
//! rest of the string: the node name and the `params` pointer the factory
//! itself wrote. What the host path adds is a prefix that changes per machine,
//! per mutation, and per run.
//!
//! So the prefix is cut and the tail kept: `<node>/config.json` for a single
//! cell, `<node>/<sub>/config.json` for a node inside a subtree. Both audiences
//! — the `--apply` caller and the receipt reader — see the same shorter string,
//! and the operator keeps the two things that locate the defect.
//!
//! The two refusals raised at that site share the defect and are checked
//! together: `invalid_params` (the factory refuses the params) and
//! `contract_incomplete` (the builder-mandatory contract keys are missing).
//! Both are raised from the same `cfg_path`, so a fix that healed only one
//! would leave the class alive.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

// ── topology helpers (same shape as the gh292 suite) ─────────────────────────

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
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

/// Boot a one-hive colony with the echo factory, then register `templates/`.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    rescan_templates(&h, td.path().join("templates")).await;
    h
}

fn rejected(outcome: &MutationOutcome) -> (&str, &str) {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => (error_code.as_str(), details.as_str()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ── the declarations under test ─────────────────────────────────────────────

/// A single-cell template whose `params` the echo factory refuses: the factory
/// names `params.emitted_target`, which is the pointer half the refusal keeps.
fn write_broken_params(root: &std::path::Path) {
    let dir = root.join("templates").join("broken_params");
    write(
        &dir,
        "template.json",
        r#"{"name":"broken_params","version":"1.0.0"}"#,
    );
    write(
        &dir,
        "config.json",
        r#"{"cell":{"type":"echo"},"params":{},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// A single-cell template with acceptable params and NO contract block — the
/// second refusal raised from the same `cfg_path`.
fn write_no_contract(root: &std::path::Path) {
    let dir = root.join("templates").join("no_contract");
    write(
        &dir,
        "template.json",
        r#"{"name":"no_contract","version":"1.0.0"}"#,
    );
    write(
        &dir,
        "config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink"}}"#,
    );
}

/// A composite whose nested cell carries the broken params, so the refusal is
/// raised for a node one level below the staged root. The kept tail has to name
/// both segments, or the reader cannot tell which cell of the subtree refused.
fn write_broken_subtree(root: &std::path::Path) {
    let outer = root.join("templates").join("broken_subtree");
    write(
        &outer,
        "template.json",
        r#"{"name":"broken_subtree","version":"1.0.0"}"#,
    );
    write(
        &outer,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        &outer,
        "inner/config.json",
        r#"{"cell":{"type":"echo"},"params":{},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Every assertion this file makes about one refusal string, in one place: the
/// host prefix is gone, the `.staging` segment is gone, and the two things a
/// reader navigates by are still there.
fn assert_addressable(details: &str, root: &std::path::Path, expected_tail: &str, pointer: &str) {
    let root_str = root.to_string_lossy();
    assert!(
        !details.contains(root_str.as_ref()),
        "the refusal carries the colony root, which addresses nobody who reads it \
         (GH #507): {details}"
    );
    assert!(
        !details.contains(".staging"),
        "the refusal names a directory that no longer exists when the sentence is \
         read (GH #507): {details}"
    );
    assert!(
        details.contains(expected_tail),
        "the refusal must still locate the config it is about as `{expected_tail}`: {details}"
    );
    assert!(
        details.contains(pointer),
        "the refusal must still carry the reason it was raised for (`{pointer}`): {details}"
    );
}

// ── 1. invalid_params ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_refusal_names_the_node_and_the_pointer_but_no_host_path() {
    let td = tempfile::TempDir::new().unwrap();
    write_broken_params(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "grown", "template": "broken_params@1.0.0"}]}}),
    )
    .await;
    let (code, details) = rejected(&outcome);
    assert_eq!(
        code, "invalid_params",
        "the factory's refusal is the one under test, got {outcome:?}"
    );
    assert_addressable(
        details,
        td.path(),
        "grown/config.json",
        "params.emitted_target",
    );
    h.shutdown().await;
}

// ── 2. contract_incomplete, the twin raised from the same path ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_contract_refusal_names_the_node_but_no_host_path() {
    let td = tempfile::TempDir::new().unwrap();
    write_no_contract(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "grown", "template": "no_contract@1.0.0"}]}}),
    )
    .await;
    let (code, details) = rejected(&outcome);
    assert_eq!(
        code, "contract_incomplete",
        "the missing contract keys are the second refusal from that path, got {outcome:?}"
    );
    assert_addressable(details, td.path(), "grown/config.json", "version");
    h.shutdown().await;
}

// ── 3. a node inside a subtree keeps both segments ──────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_from_inside_a_subtree_keeps_the_path_below_the_staged_root() {
    let td = tempfile::TempDir::new().unwrap();
    write_broken_subtree(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "unit", "template": "broken_subtree@1.0.0"}]}}),
    )
    .await;
    let (code, details) = rejected(&outcome);
    assert_eq!(
        code, "invalid_params",
        "the nested cell's params are refused the same way, got {outcome:?}"
    );
    assert_addressable(
        details,
        td.path(),
        "unit/inner/config.json",
        "params.emitted_target",
    );
    h.shutdown().await;
}

// ── 4. the seed loader raises from the same staging tree ────────────────────

/// A single-cell template whose `seed/` carries a header line that is not JSON.
/// The seeder runs after the config patch, deeper inside the staged node, and
/// rendered the same host path into its refusal.
fn write_broken_seed(root: &std::path::Path) {
    let dir = root.join("templates").join("broken_seed");
    write(
        &dir,
        "template.json",
        r#"{"name":"broken_seed","version":"1.0.0"}"#,
    );
    write(
        &dir,
        "config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(&dir, "seed/items.jsonl", "not a json object\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seed_refusal_names_the_file_below_the_node_but_no_host_path() {
    let td = tempfile::TempDir::new().unwrap();
    write_broken_seed(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_nodes": [{"name": "grown", "template": "broken_seed@1.0.0"}]}}),
    )
    .await;
    let (code, details) = rejected(&outcome);
    assert_eq!(
        code, "schema",
        "an unloadable seed is a schema refusal, got {outcome:?}"
    );
    assert_addressable(details, td.path(), "grown/seed/items.jsonl", "header parse");
    h.shutdown().await;
}
