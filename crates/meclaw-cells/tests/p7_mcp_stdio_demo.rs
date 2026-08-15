//! P7 package demo — the `mcp` cell speaks stdio to a real child process
//! inside a live colony: `initialize` + `tools/list` at boot, `tools/call` on
//! demand, and `__list_tools__` out of the discovery cache.
//!
//! Positive receipts only: every claim is proven by a message ARRIVING at
//! `/sink`, never by the absence of something.

use meclaw_cells::McpCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");

/// Wire `/sink` first (anti-cascade), then `/mcp` on the stdio transport, then
/// the edge between them. Returns the handle, the sink receiver and the
/// tempdir (kept alive by the caller).
async fn topology(extra_args: &[&str]) -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();

    let (recv_tx, recv_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("mcp");
    std::fs::create_dir_all(&cell_dir).unwrap();

    let mut args = vec!["mcp".to_string()];
    args.extend(extra_args.iter().map(|s| s.to_string()));

    let factory = Arc::new(McpCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/mcp"),
            json!({
                "transport": "stdio",
                "command": FIXTURE,
                "args": args,
                "external_timeout_ms": 5000,
                "query_timeout_ms": 1000,
                "kill_grace_ms": 500
            }),
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
        .expect("spawn stdio mcp cell");
    h.register_spawned(Path::new("/mcp"), spawned).await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/mcp"),
        Path::new("/sink"),
    )
    .await;

    (h, recv_rx, td)
}

/// A `tool_call` probe in the phase-9 store convention.
fn tool_call(name: &str, arguments: serde_json::Value, call_id: &str) -> Message {
    let inner = json!({"name": name, "arguments": arguments}).to_string();
    MessageBuilder::new(Path::new("/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[
            {"origin":"assistant","type":"tool_call","text": inner, "id": call_id}
        ]})))
        .build()
}

async fn receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no receipt at /sink within 30s")
        .expect("sink channel closed")
}

fn tail_turn(m: &Message) -> &serde_json::Value {
    let Body::Inline(v) = &m.body else {
        panic!("expected an inline body")
    };
    v["messages"]
        .as_array()
        .and_then(|a| a.last())
        .expect("at least one turn")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_tool_call_over_stdio_round_trips() {
    let (h, mut sink, _td) = topology(&[]).await;

    h.send(tool_call("echo", json!({"text": "hi"}), "call_1"))
        .await;
    let m = receipt(&mut sink).await;

    let turn = tail_turn(&m);
    assert_eq!(turn["type"], "tool_result", "not a tool_result: {turn}");
    assert_eq!(turn["id"], "call_1");
    assert!(
        turn["text"].as_str().unwrap_or_default().contains("hi"),
        "the child's answer did not reach the sink: {turn}"
    );
    assert_eq!(m.headers.hop["mcp_tool"], "echo");
    assert!(
        m.headers.hop.get("error_code").is_none(),
        "unexpected error_code: {:?}",
        m.headers.hop
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_an_unknown_tool_becomes_a_typed_mcp_error() {
    let (h, mut sink, _td) = topology(&[]).await;

    h.send(tool_call("nope", json!({}), "call_1")).await;
    let m = receipt(&mut sink).await;

    assert_eq!(
        m.headers.hop["error_code"], "mcp_error",
        "wrong error class: {:?}",
        m.headers.hop
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_discovery_over_stdio_fills_the_cache() {
    let (h, mut sink, _td) = topology(&[]).await;

    // The discovery snapshot is written by handle_event out of the stdio
    // handshake; the `system.tools` listing is the positive receipt that the
    // handshake actually ran over the child process.
    for attempt in 0..30 {
        h.send(tool_call("__list_tools__", json!({}), "call_1"))
            .await;
        let m = receipt(&mut sink).await;
        let Body::Inline(v) = &m.body else {
            panic!("expected an inline body")
        };
        if v["system"]["tools"]["mcp"].get("echo").is_some() {
            return;
        }
        assert!(attempt < 29, "discovery cache never listed the tool: {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
