//! Slice 1 (roadmap Z.138): 14-B locality runs on runtime mutations.
//!
//! Task 1.2: smoke test for the `ColonyMsg::SetNodeContract` variant.
//! Task 1.4: semantic locality tests — the negative rejects are the builder
//! feedback, `error_code == "edge_schema"` is contract; the participation rule
//! (an edge-less node carries no obligation) keeps `remove_nodes` disconnects
//! legal.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, NodeContract,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

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

/// Hop topology (boots green): producer `/p` with `emits.hop.h1`, consumer
/// `/c` with `consumes.hop.h1 required:true`, boot edge `p → c` (the fan-in
/// check is satisfied at bootstrap), plus a third cell `/t` WITHOUT
/// `emits.hop.h1`. `/c` echoes to `/sink` — the capture-receipt target in the
/// good-case test; in the reject/disconnect tests no messages flow, so the
/// value is inert there.
fn write_hop_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/p")).unwrap();
    std::fs::create_dir_all(td.join("main/c")).unwrap();
    std::fs::create_dir_all(td.join("main/t")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./p","to":"./c"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/p/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"hop":{"h1":{"type":"string"}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/c/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"h1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/t/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Context topology (boots green): the setter edge `s → c2` with
/// `modifier.set_context.c1` supplies the consumer `/c2`
/// (`consumes.context.c1 required:true`); the second edge `x → c2` keeps `/c2`
/// participating in the post_state after the setter kill (≥1 incident edge).
fn write_context_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/s")).unwrap();
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/c2")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./s","to":"./c2","modifier":{"set_context":{"c1":"'v1'"}}},
            {"from":"./x","to":"./c2"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/s/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/c2/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/s"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"context":{"c1":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
}

/// Transit topology (F1 / K-H1 shape, only boots green since the F1 fix):
/// `entry → /sub` carries `set_hop.hmark`, `/sub → /sub/cellA` is the hive
/// transit, `cellA` HONESTLY declares `consumes.hop.hmark required:true`.
/// A third cell `/x` (edge-free, no required keys) serves as the source for the
/// mutation twins.
fn write_transit_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/entry")).unwrap();
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/sub/cellA")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./entry","to":"./sub","modifier":{"set_hop":{"hmark":"'HM-R2'"}}},
            {"from":"./sub","to":"./sub/cellA"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/entry/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sub"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/entry"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/config.json"),
        r#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/sub/cellA/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"hmark":{"type":"string","required":true}}}}}"#,
    )
    .unwrap();
}

/// Sends a mutation and reads the outcome via the ack oneshot
/// (pattern: phase_11_contract_via_mutation.rs).
async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("mutation ack within 30s")
        .expect("ack sender not dropped")
}

/// An add_edges whose source does NOT supply the required consumes.hop key is
/// rejected pre-destructively (fan-in intersection, 14-B).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_edge_breaking_hop_fanin_is_rejected_edge_schema() {
    let td = TempDir::new().unwrap();
    write_hop_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("hop topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"t","to":"c"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "details: {details}");
            assert!(
                details.contains("14-B locality"),
                "details must carry the 14-B locality marker, got: {details}"
            );
        }
        other => panic!("expected Rejected(edge_schema), got {other:?}"),
    }
    h.shutdown().await;
}

/// A remove_edges that cuts the only set_context setter path of a required
/// consumes.context consumer is rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_remove_edge_breaking_context_reachability_is_rejected() {
    let td = TempDir::new().unwrap();
    write_context_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("context topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"s","to":"c2"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "details: {details}");
        }
        other => panic!("expected Rejected(edge_schema), got {other:?}"),
    }
    h.shutdown().await;
}

/// A remove_nodes disconnect of a hop consumer stays LEGAL
/// (participation rule: an edge-less node carries no obligation).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_disconnect_of_hop_consumer_is_committed() {
    let td = TempDir::new().unwrap();
    write_hop_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("hop topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[{"match":{"name":"c"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect of the hop consumer must commit (participation rule), got {outcome:?}"
    );
    h.shutdown().await;
}

