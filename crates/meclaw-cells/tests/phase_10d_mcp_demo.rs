//! Phase-10-D Demo-Tests. T24: Tool-Call Round-Trip (success + failure).
//!
//! Anti-cascade duty: `/sink` MUST be registered before the probe at `/mcp` —
//! otherwise the emission ends up on the TtlExpired dead-letter path.
//!
//! Plan-Adaptionen (dokumentiert per Auftrag):
//! - `MockMcp::start(Vec<MockResponse>)` instead of a handler-closure API.
//! - `h.spawn(path, closure)` for `/sink` (CaptureCell via a direct mpsc-channel impl).
//! - Factory-Spawn via `factory.spawn_cell(...)` + `h.register_spawned(...)`.
//! - Routing via `reply_to` in the probe (no `h.add_edge`).
//! - `h.runtime().outputs_tx` (not `h.outputs_tx()`).
//! - `mpsc::channel::<Message>` + `recv_rx.recv()` instead of `CaptureCell::wait_for`.
//! - `MessageBuilder::new(Path).body(Body::Inline(json!)).build()` for probes.

#[path = "mock_mcp.rs"]
mod mock_mcp;

use meclaw_cells::McpCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_mcp::{
    MockMcp, canned_initialize, canned_rpc_error, canned_tools_call_text, canned_tools_list,
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// T24 — Demo 1: Tool-Call Round-Trip (success + failure)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_1_tool_call_success_and_failure_via_sink() {
    // Mock sequence: 1) initialize (from run_io), 2) tools/list, 3) probe-success, 4) probe-failure.
    let server = MockMcp::start(vec![
        canned_initialize(),
        canned_tools_list(vec![json!({"name":"echo","inputSchema":{"type":"object"}})]),
        canned_tools_call_text("OK"),
        canned_rpc_error(-32000, "kapow"),
    ])
    .await;

    let h = ColonyHandle::new();

    // 1. /sink first (Anti-Cascade).
    let (recv_tx, mut recv_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    // 2. /mcp via factory.
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("mcp");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(McpCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/mcp"),
            json!({"endpoint": server.endpoint(), "external_timeout_ms": 5000, "query_timeout_ms": 1000}),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/mcp"), spawned).await;
    // W2 (A1): /mcp reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/mcp"),
        Path::new("/sink"),
    )
    .await;

    // 3. Allow run_io to complete initialize + tools/list before probes.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 4. Probe 1: success.
    let inner_ok = json!({"name":"echo","arguments":{"text":"hi"}}).to_string();
    h.send(
        MessageBuilder::new(Path::new("/mcp"))
            .reply_to(Path::new("/sink"))
            .body(Body::Inline(json!({"messages":[
                {"origin":"assistant","type":"tool_call","text": inner_ok,"id":"call_1"}
            ]})))
            .build(),
    )
    .await;

    // 5. Probe 2: failure.
    let inner_err = json!({"name":"boom","arguments":{}}).to_string();
    h.send(
        MessageBuilder::new(Path::new("/mcp"))
            .reply_to(Path::new("/sink"))
            .body(Body::Inline(json!({"messages":[
                {"origin":"assistant","type":"tool_call","text": inner_err,"id":"call_2"}
            ]})))
            .build(),
    )
    .await;

    // 6. Collect two emissions at /sink within 3s.
    let m1 = tokio::time::timeout(Duration::from_secs(30), recv_rx.recv())
        .await
        .expect("timeout waiting for receipt 1")
        .expect("channel closed before receipt 1");
    let m2 = tokio::time::timeout(Duration::from_secs(30), recv_rx.recv())
        .await
        .expect("timeout waiting for receipt 2")
        .expect("channel closed before receipt 2");

    // Helper: extract inline body.
    let body_of = |m: &Message| -> serde_json::Value {
        match &m.body {
            Body::Inline(v) => v.clone(),
            Body::Blob(_) => panic!("expected inline body"),
        }
    };
    let b1 = body_of(&m1);
    let _b2 = body_of(&m2);

    // Per MecLaw header-merge spec: cell-emitted `header` block is stripped
    // from the body and merged into msg.headers by the colony router.
    // Assert headers via m1.headers / m2.headers; body via b1["messages"].

    // Receipt 1 — success:
    assert_eq!(
        m1.headers.hop.get("mcp_tool").and_then(|v| v.as_str()),
        Some("echo"),
        "m1 mcp_tool header mismatch"
    );
    assert!(
        m1.headers
            .hop
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .is_some(),
        "m1 duration_ms header missing"
    );
    assert!(
        m1.headers.hop.get("error_code").is_none(),
        "m1 must not have error_code header; got: {:?}",
        m1.headers.hop.get("error_code")
    );
    let msgs1 = b1["messages"].as_array().expect("b1 messages array");
    let t1 = msgs1.last().expect("b1 messages non-empty").clone();
    assert_eq!(t1["type"], "tool_result", "b1 last turn type");
    assert_eq!(t1["id"], "call_1", "b1 last turn id");
    assert!(
        t1["text"].as_str().unwrap_or("").contains("OK"),
        "b1 text must contain 'OK'; got: {}",
        t1["text"]
    );

    // Receipt 2 — failure:
    assert_eq!(
        m2.headers.hop.get("mcp_tool").and_then(|v| v.as_str()),
        Some("boom"),
        "m2 mcp_tool header mismatch"
    );
    assert_eq!(
        m2.headers.hop.get("error_code").and_then(|v| v.as_str()),
        Some("mcp_error"),
        "m2 error_code header mismatch"
    );

    h.shutdown().await;
}

// ---------------------------------------------------------------------------
// T25 — Demo 2: Discovery (Cache-Probe + __list_tools__)
// ---------------------------------------------------------------------------

use meclaw_cells::mcp::db::load_discovery_cache;
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_2_discovery_cache_and_list_tools_emit() {
    // Mock sequence: only 2 responses — initialize + tools/list (from run_io).
    // __list_tools__ probe does NOT go to the mock (handled in-Cell from cache).
    let server = MockMcp::start(vec![
        canned_initialize(),
        canned_tools_list(vec![
            json!({"name":"echo","inputSchema":{"type":"object"}}),
            json!({"name":"add","inputSchema":{"type":"object","properties":{"a":{"type":"number"}}}}),
        ]),
    ])
    .await;

    let h = ColonyHandle::new();

    // 1. /sink first (Anti-Cascade).
    let (recv_tx, mut recv_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    // 2. /mcp via factory.
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("mcp");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(McpCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/mcp"),
            json!({"endpoint": server.endpoint(), "external_timeout_ms": 5000, "query_timeout_ms": 1000}),
            h.runtime().outputs_tx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned(Path::new("/mcp"), spawned).await;
    // W2 (A1): /mcp reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/mcp"),
        Path::new("/sink"),
    )
    .await;

    // Phase A: idle wait — give I/O sub-task time to finish
    // initialize + tools/list and handle_event(DiscoveryReady) to persist cache.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Fresh-Connection-Probe on cell.db.
    let (conn, _status) = open_or_create_cell_db_with_status(&cell_dir.join("cell.db")).unwrap();
    let cache = load_discovery_cache(&conn).unwrap();
    let names: Vec<&str> = cache.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"echo"),
        "cache missing 'echo'; got: {:?}",
        names
    );
    assert!(
        names.contains(&"add"),
        "cache missing 'add'; got: {:?}",
        names
    );

    // Phase B: __list_tools__ probe.
    let inner_list = json!({"name":"__list_tools__"}).to_string();
    h.send(
        MessageBuilder::new(Path::new("/mcp"))
            .reply_to(Path::new("/sink"))
            .body(Body::Inline(json!({"messages":[
                {"origin":"assistant","type":"tool_call","text": inner_list,"id":"list_1"}
            ]})))
            .build(),
    )
    .await;

    // Collect one emission at /sink within 3s.
    let m = tokio::time::timeout(Duration::from_secs(30), recv_rx.recv())
        .await
        .expect("timeout waiting for __list_tools__ receipt")
        .expect("channel closed before receipt");

    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("expected inline body"),
    };

    // Per MecLaw header-merge spec: colony router strips `header` from body,
    // merges into msg.headers.
    assert_eq!(
        m.headers.hop.get("mcp_tool").and_then(|v| v.as_str()),
        Some("__list_tools__"),
        "mcp_tool header mismatch; headers: {:?}",
        m.headers
    );

    // Discovery slots land in body["system"]["tools"]["mcp"][<tool>].
    // provider_key_from_path("/mcp") == "mcp".
    assert!(
        body["system"]["tools"]["mcp"]["echo"]
            .get("inputSchema")
            .is_some()
            || body["system"]["tools"]["mcp"]["echo"].is_object(),
        "echo slot missing or empty; body: {}",
        body
    );
    assert!(
        body["system"]["tools"]["mcp"]["add"]
            .get("inputSchema")
            .is_some()
            || body["system"]["tools"]["mcp"]["add"].is_object(),
        "add slot missing or empty; body: {}",
        body
    );

    h.shutdown().await;
}
