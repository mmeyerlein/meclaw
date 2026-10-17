//! Phase-10-D: `McpCell` — implements `LongRunningCell` from 10-A.
//! The handler is the DB authority (discovery-cache upsert + synchronous tool
//! calls). The I/O side runs `initialize`+`tools/list` once, then
//! `pending().await`. State is single-threaded in the handler sub-task (no mutex —
//! phase-1 discipline).

use crate::credential::{CREDENTIAL_PENDING_DETAIL, CredentialSlots, Secret};
use crate::mcp::io::{McpEvent, McpIo, McpReconfig, RunIoConfig, StdioIoConfig, run_io};
use crate::mcp::wire::McpClient;
use crate::stdio_child::ChildEvent;
use crate::stdio_child::ChildSpec;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink};
use std::future::Future;
use tokio::sync::mpsc;

/// How `handle()` reaches the provider. Mirrors the parsed transport.
pub enum McpLink {
    /// A synchronous POST per call.
    Http(McpClient),
    /// A command to the I/O sub-task, answered through a `oneshot`.
    Stdio,
    /// GH #489: nothing to speak to — no provider was named.
    Unset,
}

/// GH #489: the `endpoint_unset` receipt's wording. One constant because a
/// reader who only has the log must not have to work out whether two different
/// sentences describe the same state (same argument as `llm`'s
/// `credential_pending`).
const ENDPOINT_UNSET_DETAIL: &str = "no MCP provider is configured for this cell (endpoint unset); \
     name a server and reconnect it -- nothing was called";

/// The `mcp` cell. State is single-threaded in the handler sub-task of
/// `cell_task_long_running`. `initial_io_cfg` is pulled out once by `split_io`
/// and handed to the I/O sub-task.
pub struct McpCell {
    /// The transport `handle()` speaks.
    pub(crate) link: McpLink,
    /// A timeout for every HTTP op from `handle`. Also passed through by
    /// `split_io` into the `RunIoConfig` for `run_io`. β: mutable via a params
    /// update (path A, handle side — the next `call_tool` uses it immediately;
    /// post-discovery the I/O task has no value left to re-read live).
    pub(crate) external_timeout_ms: u64,
    /// β: live effective `query_timeout_ms` (path C, cell.db ops via DbConn).
    pub(crate) query_timeout_ms: u64,
    /// Provider key for the `system.tools.<provider>.<tool>=<schema>` emissions
    /// (see the conventions section in the plan).
    pub(crate) provider_key: String,
    /// Initial I/O config, consumed once by `split_io`.
    pub(crate) initial_io_cfg: Option<McpIo>,
    /// GH #1235: the bearer comes sealed from the vault (`auth.bearer_grant_id`).
    pub(crate) credential: Option<McpCredential>,
}

/// GH #1235 (OR-programm-tm.H5.orch-4): the grant of an http cell whose bearer
/// the vault delivers. Same round as the proxy's bot token (GH #1059): asked in
/// `on_start`, nothing parked -- a call before the box is answered
/// `credential_pending` at once, and the I/O half runs no handshake until then.
pub(crate) struct McpCredential {
    /// `params.auth.bearer_grant_id`.
    grant: String,
    /// The cell's own path: the request leaves through its edges
    /// (`hop.route == "credential_request"`).
    path: meclaw_core::Path,
    /// The slot of that grant (RAM only, `<sealed>` under `Debug`).
    slots: CredentialSlots<()>,
    /// Hands the opened bearer to the waiting I/O half, once.
    bearer_tx: mpsc::Sender<Secret>,
}

