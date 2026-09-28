//! GH #245 — the collector refuses a lane nothing behind its door reads.
//!
//! GH #245 was about the curator's stubs: the collector compressed a tool round,
//! left a stub naming `thread_recall(call_id=…)`, and served that tool on the
//! `in_thread_call` lane — which no contract declared, so every stub was a dead
//! end. `collector@2.0.1` declared the lane, and this file pinned both the
//! declaration and the route through the shipped `talky`.
//!
//! That half retired with GH #889 (R-27-1): the collector gathers the round and
//! hands it on uncut, so it leaves no stub, serves no `thread_recall` and
//! declares no `in_thread_call`. The two tests that pinned the lane went with it.
//!
//! What stays is the guard against the drift going the other way: `in_batch`
//! was declared by a collector whose state machine never dispatched on it — a
//! lane into silence. It left in `collector@2.0.1`, so an edge that names it is
//! refused rather than parked forever.
//!
//! # What is under test, and how honestly
//!
//! The hive file is the SHIPPED artefact, read from `templates/` and planted
//! verbatim: `templates/collector/config.json` carries the contract this file is
//! about. What is substituted is only what this crate cannot spawn: `assemble`
//! is a `code` cell and `window` is a `store` cell, both of which live in
//! `meclaw-cells` (downstream of here), so they stand in as echo cells. The
//! check under test reads the hive's own `params`, and those are the real ones.
//!
//! No model and no network.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

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

/// The repository's `templates/` directory — the shipped library, not a fixture.
fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let dir = root.join(rel);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), body).unwrap();
}

/// Plant a shipped hive `config.json` verbatim at `rel`.
fn plant(root: &std::path::Path, rel: &str, template: &str) {
    let src = templates_root().join(template).join("config.json");
    let body = std::fs::read_to_string(&src)
        .unwrap_or_else(|e| panic!("the shipped {template}/config.json must be readable: {e}"));
    write(root, rel, &body);
}

/// An echo cell standing in for a cell this crate cannot spawn. `emitted_header`
/// is what lets a stand-in produce the route its hive's out-door is looking for.
fn echo_cell(root: &std::path::Path, rel: &str, emitted_target: &str, route: Option<&str>) {
    let header = match route {
        Some(r) => format!(r#","emitted_header":{{"key":"route","value":"{r}"}}"#),
        None => String::new(),
    };
    write(
        root,
        rel,
        &format!(
            r#"{{"cell":{{"type":"echo"}},
                "params":{{"emitted_target":"{emitted_target}"{header}}},
                "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    );
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

/// The shipped collector as a hive of its own, with a caller beside it. The two
/// interior cells are stand-ins (see the module note); the hive file is real.
fn write_collector_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    echo_cell(root, "main/caller", "/caller", None);
    plant(root, "main/collector", "collector");
    echo_cell(
        root,
        "main/collector/assemble",
        "/collector",
        Some("answer"),
    );
    echo_cell(root, "main/collector/window", "/collector", None);
}

/// The caller addresses the HIVE and names a lane — no cell of the hive appears
/// in the edge, which is the whole point of a lane contract.
fn wire_lane(route: &str) -> Value {
    json!({"diff": {"add_edges": [
        {"from": "./caller", "to": "./collector",
         "modifier": {"set_hop": {"route": format!("'{route}'")}}}
    ]}})
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    h
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_collector_refuses_a_lane_nothing_behind_its_door_reads() {
    // `in_batch` was declared and never dispatched on: a caller could wire it,
    // the mutation passed, and every message on it was swallowed in silence.
    // A contract that admits a lane it cannot serve is the same lie as one that
    // hides a lane it does serve, so it left with 2.0.1.
    let td = tempfile::TempDir::new().unwrap();
    write_collector_topology(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(&h, wire_lane("in_batch")).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            // The lanes on offer are named too, which is what proves the check
            // read the shipped contract. `in_thread_call` was the witness here
            // until GH #889 removed it; `in_turn` is the collector's first lane.
            assert!(
                details.contains("in_batch") && details.contains("in_turn"),
                "the refusal names the lane asked for and the lanes on offer: {details}"
            );
        }
        other => panic!("a lane the state machine never reads must be refused, got {other:?}"),
    }
    h.shutdown().await;
}
