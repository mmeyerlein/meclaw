//! GH #133 — a deep endpoint past a sealed hive's port is rejected, and the
//! reject is pre-destructive.
//!
//! A hive template documents its ports; until now that was prose only, and
//! `add_edges` accepted any endpoint inside the hive. A parent could wire around
//! the port straight onto an interior cell, bypassing whatever the hive puts in
//! front of it. `params.ports` (opt-in) seals the scope.
//!
//! Four things are proven end-to-end against a real colony:
//!
//! 1. **OPT-IN**: the very same deep edge into a hive WITHOUT `params.ports`
//!    commits — nothing about the historical behaviour moved.
//! 2. **REJECT**: into the sealed hive it is `hive_port_boundary`.
//! 3. **PRE-DESTRUCTIVE**: after the reject the colony's graph (nodes, edges,
//!    `graph_version`) is byte-identical to what it was before the attempt.
//! 4. **THE PORT STILL WORKS**: the same parent, same mutation shape, onto the
//!    declared port — commits. So the gate rejects on the boundary, not on the
//!    edge shape.

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

/// Root hive `/` with a caller cell and a hive `/aff` holding `brief` (the
/// declared port) and `store` (the interior node the parent must not reach).
/// `hive_params` decides whether `/aff` is sealed.
fn write_topology(td: &std::path::Path, hive_params: &str) {
    hive(td, "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td, "main/caller", "/caller");
    hive(td, "main/aff", hive_params);
    echo_cell(td, "main/aff/brief", "/aff/brief");
    echo_cell(td, "main/aff/store", "/aff/store");
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

/// A comparable fingerprint of the whole colony graph: node paths and edge
/// pairs (the load-bearing halves) plus `graph_version`, which is still
/// constant-0 substrate-wide and therefore carried along rather than relied on.
/// Two identical fingerprints mean nothing moved — the positive receipt that the
/// reject was pre-destructive.
fn fingerprint(reply: &ReadGraphReply) -> (Vec<String>, Vec<(String, String)>, u64) {
    let mut nodes: Vec<String> = reply.nodes.iter().map(|n| n.path.clone()).collect();
    nodes.sort();
    let mut edges: Vec<(String, String)> = reply
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    edges.sort();
    (nodes, edges, reply.graph_version)
}

/// The deep endpoint into the hive: `caller -> aff/store`, past `brief`.
fn deep_edge() -> Value {
    json!({"diff":{"add_edges":[{"from":"./caller","to":"./aff/store"}]}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_without_a_port_declaration_still_accepts_a_deep_endpoint() {
    // The OPT-IN half. Same topology, same mutation — only `params.ports` is
    // missing. This is the byte-identical-behaviour proof: whoever declares
    // nothing keeps exactly the substrate they had.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), r#"{"graph":{"edges":[]}}"#);

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    match send_mutation(&h, deep_edge()).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("an unsealed hive must still accept the deep endpoint, got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_endpoint_past_a_declared_port_rejects_and_changes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), r#"{"ports":["brief"],"graph":{"edges":[]}}"#);

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    let before = fingerprint(&read_graph(&h).await);

    // ── NEGATIVE: reaching in past the port. ──
    match send_mutation(&h, deep_edge()).await {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(
                error_code, "hive_port_boundary",
                "deep endpoint past the port must reject with hive_port_boundary, got \
                 {error_code} / {details}"
            );
            assert!(
                details.contains("/aff/store") && details.contains("brief"),
                "the reject names the offending endpoint AND the declared ports: {details}"
            );
        }
        other => panic!("expected Rejected{{hive_port_boundary}}, got {other:?}"),
    }

    // ── PRE-DESTRUCTIVE: nothing moved. ──
    let after = fingerprint(&read_graph(&h).await);
    assert_eq!(
        before, after,
        "a pre-destructive reject leaves nodes, edges and graph_version untouched"
    );

    // ── REACHING OUT is the same breach, seen from the other end. ──
    match send_mutation(
        &h,
        json!({"diff":{"add_edges":[{"from":"./aff/store","to":"./caller"}]}}),
    )
    .await
    {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "hive_port_boundary",
            "a reply lane straight out of an interior cell bypasses the port too"
        ),
        other => panic!("expected Rejected{{hive_port_boundary}}, got {other:?}"),
    }

    // ── POSITIVE control: the declared port commits, so the gate rejects on
    //    the boundary and not on the edge shape. ──
    match send_mutation(
        &h,
        json!({"diff":{"add_edges":[{"from":"./caller","to":"./aff/brief"}]}}),
    )
    .await
    {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the declared port must stay wireable, got {other:?}"),
    }

    // ── POSITIVE control 2: the hive path itself is an address (transit). ──
    match send_mutation(
        &h,
        json!({"diff":{"add_edges":[{"from":"./caller","to":"./aff"}]}}),
    )
    .await
    {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the hive path itself must stay wireable, got {other:?}"),
    }

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inside_the_sealed_hive_every_edge_stays_legal() {
    // The hive's own graph must not be caught by its own seal: a mutation
    // scoped INTO the hive wires two interior nodes, neither of which is a port.
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), r#"{"ports":["brief"],"graph":{"edges":[]}}"#);

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    match send_mutation(
        &h,
        json!({"scope":"/aff","diff":{"add_edges":[{"from":"./store","to":"./brief"}]}}),
    )
    .await
    {
        MutationOutcome::Committed { .. } => {}
        other => panic!("an intra-hive edge must commit, got {other:?}"),
    }
    h.shutdown().await;
}