impl McpCell {
    /// Constructor. `provider_key` is typically derived in the factory from the
    /// cell path (`/main/mcp` → `main_mcp`). `client` is the initially built
    /// reqwest client; `split_io` clones it internally through
    /// `RunIoConfig` (Arc-internal).
    pub fn new(
        client: McpClient,
        external_timeout_ms: u64,
        query_timeout_ms: u64,
        provider_key: String,
    ) -> Self {
        let io_client = client.clone();
        Self {
            link: McpLink::Http(client),
            external_timeout_ms,
            query_timeout_ms,
            provider_key,
            initial_io_cfg: Some(McpIo::Http(RunIoConfig {
                client: io_client,
                external_timeout_ms,
                // Replaced by the substrate via `attach_liveness`.
                liveness: meclaw_colony::IoLivenessMark::disabled(),
            })),
            credential: None,
        }
    }

    /// GH #1235: the bearer comes from the vault, sealed, under `grant`. The
    /// cell asks in `on_start`; its I/O half sleeps -- no handshake, no request
    /// -- until the box opened, and re-asks after every round that ends without
    /// one (`wait`, doubling up to its ceiling). Only on the http transport;
    /// the client this cell was built with carries no token.
    #[must_use]
    pub fn with_credential(
        mut self,
        path: meclaw_core::Path,
        grant: &str,
        wait: crate::proxy::io::CredentialWait,
    ) -> Self {
        let (bearer_tx, bearer_rx) = mpsc::channel(1);
        if let Some(McpIo::Http(cfg)) = self.initial_io_cfg.take() {
            self.initial_io_cfg = Some(McpIo::HttpGranted {
                cfg,
                wait,
                bearer_rx,
            });
        }
        let on_expired: crate::credential::ExpiryFn<()> =
            std::sync::Arc::new(|_, _| Box::pin(async {}));
        self.credential = Some(McpCredential {
            grant: grant.to_string(),
            path,
            slots: CredentialSlots::new(
                wait.wait_ms,
                crate::credential::default_credential_wait_max(),
                on_expired,
            ),
            bearer_tx,
        });
        self
    }

    /// GH #1235: ask the access hive for the bearer.
    async fn ask_for_bearer(&mut self, sink: &OriginSink) {
        let Some(c) = self.credential.as_mut() else {
            return;
        };
        let content = match c.slots.request(&c.grant) {
            Ok(content) => content,
            Err(e) => {
                tracing::error!(error = %e, "mcp: no random source for a credential request");
                return;
            }
        };
        tracing::info!(grant = %c.grant, "mcp: asking the vault for the bearer token");
        let _ = sink
            .emit(meclaw_core::CellOutput {
                target: c.path.clone(),
                content,
            })
            .await;
    }

    /// GH #1235: take a sealed delivery. Opened -> the handler's client and the
    /// I/O half get the bearer (the handshake starts). Not opened -> the cell
    /// stays asleep; its next round asks again. Never echoes a value.
    async fn accept_bearer(&mut self, content: &meclaw_core::serde_json::Value) {
        let Some(c) = self.credential.as_mut() else {
            tracing::warn!(
                "mcp: a sealed box arrived, but this cell has no auth.bearer_grant_id -- discarded"
            );
            return;
        };
        match c.slots.accept_sealed(content).await {
            Ok(accepted) => {
                let Some(bearer) = c.slots.secret_handle(&accepted.grant) else {
                    tracing::warn!(
                        grant = %accepted.grant,
                        "mcp: the sealed bearer opened empty -- the cell stays asleep"
                    );
                    return;
                };
                tracing::info!(grant = %accepted.grant, "mcp: bearer received sealed and opened in RAM");
                if let McpLink::Http(client) = &mut self.link {
                    *client = client.with_bearer(bearer.expose());
                }
                let _ = c.bearer_tx.send(bearer).await;
            }
            Err(refusal) => match refusal.detail() {
                None => tracing::warn!(
                    "mcp: a sealed box of an earlier credential round arrived late and was discarded"
                ),
                Some(detail) => tracing::warn!(
                    %detail,
                    "mcp: the sealed bearer was refused -- the cell stays asleep and asks again \
                     after the round's wait"
                ),
            },
        }
    }

