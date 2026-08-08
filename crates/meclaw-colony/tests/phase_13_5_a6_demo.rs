//! Phase-13.5-A6 Demo: Cell→/colony-Routing volle Symmetrie via Cell-Emit-Pfad.
//!
//! Drei Beweis-Cases mit EmitOnceMockCell (Cell-Emit, KEIN manuelles reply_to):
//!   1. Cell emittiert /colony/mutations mit valider add_edges-Diff → Reply
//!      {mutation: {outcome: "committed"}} bei der Cell.
//!   2. Cell emittiert /colony/registry → Reply {registry: [...]} bei der Cell.
//!   3. Cell emittiert /colony/bogus → ColonyEndpointUnimplemented DLQ
//!      mit sender = Cell-pfad (Auto-stempel via OutputSink/build_follow_up_with
//!      pro spec Z.891).
//!
//! Mechanismus-Beweis: OutputSink füllt CellEmission.sender_path beim
//! Cell-emit; Outputs-Arm baut Reply mit reply_to=sender_path. Probe sendet
//! KEIN manuelles reply_to — der Auto-Stempel wird beim Reply der Mechanismus.
//!
//! Anti-Cascade-Disziplin (Phase-6.5-Lesson): /probe wird via
//! `register_spawned` VOR `bootstrap_from_filesystem` registriert, damit
//! der Reply-Pfad an /probe resolved ist.

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
/// `cell_dir` muss existieren — wir nutzen ein eigenes Temp-Sub-Verzeichnis,
/// damit `open_or_create_cell_db` einen sauberen Pfad hat.
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
    // Topologie:
    //   /a, /b — echo-Cells (existierende Nodes, damit add_edges validiert).
    //   /probe — EmitOnceMockCell, emittiert beim ersten Input einen
    //            /colony/mutations-CellOutput mit valider add_edges-Diff.
    //   Erwartet: /probe.capture_rx empfängt Reply mit
    //   body.mutation.outcome="committed".
    //
    // KEIN manuelles reply_to in der Probe — Auto-Stempel via OutputSink
    // (Spec Z.891) ist der Beweis-Mechanismus.

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

    // /probe braucht ein eigenes cell_dir (außerhalb von td, damit der
    // bootstrap-walk es nicht sieht).
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
    // landet bei der Cell via auto reply_to-Stempel.

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
// Test 3: Unknown-Endpunkt DLQ mit sender_path = Cell-Pfad.
//
// CRITICAL: must-fix #1 — sender_path im DLQ-Entry MUSS /probe sein.
// Beweist Auto-reply_to-Stempel via OutputSink/build_follow_up_with (Spec Z.891).
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

    // Dispatcher braucht einen Moment. Unter Workspace-Last (default
    // test-threads) ist der Tokio-Scheduler-Druck größer — deterministisches
    // poll-then-drain statt fixed-sleep, sonst flake'd der Test (siehe Phase-8/
    // Phase-11/Phase-13-50-cell-Klasse der timing-fragilen demos).
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
        "A6-E2E-CRITICAL: DLQ sender_path is the Cell-pfad (auto-stempel via OutputSink); got: {:?}",
        entry.sender_path
    );

    h.shutdown().await;
}
