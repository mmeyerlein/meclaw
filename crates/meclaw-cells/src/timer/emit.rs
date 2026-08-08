//! Phase-10-B: emit helper for op error replies from `handle`. Target =
//! `msg.reply_to`; with `reply_to` absent the emission falls back to the cell's
//! own `msg.target` (W2d: NO longer the `/colony/dead_letters` READ endpoint — an
//! out-edge without a match ends up as `no_route` in the DLQ). UBF body with
//! `header.error_code`. The parent context (parent_message_id + trace_id +
//! input_ttl + input_headers) already lives in the `OutputSink`.

use meclaw_core::{CellOutput, Message, OutputSink};
use serde_json::json;

/// Emit an op error reply via `OutputSink`. Target = `msg.reply_to`, fallback
/// `msg.target` (the cell's own path). `error_code` lands as
/// `content.header.error_code`, `detail` as `content.meta.detail`.
pub async fn emit_op_error(sink: &OutputSink, msg: &Message, error_code: &str, detail: &str) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let content = json!({
        "header": {
            "error_code": error_code,
            "msg_type":   "timer_op_error",
        },
        "messages": [],
        "meta": { "detail": detail },
    });
    let _ = sink.push(CellOutput { target, content }).await;
}
