//! Track T (#104) — fitness battery for the `mcp` cell on the stdio
//! transport, against the checked-in line-JSON fixture child.
//!
//! Lifecycle/death coverage lives in `mcp_stdio_lifecycle.rs` and
//! `p7_mcp_stdio_demo.rs`; this battery pins the TOOL-LANE roundtrip a coding
//! agent depends on:
//!
//! - a `tool_call` reaches the child as `tools/call` and the answer comes
//!   back as ONE `tool_result` on the same id with `mcp_tool` in the header;
//! - a provider-side JSON-RPC error is the typed `mcp_error` — same lane,
//!   same id, loud;
//! - discovery works from the cache (`__list_tools__`).

use meclaw_cells::McpCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");

async fn topology() -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("mcp");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(McpCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/mcp"),
            json!({
                "transport": "stdio",
                "command": FIXTURE,
                "args": ["mcp"],
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
    h.add_edge(Uuid::now_v7(), Path::new("/mcp"), Path::new("/sink"))
        .await;
    (h, recv_rx, td)
}

fn tool_call(name: &str, arguments: meclaw_core::JsonValue, id: &str) -> Message {
    let inner = json!({"name": name, "arguments": arguments}).to_string();
    MessageBuilder::new(Path::new("/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[
            {"origin":"assistant","type":"tool_call","text": inner, "id": id}
        ]})))
        .build()
}

async fn receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no receipt at /sink within 30s")
        .expect("sink channel closed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stdio_tool_call_round_trips_on_the_same_id() {
    let (h, mut rx, _td) = topology().await;

    h.send(tool_call(
        "echo",
        json!({"text": "workshop ping"}),
        "call-1",
    ))
    .await;

    let m = receipt(&mut rx).await;
    assert_eq!(m.headers.hop["mcp_tool"], "echo");
    assert!(
        m.headers.hop.get("error_code").is_none(),
        "happy path carries no error_code: {:?}",
        m.headers.hop
    );
    let Body::Inline(body) = &m.body else {
        panic!("inline expected")
    };
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(body["messages"][0]["id"], "call-1");
    assert!(
        body["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("workshop ping"),
        "the child's answer travels in text: {body}"
    );

    // A second call over the SAME child: the stream stays usable.
    h.send(tool_call("echo", json!({"text": "second"}), "call-2"))
        .await;
    let m = receipt(&mut rx).await;
    let Body::Inline(body) = &m.body else {
        panic!("inline expected")
    };
    assert_eq!(body["messages"][0]["id"], "call-2");
    assert!(
        body["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("second")
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_error_is_the_typed_mcp_error_on_the_same_lane() {
    let (h, mut rx, _td) = topology().await;

    // The fixture answers an unknown tool with JSON-RPC -32601.
    h.send(tool_call("no_such_tool", json!({}), "call-1")).await;

    let m = receipt(&mut rx).await;
    assert_eq!(m.headers.hop["error_code"], "mcp_error");
    let Body::Inline(body) = &m.body else {
        panic!("inline expected")
    };
    assert_eq!(
        body["messages"][0]["id"], "call-1",
        "the error closes the round on the SAME id"
    );

    // And the cell still serves afterwards.
    h.send(tool_call("echo", json!({"text": "alive"}), "call-2"))
        .await;
    let m = receipt(&mut rx).await;
    let Body::Inline(body) = &m.body else {
        panic!("inline expected")
    };
    assert_eq!(body["messages"][0]["id"], "call-2");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_answers_from_the_cache() {
    let (h, mut rx, _td) = topology().await;

    // The boot discovery (initialize + tools/list) runs asynchronously in the
    // I/O task, so the cache read is eventually consistent — poll bounded.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        h.send(tool_call("__list_tools__", json!({}), "call-1"))
            .await;

        // The listing is a `system.tools.<provider>` emission, not a text
        // turn — exactly the form an llm cell ingests as tool schemas.
        let m = receipt(&mut rx).await;
        assert_eq!(m.headers.hop["mcp_tool"], "__list_tools__");
        let Body::Inline(body) = &m.body else {
            panic!("inline expected")
        };
        let tools = body["system"]["tools"]
            .as_object()
            .expect("system.tools object");
        let (_provider, provider_tools) = tools.iter().next().expect("one provider entry");
        if provider_tools.get("echo").is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "discovery cache never named the child's tool: {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    h.shutdown().await;
}
