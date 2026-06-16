//! Phase-10-D: `McpCell` — implementiert `LongRunningCell` aus 10-A.
//! Handler ist DB-Authority (Discovery-Cache-Upsert + Tool-Call-Synchron).
//! I/O läuft `initialize`+`tools/list` einmal, dann `pending().await`.
//! State single-threaded im Handler-Sub-Task (kein Mutex —
//! Phase-1-Disziplin).

use crate::mcp::io::{McpEvent, McpReconfig, RunIoConfig, run_io};
use crate::mcp::wire::McpClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink};
use std::future::Future;
use tokio::sync::mpsc;

/// `mcp`-Cell. State single-threaded im Handler-Sub-Task von
/// `cell_task_long_running`. `initial_io_cfg` wird einmal durch
/// `split_io` rausgezogen + an die I/O-Sub-Task übergeben.
pub struct McpCell {
    /// Reqwest-/MCP-Client für `handle(tool_call)`-Pfad (synchroner POST).
    pub(crate) client: McpClient,
    /// A-Timeout für jede HTTP-Op aus `handle`. Auch von `split_io`
    /// in die `RunIoConfig` für `run_io` durchgereicht. β: mutable via
    /// params-update (Weg A, handle-side — der nächste `call_tool` nutzt es
    /// sofort; der I/O-Task hat post-Discovery keinen live-nachzulesenden Wert).
    pub(crate) external_timeout_ms: u64,
    /// β: live effective `query_timeout_ms` (Weg C, cell.db-Ops via DbConn).
    pub(crate) query_timeout_ms: u64,
    /// Provider-Key für `system.tools.<provider>.<tool>=<schema>`-Emits
    /// (siehe Konventionen-Sektion im Plan).
    pub(crate) provider_key: String,
    /// I/O-Initialkonfig, einmal von `split_io` konsumiert.
    pub(crate) initial_io_cfg: Option<RunIoConfig>,
}

impl McpCell {
    /// Konstruktor. `provider_key` wird typisch in der Factory aus dem
    /// Cell-Pfad abgeleitet (`/main/mcp` → `main_mcp`). `client` ist der
    /// initial gebaute Reqwest-Client; `split_io` cloned ihn intern über
    /// `RunIoConfig` (Arc-internal).
    pub fn new(
        client: McpClient,
        external_timeout_ms: u64,
        query_timeout_ms: u64,
        provider_key: String,
    ) -> Self {
        let io_client = client.clone();
        Self {
            client,
            external_timeout_ms,
            query_timeout_ms,
            provider_key,
            initial_io_cfg: Some(RunIoConfig {
                client: io_client,
                external_timeout_ms,
            }),
        }
    }
}

/// I/O-lokaler State (Single-Owner, vom I/O-Sub-Task by-value gehalten).
/// Kein Mutex, kein Arc — Phase-1-Disziplin.
pub struct McpIo {
    /// Konfiguration für `run_io` (Client + Timeout).
    pub(crate) cfg: RunIoConfig,
}

impl LongRunningCell for McpCell {
    type Event = McpEvent;
    type Reconfig = McpReconfig;
    type Io = McpIo;

    fn split_io(&mut self) -> Self::Io {
        McpIo {
            cfg: self.initial_io_cfg.take().expect("split_io called twice"),
        }
    }

