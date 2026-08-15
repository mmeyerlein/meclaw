//! Phase-13.5 Hive-Transit Demo (a): a message whose `target` is a hive
//! scope-marker is evaluated as a logical transit node — Colony takes the
//! hive's out-edges (`from = <hive-path>`), evaluates the CEL `condition`
//! against the headers, and routes the message on to the matched `to`-cell.
//!
//! Topology:
//!   td/main/config.json   — root hive (`type: "hive"`). Its `params.graph`
//!                            declares a single out-edge `from="." to="./sink"`
//!                            gated on `hop.kind == 'text'`. `from="."`
//!                            resolves to the hive's own path `/`, so the edge
//!                            lives in the EdgeTable as `from=/ to=/sink`.
//!   /sink                 — CaptureCell (registry-only via `h.spawn`), the
//!                            terminal positive-receipt signal.
//!
//! Anti-Cascade discipline (Phase-6.5 lesson): /sink is registered via
//! `h.spawn(...)` BEFORE bootstrap so the transit target is resolved when the
//! hive-hop fires. The probe carries the `origin`-format body (Phase-6 lesson:
//! never `role`/`content`).
//!
//! Demo discipline: positive proof is a POSITIVE capture receipt on /sink; a
//! "not received" case is a BOUNDED timeout (never an unbounded `recv`).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::wait::wait_for_message_log_count;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Poll-then-drain the DLQ until an entry with `resolved_target == target`
/// appears (bounded). Dead-letters land in Colony's ring buffer asynchronously
/// after the cascade drains, so a single drain can race the routing loop — this
/// mirrors the `phase_13_5_a6_demo` poll-then-drain pattern. Returns ALL drained
/// entries (the snapshot at the moment the wanted target first showed up, or at
/// the deadline), so the caller can also assert on co-resident DLQ entries
/// (discriminating power).
async fn drain_until_target(h: &ColonyHandle, target: &str) -> Vec<meclaw_colony::DeadLetter> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = h.drain_dead_letters().await;
        let hit = snapshot
            .iter()
            .any(|d| d.resolved_target.as_str() == target);
        if hit || std::time::Instant::now() >= deadline {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Find the single DLQ entry whose `resolved_target` matches `target`.
fn dlq_entry_for<'a>(
    snapshot: &'a [meclaw_colony::DeadLetter],
    target: &str,
) -> &'a meclaw_colony::DeadLetter {
    let matches: Vec<_> = snapshot
        .iter()
        .filter(|d| d.resolved_target.as_str() == target)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one DLQ entry for resolved_target={target}; snapshot={snapshot:?}"
    );
    matches[0]
}

/// Write the FS topology: a root hive at `td/main` with an out-edge
/// `from="." to="./sink"` gated on `hop.kind == 'text'`.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./sink","condition":"hop.kind == 'text'"}
        ]}}}"#,
    )
    .unwrap();
}

/// One header entry shorthand.
fn headers_with(pairs: &[(&str, &str)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), json!(*v));
    }
    m
}

/// Build a probe addressed at the hive root `/` carrying the given headers and
/// an `origin`-format UBF body.
fn probe_to_hive(headers: Map<String, Value>) -> Message {
    MessageBuilder::new(Path::new("/"))
        .body(Body::Inline(
            json!({"origin":"user","type":"text","text":"ping"}),
        ))
        .hop(headers)
        .ttl(8)
        .build()
}

