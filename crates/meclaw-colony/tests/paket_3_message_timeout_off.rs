//! Paket-3 P3-B-demos-rest — two end-to-end demos for the message_timeout
//! B-backstop: proving it is disabled by `0`/`-1` AND that LR cells are
//! exempt from it regardless of what is configured.
//!
//! # Demo requirement #3 — `cell.message_timeout: 0` and `-1` disable the backstop
//!
//! A `HangMockCell` with `sleep_ms: 300` would be killed by a positive
//! backstop of e.g. 200 ms.  With `cell.message_timeout: 0` (and separately
//! `-1`) the backstop is OFF — the handle() sleeps 300 ms, completes normally,
//! and the `/sink` CaptureCell receives the output.  No restart (spawn_count
//! stays at 1).
//!
//! # B4 pin — long-running cell ignores `cell.message_timeout`
//!
//! An LR cell (`ReceiptMockLongRunningCell` via `LongRunningReceiptFactory`)
//! is registered with `cell.message_timeout: 100` ms but its handle() sleeps
//! 400 ms.  Per spec `build_long_running_task` has no backstop wrapper — the
//! configured `message_timeout` is ignored for LR cells.  The handle
//! completes after 400 ms and the `/sink` CaptureCell receives the output.

use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::Value;
use meclaw_core::{MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::{EchoCellFactory, HangCellFactory, LongRunningReceiptFactory};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

// ─── shared helpers ───────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

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

/// Wait up to 30 s for `sink_rx` to receive at least one message.  Returns the
/// received message, or `None` on timeout (generous 30 s failure-marker per
/// CONTRIBUTING.md coding-standards).
async fn wait_for_sink(
    sink_rx: &mut mpsc::Receiver<meclaw_core::Message>,
) -> Option<meclaw_core::Message> {
    tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .ok()
        .flatten()
}

// ─── Demo requirement #3 ─────────────────────────────────────────────────────

/// Helper that runs one variant of the backstop-disabled proof.
///
/// Boots a topology with a single `hang` cell whose `cell.message_timeout` is
/// set to `timeout_value` (must be `0` or `-1` to disable the backstop).
/// The cell's `sleep_ms` is set to 300 ms — long enough that a small positive
/// backstop (~200 ms) would kill it, but fine when the backstop is off.
///
/// Positive receipt: `/sink` receives the emission that `HangMockCell` sends
/// after waking up.  Spawn-count stays at 1 (no restart).
async fn run_backstop_disabled_variant(timeout_value: i64) {
    let td = tempfile::TempDir::new().unwrap();

    // Root hive.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Anchor echo — activates /hang via edge.
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/hang"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // The hang cell: sleep_ms=300 ms in handle(); message_timeout set to
    // timeout_value (0 or -1 → backstop OFF); emits to /sink on completion.
    let config = format!(
        r#"{{"cell":{{"type":"hang","message_timeout":{}}},"params":{{"sleep_ms":300,"emitted_target":"/sink"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#,
        timeout_value
    );
    write(td.path(), "main/hang/config.json", &config);

    let spawn_count = Arc::new(AtomicU32::new(0));
    let hang_factory: Arc<dyn CellFactory> = Arc::new(HangCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let echo_factory: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("hang".to_string(), hang_factory.clone()),
            ("echo".to_string(), echo_factory.clone()),
        ],
    );

    // /sink resolved BEFORE bootstrap (anti-cascade: hang→/sink emits on
    // handle() completion; /sink must be live when the first emission lands).
    let (sink_tx, mut sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    registry.insert("hang".to_string(), hang_factory);
    registry.insert("echo".to_string(), echo_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // Activate /hang via an anchor→hang edge (edge-derived activity).
    let act = send_mutation(
        &h,
        json!({
            // W2b (Ruling A1): wire /hang -> /sink so the cell's post-sleep echo
            // reaches /sink (identity-fallback gone). Both endpoints live.
            "diff": { "add_edges": [{"from": "anchor", "to": "hang"}, {"from": "hang", "to": "sink"}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges anchor->hang must commit (timeout_value={timeout_value}), got {act:?}"
    );

    // Probe /hang → wake (Dormant → Awake); handle() sleeps 300 ms then
    // emits to /sink.  With message_timeout=0/-1 the backstop is OFF, so
    // the 300 ms sleep is not interrupted.
    h.send(MessageBuilder::new(Path::new("/hang")).build())
        .await;

    // Positive receipt: /sink receives the output within the 30-s window.
    let received = wait_for_sink(&mut sink_rx).await;
    assert!(
        received.is_some(),
        "Demo requirement #3 (timeout_value={timeout_value}): /sink must receive the hang cell's \
         emission after the 300 ms sleep — backstop is OFF, so the handle completes normally"
    );

    // No restart: spawn_count == 1 (initial wake only; a restart would bump it to 2).
    let sc = spawn_count.load(Ordering::Relaxed);
    assert_eq!(
        sc, 1,
        "Demo requirement #3 (timeout_value={timeout_value}): spawn_count must stay 1 \
         (no backstop-restart); got {sc}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_timeout_zero_and_minus_one_disable_backstop() {
    // timeout_value = 0 → backstop OFF
    run_backstop_disabled_variant(0).await;
    // timeout_value = -1 → backstop OFF
    run_backstop_disabled_variant(-1).await;
}

// ─── B4 pin: LR cell ignores message_timeout ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_running_cell_ignores_message_timeout() {
    let td = tempfile::TempDir::new().unwrap();

    // Root hive.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Anchor echo — activates /lr via edge so the colony routes probes to it.
    write(
        td.path(),
        "main/anchor/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/lr"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // LR cell: message_timeout=100 ms (small), but LR has no backstop wrapper →
    // the 400 ms handle() is NOT killed.  `emitted_target` is NOT in the FS config
    // here — it is injected via the factory (LongRunningReceiptFactory).
    write(
        td.path(),
        "main/lr/config.json",
        r#"{"cell":{"type":"lr_receipt","message_timeout":100},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // LongRunningReceiptFactory wired to sleep 400 ms and emit to /sink.
    let lr_factory: Arc<dyn CellFactory> = Arc::new(LongRunningReceiptFactory::new_with_sleep(
        400,
        Path::new("/sink"),
    ));
    let echo_factory: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("lr_receipt".to_string(), lr_factory.clone()),
            ("echo".to_string(), echo_factory.clone()),
        ],
    );

    // /sink resolved BEFORE bootstrap (anti-cascade).
    let (sink_tx, mut sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    registry.insert("lr_receipt".to_string(), lr_factory);
    registry.insert("echo".to_string(), echo_factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // Activate /lr via an edge (Active LR cells still need an edge to be
    // routable; they spawn eagerly on boot but the colony rejects inactive targets).
    let act = send_mutation(
        &h,
        json!({
            // W2b (Ruling A1): wire /lr -> /sink so the LR cell's receipt reaches
            // /sink (identity-fallback gone). Both endpoints live.
            "diff": { "add_edges": [{"from": "anchor", "to": "lr"}, {"from": "lr", "to": "sink"}] },
            "ctx": {},
            "scope": "/"
        }),
    )
    .await;
    assert!(
        matches!(act, MutationOutcome::Committed { .. }),
        "add_edges anchor->lr must commit, got {act:?}"
    );

    // Probe /lr — handle() sleeps 400 ms then emits to /sink.
    // message_timeout=100 ms is configured but LR cells have no B-backstop
    // wrapper (`build_long_running_task` does not apply one per spec) → the
    // 400 ms handle() completes normally.
    h.send(MessageBuilder::new(Path::new("/lr")).build()).await;

    // Positive receipt: /sink receives the emission within 30 s.
    let received = wait_for_sink(&mut sink_rx).await;
    assert!(
        received.is_some(),
        "B4 pin: /sink must receive the LR cell's emission after the 400 ms handle() — \
         message_timeout=100 ms is ignored for LR cells (no backstop wrapper)"
    );

    h.shutdown().await;
}
