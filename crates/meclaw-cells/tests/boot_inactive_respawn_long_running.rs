//! Phase-13.5 Slice 4 T7b: the three production Long-Running factories
//! (`proxy`/`timer`/`mcp`) override `CellFactory::build_boot_inactive_respawn`
//! so a cell that booted INACTIVE obtains a REAL respawn closure. A later
//! `add_edges` reconnect calls that closure and the Long-Running task starts
//! IMMEDIATELY — no reboot needed (spec § Connectivity and activity,
//! `docs/meclaw-overview.md`: "long-running cells of the reactivated subtree are
//! started **immediately**").
//!
//! Honest-wiring assertion: each factory's hook must return `Some`, and the
//! returned closure must build a LIVE `cell_task_long_running` task (proven by
//! a real `JoinHandle` that is still running and exits cleanly only after the
//! mailbox is dropped). An inert no-op closure (the boot-gating fallback) would
//! return a task that finishes instantly and would never accept a message — so
//! the post-spawn liveness probe discriminates real-vs-inert.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::factory::McpCellFactory;
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::timer::factory::TimerCellFactory;
use meclaw_colony::{CellFactory, ContractView, RespawnFn};
use meclaw_core::{Body, CellEmission, JsonValue, MessageBuilder, Path, serde_json::json};
use mock_mcp::{MockMcp, canned_initialize, canned_tools_list};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Drive the boot-inactive hook for a single Long-Running factory and prove the
/// returned respawn closure spawns a live task that exits cleanly on mailbox
/// drop. Shared by the three per-factory tests so the assertion is identical.
async fn assert_hook_builds_live_task(factory: Arc<dyn CellFactory>, params: JsonValue) {
    let td = TempDir::new().unwrap();
    let cell_dir: PathBuf = td.path().join("ll");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);

    // T7b: the production factory must hand out a REAL respawn at boot-inactive
    // stage. Before the override this is `None` (default) → this `expect` fails
    // first (TDD red).
    let respawn: RespawnFn = factory
        .clone()
        .build_boot_inactive_respawn(
            Path::new("/ll"),
            params,
            out_tx,
            cell_dir,
            ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("Long-Running factory must return Some(respawn) for boot-inactive");

    // Boot-gating: building the hook must NOT have spawned a task yet. We can
    // only observe the closure spawn a real task by invoking it (this is exactly
    // what the reconnect arm's `(entry.respawn)()` does).
    let (sender, join, _peace_rx, _backstop_rx) = respawn();

    // Real-vs-inert discriminator: the closure must drive the genuine
    // `cell_task_long_running` path, NOT colony's inert boot-inactive fallback.
    // We assert via two honest, environment-independent signals:
    //
    // 1. The mailbox `Sender` is connected to a real handler receiver: the very
    //    first message is ACCEPTED by the channel (the inert fallback in
    //    `bootstrap_apply` returns a throwaway pair whose receiver is dropped at
    //    once). `try_send` (no await) keeps the probe race-free against any
    //    I/O-driven teardown of the spawned task (cell_task.rs select! arm).
    let msg = MessageBuilder::new(Path::new("/ll"))
        .body(Body::Inline(json!({})))
        .build();
    sender
        .try_send(msg)
        .expect("real task mailbox must accept the first message (not an inert dead pair)");

    // 2. Clean shutdown: dropping the mailbox closes the handler; the spawned
    //    task must JOIN without panicking. Proves a real `tokio::spawn`ed task
    //    was produced and is well-behaved on teardown.
    drop(sender);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("task must exit after mailbox close")
        .expect("task must not panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_boot_inactive_hook_builds_live_task() {
    assert_hook_builds_live_task(
        Arc::new(ProxyCellFactory),
        json!({ "bot_token": "T", "emit_to": "/x", "base_url": "http://127.0.0.1:1" }),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timer_boot_inactive_hook_builds_live_task() {
    assert_hook_builds_live_task(Arc::new(TimerCellFactory), json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_boot_inactive_hook_builds_live_task() {
    // Reachable mock provider: since the Befund-#9 fix, mcp's `run_io` panics
    // on an init error (supervision signal, Z.1369) — an unreachable endpoint
    // would trip the "must not panic" teardown assert instead of probing the
    // real-vs-inert discrimination this test is about.
    let server = MockMcp::start(vec![
        canned_initialize(),
        canned_tools_list(vec![
            meclaw_core::serde_json::json!({"name": "echo", "inputSchema": {"type": "object"}}),
        ]),
    ])
    .await;
    assert_hook_builds_live_task(
        Arc::new(McpCellFactory),
        json!({ "endpoint": server.endpoint() }),
    )
    .await;
}