/// Bounded wait for a /sink receipt.
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Assert /sink does NOT receive within a bounded window.
async fn assert_not_received(rx: &mut mpsc::Receiver<Message>, ctx: &str) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "{ctx}: /sink must NOT receive (hive condition guard), got {got:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_to_hive_routes_via_cel_edge_to_cell() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    // Anti-Cascade: register /sink BEFORE the probe goes out.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Bootstrap persists the root hive_scope `/` + the transit edge.
    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    // Positive: kind=text → hive out-edge condition true → transit to /sink.
    h.send(probe_to_hive(headers_with(&[("kind", "text")])))
        .await;
    let got = recv_bounded(&mut sink_rx).await;
    assert!(
        got.is_some(),
        "message to hive / must transit to /sink via the CEL out-edge"
    );

    // Negative: kind=other → condition false → no out-edge matches → /sink
    // must NOT receive (the message dead-letters as hive_no_route instead).
    h.send(probe_to_hive(headers_with(&[("kind", "other")])))
        .await;
    assert_not_received(&mut sink_rx, "kind=other").await;

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (b) — hive_no_route is distinct from unresolved_path.
//
// A reachable hive whose out-edge condition evaluates to `false` (no matching
// out-edge) dead-letters as `hive_no_route` — NOT `unresolved_path`. The same
// test routes to a path that is neither a cell nor a hive and asserts that case
// dead-letters as `unresolved_path`. Both DLQ entries are inspected in one test
// to prove the discriminating power (F3): the hive WAS reachable, the graph just had no
// onward route; the bogus path was never reachable at all.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_no_route_when_no_edge_matches_distinct_from_unresolved_path() {
    let td = TempDir::new().unwrap();
    // Root hive `/` with a single out-edge gated on `hop.kind == 'text'`.
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./sink","condition":"hop.kind == 'text'"}
        ]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    // Anti-Cascade: register /sink BEFORE the probe goes out (even though the
    // no-route probe must never reach it — the positive demo proves the path).
    let (sink_tx, _sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    // (1) Probe at the reachable hive `/` with kind=other → out-edge condition
    // false → no out-edge matches → hive_no_route.
    h.send(probe_to_hive(headers_with(&[("kind", "other")])))
        .await;
    let snap = drain_until_target(&h, "/").await;
    let hive_dl = dlq_entry_for(&snap, "/");
    assert_eq!(
        hive_dl.reason.as_code(),
        "hive_no_route",
        "reachable hive with no matching out-edge → hive_no_route; got {:?}",
        hive_dl.reason
    );

    // (2) Probe at a path that is neither a cell nor a hive → unresolved_path.
    let bogus = MessageBuilder::new(Path::new("/nope"))
        .body(Body::Inline(
            json!({"origin":"user","type":"text","text":"ping"}),
        ))
        .ttl(8)
        .build();
    h.send(bogus).await;
    let snap2 = drain_until_target(&h, "/nope").await;
    let bogus_dl = dlq_entry_for(&snap2, "/nope");
    assert_eq!(
        bogus_dl.reason.as_code(),
        "unresolved_path",
        "non-existent path (no cell, no hive) → unresolved_path; got {:?}",
        bogus_dl.reason
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (c) — hive-to-hive chain routes end-to-end.
//
// Hive-A out-edge `to` Hive-B; Hive-B out-edge `to` `/sink` (CaptureCell).
// A probe at Hive-A with enough TTL transits two hive hops and `/sink` receives
// a POSITIVE capture receipt — the chain works over two transit hops.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_to_hive_chain_routes_end_to_end() {
    let td = TempDir::new().unwrap();
    // /hive_a (root hive `/`) → /hive_a/b (nested hive) → /sink.
    // Edges (unconditional → always match) live in each hive's params.graph.
    //   /  : from="." to="./b"     (root → nested hive)
    //   /b : from="." to="/sink"   (nested hive → sink cell)
    std::fs::create_dir_all(td.path().join("main/b")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./b"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    // Probe at the root hive `/` with ample TTL for two transit hops.
    let probe = MessageBuilder::new(Path::new("/"))
        .body(Body::Inline(
            json!({"origin":"user","type":"text","text":"ping"}),
        ))
        .ttl(8)
        .build();
    h.send(probe).await;

    let got = recv_bounded(&mut sink_rx).await;
    assert!(
        got.is_some(),
        "probe at hive-A must transit hive-A → hive-B → /sink (two transit hops)"
    );
    assert_eq!(
        got.unwrap().target,
        Path::new("/sink"),
        "terminal hop targets /sink"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (d) — hive cycle terminates via TTL (loop-guard, no runtime cycle
// detector).
//
// Hive-A `to` Hive-B, Hive-B `to` Hive-A (a 2-hive cycle). A probe at Hive-A
// with a small, defined TTL must NOT hang: each transit hop decrements TTL, and
// once TTL hits zero the message dead-letters as `ttl_expired`. The test itself
// is bounded (the drain loop has a 5s deadline) so a regression that fails to
// terminate surfaces as a timeout, not an infinite hang.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_cycle_terminates_with_ttl_expired() {
    let td = TempDir::new().unwrap();
    // /  (root hive) → /b ; /b → /  → cycle.
    std::fs::create_dir_all(td.path().join("main/b")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./b"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"/"}
        ]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    // Small TTL: each transit hop decrements it; the cycle dies within a few
    // hops. ttl=4 keeps the test fast while exercising several cycle laps.
    let probe = MessageBuilder::new(Path::new("/"))
        .body(Body::Inline(
            json!({"origin":"user","type":"text","text":"ping"}),
        ))
        .ttl(4)
        .build();
    h.send(probe).await;

    // The terminating hop dead-letters with ttl_expired. The drained snapshot is
    // bounded (5s deadline) — a non-terminating cycle would surface as the
    // "no ttl_expired entry" assertion below, never an infinite hang.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let ttl_dl = loop {
        let snap = h.drain_dead_letters().await;
        if let Some(dl) = snap
            .iter()
            .find(|d| d.reason.as_code() == "ttl_expired")
            .cloned()
        {
            break Some(dl);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        ttl_dl.is_some(),
        "hive cycle must terminate with a ttl_expired dead-letter (TTL loop-guard)"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (g) — transit-hop trace chain is gap-less (Reviewer-Auflage 1).
//
// Topology Cell → Hive → Cell: a source cell (/source, EchoMockCell) emits to a
// hive path `/hive`, whose single out-edge transits to a terminal cell /sink.
// After the run, the message_log is inspected:
//   1. a row exists with `to_path == /hive` (the transit hop IS logged,
//      Auflage-1 Variante B).
//   2. the parent_message_id chain is gap-less: no orphans (every non-NULL
//      parent_message_id references an existing message_log.id), and exactly one
//      root (parent IS NULL / @external).
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transit_hop_trace_chain_is_gapless() {
    let td = TempDir::new().unwrap();
    // Root hive `/` carries the hive `/hive` as a nested directory + the
    // transit edge /hive → /sink. /source is a registry-only EchoMockCell.
    std::fs::create_dir_all(td.path().join("main/hive")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/hive/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    // Anti-Cascade: /sink resolved before anything routes.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    // Source cell echoes to the hive path /hive.
    h.spawn(Path::new("/source"), || {
        EchoMockCell::new(Path::new("/source")).echo_to(Path::new("/hive"))
    })
    .await;
    // W2 (A1): /source → /hive now needs a wired edge (identity gone); the
    // hive's own out-edges then transit onward to /sink.
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/source"),
        Path::new("/hive"),
    )
    .await;

    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    // Probe at /source → source emits to /hive → hive transits to /sink.
    let probe = MessageBuilder::new(Path::new("/source"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .ttl(8)
        .build();
    let trace_id = probe.trace_id.to_string();
    h.send(probe).await;

    // Positive proof first: /sink receives.
    let got = recv_bounded(&mut sink_rx).await;
    assert!(got.is_some(), "Cell → Hive → Cell must deliver to /sink");

    // Three hops: external→/source, /source→/hive, /hive→/sink.
    let db_path = h.tempdir_path().join("colony.db");
    wait_for_message_log_count(&db_path, &trace_id, 3, Duration::from_secs(5)).await;
    h.shutdown().await;

    assert_trace_gapless_with_hive_hop(&db_path, &trace_id, "/hive");
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (g, second) — gap-less trace over a Hive → Hive chain (two transit hops).
//
// Topology Cell → Hive-A → Hive-B → Cell. Proves the parent chain stays gap-less
// across two consecutive transit hops, and that BOTH hive hops are logged.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transit_hop_trace_chain_is_gapless_over_hive_chain() {
    let td = TempDir::new().unwrap();
    // Root hive `/` ; nested hive /a (transit → /a/b) ; nested hive /a/b
    // (transit → /sink). /source echoes to /a.
    std::fs::create_dir_all(td.path().join("main/a/b")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./b"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/a/b/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/source"), || {
        EchoMockCell::new(Path::new("/source")).echo_to(Path::new("/a"))
    })
    .await;
    // W2 (A1): /source → /a now needs a wired edge (identity gone); the hive
    // chain /a → /a/b → /sink transits onward via the hives' own out-edges.
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/source"),
        Path::new("/a"),
    )
    .await;

    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    let probe = MessageBuilder::new(Path::new("/source"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .ttl(8)
        .build();
    let trace_id = probe.trace_id.to_string();
    h.send(probe).await;

    let got = recv_bounded(&mut sink_rx).await;
    assert!(
        got.is_some(),
        "Cell → Hive-A → Hive-B → Cell must deliver to /sink"
    );

    // Four hops: external→/source, /source→/a, /a→/a/b, /a/b→/sink.
    let db_path = h.tempdir_path().join("colony.db");
    wait_for_message_log_count(&db_path, &trace_id, 4, Duration::from_secs(5)).await;
    h.shutdown().await;

    // Both hive hops are logged.
    assert_trace_gapless_with_hive_hop(&db_path, &trace_id, "/a");
    assert_trace_gapless_with_hive_hop(&db_path, &trace_id, "/a/b");
}

/// Assert (1) the transit hive-hop is logged (a row with `to_path == hive_path`)
/// and (2) the `parent_message_id` chain is gap-less: exactly one NULL root and
/// no orphans (every non-NULL parent references an existing message_log.id).
/// SQL columns per `phase_5_quiescence_routing`: `to_path`, `parent_message_id`,
/// `id`, `trace_id`.
fn assert_trace_gapless_with_hive_hop(db_path: &std::path::Path, trace_id: &str, hive_path: &str) {
    let conn = rusqlite::Connection::open(db_path).unwrap();

    // (1) The transit hive-hop IS logged.
    let hive_hops: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ?1 AND to_path = ?2",
            rusqlite::params![trace_id, hive_path],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        hive_hops >= 1,
        "the transit hop to {hive_path} must be present in message_log"
    );

    // (2a) Exactly one trace root (parent_message_id IS NULL → the @external
    // source message).
    let null_roots: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ? AND parent_message_id IS NULL",
            [trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        null_roots, 1,
        "exactly one trace root (parent_message_id IS NULL) over the hive chain"
    );

    // (2b) No orphans: every non-NULL parent references an existing row.
    let orphans: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log m
             WHERE m.trace_id = ?1 AND m.parent_message_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM message_log p WHERE p.id = m.parent_message_id)",
            [trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        orphans, 0,
        "no orphan hops — parent chain is gap-less through the hive transit hop(s)"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (e) — add_edges mutation with a HIVE endpoint is accepted AND fires.
//
// Validation-symmetry (Phase 13.5 step-6, Teil A): an `add_edges` mutation may
// reference a hive by its short-name as an edge endpoint. The mutation must
// commit (not reject with edge_schema), AND the edge must FIRE: a probe at the
// source cell `/a` emits, the mutation edge /a -> /hub routes the message at the
// hive `/hub`, which transits via its bootstrap out-edge to the internal cell
// `/hub/inner` (CaptureCell). Positive proof is a POSITIVE capture receipt on
// `/hub/inner` — not merely "committed".
//
// Non-root hive on purpose: the short-name of the root hive `/` is "" (empty);
// `/hub` has the non-empty, unique short-name "hub".
// ───────────────────────────────────────────────────────────────────────────

/// `(name, factory)` list with `echo` registered (the colony opens colony.db
/// with the same registry; `/a` is a bootstrap echo cell).
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_edges_mutation_with_hive_endpoint_accepted_and_fires() {
    let td = TempDir::new().unwrap();
    // FS topology under main/:
    //   /        root hive (no out-edges)
    //   /a       echo cell, echo_to=/a (self-loop fallback if no edge fires)
    //   /hub     nested hive, bootstrap out-edge from="." to="./inner"
    //   /hub/inner   internal cell — registered as a CaptureCell via h.spawn
    std::fs::create_dir_all(td.path().join("main/a")).unwrap();
    std::fs::create_dir_all(td.path().join("main/hub")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/hub/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./inner"}
        ]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // Anti-Cascade: register the hive-internal cell /hub/inner BEFORE bootstrap
    // so the hive's transit target is resolved when the edge fires.
    let (inner_tx, mut inner_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/hub/inner"), move || {
        CaptureCell::new(inner_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // The mutation edge references the hive `/hub` by its short-name "hub".
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_edges": [
                    {"from": "a", "to": "hub"}
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges with a hive endpoint must commit (validation-symmetry), got {outcome:?}"
    );

    // FIRE proof: probe /a → echo emits → mutation edge /a -> /hub fires →
    // hive /hub transits via its bootstrap out-edge to /hub/inner → receipt.
    let probe = MessageBuilder::new(Path::new("/a"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .ttl(8)
        .build();
    h.send(probe).await;

    let got = recv_bounded(&mut inner_rx).await;
    assert!(
        got.is_some(),
        "mutation-added edge to a HIVE endpoint must fire: /a -> /hub -> /hub/inner"
    );
    assert_eq!(
        got.unwrap().target,
        Path::new("/hub/inner"),
        "terminal hop targets the hive-internal cell"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Substrat-Fix Befund 2 — add_edges that forms a HIVE-involved cycle COMMITS.
//
// Two nested hives /x and /y. An `add_edges` mutation declaring x -> y AND
// y -> x forms a 2-hive cycle in the post-state graph. Spec overview
// § Validation: meclaw-Core does not reject cycles generally — the mutation
// COMMITS. Endpoint-existence still applies (both hive short-names "x"/"y" are
// valid Cell ∪ Hive endpoints); the runtime TTL loop-guard
// (`hive_cycle_terminates_with_ttl_expired`) bounds any traversal.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_edges_hive_cycle_commits() {
    let td = TempDir::new().unwrap();
    // Root hive `/` + two nested hives /x and /y (no out-edges of their own).
    std::fs::create_dir_all(td.path().join("main/x")).unwrap();
    std::fs::create_dir_all(td.path().join("main/y")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/x/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/y/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // x -> y -> x: a 2-hive cycle in the post-state graph. Hive short-names
    // "x"/"y" are valid endpoints (Cell ∪ Hive); cycles are tolerated.
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_edges": [
                    {"from": "x", "to": "y"},
                    {"from": "y", "to": "x"}
                ]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "hive cycle must commit (cycles tolerated, Befund 2), got {outcome:?}"
    );

    h.shutdown().await;
}

// ───────────────────────────────────────────────────────────────────────────
// Demo (f) — a hive-transit edge with a CEL `condition` survives a colony
// reboot (Durable-Edges × Hive-Transit combination).
//
// This is the joint proof: the Durable-Edges slice persists an edge's
// `condition`/`modifier` into `colony.db` so it survives a reboot; Hive-Transit
// routes a message addressed at a hive scope-marker via that hive's out-edges.
// Demo (f) shows BOTH at once — a Root-Hive `/` out-edge `from="." to="./sink"`
// gated on `hop.kind == 'text'` is bootstrapped, the colony is shut down,
// and a FRESH colony is booted on the SAME `colony.db`. The edge is HYDRATED
// from `colony.db` (the second boot classifies as `BootState::Reboot` and
// ignores the `params.graph` hint), and the CEL condition still gates the
// transit correctly:
//   - kind=text  → condition true  → transit to /sink (POSITIVE capture receipt).
//   - kind=other → condition false → no out-edge → /sink does NOT receive.
//
// If the condition had NOT been persisted durably (rehydrated as an identity /
// None edge), the kind=other counter-probe would ALSO transit to /sink — the
// negative assertion below is what proves the CEL survived as a real condition,
// not a silently-dropped identity edge.
//
// Reboot mechanism is exactly the durable_edges_demo pattern: boot via
// `bootstrap_from_filesystem` (persists hive_scope `/` + the transit edge),
// graceful shutdown, then a fresh `ColonyHandle` on the SAME directory. The
// second boot does NOT re-run bootstrap — the persisted EdgeTable + HiveScope
// table carry the topology across the reboot. `/sink` is re-registered via
// `h.spawn` BEFORE the post-reboot probe (Anti-Cascade).
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_transit_edge_with_cel_survives_reboot() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    // ── Boot 1 (first boot, bootstrap persists hive_scope `/` + transit edge) ─
    let h = ColonyHandle::new_with_factories_at(&td, vec![]);

    // Anti-Cascade: register /sink BEFORE bootstrap so the transit target is
    // resolved when the hive-hop fires.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // Pre-reboot sanity: kind=text → hive out-edge condition true → transit.
    h.send(probe_to_hive(headers_with(&[("kind", "text")])))
        .await;
    let got = recv_bounded(&mut sink_rx).await;
    assert!(
        got.is_some(),
        "Boot 1: message to hive / must transit to /sink via the CEL out-edge"
    );

    h.shutdown().await;

    // ── Boot 2 (reboot at the SAME directory — edge HYDRATED from colony.db) ──
    // FRESH colony on the SAME colony.db. bootstrap is NOT re-run: the second
    // boot classifies as Reboot and hydrates hive_scope `/` + the transit edge
    // (with its persisted CEL condition) from colony.db. params.graph is a hint
    // that is ignored after the first bootstrap.
    let h2 = ColonyHandle::new_with_factories_at(&td, vec![]);

    // Anti-Cascade: re-register /sink (the in-memory registry resets on reboot)
    // BEFORE the probe goes out.
    let (sink_tx2, mut sink_rx2) = mpsc::channel::<Message>(8);
    h2.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx2.clone())
    })
    .await;

    // PROOF: the CEL condition survived the reboot — kind=text still transits.
    h2.send(probe_to_hive(headers_with(&[("kind", "text")])))
        .await;
    let got = recv_bounded(&mut sink_rx2).await;
    assert!(
        got.is_some(),
        "Boot 2: kind=text must transit to /sink — hive-transit CEL edge survived reboot"
    );

    // Counter-probe: kind=other → condition false → no out-edge matches → /sink
    // must NOT receive. This is what proves the condition was persisted as a
    // REAL condition (not silently rehydrated as an identity/None edge, which
    // would route unconditionally).
    h2.send(probe_to_hive(headers_with(&[("kind", "other")])))
        .await;
    assert_not_received(&mut sink_rx2, "Boot 2 kind=other").await;

    h2.shutdown().await;
}
