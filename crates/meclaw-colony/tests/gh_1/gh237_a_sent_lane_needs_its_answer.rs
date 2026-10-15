//! GH #237 — a caller that sends a lane must subscribe to the answer, end-to-end.
//!
//! The port form of `params.required_drains` (GH #147) fires when something
//! outside a hive wires one of its ports. Since the boundary seals (#197/#228)
//! the shipped library has no ports at all, so that rule can never fire again
//! and `memory-hive` lost the one guarantee it had: a mutation that wires the
//! inline ingress WITHOUT its reject drain used to come back
//! `required_drain_missing` and change nothing. It committed, and a refused
//! block became an unrouted dead end nobody learns about.
//!
//! The same obligation, stated in the vocabulary the seal left standing:
//! *a caller that sends me `in_remember` must subscribe to `reject`.* Five
//! things proven against a real colony:
//!
//! 1. **OPT-IN**: the same ingress into a hive that declares no pairing commits.
//! 2. **REJECT**: with the declaration, the bare ingress is
//!    `required_drain_missing`.
//! 3. **PRE-DESTRUCTIVE**: after the reject the graph is what it was.
//! 4. **THE PAIR COMMITS**: ingress and subscription in ONE mutation goes
//!    through. The rule does not forbid the lane, it forbids half of it.
//! 5. **THE ANSWER HAS TO LEAVE**: a subscription that keeps the lane inside
//!    the hive, or takes a different lane, is not the drain.

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

fn echo_cell(td: &std::path::Path, rel: &str, emitted_target: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
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

/// The hive's own inside: a door for the lane it accepts and an exit for the
/// one it emits. Both are edges the hive itself is an endpoint of — which is
/// the only way a sealed hive can be reached at all.
const INSIDE: &str = r#""graph":{"edges":[
    {"from":".","to":"./glue","condition":"has(hop.route) && hop.route == 'in_remember'"},
    {"from":"./glue","to":".","condition":"has(hop.route) && hop.route == 'reject'"}]}"#;

/// A sealed hive that says what it takes and what it gives back, and nothing
/// about who has to listen.
fn declares_nothing() -> String {
    format!(
        r#"{{"ports":[],"contract":{{
            "accepts":[{{"route":"in_remember","because":"a block to remember now"}}],
            "emits":[{{"route":"reject","because":"a block this hive would not take"}}]}},
          {INSIDE}}}"#
    )
}

/// The same hive, insisting on the pairing.
fn declares_the_pairing() -> String {
    format!(
        r#"{{"ports":[],"contract":{{
            "accepts":[{{"route":"in_remember","because":"a block to remember now"}}],
            "emits":[{{"route":"reject","because":"a block this hive would not take"}}]}},
          "required_drains":[{{"accepts":"in_remember","emits":"reject",
            "because":"a refused block leaves on this lane and nobody learns of it"}}],
          {INSIDE}}}"#
    )
}

fn write_topology(td: &std::path::Path, hive_params: &str) {
    hive(td, "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td, "main/caller", "/caller");
    echo_cell(td, "main/sink", "/sink");
    hive(td, "main/mem", hive_params);
    echo_cell(td, "main/mem/glue", "/mem/glue");
    echo_cell(td, "main/mem/store", "/mem/store");
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

/// The caller's half: an edge onto the HIVE PATH that stamps the lane. No cell
/// of the hive appears in it, which is the whole point of the boundary.
fn ingress() -> Value {
    json!({"from": "./caller", "to": "./mem",
           "modifier": {"set_hop": {"route": "'in_remember'"}}})
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    h
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_that_declares_no_pairing_accepts_the_bare_ingress() {
    // The opt-in half: a hive that only states its lanes keeps exactly the
    // substrate it had. Every contracted template shipped before this change
    // is in this state.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), &declares_nothing());
    let h = boot(&td).await;

    match send_mutation(&h, json!({"diff": {"add_edges": [ingress()]}})).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("a hive without the declaration keeps the old behaviour, got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sending_the_lane_without_taking_its_answer_is_refused_and_changes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), &declares_the_pairing());
    let h = boot(&td).await;

    let before = fingerprint(&read_graph(&h).await);
    let outcome = send_mutation(&h, json!({"diff": {"add_edges": [ingress()]}})).await;
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
async fn ingress_and_subscription_in_one_mutation_commits() {
    // The rule's actual purpose. Both edges in one diff is what the memory
    // hive's README has been asking for in prose since the day it shipped.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), &declares_the_pairing());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [
            ingress(),
            {"from": "./mem", "to": "./sink",
             "condition": "has(hop.route) && hop.route == 'reject'"}
        ]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!("ingress + subscription in one mutation must commit, got {other:?}"),
    }

    let edges = fingerprint(&read_graph(&h).await).1;
    assert!(
        edges.contains(&("/caller".to_string(), "/mem".to_string())),
        "the ingress is there: {edges:?}"
    );
    assert!(
        edges.contains(&("/mem".to_string(), "/sink".to_string())),
        "and so is the subscription: {edges:?}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_that_never_leaves_the_hive_is_not_the_answer() {
    // An edge from the hive path back into its own interior looks like a drain
    // to anything that counts out-edges. It is the hive talking to itself,
    // which is what produced the refusal in the first place.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), &declares_the_pairing());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [
            ingress(),
            {"from": "./mem", "to": "./mem/store",
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_to_some_other_lane_is_not_the_answer() {
    // Taking the hive's ANSWER is not taking its REFUSAL. A check that only
    // asked "does anything leave" would pass this and lose every rejection.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), &declares_the_pairing());
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [
            ingress(),
            {"from": "./mem", "to": "./sink",
             "condition": "has(hop.route) && hop.route == 'bundle'"}
        ]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "required_drain_missing", "{outcome:?}")
        }
        other => panic!("a subscription to another lane must not satisfy the rule, got {other:?}"),
    }
    h.shutdown().await;
}