    /// Constructor for the stdio transport. The cell holds no client at all —
    /// the I/O sub-task owns the child process, and `handle()` talks to it
    /// through the reconfig channel.
    pub fn new_stdio(
        spec: ChildSpec,
        external_timeout_ms: u64,
        query_timeout_ms: u64,
        provider_key: String,
    ) -> Self {
        Self {
            link: McpLink::Stdio,
            external_timeout_ms,
            query_timeout_ms,
            provider_key,
            initial_io_cfg: Some(McpIo::Stdio(StdioIoConfig {
                spec,
                external_timeout_ms,
                // Replaced by the substrate via `attach_liveness`.
                liveness: meclaw_colony::IoLivenessMark::disabled(),
            })),
            credential: None,
        }
    }

    /// GH #489: constructor for a cell whose provider was never named
    /// (`endpoint` absent or empty on the `http` transport).
    ///
    /// It is a full cell — mailbox, `cell.db`, params updates, the lot — with
    /// one thing missing: a far side. So it runs no handshake, holds no client,
    /// and answers every call with the `endpoint_unset` receipt instead of
    /// panicking against an address nobody chose. The two timeouts still apply:
    /// `query_timeout_ms` governs its `cell.db`, and `external_timeout_ms` is
    /// kept so that a params update on a cell in this state round-trips exactly
    /// as it does on a configured one.
    pub fn new_unset(
        external_timeout_ms: u64,
        query_timeout_ms: u64,
        provider_key: String,
    ) -> Self {
        Self {
            link: McpLink::Unset,
            external_timeout_ms,
            query_timeout_ms,
            provider_key,
            initial_io_cfg: Some(McpIo::Unset),
            credential: None,
        }
    }
}

impl LongRunningCell for McpCell {
    type Event = McpEvent;
    type Reconfig = McpReconfig;
    type Io = McpIo;

    fn split_io(&mut self) -> Self::Io {
        self.initial_io_cfg.take().expect("split_io called twice")
    }

    /// Issue #7: take the mark for whichever transport this cell runs.
    fn attach_liveness(io: &mut Self::Io, mark: meclaw_colony::IoLivenessMark) {
        match io {
            McpIo::Http(cfg) | McpIo::HttpGranted { cfg, .. } => cfg.liveness = mark,
            McpIo::Stdio(cfg) => cfg.liveness = mark,
            // GH #489: a task that owes no round trip reports no liveness —
            // see `run_unset_io` for why announcing here would be the wrong
            // signal rather than a harmless extra one.
            McpIo::Unset => {}
        }
    }

