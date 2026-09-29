//! Phase-10-B: emit helper for op error replies from `handle`. Target =
//! `msg.reply_to`; with `reply_to` absent the emission falls back to the cell's
//! own `msg.target` (W2d: NO longer the `/colony/dead_letters` READ endpoint — an
//! out-edge without a match ends up as `no_route` in the DLQ). UBF body with
//! `header.error_code`. The parent context (parent_message_id + trace_id +
//! input_ttl + input_headers) already lives in the `OutputSink`.
//!
//! GH #81 added the second half: when the op arrived as a `tool_call` turn, both
//! the error and the success answer carry a `tool_result` turn with the inbound
//! id, which is what makes the timer usable as a tool lane without a bridge
//! cell in front of it. Without an inbound id nothing changes — the legacy
//! raw-body path keeps its exact shape, errors included.

use meclaw_core::{CellOutput, Message, OutputSink, Uuid};
use serde_json::json;

/// One `tool_result` turn carrying `id`. The turn type every tool cell answers
/// with (`crate::tool::build_tool_result_body`), spelled here so the timer's
/// own `meta.detail`/`msg_type` slots survive next to it.
fn tool_result_turn(id: &str, text: String) -> serde_json::Value {
    json!({
        "origin": "tool",
        "type":   "tool_result",
        "id":     id,
        "text":   text,
    })
}

/// Emit an op error reply via `OutputSink`. Target = `msg.reply_to`, fallback
/// `msg.target` (the cell's own path). `error_code` lands as
/// `content.header.error_code`, `detail` as `content.meta.detail`.
///
/// GH #81: with `tool_call_id` present the reply additionally carries the
/// `tool_result` turn a tool loop waits for, plus `finish_reason: "error"` the
/// way the other tool cells mark a failed call. With `None` the body is
/// byte-identical to the pre-#81 one (`messages: []`).
pub async fn emit_op_error(
    sink: &OutputSink,
    msg: &Message,
    error_code: &str,
    detail: &str,
    tool_call_id: Option<&str>,
) {
    emit_op_error_for(sink, msg, error_code, detail, tool_call_id, None).await;
}

/// [`emit_op_error`] for an op that named a `schedule_id` (GH #904, PP-7):
/// the id rides back as `content.header.schedule_id`, exactly as the op
/// carried it, so a caller can match a refused order to the order it sent --
/// the curator's clock got a bare `at_in_past` before. With `None` the body is
/// byte-identical to [`emit_op_error`]'s.
pub async fn emit_op_error_for(
    sink: &OutputSink,
    msg: &Message,
    error_code: &str,
    detail: &str,
    tool_call_id: Option<&str>,
    schedule_id: Option<&str>,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let mut header = json!({
        "error_code": error_code,
        "msg_type":   "timer_op_error",
    });
    if let Some(id) = schedule_id {
        header["schedule_id"] = json!(id);
    }
    let turns = match tool_call_id {
        Some(id) => {
            header["finish_reason"] = json!("error");
            json!([tool_result_turn(id, detail.to_string())])
        }
        None => json!([]),
    };
    let content = json!({
        "header":   header,
        "messages": turns,
        "meta":     { "detail": detail },
    });
    let _ = sink.push(CellOutput { target, content }).await;
}

/// GH #81: the answer to a successful `add`/`modify`/`remove`/`trigger` that
/// arrived as a `tool_call`.
///
/// Emitted ONLY when an inbound id was supplied — a successful op on the legacy
/// raw-body path stays unacked, as it always was. `text` is the JSON status an
/// agent reads back; the header carries the same facts for edge conditions.
pub async fn emit_op_ack(
    sink: &OutputSink,
    msg: &Message,
    op: &str,
    schedule_id: Uuid,
    tool_call_id: &str,
) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let text = json!({
        "op":          op,
        "schedule_id": schedule_id.to_string(),
        "status":      "ok",
    })
    .to_string();
    let content = json!({
        "header": {
            "msg_type":    "timer_op_ack",
            "op":          op,
            "schedule_id": schedule_id.to_string(),
        },
        "messages": [tool_result_turn(tool_call_id, text)],
    });
    let _ = sink.push(CellOutput { target, content }).await;
}
