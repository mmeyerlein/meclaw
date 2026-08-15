//! Paket-5 T8 (Reviewer-Auflage A1): global edge dedup on the mutation path.
//!
//! A duplicate edge (same from/to/condition/modifier) appended to the edge
//! table means DOUBLE delivery on the routing cascade — routing iterates
//! `by_from` and `edge_table::insert` appends without dedup. The Phase-15
//! builder re-applies COMPLETE diffs incl. `add_edges`, so re-apply MUST be
//! idempotent. Spec Z.265: edge identity = from + to + condition + modifier
//! (NOT the UUID).
//!
//! This demo applies the SAME `add_edges` op twice — once as two identical
//! entries in ONE diff, once as a second sequential mutation — and proves:
//!   1. the persisted edge table holds EXACTLY ONE such edge, and
//!   2. a probe routed along that edge is delivered EXACTLY ONCE (one /sink
//!      receipt per probe, not two).
//!
//! Anti-Cascade discipline (Phase-6.5 lesson): /sink (CaptureCell) is the
//! positive receipt signal and is registered via `h.spawn(...)` BEFORE
//! bootstrap so /a's first emit resolves. /a self-echoes (`echo_to = /a`);
//! the mutation edge /a -> /sink overrides the emit target when it fires, and
//! the probe carries a low ttl so a no-match self-loop dies bounded.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// `(name, factory)` list with `echo` registered.
fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

/// Same list as a `CellFactoryRegistry` for `bootstrap_from_filesystem`.
fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// Root hive (persists the hive_scope `/`) + `/a` self-echoing source cell.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/a")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Register /sink (CaptureCell) BEFORE bootstrap (anti-cascade), then run
/// bootstrap. Returns the /sink capture receiver.
async fn boot_topology(h: &ColonyHandle, td: &TempDir) -> mpsc::Receiver<Message> {
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");
    sink_rx
}

/// Send a mutation and await the outcome.
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
        .unwrap();
    ack_rx.await.unwrap()
}

/// Probe to /a. The probe is a Source establishing `kind` in `context`
/// (Ingress-at-birth) so it survives the /a Echo-Cell hop — input.hop verfaellt
/// on cell emission, only context travels through (`carry_context_with_hop`); the
/// dedup edge condition (`context.kind == 'text'`) is evaluated at /a's emit.
/// Low ttl bounds the no-match self-loop.
fn probe_to_a() -> Message {
    let mut headers = Map::new();
    headers.insert("kind".into(), json!("text"));
    MessageBuilder::new(Path::new("/a"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .context(headers)
        .ttl(4)
        .build()
}

/// Bounded wait for ONE /sink receipt. `Some` if received within the timeout.
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// After the FIRST receipt is in hand, assert NO second receipt arrives within
/// a bounded window — the single-delivery proof.
async fn assert_no_second_receipt(rx: &mut mpsc::Receiver<Message>) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "single-delivery: /sink must receive the probe EXACTLY ONCE — a second \
         receipt means the duplicate edge was inserted (double delivery), got {got:?}"
    );
}

/// Count edges /a -> /sink persisted in colony.db (the duplicate-insert would
/// also push a second `InsertEdge` writer-op → row in the edges table).
fn db_edge_count(db_dir: &std::path::Path, from: &str, to: &str) -> i64 {
    let conn = rusqlite::Connection::open(db_dir.join("colony.db")).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
        rusqlite::params![from, to],
        |r| r.get(0),
    )
    .unwrap()
}

/// The same `add_edges` op applied twice (two identical entries in ONE diff
/// AND a second sequential mutation) inserts the edge exactly ONCE; a probe is
/// delivered exactly ONCE.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a1_duplicate_add_edges_inserts_once_and_delivers_once() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let mut sink_rx = boot_topology(&h, &td).await;

    // Mutation 1: TWO identical add_edges entries in ONE diff.
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_edges": [
                    {"from": "a", "to": "sink", "condition": "context.kind == 'text'"},
                    {"from": "a", "to": "sink", "condition": "context.kind == 'text'"}
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges (two identical entries) must commit, got {outcome:?}"
    );

    // Mutation 2: the SAME op again as a second sequential mutation.
    let outcome2 = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_edges": [
                    {"from": "a", "to": "sink", "condition": "context.kind == 'text'"}
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome2, MutationOutcome::Committed { .. }),
        "re-applied add_edges must commit (idempotent), got {outcome2:?}"
    );

    // PROOF 1: exactly ONE edge /a -> /sink persisted (drain the writer first by
    // a graceful shutdown so the fire-and-forget InsertEdge ops are flushed).
    h.shutdown().await;
    assert_eq!(
        db_edge_count(td.path(), "/a", "/sink"),
        1,
        "A1: three identical add_edges (2 in diff 1 + 1 in diff 2) must persist EXACTLY ONE edge"
    );

    // PROOF 2 (single delivery): reboot, send ONE probe, expect EXACTLY ONE
    // /sink receipt — not two. Reboot re-registers /sink + /a; the persisted
    // single edge carries the route.
    let h2 = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let (sink_tx2, mut sink_rx2) = mpsc::channel::<Message>(8);
    h2.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx2.clone())
    })
    .await;
    h2.spawn(Path::new("/a"), || {
        meclaw_testing::mocks::EchoMockCell::new(Path::new("/a")).echo_to(Path::new("/a"))
    })
    .await;

    h2.send(probe_to_a()).await;
    let first = recv_bounded(&mut sink_rx2).await;
    assert!(
        first.is_some(),
        "probe must route to /sink via the single edge"
    );
    assert_no_second_receipt(&mut sink_rx2).await;

    h2.shutdown().await;

    // Silence the unused first-boot receiver.
    let _ = &mut sink_rx;
}