/// Good case: an add_edges with modifier.set_hop that supplies the required key
/// → committed; afterwards a POSITIVE capture receipt (CLAUDE.md discipline): a
/// probe flows over the new edge `t → c` (set_hop supplies h1) through the
/// consumer `/c` to `/sink` — the receipt body carries the echo turns of `/t`
/// AND `/c` and proves that `/c` received the message ("message flow intact
/// afterwards"). Only then the DLQ guard (empty).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_edge_satisfying_hop_via_set_hop_is_committed() {
    let td = TempDir::new().unwrap();
    write_hop_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // /sink (CaptureCell) VOR Bootstrap registrieren (Anti-Cascade-Lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("hop topology must boot green");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"t","to":"c","modifier":{"set_hop":{"h1":"'v1'"}}},
            // W2b (Ruling A1): /c's echo to /sink needs a wired catch-all out-edge
            // (identity-fallback gone). Both endpoints are live (/c registered, /sink
            // spawned pre-bootstrap), so it rides this same mutation rather than the
            // shared write_hop_topology (whose reject/disconnect tests don't spawn /sink).
            {"from":"c","to":"sink"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "set_hop satisfies the required key — must commit, got {outcome:?}"
    );

    // Probe → /t: /t echoes to /c, the NEW edge t→c (set_hop h1) applies,
    // /c echoes to /sink. A UBF-conformant body (phase-6 lesson, no
    // InvalidUbfBody DLQ).
    let probe = MessageBuilder::new(Path::new("/t"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "hop-probe"}]
        })))
        .build();
    h.send(probe).await;

    // Positives Receipt (30s-Failure-Marker-Konvention).
    let received = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink must receive a receipt within 30s — proves the t→c→sink flow")
        .expect("CaptureCell channel must deliver a message");
    assert_eq!(
        received.target.as_str(),
        "/sink",
        "receipt target must be /sink, got {}",
        received.target.as_str()
    );
    let body = match &received.body {
        Body::Inline(v) => v.to_string(),
        other => panic!("expected inline UBF body at /sink, got {other:?}"),
    };
    assert!(
        body.contains("echo from /t"),
        "the receipt must carry the /t echo turn (the probe went through /t): {body}"
    );
    assert!(
        body.contains("echo from /c"),
        "the receipt must carry the /c echo turn — proves the consumer /c RECEIVED \
         the message over the new set_hop edge: {body}"
    );

    // DLQ guard AFTER the flow: no dead-letter entry in the good case.
    let dead = h.drain_dead_letters().await;
    assert!(dead.is_empty(), "DLQ must be empty, got {dead:?}");
    h.shutdown().await;
}

/// F1 twin (mandatory point 1, good case): the K-H1 transit topology boots with
/// an honest contract, and an UNRELATED mutation commits — the post_state
/// re-validation in `handle_mutation` runs the same transit walk as the boot
/// path (before the fix it would have rejected transit-blind here, even though
/// the mutation does not touch `cellA` at all).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_unrelated_edge_commits_with_live_transit_required_hop() {
    let td = TempDir::new().unwrap();
    write_transit_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("transit topology with honest required hop must boot green (F1)");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"entry"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "unrelated edge must commit — post-state walk crosses the transit, got {outcome:?}"
    );
    h.shutdown().await;
}

/// F1 twin (negative): an add_edges that wires a key-less source INTO the hive
/// empties the transit intersection of `cellA`'s required `hop.hmark` →
/// pre-destructive reject `edge_schema` with the 14-B marker (the mutation-path
/// check must not become vacuous).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_add_edge_breaking_transit_intersection_is_rejected() {
    let td = TempDir::new().unwrap();
    write_transit_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("transit topology with honest required hop must boot green (F1)");

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"sub"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "details: {details}");
            assert!(
                details.contains("14-B locality"),
                "details must carry the 14-B locality marker, got: {details}"
            );
        }
        other => panic!("expected Rejected(edge_schema), got {other:?}"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn set_node_contract_acks() {
    let h = ColonyHandle::new();

    let contract = NodeContract {
        header_view: meclaw_colony::mutation::validate::HeaderNodeView::default(),
        emits: None,
        validate_emits: false,
    };

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::SetNodeContract {
            path: Path::new("/a"),
            contract,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");

    // 30s failure-marker convention (robust against cargo-parallel load).
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("SetNodeContract ack within 30s")
        .expect("ack sender not dropped");
}
