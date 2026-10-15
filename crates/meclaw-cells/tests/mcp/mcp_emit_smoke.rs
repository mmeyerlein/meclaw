//! T13: emit_tool_result_success / emit_tool_result_error build UBF-valid
//! bodies with the expected headers + tool_result-Turn.

use meclaw_cells::mcp::emit::{emit_tool_result_error, emit_tool_result_success};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use tokio::sync::mpsc;

fn sink_for(
    rx_buf: usize,
) -> (
    OutputSink,
    mpsc::Receiver<CellEmission>,
    meclaw_core::Message,
) {
    let (tx, rx) = mpsc::channel::<CellEmission>(rx_buf);
    let msg = MessageBuilder::new(Path::new("/mcp"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[]})))
        .build();
    let sink = OutputSink::new(
        tx,
        Path::new("/mcp"),
        msg.id,
        msg.trace_id,
        msg.ttl,
        msg.headers.clone(),
        None,
    );
    (sink, rx, msg)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn success_emits_tool_result_with_required_headers() {
    let (sink, mut rx, msg) = sink_for(8);
    emit_tool_result_success(
        &sink,
        &msg,
        "echo",
        17,
        "call_1",
        json!({"content":[{"type":"text","text":"hi"}]}),
    )
    .await;
    let em = rx.recv().await.unwrap();
    assert_eq!(em.target.as_str(), "/sink");
    meclaw_core::validate_ubf_body(&em.content).expect("tool_result body must be valid UBF");
    let header = &em.content["header"];
    assert_eq!(header["mcp_tool"], "echo");
    assert_eq!(header["duration_ms"], 17);
    assert!(header.get("error_code").is_none());
    let last = em.content["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["origin"], "tool");
    assert_eq!(last["type"], "tool_result");
    assert_eq!(last["id"], "call_1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn error_emits_tool_result_with_error_code() {
    let (sink, mut rx, msg) = sink_for(8);
    emit_tool_result_error(&sink, &msg, "echo", 42, "call_2", "mcp_error", "boom").await;
    let em = rx.recv().await.unwrap();
    meclaw_core::validate_ubf_body(&em.content).expect("error tool_result body must be valid UBF");
    let header = &em.content["header"];
    assert_eq!(header["mcp_tool"], "echo");
    assert_eq!(header["duration_ms"], 42);
    assert_eq!(header["error_code"], "mcp_error");
    let last = em.content["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["origin"], "tool");
    assert_eq!(last["id"], "call_2");
    assert!(last["text"].as_str().unwrap().contains("boom"));
}

use meclaw_cells::mcp::db::DiscoveredTool;
use meclaw_cells::mcp::emit::emit_system_tools_listing;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_listing_emits_system_tools_slots() {
    let (sink, mut rx, msg) = sink_for(8);
    let tools = vec![
        DiscoveredTool {
            name: "echo".into(),
            schema_json: json!({"type":"object"}).to_string(),
        },
        DiscoveredTool {
            name: "add".into(),
            schema_json: json!({"type":"object","x":1}).to_string(),
        },
    ];
    emit_system_tools_listing(&sink, &msg, "main_mcp", &tools, 5).await;
    let em = rx.recv().await.unwrap();
    meclaw_core::validate_ubf_body(&em.content).expect("discovery listing body must be valid UBF");
    assert_eq!(
        em.content["system"]["tools"]["main_mcp"]["echo"]["type"],
        "object"
    );
    assert_eq!(em.content["system"]["tools"]["main_mcp"]["add"]["x"], 1);
    let header = &em.content["header"];
    assert_eq!(header["mcp_tool"], "__list_tools__");
    assert_eq!(header["duration_ms"], 5);
}
