//! Core-Befund #9 — `add_nodes` of an `mcp` cell whose provider endpoint is
//! unreachable: the cell-init failure (post-commit `initialize` connect error)
//! must take the spec'd supervision path, NOT vanish from the registry.
//!
//! Spec (overview § Fehler-Verhalten, Z.1369): a cell-init follow-up failure
//! from an invalid substituted value (the unreachable `endpoint` here) is
//! caught at "Cell-Init nach Commit" and reacts with "Restart one_for_one,
//! nach N Retries `failed`-Status". Spec (overview § Restart-Strategie): the
//! exhausted entry is RETAINED in the registry as `failed` and may return via
//! the reconnect semantics — it is never silently removed.
//!
//! Pre-fix behaviour this test pins away (the Befund-#9 repro): `mcp::io::run_io`
//! returned gracefully on the `initialize` error, the long-running task ended
//! clean, the watcher classified `DeathKind::Normal`, and `handle_cell_died`
//! did `registry.remove` — the mutation answered `committed`, the cell dir +
//! `cell.db` existed, but the cell NEVER appeared in the registry afterwards.
//!
//! Lives in `meclaw-cells/tests/` (NOT `meclaw-colony/tests/`) because it
//! drives the real `McpCellFactory` under a live Colony mutation path — the
//! cross-cutting demo pattern from Phase 9 V1 (`meclaw-colony` keeps NO
//! `meclaw-cells` dependency).

use meclaw_cells::McpCellFactory;
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Root hive marker only — the mcp cell arrives via mutation.
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

/// Reserve a localhost port, then drop the listener — connecting to it after
/// the drop is deterministically refused (nothing listens there anymore).
fn unreachable_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}/rpc")
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

/// Read the RAM registry entry DTO for `path` (`None` if absent — the
/// pre-fix Befund-#9 behaviour makes this `None` for `/mcp`).
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

/// Install a single-cell `mcp` template pointing at `endpoint`, then load it
/// via `RescanTemplates`. Contract carries only the substrate-mandatory keys.
async fn install_mcp_template(
    td: &tempfile::TempDir,
    h: &ColonyHandle,
    name: &str,
    endpoint: &str,
) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    let config = json!({
        "cell": {"type": "mcp", "timeout": -1},
        "params": {
            "endpoint": endpoint,
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

/// Befund #9: `add_nodes` of an mcp cell with an unreachable endpoint commits,
/// then the init failure runs the one_for_one restart cycle to exhaustion and
/// the registry RETAINS the entry as `failed` (Z.1369 + § Restart-Strategie).
/// Pre-fix the entry silently vanished (`DeathKind::Normal` → `registry.remove`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_mcp_unreachable_endpoint_commits_then_marks_failed() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let factory: Arc<dyn CellFactory> = Arc::new(McpCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("mcp".to_string(), factory)]);
    bootstrap_from_filesystem(td.path(), &mcp_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    install_mcp_template(&td, &h, "mcp-unreachable", &unreachable_endpoint()).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"mcp","template":"mcp-unreachable"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of /mcp must commit (init failure is post-commit per Z.1369), got {outcome:?}"
    );

    // Poll until the restart cycle exhausts (failure-marker timeout: 30s
    // convention). Expected terminal state: entry RETAINED, failed, inactive.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last: Option<RegistryEntryDto> = None;
    while tokio::time::Instant::now() < deadline {
        last = ram_entry(&h, "/mcp").await;
        if last.as_ref().is_some_and(|e| e.failed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let entry = last.expect(
        "/mcp registry entry must be RETAINED after init-failure restart exhaustion \
         (Befund #9: pre-fix it was silently removed via DeathKind::Normal)",
    );
    assert!(
        entry.failed,
        "/mcp must be marked failed after exhausting the restart limit (Z.1369), got {entry:?}"
    );
    assert!(
        !entry.active,
        "/mcp must be inactive once failed (§ Restart-Strategie), got {entry:?}"
    );
}