    /// I/O sub-task — delegates to `crate::mcp::io::run_io`.
    /// `+ Send` is load-bearing (AFIT does not bind Send; the `tokio::spawn`
    /// in `cell_task_long_running` needs it). Pattern symmetric to
    /// `proxy::cell::ProxyCell::run_io` and the trait docs in
    /// `crates/meclaw-colony/src/long_running_cell.rs:96-110`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_io(io, events_tx, reconfig_rx)
    }

    /// GH #1235: a granted cell asks for its bearer before anything else; key
    /// pair and token are RAM-only, so a respawn asks again.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move { self.ask_for_bearer(sink).await }
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
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // GH #1235: the sealed bearer. Never touches `cell.db`.
            if let meclaw_core::Body::Inline(ref v) = msg.body
                && v.get("sealed").is_some()
            {
                self.accept_bearer(v).await;
                return;
            }
            // β: params-update slot (config.md § Access l.20), handled FIRST.
            // Mutable: external_timeout_ms (path A, next call_tool) + query_timeout_ms
            // (path C, DbConn live). Immutable: endpoint + auth (credential/identity).
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
                        // Live apply: external_timeout_ms (path A) + query_timeout_ms
                        // (path C, DbConn). Both effective on the NEXT op.
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
            // GH #489: no provider was named, so there is nothing to call and
            // nothing cached to list. Every round takes the same receipt,
            // including the discovery round: handing a brain an empty tool
            // listing would be a menu that is silently empty and a model that
            // never learns why (`docs/development-rules.md` § 2c — an empty
            // result and a forgotten call must never look alike).
            if matches!(self.link, McpLink::Unset) {
                crate::mcp::emit::emit_tool_result_error(
                    sink,
                    &msg,
                    &parsed.name,
                    0,
                    &parsed.call_id,
                    "endpoint_unset",
                    ENDPOINT_UNSET_DETAIL,
                )
                .await;
                return;
            }
            // GH #1235: a granted cell before its box has no token and has run
            // no handshake; nothing is called, the round says so.
            if let Some(c) = &self.credential
                && c.slots.secret(&c.grant).is_none()
            {
                crate::mcp::emit::emit_tool_result_error(
                    sink,
                    &msg,
                    &parsed.name,
                    0,
                    &parsed.call_id,
                    "credential_pending",
                    CREDENTIAL_PENDING_DETAIL,
                )
                .await;
                return;
            }

            // T19: __list_tools__ reads cache, emits system.tools listing — no call_tool.
            if parsed.name == "__list_tools__" {
                let started = tokio::time::Instant::now();
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
            let started = tokio::time::Instant::now();
            let r = match &self.link {
                McpLink::Http(client) => {
                    client
                        .call_tool(&parsed.name, parsed.arguments, timeout)
                        .await
                }
                // stdio: the I/O sub-task owns the pipes, so the request goes
                // out as a command and the answer comes back on a oneshot.
                McpLink::Stdio => {
                    let params = serde_json::json!({
                        "name": &parsed.name,
                        "arguments": parsed.arguments,
                    });
                    crate::mcp::stdio::rpc_over_task(reconfig_tx, "tools/call", params, timeout)
                        .await
                }
                // GH #489: not reachable — an unset link took its receipt
                // above, before a call was ever built. The arm exists because
                // exhaustiveness is checked, and it repeats the SAME receipt so
                // that even an impossible path cannot produce a second code for
                // one state.
                McpLink::Unset => {
                    crate::mcp::emit::emit_tool_result_error(
                        sink,
                        &msg,
                        &parsed.name,
                        0,
                        &parsed.call_id,
                        "endpoint_unset",
                        ENDPOINT_UNSET_DETAIL,
                    )
                    .await;
                    return;
                }
            };
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
        sink: &'a OriginSink,
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
                // An unsolicited frame. The POC protocol has no notifications,
                // so this is diagnostics only; the typed-emission consumers
                // (harness cell type) are a later package.
                // GH #1235: a round ended without the box -- ask again.
                McpEvent::CredentialRoundExpired { round, wait_ms } => {
                    tracing::warn!(
                        round,
                        wait_ms,
                        "mcp: no sealed bearer arrived in time -- asking again"
                    );
                    self.ask_for_bearer(sink).await;
                }
                McpEvent::Child(ChildEvent::Frame(v)) => {
                    tracing::debug!(frame = %v, "mcp stdio: unsolicited frame");
                }
                McpEvent::Child(ChildEvent::Malformed { raw }) => {
                    tracing::warn!(line = %raw, "mcp stdio: non-json line from child");
                }
                // Ratified liveness slice D1: the child is gone, so this cell
                // cannot serve anything until it has a fresh one. Panicking is
                // the only signal the supervisor understands — it classifies
                // DeathKind::Panic, restarts one_for_one (fresh child, fresh
                // discovery), and after restart_limit RETAINS the entry as
                // `failed`. A graceful return would read as DeathKind::Normal
                // and remove the cell from the registry (core finding #9).
                //
                // Delimitation against the panic-hook invariant ("panic BEFORE
                // output, the panicking cell emits nothing"): that invariant
                // governs a panic whose trigger is known before the emit. Here
                // the panic is a deliberate termination AFTER a completed
                // emit — an in-flight `handle()` has already awaited its
                // OutputSink push and returned (the serve loop resolves every
                // pending request before it announces the death), so no output
                // is lost and no extra hop is produced.
                McpEvent::Child(ChildEvent::Exited(exit)) => {
                    panic!("mcp stdio child died: {}", exit.detail());
                }
            }
        }
    }
}
