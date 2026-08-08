//! Phase-13.5-A6 demo: cell→/colony routing, full symmetry via the cell-emit path.
//!
//! Three proof cases with EmitOnceMockCell (cell emit, NO manual reply_to):
//!   1. The cell emits /colony/mutations with a valid add_edges diff → reply
//!      {mutation: {outcome: "committed"}} arrives at the cell.
//!   2. The cell emits /colony/registry → reply {registry: [...]} at the cell.
//!   3. The cell emits /colony/bogus → a ColonyEndpointUnimplemented DLQ entry
//!      with sender = the cell path (auto-stamped via
//!      OutputSink/build_follow_up_with per spec Z.891).
//!
//! Mechanism proof: OutputSink fills CellEmission.sender_path on the cell emit;
//! the outputs arm builds the reply with reply_to=sender_path. The probe sends
//! NO manual reply_to — the auto-stamp becomes the mechanism on the reply.
//!
//! Anti-cascade discipline (phase-6.5 lesson): /probe is registered via
//! `register_spawned` BEFORE `bootstrap_from_filesystem`, so that the reply path
//! to /probe resolves.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::{ColonyHandle, EmitOnceMockCellFactory};
use std::fs::{self, create_dir_all};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Helper: spawn an `EmitOnceMockCellFactory`-Cell on `path` (registered
/// dormant in Colony, will be woken on first message).
///
/// `cell_dir` must exist — we use a dedicated temp sub-directory so that
/// `open_or_create_cell_db` has a clean path.
async fn spawn_emit_once_probe(
    h: &ColonyHandle,
    cell_dir: std::path::PathBuf,
    factory: EmitOnceMockCellFactory,
) {
    let outputs_tx = h.outputs_sender();
    let inbox_tx = h.inbox_tx.clone();
    let factory: Arc<EmitOnceMockCellFactory> = Arc::new(factory);
    let spawned = factory
        .spawn_cell(
            Path::new("/probe"),
            json!({}),
            outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("EmitOnceMockCellFactory spawn_cell");
    h.register_spawned(Path::new("/probe"), spawned).await;
}

fn body_as_value(msg: &meclaw_core::Message) -> &Value {
    match &msg.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("expected inline body, got blob ref"),
    }
}

// ---------------------------------------------------------------------------
// Test 1: Mutation Round-Trip via Cell-Emit (committed, deterministic).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a6_mutation_round_trip_committed_via_cell_emit() {
    // Topology:
    //   /a, /b — echo cells (existing nodes, so that add_edges validates).
    //   /probe — EmitOnceMockCell, emits on the first input a
    //            /colony/mutations cell output with a valid add_edges diff.
    //   Expected: /probe.capture_rx receives a reply with
    //   body.mutation.outcome="committed".
    //
    // NO manual reply_to in the probe — the auto-stamp via OutputSink
    // (spec Z.891) is the proof mechanism.

    let td = TempDir::new().unwrap();
    create_dir_all(td.path().join("main/a")).unwrap();
    create_dir_all(td.path().join("main/b")).unwrap();
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/b/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new();

    // /probe needs its own cell_dir (outside td, so the bootstrap walk does not
    // see it).
    let probe_dir = h.tempdir_path().join("probe_dir");
    std::fs::create_dir_all(&probe_dir).unwrap();

    let (capture_tx, mut capture_rx) = mpsc::channel(8);
    // Note: add_edges uses **node names** (last path segment), not full paths
    // — validator iterates `registry.keys()` and strips to last segment
    // (colony.rs:1324-1327). So /a → "a", /b → "b".
    let initial_emit_content = json!({
        "messages": [],
        "diff": {"add_edges": [{"from": "a", "to": "b"}]},
        "scope": "/",
        "ctx": {}
    });
    let factory = EmitOnceMockCellFactory::new(
        Path::new("/colony/mutations"),
        initial_emit_content,
        capture_tx,
    );

    // Anti-Cascade: /probe VOR bootstrap registrieren.
    spawn_emit_once_probe(&h, probe_dir, factory).await;

    let mut factories: CellFactoryRegistry = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("bootstrap");

    // Trigger-probe an /probe — Inhalt egal, EmitOnceMockCell ignoriert ihn.
    let trigger = MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin":"user","type":"text","text":"trigger"}]}),
        ))
        .build();
    h.send(trigger).await;

    let reply = match tokio::time::timeout(Duration::from_secs(30), capture_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/probe capture_rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!(
                "A6-E2E MUST-FIX #1 BLOCKED: /probe didn't receive mutation-reply within 3s; DLQ: {dlq:?}"
            );
        }
    };
    let reply_body = body_as_value(&reply);
    let mutation = reply_body
        .get("mutation")
        .unwrap_or_else(|| panic!("reply has 'mutation' slot per F5; body={reply_body:?}"));
    let outcome = mutation.get("outcome").and_then(|v| v.as_str());
    assert_eq!(
        outcome,
        Some("committed"),
        "A6-E2E: mutation reply has outcome=committed (valid add_edges /a→/b); got: {outcome:?} body={reply_body:?}"
    );
    assert!(
        mutation.get("id").is_some(),
        "committed reply has mutation-id; mutation={mutation:?}"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 2: Read Round-Trip /colony/registry via Cell-Emit.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a6_read_registry_round_trip_via_cell_emit() {
    // EmitOnceMockCell emit /colony/registry → Reply {registry: [...]}
    // arrives at the cell via the auto reply_to stamp.

    let td = TempDir::new().unwrap();
    create_dir_all(td.path().join("main/a")).unwrap();
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    fs::write(
        td.path().join("main/a/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new();
    let probe_dir = h.tempdir_path().join("probe_dir");
    std::fs::create_dir_all(&probe_dir).unwrap();

    let (capture_tx, mut capture_rx) = mpsc::channel(8);
    let factory = EmitOnceMockCellFactory::new(
        Path::new("/colony/registry"),
        json!({"messages": [], "query": {"limit": 100}}),
        capture_tx,
    );
    spawn_emit_once_probe(&h, probe_dir, factory).await;

    let mut factories: CellFactoryRegistry = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("bootstrap");

    let trigger = MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"trigger"}]}),
        ))
        .build();
    h.send(trigger).await;

    let reply = match tokio::time::timeout(Duration::from_secs(30), capture_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/probe capture_rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("A6-E2E: /probe didn't receive registry-reply within 3s; DLQ: {dlq:?}");
        }
    };
    let reply_body = body_as_value(&reply);
    let registry = reply_body
        .get("registry")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("reply has 'registry' array slot; body={reply_body:?}"));
    let paths: Vec<&str> = registry
        .iter()
        .filter_map(|e| e.get("path").and_then(|p| p.as_str()))
        .collect();
    assert!(
        paths.contains(&"/a"),
        "A6-E2E: /colony/registry reply via cell-emit includes /a; got: {paths:?}"
    );
    assert!(
        paths.contains(&"/probe"),
        "A6-E2E: /probe self appears in registry; got: {paths:?}"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// Test 3: unknown-endpoint DLQ with sender_path = the cell path.
//
// CRITICAL: must-fix #1 — sender_path in the DLQ entry MUST be /probe.
// Proves the auto reply_to stamp via OutputSink/build_follow_up_with (spec Z.891).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_13_5_a6_unknown_endpoint_dlq_sender_is_cell_path() {
    let td = TempDir::new().unwrap();
    create_dir_all(td.path().join("main")).unwrap();
    fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new();
    let probe_dir = h.tempdir_path().join("probe_dir");
    std::fs::create_dir_all(&probe_dir).unwrap();

    let (capture_tx, _capture_rx) = mpsc::channel(8);
    let factory = EmitOnceMockCellFactory::new(
        Path::new("/colony/bogus"),
        json!({"messages": []}),
        capture_tx,
    );
    spawn_emit_once_probe(&h, probe_dir, factory).await;

    let factories: CellFactoryRegistry = CellFactoryRegistry::new();
    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("bootstrap");

    let trigger = MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"trigger"}]}),
        ))
        .build();
    h.send(trigger).await;

    // The dispatcher needs a moment. Under workspace load (default test-threads)
    // the Tokio scheduler pressure is higher — deterministic poll-then-drain
    // instead of a fixed sleep, otherwise the test flakes (see the phase-8/
    // phase-11/phase-13 50-cell class of timing-fragile demos).
    let bogus_dlq;
    let dlq;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = h.drain_dead_letters().await;
        let matched: Vec<_> = snapshot
            .iter()
            .filter(|d| d.resolved_target.as_str() == "/colony/bogus")
            .cloned()
            .collect();
        if !matched.is_empty() || std::time::Instant::now() >= deadline {
            bogus_dlq = matched;
            dlq = snapshot;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let bogus_dlq: Vec<_> = bogus_dlq.iter().collect();
    assert_eq!(
        bogus_dlq.len(),
        1,
        "A6-E2E: /colony/bogus produces exactly one DLQ entry via cell-emit; got {} entries; dlq={dlq:?}",
        bogus_dlq.len()
    );
    let entry = &bogus_dlq[0];
    assert!(
        matches!(
            entry.reason,
            meclaw_colony::DeadLetterReason::ColonyEndpointUnimplemented
        ),
        "A6-E2E: DLQ reason is ColonyEndpointUnimplemented; got: {:?}",
        entry.reason
    );
    // CRITICAL — must-fix #1 proof:
    assert_eq!(
        entry.sender_path.as_str(),
        "/probe",
        "A6-E2E-CRITICAL: DLQ sender_path is the cell path (auto-stamped via OutputSink); got: {:?}",
        entry.sender_path
    );

    h.shutdown().await;
}
