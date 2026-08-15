//! Phase-10-D T13: emit helpers for `McpCell`. Builds UBF-valid `tool_result`
//! bodies and pushes them via `OutputSink`. Target = `msg.reply_to`, fallback
//! `msg.target` (W2d: its own path, not the READ endpoint).
//!
//! Body-Shape (Phase-9-store-Pattern + UBF-Schema):
//! - `header` is a top-level slot (NOT `content.header`).
//! - `messages[]` holds exactly one turn with `origin:"tool"`,
//!   `type:"tool_result"` and a mandatory `id` (UBF schema constraint for
//!   `tool_result`).
//!
//! Spec anchor: `docs/cell-types.md` § `mcp` l.474-478.

use meclaw_core::{
    CellOutput, Message, OutputSink,
    serde_json::{Value as JsonValue, json},
};

/// Emit a successful `tool_result`-Turn. `tool_call_id` is echoed into
/// the turn's `id` (UBF-required for `tool_result`). Target = `msg.reply_to`,
/// fallback `msg.target` (W2d: its own path, not the READ endpoint). Header carries `mcp_tool` + `duration_ms`
/// (no `error_code`). Payload is JSON-serialized into the turn's `text`.
pub async fn emit_tool_result_success(
    sink: &OutputSink,
    msg: &Message,
    mcp_tool: &str,
    duration_ms: u64,
    tool_call_id: &str,
    payload: JsonValue,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let body = json!({
        "header": { "mcp_tool": mcp_tool, "duration_ms": duration_ms },
        "messages": [{
            "origin": "tool",
            "type": "tool_result",
            "text": payload.to_string(),
            "id": tool_call_id
        }]
    });
    let _ = sink
        .push(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Emit a failed `tool_result`-Turn. `error_code` ∈ {`"provider_timeout"`,
/// `"mcp_error"`} (POC error-code-set per plan § Error-Code-Set). `detail`
/// lands in the turn's `text`. `tool_call_id` is echoed into `id`.
/// Target = `msg.reply_to`, fallback `msg.target` (W2d: its own path, not the READ endpoint).
pub async fn emit_tool_result_error(
    sink: &OutputSink,
    msg: &Message,
    mcp_tool: &str,
    duration_ms: u64,
    tool_call_id: &str,
    error_code: &str,
    detail: &str,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let body = json!({
        "header": {
            "mcp_tool": mcp_tool,
            "duration_ms": duration_ms,
            "error_code": error_code
        },
        "messages": [{
            "origin": "tool",
            "type": "tool_result",
            "text": detail,
            "id": tool_call_id
        }]
    });
    let _ = sink
        .push(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Emit a UBF message populating `system.tools.<provider_key>.<tool_name> = <schema>`
/// for every cached tool. Target = `msg.reply_to`, fallback `msg.target` (W2d: its own path, not the READ endpoint).
/// Header carries `mcp_tool = "__list_tools__"` + `duration_ms` (required).
/// Body has `messages: []` (UBF-valid) + populated `system.tools.<provider>` map.
///
/// `provider_key` is the cell-path-derived identifier (e.g. `main_mcp` for `/main/mcp`).
/// Spec anchor: `docs/cell-types.md` § `mcp` l.478 (`mcp_tool` + `duration_ms` in the header).
pub async fn emit_system_tools_listing(
    sink: &OutputSink,
    msg: &Message,
    provider_key: &str,
    tools: &[crate::mcp::db::DiscoveredTool],
    duration_ms: u64,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let mut provider_map = meclaw_core::serde_json::Map::new();
    for t in tools {
        let schema: JsonValue =
            meclaw_core::serde_json::from_str(&t.schema_json).unwrap_or(JsonValue::Null);
        provider_map.insert(t.name.clone(), schema);
    }
    let body = json!({
        "header": { "mcp_tool": "__list_tools__", "duration_ms": duration_ms },
        "system": { "tools": { provider_key: provider_map } },
        "messages": []
    });
    let _ = sink
        .push(CellOutput {
            target,
            content: body,
        })
        .await;
}
