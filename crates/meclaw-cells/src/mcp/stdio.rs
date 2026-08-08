//! The stdio transport of the `mcp` cell: JSON-RPC over line-JSON to a child
//! process, riding the shared `stdio_child` core.
//!
//! Everything protocol-specific lives here — request ids, the `initialize`
//! handshake, the error mapping — while spawning, framing, correlation and
//! reaping stay generic in `crate::stdio_child`.

use crate::mcp::io::{McpEvent, McpReconfig, StdioIoConfig};
use crate::mcp::jsonrpc::{JsonRpcRequest, RequestId};
use crate::mcp::wire::{McpError, build_initialize_params, tools_from_result};
use crate::stdio_child::{
    ChildCommand, CorrelationKey, ServeConfig, StdioChild, StdioChildError, serve::serve_child,
};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Correlate on the JSON-RPC `id`, normalised to a string.
///
/// `RequestId` serialises transparently as a string, but a lenient server may
/// echo a number; both must find their way home.
pub fn correlator(v: &JsonValue) -> Option<CorrelationKey> {
    match v.get("id")? {
        JsonValue::Null => None,
        JsonValue::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// The stdio I/O sub-task: spawn, handshake, then serve forever.
///
/// Init failures panic rather than return — identical to the http branch and
/// for the same reason (core finding #9: a clean return reads as
/// `DeathKind::Normal` and removes the registry entry).
pub async fn run_stdio_io(
    cfg: StdioIoConfig,
    events_tx: mpsc::Sender<McpEvent>,
    reconfig_rx: mpsc::Receiver<McpReconfig>,
) {
    // Phase-10-A lesson: explicit bindings prevent unintended Drop.
    let events_tx = events_tx;
    let reconfig_rx = reconfig_rx;
    let StdioIoConfig {
        spec,
        external_timeout_ms,
    } = cfg;
    let timeout = Duration::from_millis(external_timeout_ms);

    let mut child = match StdioChild::spawn(&spec) {
        Ok(c) => c,
        Err(e) => panic!("mcp init failed (spawn): {}", e.detail()),
    };
    if let Err(e) =
        rpc_over_child(&mut child, "initialize", build_initialize_params(), timeout).await
    {
        panic!("mcp init failed (initialize): {e:?}");
    }
    let tools = match rpc_over_child(&mut child, "tools/list", json!({}), timeout).await {
        Ok(v) => match tools_from_result(&v) {
            Ok(t) => t,
            Err(e) => panic!("mcp init failed (tools/list): {e:?}"),
        },
        Err(e) => panic!("mcp init failed (tools/list): {e:?}"),
    };
    if events_tx
        .send(McpEvent::DiscoveryReady { tools })
        .await
        .is_err()
    {
        return;
    }

    let serve_cfg = ServeConfig {
        write_timeout: timeout,
        kill_grace: Duration::from_millis(spec.kill_grace_ms),
    };
    // Parks forever once the child is gone — the A1′ lifetime contract.
    serve_child(child, correlator, serve_cfg, reconfig_rx, events_tx).await;
}

/// One JSON-RPC exchange directly on the child, used during the handshake
/// before the serve loop owns the pipes.
async fn rpc_over_child(
    child: &mut StdioChild,
    method: &str,
    params: JsonValue,
    timeout: Duration,
) -> Result<JsonValue, McpError> {
    let (line, id) = build_request(method, params)?;
    let want = id.clone();
    let raw = child
        .request_once(
            &line,
            move |v| correlator(v).as_deref() == Some(want.as_str()),
            timeout,
        )
        .await
        .map_err(map_child_error)?;
    decode_rpc(raw)
}

/// One JSON-RPC exchange through the running serve loop, used by `handle()`.
///
/// The answer comes back on a `oneshot` rather than the events channel: the
/// handler is awaiting this future inside its own `select!` arm and could not
/// pull an event in the meantime.
pub(crate) async fn rpc_over_task(
    reconfig_tx: &mpsc::Sender<McpReconfig>,
    method: &str,
    params: JsonValue,
    timeout: Duration,
) -> Result<JsonValue, McpError> {
    let (line, id) = build_request(method, params)?;
    let (tx, rx) = oneshot::channel();
    let cmd = ChildCommand::Send {
        line,
        correlate: Some(id),
        reply: Some(tx),
    };
    if reconfig_tx.send(McpReconfig::Child(cmd)).await.is_err() {
        return Err(McpError::Transport("io task gone".into()));
    }
    // A-timeout (rule 12): this await IS the bounded operation — the stream
    // read inside the serve loop deliberately has none.
    let raw = match tokio::time::timeout(timeout, rx).await {
        Err(_) => return Err(McpError::Timeout),
        Ok(Err(_)) => return Err(McpError::Transport("io task dropped the reply".into())),
        Ok(Ok(Err(e))) => return Err(map_child_error(e)),
        Ok(Ok(Ok(v))) => v,
    };
    decode_rpc(raw)
}

/// Build a JSON-RPC request line plus the correlation key it will answer to.
fn build_request(method: &str, params: JsonValue) -> Result<(JsonValue, CorrelationKey), McpError> {
    let req = JsonRpcRequest::new(RequestId::new(), method, params);
    let id = req.id.as_str().to_string();
    let line =
        serde_json::to_value(&req).map_err(|e| McpError::Transport(format!("serialize: {e}")))?;
    Ok((line, id))
}

/// Split a JSON-RPC response into result or error, with the same
/// classification the http transport produces — so the cell's `error_code`
/// contract does not depend on the transport.
fn decode_rpc(raw: JsonValue) -> Result<JsonValue, McpError> {
    if let Some(err) = raw.get("error").filter(|e| !e.is_null()) {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        return Err(McpError::Rpc { code, message });
    }
    raw.get("result")
        .cloned()
        .ok_or_else(|| McpError::Transport("missing result and error".into()))
}

/// Map a child-layer failure onto the cell's error vocabulary.
///
/// Only a timeout keeps its own identity (it becomes `provider_timeout`);
/// everything else — spawn, pipe, a dead child — is a transport failure and
/// surfaces as `mcp_error`, the code `docs/cell-types.md` § `mcp` already
/// defines. A dead child needs no new code: from the topology's point of view
/// it is the same event as an unreachable HTTP backend.
fn map_child_error(e: StdioChildError) -> McpError {
    match e {
        StdioChildError::Timeout => McpError::Timeout,
        other => McpError::Transport(other.detail()),
    }
}
