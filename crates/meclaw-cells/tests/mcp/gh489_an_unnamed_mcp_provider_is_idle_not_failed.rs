//! GH #489 — an `mcp` occupant that was never given a provider must be IDLE,
//! not a permanently `failed` registry entry.
//!
//! The shipped `tools` hive carries an `mcp` occupant whose `endpoint` was
//! written `${MCP_ENDPOINT:-http://127.0.0.1:8765/mcp}`. Nobody runs that
//! address, so on every boot that spawned the cell the http I/O task panicked
//! five times against it and the supervisor retained the entry as `failed` —
//! the default outcome of growing an agent from the shipped catalogue was a
//! colony with a failure in it, produced by a guess rather than by a
//! misconfiguration.
//!
//! The panic itself stays (`mcp_init_failure_restart_to_failed.rs` pins it):
//! an endpoint that was NAMED and does not answer is a real fault and the
//! panic IS its supervision signal. What this test pins is the other case —
//! no endpoint was named at all. There is no provider to be unreachable, so
//! there is nothing to supervise: the cell spawns, runs no handshake, stays
//! `active` and answers every call with an `endpoint_unset` receipt.
//!
//! Both halves matter and neither is the other's default: an empty result and
//! a forgotten call must never look alike (`docs/development-rules.md` § 2c).
//!
//! Lives in `meclaw-cells/tests/` (not `meclaw-colony/tests/`) for the same
//! reason its sibling does: it drives the real `McpCellFactory` under a live
//! colony (`meclaw-colony` keeps no `meclaw-cells` dependency).

use meclaw_cells::McpCellFactory;
use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::db::setup_mcp_schema;
use meclaw_cells::mcp::params::{McpParams, McpTransport};
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, LongRunningCell, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

// ─── the parse half: absent and empty mean the same thing ────────────────────

/// An `mcp` config with no `endpoint` at all parses — it does not spawn a
/// bridge to nowhere and it is not a rejection either.
#[test]
fn an_absent_endpoint_parses_as_an_unnamed_provider() {
    let p = McpParams::parse(&json!({})).expect("no endpoint must parse, GH #489");
    assert!(
        matches!(p.transport, McpTransport::Unset),
        "expected the unnamed-provider transport, got {:?}",
        p.transport
    );
}

/// `${MCP_ENDPOINT:-}` substitutes to the empty string, which is the shape an
/// operator who never set the variable actually gets. It must land in exactly
/// the same state as the absent key (GH #270's rule kept: empty and absent are
/// never two different outcomes).
#[test]
fn an_empty_endpoint_parses_as_an_unnamed_provider() {
    let p = McpParams::parse(&json!({"endpoint": ""})).expect("empty endpoint must parse");
    assert!(
        matches!(p.transport, McpTransport::Unset),
        "expected the unnamed-provider transport, got {:?}",
        p.transport
    );
}

/// The factory's pre-spawn validation runs the same parse, so a mutation that
/// installs the shipped occupant without an `MCP_ENDPOINT` is accepted rather
/// than refused at the door.
#[test]
fn the_factory_accepts_a_config_that_names_no_provider() {
    let f: Arc<McpCellFactory> = Arc::new(McpCellFactory);
    f.validate_params(&json!({"external_timeout_ms": 30_000}))
        .expect("GH #489: naming no provider is a state, not a config error");
}

// ─── the receipt half: a call gets an answer with a name on it ───────────────

fn build_tool_call_msg(target: &str, tool: &str) -> meclaw_core::Message {
    let inner = json!({"name": tool, "arguments": {}}).to_string();
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [
                {"origin": "assistant", "type": "tool_call", "text": inner, "id": "call_1"}
            ]
        })))
        .build()
}

/// Drive one message into an mcp cell that names no provider and return the
/// single emission.
async fn call_unnamed(tool: &str) -> CellEmission {
    let tmp = TempDir::new().unwrap();
    let (conn, _) = open_or_create_cell_db_with_status(&tmp.path().join("cell.db")).unwrap();
    setup_mcp_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(1)));

    let mut cell = McpCell::new_unset(30_000, 5_000, "main_mcp".into());
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let msg = build_tool_call_msg("/mcp", tool);
    let sink = OutputSink::new(
        tx,
        Path::new("/mcp"),
        msg.id,
        msg.trace_id,
        msg.ttl,
        msg.headers.clone(),
        None,
    );
    let (rc_tx, _rc_rx) = mpsc::channel(1);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    rx.recv().await.expect("an unnamed provider still answers")
}

/// A tool call into a cell with no provider is answered, not swallowed, and the
/// answer carries the reason by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_is_answered_with_the_endpoint_unset_receipt() {
    let em = call_unnamed("echo").await;
    let header = &em.content["header"];
    assert_eq!(header["error_code"], "endpoint_unset", "header: {header}");
    assert_eq!(header["mcp_tool"], "echo", "header: {header}");
    let last = em.content["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["id"], "call_1", "the call id must come back: {last}");
}

/// The discovery round takes the same receipt. An empty tool listing would be
/// the silence this issue is about: a brain that asked what there is would be
/// handed an empty menu and no way to learn why it is empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_discovery_round_is_answered_with_the_same_receipt() {
    let em = call_unnamed("__list_tools__").await;
    let header = &em.content["header"];
    assert_eq!(header["error_code"], "endpoint_unset", "header: {header}");
}

// ─── the colony half: no failed cell, on this boot or any later one ──────────

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

/// Install a single-cell `mcp` template that names NO provider — the shape the
/// shipped occupant has once its default guess is gone and `MCP_ENDPOINT` is
/// unset.
async fn install_unnamed_mcp_template(td: &TempDir, h: &ColonyHandle, name: &str) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    let config = json!({
        "cell": {"type": "mcp", "timeout": -1},
        "params": {
            "endpoint": "",
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
    ack_rx.await.unwrap().expect("rescan must not abort");
}

/// The regression itself: an mcp cell that names no provider commits, spawns,
/// and is still `active` — never `failed` — long after the restart cycle of the
/// pre-fix behaviour would have exhausted itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_mcp_cell_that_names_no_provider_never_reaches_failed() {
    let td = TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(McpCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("mcp".to_string(), factory)]);
    bootstrap_from_filesystem(td.path(), &mcp_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    install_unnamed_mcp_template(&td, &h, "mcp-unnamed").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"mcp","template":"mcp-unnamed"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of an unnamed mcp must commit, got {outcome:?}"
    );

    // Five panics + their restart backoff fit comfortably inside this window
    // (the pre-fix repro reached `failed` in well under a second with a
    // 500 ms external timeout against a refused connect).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let entry = ram_entry(&h, "/mcp").await;
        if let Some(e) = &entry {
            assert!(
                !e.failed,
                "GH #489: a cell that names no provider must never be marked failed, got {e:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let entry = ram_entry(&h, "/mcp")
        .await
        .expect("/mcp must be in the registry");
    assert!(!entry.failed, "still not failed: {entry:?}");
    assert!(
        entry.active,
        "an idle cell is an ACTIVE cell with nothing to do, got {entry:?}"
    );
}
