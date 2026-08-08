//! P8 — the ONE paid run: the cell against the real Claude Code CLI.
//!
//! **This test costs money and is `#[ignore]`d.** `cargo test` never runs it;
//! it exists so the claim "the fixture speaks the same dialect as the real
//! thing" stays checkable instead of being a one-off assertion in a report.
//!
//! Run it deliberately:
//!
//! ```text
//! HARNESS_SMOKE_MODEL=sonnet \
//!   cargo test -p meclaw-cells --test harness_real_cli_smoke -- --ignored --nocapture
//! ```
//!
//! The model comes from `HARNESS_SMOKE_MODEL` and is never defaulted in code
//! (standing rule: models live in `${VAR}`). The run is bounded by
//! `--max-turns` and the smallest sensible task, and the receipt it prints —
//! session id, effective model, turns, cost — is what goes into the package
//! report. Nothing here is asserted from configuration: every number is read
//! back out of the harness's own result event.
#![cfg(unix)]

use meclaw_cells::harness::HarnessCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// A live colony with a `harness` cell wired to the REAL `claude` binary.
async fn topology(model: &str) -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(64);
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
                "command": "claude",
                "emit_to": "/sink",
                "workspace_root": workspaces.display().to_string(),
                "model": model,
                // Containment: file writing only. The task below deliberately
                // also asks for a shell command, so the run exercises what a
                // harness does when it wants a tool it was not given.
                "allowed_tools": ["Write"],
                "permission_mode": "acceptEdits",
                "max_turns": 3,
                // A real harness needs its own credentials and PATH; nothing
                // else of this process's environment travels with it.
                "env_passthrough": ["PATH", "HOME", "USER", "LANG", "TERM"],
                "startup_timeout_ms": 120000,
                "external_timeout_ms": 30000,
                "query_timeout_ms": 5000,
                "kill_grace_ms": 5000
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

fn tool_call(name: &str, arguments: serde_json::Value, call_id: &str) -> Message {
    let inner = json!({"name": name, "arguments": arguments}).to_string();
    MessageBuilder::new(Path::new("/harness"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[
            {"origin":"assistant","type":"tool_call","text": inner, "id": call_id}
        ]})))
        .build()
}

#[ignore = "costs money: runs the real claude CLI"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smoke_the_real_cli_runs_a_task_and_reports_its_cost() {
    let model = std::env::var("HARNESS_SMOKE_MODEL").expect(
        "set HARNESS_SMOKE_MODEL (no model default in code — standing rule: models come from ${VAR})",
    );
    let (h, mut sink, td) = topology(&model).await;

    // Two things in one task, on purpose: the first is trivial and allowed,
    // the second needs a tool the cell did not grant. That is the cheapest way
    // to find out what the real CLI does when it wants permission.
    h.send(tool_call(
        "start_task",
        json!({
            "task_id": "smoke-1",
            "prompt": "Create a file named hello.txt containing exactly the word hello. \
                       Then run `ls -la` to verify it. Then stop.",
            "workspace": "wt-1"
        }),
        "call_1",
    ))
    .await;

    let mut result = None;
    let mut saw_question = false;
    // Generous: a real model call takes as long as it takes.
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    while std::time::Instant::now() < deadline {
        let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(300), sink.recv()).await else {
            break;
        };
        let hop = &m.headers.hop;
        let event = hop
            .get("harness_event")
            .and_then(|v| v.as_str())
            .unwrap_or("<none>");
        println!("receipt: {event} {hop:?}");
        if event == "question" {
            saw_question = true;
        }
        if event == "result" {
            result = Some(m);
            break;
        }
    }

    let result = result.expect("no result emission from the real CLI within 300s");
    let hop = &result.headers.hop;

    // The receipt. Every value is read back from the harness's own events —
    // none of it is echoed configuration.
    println!("\n=== SMOKE RECEIPT ===");
    println!("status:      {}", hop["status"]);
    println!("session_id:  {:?}", hop.get("session_id"));
    println!("model:       {:?}", hop.get("model"));
    println!("num_turns:   {:?}", hop.get("num_turns"));
    println!("cost_usd:    {:?}", hop.get("cost_usd"));
    println!("duration_ms: {:?}", hop.get("duration_ms"));
    println!("saw a can_use_tool question: {saw_question}");
    if let Body::Inline(body) = &result.body {
        println!("summary:     {}", body["messages"][0]["text"]);
    }
    println!("=====================\n");

    assert_eq!(hop["status"], "ok", "the real run did not succeed: {hop:?}");
    assert!(
        hop.get("session_id").is_some(),
        "no session id — the audit trail depends on it: {hop:?}"
    );
    assert!(
        hop.get("model").is_some(),
        "no effective model reported: {hop:?}"
    );

    // The point of the whole cell type: the artifact is on disk, not in prose.
    let hello = td.path().join("workspaces/wt-1/hello.txt");
    let content = std::fs::read_to_string(&hello)
        .unwrap_or_else(|e| panic!("the harness reported success but wrote no {hello:?}: {e}"));
    assert!(
        content.to_lowercase().contains("hello"),
        "unexpected file content: {content:?}"
    );
}
