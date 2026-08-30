//! I/O sub-task frames and the `run_io` entry point, for BOTH mcp transports.
//!
//! - `http` (phase-10-D): `initialize` + `tools/list` once, then park. No
//!   persistent connection, so nothing to observe afterwards.
//! - `stdio` (P7): spawn the child, run the same handshake over line-JSON,
//!   then hand the child to the shared `serve_child` loop, whose stream read
//!   carries the liveness signal this cell type never had before.
//!
//! Handler → I/O uses `McpReconfig`, I/O → handler uses `McpEvent`.
//! `reconfig_rx` MUST be bound explicitly inside the `run_io` `async move`
//! scope (phase-10-A lesson, the second-order trap).

use crate::mcp::db::DiscoveredTool;
use crate::mcp::wire::McpClient;
use crate::stdio_child::{ChildCommand, ChildEvent, ChildSpec};
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// I/O → Handler.
#[derive(Debug, Clone)]
pub enum McpEvent {
    /// Tool list from the provider, fresh after `initialize`+`tools/list`.
    DiscoveryReady {
        /// Tools snapshot (name + JSON schema as string).
        tools: Vec<DiscoveredTool>,
    },
    /// stdio only: something happened on the child connection.
    Child(ChildEvent),
}

impl From<ChildEvent> for McpEvent {
    fn from(e: ChildEvent) -> Self {
        McpEvent::Child(e)
    }
}

/// Handler → I/O.
#[derive(Debug)]
pub enum McpReconfig {
    /// stdio only: a command for the child-serving loop. On the http
    /// transport no reconfig hint exists (tool calls run synchronously in
    /// `handle()` against a fresh connection).
    Child(ChildCommand),
}

impl From<McpReconfig> for ChildCommand {
    fn from(r: McpReconfig) -> Self {
        match r {
            McpReconfig::Child(c) => c,
        }
    }
}

/// Configuration for the http I/O sub-task.
pub struct RunIoConfig {
    /// Cloned client (Arc internally via reqwest::Client).
    pub client: McpClient,
    /// A timeout per HTTP op (initialize, tools/list). From
    /// `params.external_timeout_ms`.
    pub external_timeout_ms: u64,
    /// Issue #7: progress mark, set once the provider has answered the
    /// discovery handshake. Default = disabled (reports nowhere).
    pub liveness: meclaw_colony::IoLivenessMark,
}

/// Configuration for the stdio I/O sub-task.
pub struct StdioIoConfig {
    /// How to start the child process.
    pub spec: ChildSpec,
    /// A timeout per provider op (handshake requests, writes).
    pub external_timeout_ms: u64,
    /// Issue #7: progress mark, set once the child has answered the discovery
    /// handshake. Default = disabled (reports nowhere).
    pub liveness: meclaw_colony::IoLivenessMark,
}

/// The owned I/O state handed to the sub-task, one variant per transport.
///
/// Issue #7 (honest limit): the mark below covers the discovery handshake, which
/// on the `http` transport is the whole external life of this task — it parks
/// afterwards. On `stdio` the shared `serve_child` loop keeps exchanging frames
/// past the handshake and does not mark them yet, so a stdio mark reads as "the
/// child answered at start", not "the child answered recently".
pub enum McpIo {
    /// HTTP + JSON-RPC.
    Http(RunIoConfig),
    /// Child process speaking line-JSON.
    Stdio(StdioIoConfig),
    /// GH #489: no provider was named, so there is no I/O to run.
    Unset,
}

/// I/O sub-task. Dispatches on the transport; both branches share the
/// contract that they NEVER return voluntarily (A1′) and that an init failure
/// panics rather than returning, because a clean return would be classified as
/// `DeathKind::Normal` and remove the registry entry (core finding #9).
///
/// The `impl Future + Send` return type is required because the generic
/// `tokio::spawn` call in `cell_task_long_running` needs `Send`.
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    io: McpIo,
    events_tx: mpsc::Sender<McpEvent>,
    reconfig_rx: mpsc::Receiver<McpReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        match io {
            McpIo::Http(cfg) => run_http_io(cfg, events_tx, reconfig_rx).await,
            McpIo::Stdio(cfg) => crate::mcp::stdio::run_stdio_io(cfg, events_tx, reconfig_rx).await,
            McpIo::Unset => run_unset_io(events_tx, reconfig_rx).await,
        }
    }
}

