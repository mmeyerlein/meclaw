//! GH #173 — a hive template declares a contract, and the contract is checked.
//!
//! A template is supposed to be a class: instantiate it, wire to its interface,
//! swap it later for another implementation with a different inside. For hive
//! templates none of that held. The interface was prose in `description`, and
//! the prose named cells three levels down, so every instantiation wrote the
//! template's internal layout into the caller's own topology.
//!
//! `params.contract` states the interface in the only vocabulary that survives a
//! reimplementation: LANES. Which `hop.route` values the hive accepts at its own
//! path, which ones it emits back out of it. Cell names appear nowhere.
//!
//! Five things proven against a real colony:
//!
//! 1. **OPT-IN**: a hive that declares nothing keeps the substrate it had.
//! 2. **THE CALLER IS CHECKED**: an edge into the hive that stamps a lane the
//!    contract does not accept is `hive_contract`, and pre-destructive.
//! 3. **THE DECLARED LANE COMMITS** — the rule refuses the typo, not the wiring.
//! 4. **THE HIVE IS CHECKED TOO**: a declared lane with no door behind it is
//!    refused, so the contract cannot rot into decoration while the inside
//!    changes underneath it.
//! 5. **CONSERVATIVE**: a route the caller computes instead of stating is left
//!    alone — a check that cannot place an edge must never reject it.

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

/// A sealed hive `/mem` in the shape the migrated templates have: no ports, one
/// door per accepted lane, one exit back out through the hive path.
const SOUND: &str = r#"{
  "ports": [],
  "contract": {
    "accepts": [{"route": "in_batch", "context": ["session_id"],
                 "because": "one closed session as a single write batch"}],
    "emits": [{"route": "episode", "because": "one message per turn of the batch"}]
  },
  "graph": {"edges": [
    {"from": ".", "to": "./glue", "condition": "has(hop.route) && hop.route == 'in_batch'"},
    {"from": "./glue", "to": ".", "condition": "has(hop.route) && hop.route == 'episode'"}
  ]}}"#;

/// The same hive whose door was moved to another lane while the contract kept
/// saying `in_batch` — the drift the check exists to catch.
const DOORLESS: &str = r#"{
  "ports": [],
  "contract": {
    "accepts": [{"route": "in_batch", "because": "one closed session as a single write batch"}],
    "emits": []
  },
  "graph": {"edges": [
    {"from": ".", "to": "./glue", "condition": "has(hop.route) && hop.route == 'in_other'"}
  ]}}"#;

/// No contract at all: the state of every hive shipped before this field.
const UNDECLARED: &str = r#"{"ports": [], "graph": {"edges": [
    {"from": ".", "to": "./glue", "condition": "has(hop.route) && hop.route == 'in_batch'"}
  ]}}"#;

fn write_topology(td: &std::path::Path, hive_params: &str) {
    hive(td, "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td, "main/caller", "/caller");
    echo_cell(td, "main/sink", "/sink");
    hive(td, "main/mem", hive_params);
    echo_cell(td, "main/mem/glue", "/mem/glue");
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

/// The caller addresses the HIVE and names a lane. No cell of the hive appears
/// anywhere in this edge — which is the whole point.
///
/// GH #291: the edge also PROMOTES `session_id`, because `SOUND`'s `in_batch`
/// lane declares `"context": ["session_id"]` and that declaration is now a
/// requirement rather than a note for a reader. The wiring was the weaker half
/// of the two: the contract said all along what a caller owes, this edge simply
/// never paid it and nothing could see that. The lane is the same lane, the
/// edge still names the hive and nothing below it.
fn wire_lane(route: &str) -> Value {
    json!({"diff": {"add_edges": [
        {"from": "./caller", "to": "./mem",
         "modifier": {"set_hop": {"route": format!("'{route}'")},
                      "set_context": {"session_id": "'s-1'"}}}
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
async fn a_hive_that_declares_no_contract_keeps_the_old_behaviour() {
    // The opt-in half: whoever declares nothing keeps exactly the substrate they
    // had. Every hive instantiated before this field is in this state, including
    // the ones running in a live colony right now.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), UNDECLARED);
    let h = boot(&td).await;

    match send_mutation(&h, wire_lane("in_nonsense")).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("an undeclared hive keeps the old behaviour, got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_the_hive_does_not_accept_is_refused_and_changes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), SOUND);
    let h = boot(&td).await;

    let before = fingerprint(&read_graph(&h).await);
    // `in_bath` — one letter off, and without the contract it is a dead letter
    // at runtime that looks like a model error.
    let outcome = send_mutation(&h, wire_lane("in_bath")).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            assert!(
                details.contains("in_bath") && details.contains("in_batch"),
                "the refusal names the lane asked for and the lanes on offer: {details}"
            );
        }
        other => panic!("an undeclared lane must be refused, got {other:?}"),
    }

    assert_eq!(
        before,
        fingerprint(&read_graph(&h).await),
        "the reject is pre-destructive: the graph is what it was"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_declared_lane_commits() {
    // The rule refuses the typo, not the wiring. And the edge that commits names
    // the hive and a lane — nothing below the boundary.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), SOUND);
    let h = boot(&td).await;

    match send_mutation(&h, wire_lane("in_batch")).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the declared lane must commit, got {other:?}"),
    }
    let edges = fingerprint(&read_graph(&h).await).1;
    assert!(
        edges.contains(&("/caller".to_string(), "/mem".to_string())),
        "the caller wired the hive, not a cell inside it: {edges:?}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_lane_without_a_door_is_refused() {
    // The other direction, and the one that makes the contract worth reading: a
    // hive may not claim a lane its own graph does not carry. Without this the
    // declaration is decoration the moment somebody rearranges the inside.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), DOORLESS);
    let h = boot(&td).await;

    let outcome = send_mutation(&h, wire_lane("in_batch")).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            assert!(
                details.contains("in_batch"),
                "the refusal names the lane with no door: {details}"
            );
        }
        other => panic!("a lane with no door must be refused, got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_route_the_caller_computes_is_left_alone() {
    // Conservative by construction, exactly like the port boundary: a check that
    // cannot say which lane an edge means must not reject it. Here the route is
    // carried over from the incoming hop, so it is only knowable at runtime.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), SOUND);
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [
            {"from": "./caller", "to": "./mem",
             "modifier": {"set_hop": {"route": "hop.upstream_route"}}}
        ]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!("a computed route must not be rejected, got {other:?}"),
    }
    h.shutdown().await;
}
