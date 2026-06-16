//! Phase-10-B: Emit-Helper fuer Op-Fehler-Replies aus `handle`. Target =
//! `msg.reply_to`; bei fehlendem `reply_to` faellt die Emission auf das eigene
//! `msg.target` zurueck (W2d: NICHT mehr der `/colony/dead_letters`-READ-Endpoint —
//! ein matchloser Out-Edge endet als `no_route` in der DLQ). UBF-Body mit
//! `header.error_code`. Parent-Kontext (parent_message_id + trace_id + input_ttl
//! + input_headers) liegt bereits im `OutputSink`.

use meclaw_core::{CellOutput, Message, OutputSink};
use serde_json::json;

/// Emit ein Op-Error-Reply via `OutputSink`. Target = `msg.reply_to`, Fallback
/// `msg.target` (eigener Pfad). `error_code` landet als
/// `content.header.error_code`, `detail` als `content.meta.detail`.
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
