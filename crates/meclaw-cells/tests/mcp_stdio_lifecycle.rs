//! P7 steps 4.5/4.6 — the ratified liveness slice (D1) and panic propagation
//! from BOTH sides of the dual-task `select!` (AUDIT-PRE14-001).
//!
//! Two arms, two deaths:
//! - I/O arm: the child cannot even be spawned → `run_io` panics → the restart
//!   cycle runs to exhaustion → the registry entry is RETAINED as `failed`.
//! - Handler arm: the child dies mid-conversation → the in-flight call gets a
//!   typed `mcp_error` FIRST, then `handle_event` panics → one_for_one → the
//!   cell serves again with a fresh child.

use meclaw_cells::McpCellFactory;
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");

// ---------------------------------------------------------------------------
// Handler arm — D1
// ---------------------------------------------------------------------------

/// Spawn `/sink` + a stdio `/mcp` whose child dies instead of answering the
/// first `tools/call` (`--die-after 0`).
async fn dying_child_topology_n(
    die_after: &str,
    pid_file: Option<&std::path::Path>,
) -> (ColonyHandle, mpsc::Receiver<Message>, tempfile::TempDir) {
    let mut child_args = vec![
        "mcp".to_string(),
        "--die-after".to_string(),
        die_after.to_string(),
    ];
    if let Some(p) = pid_file {
        child_args.push("--pid-file".to_string());
        child_args.push(p.display().to_string());
    }
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("mcp");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(McpCellFactory);
    let spawned = factory
        .spawn_cell(
            Path::new("/mcp"),
            json!({
                "transport": "stdio",
                "command": FIXTURE,
                "args": child_args,
                "external_timeout_ms": 5000,
                "query_timeout_ms": 1000,
                "kill_grace_ms": 300
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

fn tool_call(call_id: &str) -> Message {
    let inner = json!({"name": "echo", "arguments": {"text": "hi"}}).to_string();
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

/// D1, both halves in one run: the in-flight call is answered with a typed
/// error (nothing is lost to the panic), and the cell is genuinely restarted —
/// proven by the child's pid CHANGING, which can only happen if one_for_one
/// respawned the cell and its I/O sub-task spawned a fresh process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dying_child_answers_the_call_and_then_restarts_the_cell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("child.pid");
    let (h, mut sink, _td) = dying_child_topology_n("0", Some(&pid_file)).await;

    let first_pid = wait_for_pid(&pid_file, None).await;

    h.send(tool_call("call_1")).await;
    let first = receipt(&mut sink).await;
    assert_eq!(
        first.headers.hop["error_code"], "mcp_error",
        "the in-flight call must be answered with a typed error, got {:?}",
        first.headers.hop
    );

    let second_pid = wait_for_pid(&pid_file, Some(first_pid)).await;
    assert_ne!(
        first_pid, second_pid,
        "one_for_one must have respawned the cell with a fresh child"
    );

    // And it serves again: the fresh child dies on ITS first call too, so the
    // receipt is another typed error — but a receipt it is.
    h.send(tool_call("call_2")).await;
    let second = receipt(&mut sink).await;
    assert_eq!(
        second.headers.hop["error_code"], "mcp_error",
        "the respawned cell must serve again, got {:?}",
        second.headers.hop
    );
}

/// Wait until the fixture has written a pid different from `previous`.
/// Polling is fine here: this is a test, not production code.
async fn wait_for_pid(path: &std::path::Path, previous: Option<u32>) -> u32 {
    for _ in 0..3000 {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(pid) = s.trim().parse::<u32>()
            && Some(pid) != previous
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "no {} child pid appeared within 30s",
        if previous.is_some() { "new" } else { "initial" }
    );
}

// ---------------------------------------------------------------------------
// I/O arm — spawn failure panics and exhausts the restart limit
// ---------------------------------------------------------------------------

fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
}

fn mcp_registry() -> meclaw_colony::CellFactoryRegistry {
    let mut r = meclaw_colony::CellFactoryRegistry::new();
    r.insert(
        "mcp".into(),
        Arc::new(McpCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
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

async fn ram_entry(h: &ColonyHandle, path: &str) -> Option<RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Install a single-cell stdio `mcp` template whose `command` cannot be run.
async fn install_unspawnable_template(td: &tempfile::TempDir, h: &ColonyHandle, name: &str) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    let config = json!({
        "cell": {"type": "mcp", "timeout": -1},
        "params": {
            "transport": "stdio",
            "command": "/nonexistent/mcp-server",
            "external_timeout_ms": 500,
            "query_timeout_ms": 1000
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// The stdio twin of core finding #9: an unspawnable child is a post-commit
/// cell-init failure. `run_io` must PANIC (not return), so the watcher sees
/// `DeathKind::Panic`, restarts one_for_one, and after the restart limit the
/// registry entry is RETAINED as `failed` instead of silently disappearing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_stdio_mcp_with_unspawnable_command_commits_then_marks_failed() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(McpCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("mcp".to_string(), factory)]);
    bootstrap_from_filesystem(td.path(), &mcp_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    install_unspawnable_template(&td, &h, "mcp-unspawnable").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"mcp","template":"mcp-unspawnable"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes must commit (the spawn failure is post-commit), got {outcome:?}"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last: Option<RegistryEntryDto> = None;
    while tokio::time::Instant::now() < deadline {
        last = ram_entry(&h, "/mcp").await;
        if last.as_ref().is_some_and(|e| e.failed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let entry = last.expect("/mcp registry entry must be RETAINED after restart exhaustion");
    assert!(entry.failed, "/mcp must be marked failed, got {entry:?}");
    assert!(
        !entry.active,
        "/mcp must be inactive once failed, got {entry:?}"
    );
}
