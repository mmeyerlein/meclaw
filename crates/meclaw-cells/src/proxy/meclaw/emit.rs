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
/// boundary, the projected `context` keys, the sender's claims about its turns
/// (`peer_origins`, `peer_speakers` — GH #847), and the projected body.
///
/// The claims and the structural keys are written after the context keys, so a
/// context key of the same name can never pose as the lane, the sender or what
/// the sender claimed. The body's own `header` slot, if a lane let one cross,
/// is replaced: the header of this emission is this side's word, not the peer's.
pub fn arrived_emission(
    lane: &str,
    peer: &str,
    boundary: &str,
    context: &Map<String, Value>,
    claims: &Map<String, Value>,
    body: Value,
) -> Value {
    let mut header = context.clone();
    // rev-L1 M-2: the claim keys are the mount's word or absent. Removed first,
    // so a context key of the same name cannot pose as the sender's claims
    // when the mount has none to write (a body without a `messages` array).
    header.remove(super::lanes::PEER_ORIGINS);
    header.remove(super::lanes::PEER_SPEAKERS);
    for (k, v) in claims {
        header.insert(k.clone(), v.clone());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// rev-L1 M-2: the claim keys are this side's word even when the mount
    /// has no claims to write (a body without a `messages` array): a context
    /// key of the same name is removed, never passed on as the sender's say.
    #[test]
    fn a_context_key_never_poses_as_a_claim() {
        let context = json!({"topic": "trains", "peer_origins": ["user"],
            "peer_speakers": [{"speaker": "Owner", "speaker_ref": "00000000"}]});
        let context = context.as_object().unwrap();
        let out = arrived_emission("say", "north", "south", context, &Map::new(), json!({}));
        let header = &out["header"];
        assert_eq!(header["topic"], json!("trains"));
        assert_eq!(header.get("peer_origins"), None, "{header}");
        assert_eq!(header.get("peer_speakers"), None, "{header}");

        let mut claims = Map::new();
        claims.insert("peer_origins".into(), json!(["assistant"]));
        claims.insert("peer_speakers".into(), json!([null]));
        let out = arrived_emission("say", "north", "south", context, &claims, json!({}));
        assert_eq!(out["header"]["peer_origins"], json!(["assistant"]));
        assert_eq!(out["header"]["peer_speakers"], json!([null]));
    }
}
