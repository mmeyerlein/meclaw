//! Phase-10-D: I/O-Sub-Task frames + `run_io`. POC-Shape:
//! - Handler← I/O: `McpEvent::DiscoveryReady` (einmalig nach
//!   `initialize`+`tools/list`).
//! - Handler→ I/O: `McpReconfig` ist im POC ein leeres Enum — kein
//!   Reconfig-Pfad nötig (Tool-Calls laufen synchron in `handle()`).
//!   `reconfig_rx` MUSS im `run_io`-`async move`-Scope trotzdem
//!   explizit gebunden werden (Phase-10-A-Lesson Second-Order-Trap).

use crate::mcp::db::DiscoveredTool;
use crate::mcp::wire::McpClient;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// I/O → Handler: ein einmaliger Discovery-Snapshot. Handler upsertet
/// die Tools in `cell.db.mcp_discovery_cache` via `handle_event`.
#[derive(Debug, Clone)]
pub enum McpEvent {
    /// Tools-Liste vom Provider, frisch nach `initialize`+`tools/list`.
    DiscoveryReady {
        /// Tools-Snapshot (Name + JSON-Schema als String).
        tools: Vec<DiscoveredTool>,
    },
}

/// Handler → I/O: unused in 10-D. `LongRunningCell` verlangt die
/// assoziierte Type — leeres Enum erfüllt sie ohne Funktionalität.
#[derive(Debug, Clone)]
pub enum McpReconfig {}

/// Konfiguration für die I/O-Sub-Task. `split_io` von `McpCell` baut
/// dies aus dem Cell-State.
pub struct RunIoConfig {
    /// Cloned client (Arc-internal über reqwest::Client).
    pub client: McpClient,
    /// A-Timeout pro HTTP-Op (initialize, tools/list). Aus
    /// `params.external_timeout_ms`.
    pub external_timeout_ms: u64,
}

/// I/O-Sub-Task. POC-Shape:
///
/// 1. Both channels are bound explicitly into the `async move`-scope
///    (Phase-10-A-Lesson Second-Order-Trap — required even though
///    `McpReconfig` is an empty enum and `reconfig_rx` will never yield
///    `Some(_)`).
/// 2. Calls `initialize` + `tools/list` ONCE, each wrapped in the
///    A-Timeout from `RunIoConfig::external_timeout_ms`. On any error:
///    `panic!` — this is a cell-init follow-up failure (overview
///    § Fehler-Verhalten Z.1369, e.g. unreachable `endpoint`), and the panic
///    is the substrate's supervision signal: the watcher classifies
///    `DeathKind::Panic` → `one_for_one` restart, after N retries the registry
///    entry is RETAINED as `failed`. A graceful return here would classify as
///    `DeathKind::Normal` → `registry.remove` — the cell would silently vanish
///    from the registry after a committed `add_nodes` (core finding #9). NO
///    in-task retry, NO reconnect (POC scope; supervision drives the retries).
/// 3. On success: pushes `McpEvent::DiscoveryReady { tools }` into
///    `events_tx`. If the receiver is gone (`send().is_err()`), returns.
/// 4. Blocks on `std::future::pending::<()>().await` until the substrate's
///    mailbox-close path aborts this future via `JoinHandle::abort()`.
///
/// The `impl Future + Send` return type is required because the generic
/// `tokio::spawn` call in `cell_task_long_running` needs `Send`.
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    cfg: RunIoConfig,
    events_tx: mpsc::Sender<McpEvent>,
    reconfig_rx: mpsc::Receiver<McpReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        // Phase-10-A-Lesson: explicit bindings prevent unintended Drop.
        let events_tx = events_tx;
        let _reconfig_rx = reconfig_rx;
        let RunIoConfig {
            client,
            external_timeout_ms,
        } = cfg;
        let timeout = Duration::from_millis(external_timeout_ms);

        // Init errors must NOT end this task gracefully: a clean return reads
        // as DeathKind::Normal and removes the registry entry. The panic is
        // the supervision signal for "Cell-Init nach Commit" (Z.1369).
        if let Err(e) = client.initialize(timeout).await {
            panic!("mcp init failed (initialize): {e:?}");
        }
        let tools = match client.list_tools(timeout).await {
            Ok(t) => t,
            Err(e) => panic!("mcp init failed (tools/list): {e:?}"),
        };
        if events_tx
            .send(McpEvent::DiscoveryReady { tools })
            .await
            .is_err()
        {
            return;
        }
        // β (mcp structural subtlety): the I/O-task has NO live-rereadable value
        // post-discovery — the only live overlay fields are external_timeout_ms
        // (Weg A, consumed handle-side on the next `call_tool`) and query_timeout_ms
        // (Weg C, applied to the handler's DbConn). So there is nothing to
        // propagate here and NO reconfig-driven `select!` is added: converting
        // this `pending()` into a `select!` that returns on `reconfig_rx` close
        // would make a graceful return read as `DeathKind::Normal` → registry
        // removal (see the task doc above) — a regression for zero functional
        // gain. Shutdown stays abort-driven via the mailbox-close path.
        std::future::pending::<()>().await;
    }
}