    /// I/O-Sub-Task — delegiert an `crate::mcp::io::run_io`.
    /// `+ Send` ist load-bearing (AFIT bindet kein Send; `tokio::spawn`
    /// in `cell_task_long_running` braucht es). Pattern symmetrisch zu
    /// `proxy::cell::ProxyCell::run_io` und der Trait-Doc in
    /// `crates/meclaw-colony/src/long_running_cell.rs:96-110`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_io(io.cfg, events_tx, reconfig_rx)
    }

    /// Inbound-Message-Handler. Parses the tail `tool_call`-turn (Phase-9-store
    /// convention: `text` = JSON-string with `name`+`arguments`, `id` = UBF-required
    /// call id), dispatches to `McpClient::call_tool` with A-Timeout, emits exactly
    /// one `tool_result`-turn via `OutputSink`. Error-branches typed in T18;
    /// `__list_tools__` discovery-path in T19.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // β: params-update slot (config.md § Zugriff Z.20), handled FIRST.
            // Mutable: external_timeout_ms (Weg A, next call_tool) + query_timeout_ms
            // (Weg C, DbConn live). Immutable: endpoint + auth (credential/identity).
            // A params-only message persists + returns silently.
            if let meclaw_core::Body::Inline(ref v) = msg.body
                && let Some(params_val) = v.get("params")
            {
                let update_obj = match params_val.as_object() {
                    Some(o) => o.clone(),
                    None => {
                        crate::mcp::emit::emit_tool_result_error(
                            sink,
                            &msg,
                            "__params__",
                            0,
                            "",
                            "invalid_input",
                            "params slot: not a JSON object",
                        )
                        .await;
                        return;
                    }
                };
                let current = crate::mcp::params::McpOverlay {
                    external_timeout_ms: self.external_timeout_ms,
                    query_timeout_ms: self.query_timeout_ms,
                };
                match crate::params_overlay::apply_update(&current, &update_obj) {
                    Ok((new_ov, overlay)) => {
                        let now = crate::params_overlay::now_unix_seconds();
                        let persist = db
                            .call_with_timeout(move |c| {
                                crate::params_overlay::persist_params_overlay(c, &overlay, now)
                            })
                            .await;
                        match persist {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                crate::mcp::emit::emit_tool_result_error(
                                    sink,
                                    &msg,
                                    "__params__",
                                    0,
                                    "",
                                    "invalid_input",
                                    &format!("cell.db params write failed: {e}"),
                                )
                                .await;
                                return;
                            }
                            Err(meclaw_colony::QueryTimeout::Interrupted) => {
                                crate::mcp::emit::emit_tool_result_error(
                                    sink,
                                    &msg,
                                    "__params__",
                                    0,
                                    "",
                                    "query_timeout",
                                    "params write exceeded query_timeout_ms",
                                )
                                .await;
                                return;
                            }
                        }
                        // Live apply: external_timeout_ms (Weg A) + query_timeout_ms
                        // (Weg C, DbConn). Both effective on the NEXT op.
                        self.external_timeout_ms = new_ov.external_timeout_ms;
                        self.query_timeout_ms = new_ov.query_timeout_ms;
                        db.set_query_timeout(Some(std::time::Duration::from_millis(
                            self.query_timeout_ms,
                        )));
                    }
                    Err(e) => {
                        crate::mcp::emit::emit_tool_result_error(
                            sink,
                            &msg,
                            "__params__",
                            0,
                            "",
                            "invalid_input",
                            &e.detail(),
                        )
                        .await;
                    }
                }
                // Standalone params-update → done (no tool_call in this message).
                return;
            }

            let parsed = match crate::mcp::parse::parse_tool_call(&msg) {
                Ok(p) => p,
                Err(reason) => {
                    crate::mcp::emit::emit_tool_result_error(
                        sink,
                        &msg,
                        "<unknown>",
                        0,
                        "",
                        "mcp_error",
                        &reason,
                    )
                    .await;
                    return;
                }
            };
            // T19: __list_tools__ reads cache, emits system.tools listing — no call_tool.
            if parsed.name == "__list_tools__" {
                let started = std::time::Instant::now();
                let cache = match db
                    .call_with_timeout(|c| crate::mcp::db::load_discovery_cache(c))
                    .await
                {
                    Ok(r) => r.unwrap_or_else(|_| Vec::new()),
                    Err(meclaw_colony::QueryTimeout::Interrupted) => Vec::new(),
                };
                let duration_ms = started.elapsed().as_millis() as u64;
                crate::mcp::emit::emit_system_tools_listing(
                    sink,
                    &msg,
                    &self.provider_key,
                    &cache,
                    duration_ms,
                )
                .await;
                return;
            }

            // T17: success-path only. T18 types the Err-branches.
            let timeout = std::time::Duration::from_millis(self.external_timeout_ms);
            let started = std::time::Instant::now();
            let r = self
                .client
                .call_tool(&parsed.name, parsed.arguments, timeout)
                .await;
            let duration_ms = started.elapsed().as_millis() as u64;
            match r {
                Ok(payload) => {
                    crate::mcp::emit::emit_tool_result_success(
                        sink,
                        &msg,
                        &parsed.name,
                        duration_ms,
                        &parsed.call_id,
                        payload,
                    )
                    .await;
                }
                Err(e) => {
                    use crate::mcp::wire::McpError;
                    let (code, detail) = match e {
                        McpError::Timeout => (
                            "provider_timeout",
                            format!("A-Timeout after {duration_ms}ms"),
                        ),
                        McpError::Transport(s) => ("mcp_error", format!("transport: {s}")),
                        McpError::Rpc { code, message } => {
                            ("mcp_error", format!("rpc {code}: {message}"))
                        }
                    };
                    crate::mcp::emit::emit_tool_result_error(
                        sink,
                        &msg,
                        &parsed.name,
                        duration_ms,
                        &parsed.call_id,
                        code,
                        &detail,
                    )
                    .await;
                }
            }
        }
    }

    /// Processes I/O events from the `run_io` sub-task. For
    /// `DiscoveryReady`, upserts the discovered tools into
    /// `cell.db.mcp_discovery_cache` via `DbConn::call` on a
    /// `spawn_blocking`-backed thread. The upsert timestamp is a
    /// UTC RFC-3339 string with second precision. DB errors are silently
    /// swallowed (POC — discovery is best-effort; a failure here does not
    /// block the cell from serving tool-calls from a pre-existing cache).
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        _sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            match event {
                McpEvent::DiscoveryReady { tools } => {
                    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                    let _ = db
                        .call_with_timeout(move |c| {
                            crate::mcp::db::upsert_discovery_tools(c, &tools, &now)
                        })
                        .await;
                }
            }
        }
    }
}
