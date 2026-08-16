//! GH #147 — a hive port that declared a paired drain must have one, end-to-end.
//!
//! The memory hive's README says the inline reject drain is "not optional once
//! the inline ingress is wired", and it is right: a block the hive refuses
//! leaves on the reject egress, and if nothing consumes that egress the refusal
//! is a dead end. Prose does not stop anybody. `params.required_drains` does.
//!
//! Four things proven against a real colony:
//!
//! 1. **OPT-IN**: the same ingress into a hive that declared nothing commits.
//! 2. **REJECT**: with the declaration, wiring the ingress alone is
//!    `required_drain_missing`.
//! 3. **PRE-DESTRUCTIVE**: after the reject the graph is what it was.
//! 4. **THE PAIR COMMITS**: ingress and drain in ONE mutation goes through —
//!    which is the whole point. The rule does not forbid the lane, it forbids
//!    the half of it that loses messages.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
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

fn echo_cell(td: &std::path::Path, rel: &str, echo_to: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"{echo_to}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn hive(td: &std::path::Path, rel: &str, params: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    )
    .unwrap();
}

/// A caller, a drain, and a hive `/mem` whose `glue` port refuses things onto a
/// `reject` route.
fn write_topology(td: &std::path::Path, hive_params: &str) {
    hive(td, "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td, "main/caller", "/caller");
    echo_cell(td, "main/sink", "/sink");
    hive(td, "main/mem", hive_params);
    echo_cell(td, "main/mem/glue", "/mem/glue");
    echo_cell(td, "main/mem/store", "/mem/store");
}

const DECLARED: &str = r#"{"ports":["glue"],
  "required_drains":[{"port":"glue","hop":{"route":"reject"},
    "because":"a refused block leaves on this route and nobody learns of it"}],
  "graph":{"edges":[]}}"#;

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

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

fn fingerprint(reply: &ReadGraphReply) -> (Vec<String>, Vec<(String, String)>) {
    let mut nodes: Vec<String> = reply.nodes.iter().map(|n| n.path.clone()).collect();
    nodes.sort();
    let mut edges: Vec<(String, String)> = reply
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    edges.sort();
    (nodes, edges)
}

/// The ingress alone — the half that loses messages.
fn ingress_only() -> Value {
    json!({"diff": {"add_edges": [{"from": "./caller", "to": "./mem/glue"}]}})
}

/// Ingress and drain together, which is what the hive is asking for.
fn ingress_with_drain() -> Value {
    json!({"diff": {"add_edges": [
        {"from": "./caller", "to": "./mem/glue"},
        {"from": "./mem/glue", "to": "./sink",
         "condition": "has(hop.route) && hop.route == 'reject'"}
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
async fn a_hive_that_declares_nothing_accepts_the_bare_ingress() {
    // The opt-in half: whoever declares nothing keeps exactly the substrate
    // they had. Every topology shipped before this change is in this state.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), r#"{"ports":["glue"],"graph":{"edges":[]}}"#);
    let h = boot(&td).await;

    match send_mutation(&h, ingress_only()).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("an undeclared hive keeps the old behaviour, got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wiring_the_ingress_without_the_drain_is_refused_and_changes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), DECLARED);
    let h = boot(&td).await;

    let before = fingerprint(&read_graph(&h).await);
    let outcome = send_mutation(&h, ingress_only()).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "required_drain_missing", "{outcome:?}");
            assert!(
                details.contains("nobody learns of it"),
                "the hive's own reason travels into the refusal: {details}"
            );
        }
        other => panic!("the bare ingress must be refused, got {other:?}"),
    }

    assert_eq!(
        before,
        fingerprint(&read_graph(&h).await),
        "the reject is pre-destructive: the graph is what it was"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ingress_and_drain_in_one_mutation_commits() {
    // The rule's actual purpose. It does not forbid the lane; it forbids
    // opening it half-way. Both edges in one diff is exactly what the memory
    // hive's README has been asking for in prose.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), DECLARED);
    let h = boot(&td).await;

    match send_mutation(&h, ingress_with_drain()).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("ingress + drain in one mutation must commit, got {other:?}"),
    }

    let edges = fingerprint(&read_graph(&h).await).1;
    assert!(
        edges.contains(&("/caller".to_string(), "/mem/glue".to_string())),
        "the ingress is there: {edges:?}"
    );
    assert!(
        edges.contains(&("/mem/glue".to_string(), "/sink".to_string())),
        "and so is the drain: {edges:?}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drain_that_does_not_leave_the_hive_is_not_a_drain() {
    // An edge from the port back into the hive's own interior looks like a
    // drain to any check that counts out-edges. It is the hive talking to
    // itself — which is what produced the refusal in the first place.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), DECLARED);
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [
            {"from": "./caller", "to": "./mem/glue"},
            {"from": "./mem/glue", "to": "./mem/store",
             "condition": "has(hop.route) && hop.route == 'reject'"}
        ]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "required_drain_missing", "{outcome:?}")
        }
        other => panic!("an interior 'drain' must not satisfy the rule, got {other:?}"),
    }
    h.shutdown().await;
}
