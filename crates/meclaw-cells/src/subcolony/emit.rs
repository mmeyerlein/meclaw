//! The three emission shapes of the `subcolony` cell type.
//!
//! Two lanes, for the same structural reason as `harness`. A `reply` and an
//! `error` answer a message and therefore travel on the `OutputSink`, inside the
//! requester's trace. An `unsolicited` frame happens while no message is being
//! handled — a child topology emitted it on its own — so it can only leave
//! through the `OriginSink`, which by contract starts a fresh trace.
//!
//! The child's body crosses VERBATIM, with `subcolony_event` added to its header
//! slot — and the substrate lifts that header slot into the emitted message's
//! `hop` compartment, which is how a parent edge conditions on the answer
//! (`hop.subcolony_event == "reply"` / `"error"`).
//!
//! What a parent does NOT see is the child's own hop compartment. A `hop` is
//! spent in one hop by definition, so whatever the child's cells set there was
//! already consumed when the message reached the child's root and egressed. Only
//! the body survives the process boundary. That is the boundary being opaque in
//! both directions, and it means a child that wants to signal something upward
//! has to say it in its BODY, not in a header.

use meclaw_core::serde_json::{Map, Value as JsonValue, json};
use meclaw_core::{CellOutput, Message, OriginSink, OutputSink, Path};

/// Pass the child's answer on, in the requester's trace.
pub async fn reply(sink: &OutputSink, msg: &Message, child_body: JsonValue) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let _ = sink
        .push(CellOutput {
            target,
            content: tagged(child_body, "reply", None),
        })
        .await;
}

/// Report a failure to answer, in the requester's trace.
pub async fn error(sink: &OutputSink, msg: &Message, error_code: &str, detail: &str) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let body = json!({
        "header": {"subcolony_event": "error", "error_code": error_code},
        "messages": [{"origin": "assistant", "type": "text", "text": detail}]
    });
    let _ = sink
        .push(CellOutput {
            target,
            content: body,
        })
        .await;
}

/// Forward a frame nobody asked for onto the configured lane.
///
/// Without a configured lane the caller drops it with a warning; that decision
/// belongs to the cell, not here.
pub async fn unsolicited(sink: &OriginSink, target: Path, child_body: JsonValue) {
    let _ = sink
        .emit(CellOutput {
            target,
            content: tagged(child_body, "unsolicited", None),
        })
        .await;
}

/// Add `subcolony_event` (and optionally `error_code`) to a child body's header
/// slot without disturbing anything else the child put there.
fn tagged(mut body: JsonValue, event: &str, error_code: Option<&str>) -> JsonValue {
    let header = match body.get_mut("header").and_then(JsonValue::as_object_mut) {
        Some(h) => h,
        None => {
            let mut fresh = Map::new();
            fresh.insert("subcolony_event".into(), json!(event));
            if let Some(code) = error_code {
                fresh.insert("error_code".into(), json!(code));
            }
            if let Some(obj) = body.as_object_mut() {
                obj.insert("header".into(), JsonValue::Object(fresh));
            }
            return body;
        }
    };
    header.insert("subcolony_event".into(), json!(event));
    if let Some(code) = error_code {
        header.insert("error_code".into(), json!(code));
    }
    body
}
