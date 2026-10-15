//! GH #666 — a swap's params go through the same override-key check as an add.
//!
//! `add_nodes[].override_params` runs through the key check of GH #294: a key
//! the addressed cell's template does not declare is refused pre-destructively
//! instead of committing as a silent no-op. `swap_nodes[].with.params` did not,
//! although `stage.rs` maps it onto the very same `override_params` contract
//! before staging it — the #294 arm looked at `add_nodes` only. A mistyped key
//! in a swap committed, the new node spawned with the shipped default, and
//! nothing said a word.
//!
//! Found while specifying GH #661: the door-side check for `operator_set`
//! params has to see swaps too, or the new gate carries the same hole from the
//! day it is built.
//!
//! The verdict is the one the add side already gives — `error_code: "schema"`,
//! stage `PostStateAddresses`, the same receipt shape. No new code, no new
//! surface.

use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::json;
use meclaw_core::{JsonValue, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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
    ack_rx.await.unwrap().expect("the rescan must not abort");
}

fn persist_factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
    }) as Arc<dyn CellFactory>
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![("persist_mock".to_string(), persist_factory())]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert("persist_mock".into(), persist_factory());
    r
}

/// A colony with one node to swap out and one single-cell template to swap in.
/// The template declares exactly one param, `p`.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let solo = td.path().join("templates").join("solo");
    write(&solo, "template.json", r#"{"name":"solo"}"#);
    write(
        &solo,
        "config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    let h = ColonyHandle::new_with_factories_at(td, factories());
    rescan_templates(&h, td.path().join("templates")).await;
    // `t2` has to be a REGISTERED node, or `match` answers `match_no_hit` before
    // the `with` half is ever judged — the boot is what puts it in the registry.
    meclaw_colony::bootstrap_from_filesystem(td.path(), &registry(), &h.runtime())
        .await
        .expect("the colony boots with /t2 standing");
    h
}

fn schema_details(outcome: &MutationOutcome) -> String {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(
                error_code, "schema",
                "the swap side answers with the add side's code: {outcome:?}"
            );
            details.clone()
        }
        other => panic!("expected a schema rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mistyped_key_in_a_swap_is_refused_like_one_in_an_add() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let bad = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"t2"},
             "with":{"template":"solo","name":"t3","params":{"q":2}}}
        ]}}),
    )
    .await;
    let details = schema_details(&bad);
    assert!(
        details.contains("'q'"),
        "the refusal names the key that exists nowhere: {details}"
    );
    assert!(
        details.contains("persist_mock"),
        "and the cell it would have configured, by its type: {details}"
    );
    assert!(
        details.contains("'solo'"),
        "and the template that declares the params: {details}"
    );
    assert!(
        details.contains("Its params are:") && details.contains("'p'"),
        "and lists the params that DO exist: {details}"
    );
    assert!(
        !td.path().join("main/t3").exists(),
        "and the node is not there: a swap is refused BEFORE anything is staged"
    );

    // The counter-proof: the same swap with a param the template carries.
    let good = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"t2"},
             "with":{"template":"solo","name":"t3","params":{"p":7}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(good, MutationOutcome::Committed { .. }),
        "a key the template declares is still settable through a swap; got {good:?}"
    );
    let raw = std::fs::read_to_string(td.path().join("main/t3/config.json"))
        .expect("the swapped-in node stands");
    let cfg: JsonValue = meclaw_core::serde_json::from_str(&raw).expect("config json");
    assert_eq!(cfg["params"]["p"], 7, "and the value reached the instance");

    h.shutdown().await;
}
