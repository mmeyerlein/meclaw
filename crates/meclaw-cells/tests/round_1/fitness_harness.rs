//! Track T (#104) — fitness battery for the `harness` cell against the stub
//! adapter binary (`stream_json_harness_fixture`), LLM-free.
//!
//! Full lifecycle coverage lives in `p8_harness_demo.rs` and the `harness_*`
//! suites; this battery pins the two lanes a coding topology wires first:
//!
//! - `accepted` answers synchronously in the CALLER's trace as a
//!   `tool_result` on the inbound call id, anchored by `task_id`;
//! - `result` arrives on the `emit_to` origin lane with the observed outcome
//!   (`status`, `workspace`, audit fields) — a fresh trace, correlated by
//!   `header.task_id` only;
//! - a task without `task_id` is refused (`invalid_input`) — the task
//!   register is non-idempotent by design, so the id is the caller's duty.

use meclaw_cells::harness::HarnessCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_stream_json_harness_fixture");

async fn topology() -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("harness");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let workspaces = td.path().join("workspaces");
    std::fs::create_dir_all(workspaces.join("wt-1")).unwrap();

    let factory = Arc::new(HarnessCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/harness"),
            json!({
                "adapter": "claude-code",
                "command": FIXTURE,
                "emit_to": "/sink",
                "workspace_root": workspaces.display().to_string(),
                "startup_timeout_ms": 5000,
                "external_timeout_ms": 5000,
                "query_timeout_ms": 1000,
                "kill_grace_ms": 500
            }),
            h.runtime().outputs_tx,
            cell_dir,
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .expect("spawn harness cell");
    h.register_spawned(Path::new("/harness"), spawned).await;
    h.add_edge(Uuid::now_v7(), Path::new("/harness"), Path::new("/sink"))
        .await;
    (h, recv_rx, td)
}

fn tool_call(name: &str, arguments: meclaw_core::JsonValue, id: &str) -> Message {
    let inner = json!({"name": name, "arguments": arguments}).to_string();
    MessageBuilder::new(Path::new("/harness"))
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

/// Collect receipts until one carries the given `harness_event`.
async fn receipt_with_event(rx: &mut mpsc::Receiver<Message>, event: &str) -> Message {
    let mut seen = Vec::new();
    for _ in 0..20 {
        let m = receipt(rx).await;
        let kind = m
            .headers
            .hop
            .get("harness_event")
            .and_then(|v| v.as_str())
            .unwrap_or("<none>")
            .to_string();
        seen.push(kind.clone());
        if kind == event {
            return m;
        }
    }
    panic!("no {event} receipt arrived; saw {seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_answers_in_trace_and_result_arrives_on_the_origin_lane() {
    let (h, mut rx, _td) = topology().await;

    h.send(tool_call(
        "start_task",
        json!({"task_id": "t-fit-1", "prompt": "run the suite", "workspace": "wt-1"}),
        "call-1",
    ))
    .await;

    // The accepted lane: synchronous, in the caller's trace, tool_result form.
    let accepted = receipt(&mut rx).await;
    assert_eq!(accepted.headers.hop["harness_event"], "accepted");
    assert_eq!(accepted.headers.hop["task_id"], "t-fit-1");
    assert!(
        accepted.parent_message_id.is_some(),
        "accepted answers IN the caller's trace (OutputSink)"
    );
    let Body::Inline(ab) = &accepted.body else {
        panic!("inline expected")
    };
    assert_eq!(ab["messages"][0]["type"], "tool_result");
    assert_eq!(ab["messages"][0]["id"], "call-1");

    // The result lane: an origin emission, fresh trace, typed outcome.
    let result = receipt_with_event(&mut rx, "result").await;
    assert!(
        result.parent_message_id.is_none(),
        "result is an ORIGIN emission (fresh trace), correlated by task_id"
    );
    assert_ne!(
        result.trace_id, accepted.trace_id,
        "the result does not reuse the caller's trace"
    );
    let hop = &result.headers.hop;
    assert_eq!(hop["task_id"], "t-fit-1");
    assert_eq!(hop["status"], "ok");
    assert!(
        hop["workspace"]
            .as_str()
            .unwrap_or_default()
            .ends_with("wt-1"),
        "the result names the workspace it ran in: {hop:?}"
    );
    assert!(hop.get("session_id").is_some(), "audit: {hop:?}");
    assert!(hop.get("cost_usd").is_some(), "cost governance: {hop:?}");
    let Body::Inline(rb) = &result.body else {
        panic!("inline expected")
    };
    assert!(
        rb["messages"][0]["text"].is_string(),
        "the harness summary travels as prose: {rb}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_without_task_id_is_refused_loudly() {
    // Non-idempotence is the cell's core invariant: a task_id runs exactly
    // once, so the id is MANDATORY input, never generated on the caller's
    // behalf.
    let (h, mut rx, _td) = topology().await;

    h.send(tool_call(
        "start_task",
        json!({"prompt": "no id", "workspace": "wt-1"}),
        "call-1",
    ))
    .await;

    let m = receipt(&mut rx).await;
    assert_eq!(m.headers.hop["error_code"], "invalid_input");

    h.shutdown().await;
}
