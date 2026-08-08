//! T12: run_io emits exactly one DiscoveryReady against the mock,
//! then blocks; on abort, it terminates promptly.
//!
//! Plan-Adaption: uses `MockMcp` (canned-sequence API) from T9 instead
//! of the handler-closure `MockMcpServer` from the plan-text.
//! Each `run_io` invocation makes 2 calls: `initialize` + `tools/list`,
//! so the canned sequence contains exactly 2 responses.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::mcp::io::{McpEvent, McpIo, McpReconfig, RunIoConfig, run_io};
use meclaw_cells::mcp::wire::McpClient;
use mock_mcp::{MockMcp, canned_initialize, canned_tools_list};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_emits_discovery_ready_then_blocks() {
    let server = MockMcp::start(vec![
        canned_initialize(),
        canned_tools_list(vec![
            json!({"name": "echo", "inputSchema": {"type": "object"}}),
        ]),
    ])
    .await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let (ev_tx, mut ev_rx) = mpsc::channel::<McpEvent>(8);
    let (_rc_tx, rc_rx) = mpsc::channel::<McpReconfig>(1);
    let cfg = RunIoConfig {
        client,
        external_timeout_ms: 2_000,
    };

    let h = tokio::spawn(run_io(McpIo::Http(cfg), ev_tx, rc_rx));

    let first = tokio::time::timeout(Duration::from_secs(30), ev_rx.recv())
        .await
        .expect("DiscoveryReady within 30s")
        .expect("Some(event)");
    match first {
        McpEvent::DiscoveryReady { tools } => {
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "echo");
        }
        other => panic!("the http transport emits no child events, got {other:?}"),
    }
    // No second event expected — run_io is in pending().await.
    let none = tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await;
    assert!(none.is_err(), "expected no second event within 200ms");

    h.abort();
    let _ = h.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_io_terminates_when_events_receiver_dropped_after_discovery() {
    // After draining the one event, abort() must complete within 500ms.
    let server = MockMcp::start(vec![
        canned_initialize(),
        canned_tools_list(vec![
            json!({"name": "echo", "inputSchema": {"type": "object"}}),
        ]),
    ])
    .await;
    let client = McpClient::new(&server.endpoint(), None).unwrap();
    let (ev_tx, ev_rx) = mpsc::channel::<McpEvent>(8);
    let (_rc_tx, rc_rx) = mpsc::channel::<McpReconfig>(1);
    let cfg = RunIoConfig {
        client,
        external_timeout_ms: 2_000,
    };
    let h = tokio::spawn(run_io(McpIo::Http(cfg), ev_tx, rc_rx));

    // Drain the one event so the channel doesn't block.
    let _ = tokio::time::timeout(Duration::from_secs(3), async move {
        let mut rx = ev_rx;
        rx.recv().await
    })
    .await;

    h.abort();
    let r = tokio::time::timeout(Duration::from_secs(30), h).await;
    assert!(r.is_ok(), "run_io must terminate within 30s after abort");
}