/// The phase-10-D http loop, unchanged in behaviour.
///
/// 1. Both channels are bound explicitly into the scope (phase-10-A lesson,
///    second-order trap — required even though nothing sends reconfig hints on
///    this transport).
/// 2. Calls `initialize` + `tools/list` ONCE, each under the A-timeout. On any
///    error: `panic!` — a cell-init follow-up failure (overview § Behavior on
///    errors l.1369) whose supervision signal IS the panic: the watcher
///    classifies `DeathKind::Panic` → `one_for_one`, and after N retries the
///    registry entry is RETAINED as `failed`. A graceful return would classify
///    as `DeathKind::Normal` → `registry.remove` (core finding #9). NO in-task
///    retry, NO reconnect — supervision drives the retries.
/// 3. Pushes `McpEvent::DiscoveryReady`, then parks.
///
/// β (mcp structural subtlety): post-discovery this task has NO live-rereadable
/// value — the only live overlay fields are `external_timeout_ms` (path A,
/// consumed handle-side on the next `call_tool`) and `query_timeout_ms`
/// (path C, on the handler's DbConn). So there is nothing to propagate and no
/// reconfig-driven `select!` is added: turning this `pending()` into a
/// `select!` that returns on `reconfig_rx` close would make a graceful return
/// read as `DeathKind::Normal` → registry removal, a regression for zero gain.
/// Shutdown stays abort-driven via the mailbox-close path.
async fn run_http_io(
    cfg: RunIoConfig,
    events_tx: mpsc::Sender<McpEvent>,
    reconfig_rx: mpsc::Receiver<McpReconfig>,
) {
    let events_tx = events_tx;
    let _reconfig_rx = reconfig_rx;
    let RunIoConfig {
        client,
        external_timeout_ms,
        liveness,
    } = cfg;
    let timeout = Duration::from_millis(external_timeout_ms);
    // Issue #7: announce before the first call — a provider that never answers
    // is visibly "never succeeded" rather than invisible.
    liveness.announce();

    if let Err(e) = client.initialize(timeout).await {
        panic!("mcp init failed (initialize): {e:?}");
    }
    let tools = match client.list_tools(timeout).await {
        Ok(t) => t,
        Err(e) => panic!("mcp init failed (tools/list): {e:?}"),
    };
    // Issue #7: the provider answered both handshake calls.
    liveness.mark_success();
    if events_tx
        .send(McpEvent::DiscoveryReady { tools })
        .await
        .is_err()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// GH #489: the I/O task of a cell that names no provider.
///
/// There is no handshake to run, so there is nothing that could fail and
/// nothing to panic about. The task binds both channels (phase-10-A lesson,
/// same as the two transports above), says once in the log that this cell is
/// idle by configuration, and parks — for the same reason `run_http_io` parks
/// rather than returning: a graceful return classifies as `DeathKind::Normal`
/// and removes the registry entry (core finding #9), and this cell is not
/// dead, it is waiting for an operator to name a server.
///
/// It marks NO liveness. `IoLivenessMark::announce` means "this task exists and
/// has not yet completed a round trip", which `/health` reports as an age that
/// only ever grows — the right signal for a provider that was named and is not
/// answering, and exactly the wrong one here, where a round trip was never owed.
/// The signal for this state is the `endpoint_unset` receipt on every call.
async fn run_unset_io(events_tx: mpsc::Sender<McpEvent>, reconfig_rx: mpsc::Receiver<McpReconfig>) {
    let _events_tx = events_tx;
    let _reconfig_rx = reconfig_rx;
    tracing::info!(
        "mcp: no endpoint configured -- this cell is idle and answers every call with \
         error_code=endpoint_unset; set the provider URL to wake it"
    );
    std::future::pending::<()>().await;
}
