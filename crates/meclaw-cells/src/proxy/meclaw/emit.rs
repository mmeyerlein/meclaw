//! The three emissions of a `meclaw` proxy.
//!
//! Everything structural goes into the `header` slot, which the substrate lifts
//! into `hop` (A8: the key is `peer_event`). A receipt does not choose its own
//! address: a cell emission is routed over its sender's out-edges, so the
//! receipts carry `route: "receipt"` and the operator draws that edge.

use serde_json::{Map, Value, json};

use super::lanes::Refusal;

/// The `hop.route` every receipt carries (ADR-0025: a receipt lifts no turn).
pub const RECEIPT_ROUTE: &str = "receipt";

/// A crossing happened: which lane, at which boundary, carrying which fields.
/// No turn: `messages` is empty.
pub fn crossed_emission(lane: &str, boundary: &str, fields: &[String]) -> Value {
    json!({
        "header": {
            "route": RECEIPT_ROUTE,
            "peer_event": "crossed",
            "lane": lane,
            "boundary": boundary,
            "fields": fields,
        },
        "messages": [],
    })
}

/// A crossing was refused: the code in the header, the detail as one
/// `assistant` text an operator can read.
pub fn refused_emission(lane: &str, boundary: &str, r: &Refusal) -> Value {
    json!({
        "header": {
            "route": RECEIPT_ROUTE,
            "peer_event": "refused",
            "lane": lane,
            "boundary": boundary,
            "error_code": r.error_code,
        },
        "messages": [
            { "origin": "assistant", "type": "text", "text": r.detail }
        ],
    })
}

/// A frame arrived: the lane as `route`, the sender as `peer`, this side's
/// boundary, the projected `context` keys, and the projected body.
///
/// The structural keys are written after the context keys, so a context key of
/// the same name can never pose as the lane or the sender. The body's own
/// `header` slot, if a lane let one cross, is replaced: the header of this
/// emission is this side's word, not the peer's.
pub fn arrived_emission(
    lane: &str,
    peer: &str,
    boundary: &str,
    context: &Map<String, Value>,
    body: Value,
) -> Value {
    let mut header = context.clone();
    header.insert("route".to_string(), Value::String(lane.to_string()));
    header.insert("peer".to_string(), Value::String(peer.to_string()));
    header.insert("boundary".to_string(), Value::String(boundary.to_string()));
    let mut out = match body {
        Value::Object(m) => m,
        // The mount projects objects only; anything else carries no fields.
        _ => Map::new(),
    };
    out.insert("header".to_string(), Value::Object(header));
    Value::Object(out)
}
