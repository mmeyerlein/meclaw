//! GH #140 — the cells inside a subtree template can be parameterised at
//! instantiation, addressed by their path in the template.
//!
//! R10 (2026-06-11) rejected `override_params` on subtree templates because the
//! flat form committed and applied nothing — commit-without-effect, a builder
//! believing an override took. The protection was right; the collateral was
//! that `collector`, `cogny` and `talky` are subtree templates, so the params
//! surface they gained in #136 could not be set at birth at all. An operator
//! edited the instance config afterwards, which is forking the template by
//! hand.
//!
//! The addressing is the cell's path inside the template, `""` for the root:
//!
//! ```json
//! {"name": "u1", "template": "unit",
//!  "override_params": {"s": {"external_timeout_ms": 12345}}}
//! ```
//!
//! What R10 protected is unchanged and lives in
//! `hardening_subtree_override_params_reject.rs`: a key that addresses nothing
//! is refused. This file is the other half — an addressed key must actually
//! arrive in the instance, or the feature is the same false accept under a
//! nicer name.

use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
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
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "persist_mock".to_string(),
        Arc::new(PersistCellFactory {
            spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }) as Arc<dyn CellFactory>,
    )]
}

/// Subtree template `unit`: root hive plus two nested cells, each carrying a
/// value the override can be seen to replace.
fn write_templates(root: &std::path::Path) {
    let unit = root.join("templates").join("unit");
    write(&unit, "template.json", r#"{"name":"unit"}"#);
    // GH #294: `ports` is an OPT-IN hive param, and an override may only name a
    // param the addressed cell declares — so the root declares it as `null`,
    // which is exactly the open, unsealed state `Option<Vec<String>>` reads out
    // of an absent key. `the_subtree_root_is_addressed_by_the_empty_string`
    // then seals it at instantiation, as before.
    write(
        &unit,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./s","to":"./e"}]},"ports":null}}"#,
    );
    write(
        &unit,
        "s/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"emitted_target":"/u1/e","max_turns":10},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &unit,
        "e/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"emitted_target":"/capture","max_turns":10},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// The params of an instantiated cell, read off the disk the mutation wrote.
fn instance_params(td: &std::path::Path, rel: &str) -> JsonValue {
    let raw = std::fs::read_to_string(td.join(rel).join("config.json"))
        .unwrap_or_else(|e| panic!("read {rel}/config.json: {e}"));
    let v: JsonValue = meclaw_core::serde_json::from_str(&raw).expect("config json");
    v["params"].clone()
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_templates(td.path());
    let h = ColonyHandle::new_with_factories_at(td, factories());
    rescan_templates(&h, td.path().join("templates")).await;
    h
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_addressed_override_reaches_the_cell_it_names() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"s":{"max_turns":40}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an addressed override commits; got {outcome:?}"
    );

    let s = instance_params(td.path(), "main/u1/s");
    assert_eq!(s["max_turns"], 40, "the addressed cell got the override");
    assert_eq!(
        s["emitted_target"], "/u1/e",
        "and kept everything the override did not name"
    );

    // The sibling is the control: an override addressed to `s` must not leak
    // into `e`, or the addressing is decorative.
    let e = instance_params(td.path(), "main/u1/e");
    assert_eq!(
        e["max_turns"], 10,
        "the unaddressed sibling is untouched: {e}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_cells_can_be_addressed_in_one_mutation() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"s":{"max_turns":40},"e":{"max_turns":99}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );

    assert_eq!(instance_params(td.path(), "main/u1/s")["max_turns"], 40);
    assert_eq!(instance_params(td.path(), "main/u1/e")["max_turns"], 99);

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_that_addresses_nothing_is_refused_and_says_what_exists() {
    // R10's actual protection, kept: the failure mode it was written against
    // was a key that took effect nowhere and committed anyway.
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"typo":{"max_turns":40}}}
        ]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "schema", "{outcome:?}");
            assert!(
                details.contains("'typo'") && details.contains("Its cells are:"),
                "the refusal names the bad key and lists the real ones: {details}"
            );
            assert!(
                details.contains("'s'") && details.contains("'e'"),
                "both cells are listed: {details}"
            );
        }
        other => panic!("an unaddressable key must be refused, got {other:?}"),
    }
    assert!(
        !td.path().join("main/u1").exists(),
        "and nothing is materialised"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_subtree_root_is_addressed_by_the_empty_string() {
    // The root of a subtree template is a hive, and a hive's params are its
    // graph and its seals. Addressing it is legal for the same reason the
    // others are: it is a cell of the template, and the map is keyed by path.
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"u1","template":"unit",
             "override_params":{"": {"ports": ["s"]}}}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );

    let root = instance_params(td.path(), "main/u1");
    assert_eq!(
        root["ports"],
        json!(["s"]),
        "the subtree root took the override: {root}"
    );
    assert!(
        root.get("graph").is_some(),
        "and kept the graph it was born with: {root}"
    );

    h.shutdown().await;
}
