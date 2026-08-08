//! P8 package demo — a task runs supervised end to end inside a live colony.
//!
//! A message arrives, a real child process starts in an assigned workspace,
//! its progress streams into the topology, and its outcome arrives as a typed
//! result. The harness is the fixture rather than the real CLI: the dialect is
//! identical, the cost is zero, and the one paid run at the end of the package
//! checks the fixture against reality.
//!
//! Positive receipts only: every claim is proven by a message ARRIVING at
//! `/sink`.

use meclaw_cells::harness::HarnessCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_stream_json_harness_fixture");

/// `/sink` first (anti-cascade), then `/harness`, then the edge.
///
/// The workspace root is a real directory tree with one workspace in it —
/// the topology's job in production (a git worktree), a `mkdir` here.
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
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/harness"),
        Path::new("/sink"),
    )
    .await;

    (h, recv_rx, td)
}

/// A tool call in the store/mcp convention.
fn tool_call(name: &str, arguments: serde_json::Value, call_id: &str) -> Message {
    let inner = json!({"name": name, "arguments": arguments}).to_string();
    MessageBuilder::new(Path::new("/harness"))
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

/// Collect receipts until one carries the given `harness_event`, returning it
/// together with everything seen on the way.
async fn receipt_with_event(
    rx: &mut mpsc::Receiver<Message>,
    event: &str,
) -> (Message, Vec<String>) {
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
            return (m, seen);
        }
    }
    panic!("no {event} receipt arrived; saw {seen:?}");
}

/// The package's headline: a coding task runs supervised from one message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_task_runs_end_to_end_and_reports_its_outcome() {
    let (h, mut sink, _td) = topology().await;

    h.send(tool_call(
        "start_task",
        json!({"task_id": "t-1", "prompt": "do the thing", "workspace": "wt-1"}),
        "call_1",
    ))
    .await;

    // 1. The request is answered immediately, in its own trace, with the id
    //    that correlates everything that follows.
    let accepted = receipt(&mut sink).await;
    assert_eq!(accepted.headers.hop["harness_event"], "accepted");
    assert_eq!(accepted.headers.hop["task_id"], "t-1");

    // 2. Progress streams in while the harness works.
    let (_progress, seen) = receipt_with_event(&mut sink, "progress").await;
    assert!(seen.contains(&"progress".to_string()), "saw {seen:?}");

    // 3. The outcome arrives as a typed result carrying what was OBSERVED.
    let (result, _) = receipt_with_event(&mut sink, "result").await;
    let hop = &result.headers.hop;
    assert_eq!(hop["status"], "ok", "wrong outcome: {hop:?}");
    assert_eq!(hop["task_id"], "t-1");
    assert!(
        hop["workspace"]
            .as_str()
            .unwrap_or_default()
            .ends_with("wt-1"),
        "the result must name the workspace it ran in: {hop:?}"
    );
    assert!(hop.get("session_id").is_some(), "audit trail: {hop:?}");
    assert!(hop.get("model").is_some(), "audit trail: {hop:?}");
    assert!(hop.get("cost_usd").is_some(), "cost governance: {hop:?}");

    let Body::Inline(body) = &result.body else {
        panic!("expected an inline body")
    };
    assert!(
        body["messages"][0]["text"].is_string(),
        "the harness's own summary must travel as prose: {body}"
    );
}

/// Containment is not advisory: a workspace outside the configured root is
/// refused before any process starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_workspace_outside_the_root_never_starts_a_process() {
    let (h, mut sink, _td) = topology().await;

    h.send(tool_call(
        "start_task",
        json!({"task_id": "t-1", "prompt": "p", "workspace": "../.."}),
        "call_1",
    ))
    .await;

    let m = receipt(&mut sink).await;
    assert_eq!(m.headers.hop["error_code"], "workspace_invalid");
}

/// The status lane (V1): the last known state comes from the task table, not
/// from live introspection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_status_answers_from_the_task_register() {
    let (h, mut sink, _td) = topology().await;

    h.send(tool_call(
        "start_task",
        json!({"task_id": "t-1", "prompt": "do the thing", "workspace": "wt-1"}),
        "call_1",
    ))
    .await;
    let (_result, _) = receipt_with_event(&mut sink, "result").await;

    h.send(tool_call("status", json!({"task_id": "t-1"}), "call_2"))
        .await;
    let m = receipt(&mut sink).await;
    let Body::Inline(body) = &m.body else {
        panic!("expected an inline body")
    };
    let payload: serde_json::Value =
        serde_json::from_str(body["messages"][0]["text"].as_str().expect("text"))
            .expect("status payload is json");
    assert_eq!(payload["task_id"], "t-1");
    assert_eq!(payload["status"], "ok");
    assert!(payload["session_id"].is_string(), "audit: {payload}");
}

/// Dedup end to end: the same task id can never be run a second time, whatever
/// the topology retries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_repeated_task_id_is_refused_by_the_register() {
    let (h, mut sink, _td) = topology().await;

    h.send(tool_call(
        "start_task",
        json!({"task_id": "t-1", "prompt": "do the thing", "workspace": "wt-1"}),
        "call_1",
    ))
    .await;
    let (_result, _) = receipt_with_event(&mut sink, "result").await;

    h.send(tool_call(
        "start_task",
        json!({"task_id": "t-1", "prompt": "do it again", "workspace": "wt-1"}),
        "call_2",
    ))
    .await;
    let m = receipt(&mut sink).await;
    assert_eq!(
        m.headers.hop["error_code"], "invalid_input",
        "a mutating task must never run twice under one id: {:?}",
        m.headers.hop
    );
}
