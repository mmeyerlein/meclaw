//! The emissions of a `webhook` proxy.
//!
//! The arrival carries its lane in the `header` slot, which the substrate
//! lifts into `hop` (`hop.route`). The receipts take the `meclaw` platform's
//! form unchanged (`crate::proxy::meclaw::emit`): `route: "receipt"`, the key
//! `peer_event`, the lane, and the mount name in the place of the boundary —
//! one receipt form for every mount of a `proxy`, so one receipt edge reads
//! them all.

use serde_json::{Map, Value};

use crate::proxy::meclaw::emit::{crossed_emission, refused_emission};
use crate::proxy::meclaw::lanes::Refusal;

/// The body slot an arrival carries besides `messages: []`.
pub const WEBHOOK_SLOT: &str = "webhook";

/// One arrival: the UBF body the mount built, with the lane as `hop.route`.
/// A `header` slot in `body` (the mount never builds one) is replaced.
pub fn arrived_emission(route: &str, body: Value) -> Value {
    let mut out = match body {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let mut header = Map::new();
    header.insert("route".into(), Value::String(route.to_string()));
    out.insert("header".into(), Value::Object(header));
    Value::Object(out)
}

/// The `crossed` receipt of one arrival on `route` at `mount`.
pub fn crossed_receipt(route: &str, mount: &str) -> Value {
    crossed_emission(route, mount, &[WEBHOOK_SLOT.to_string()])
}

/// A `refused` receipt: the code in the header, the detail as text.
pub fn refused_receipt(route: &str, mount: &str, code: &'static str, detail: &str) -> Value {
    refused_emission(route, mount, &Refusal::new(code, detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_arrival_carries_the_lane_and_stays_a_ubf_body() {
        let body = json!({"messages": [], "webhook": {"raw": "x"},
            "header": {"route": "forged"}});
        let out = arrived_emission("probe", body);
        assert_eq!(out["header"], json!({"route": "probe"}));
        assert_eq!(out["webhook"], json!({"raw": "x"}));
        meclaw_core::validate_ubf_body(&out).expect("a UBF body");
    }

    #[test]
    fn the_receipts_have_the_meclaw_form() {
        let c = crossed_receipt("probe", "probe-hook");
        assert_eq!(
            c["header"],
            json!({"route": "receipt", "peer_event": "crossed", "lane": "probe",
                   "boundary": "probe-hook", "fields": ["webhook"]})
        );
        let r = refused_receipt(
            "probe",
            "probe-hook",
            "webhook_unverified",
            "the proof does not match",
        );
        assert_eq!(r["header"]["peer_event"], json!("refused"));
        assert_eq!(r["header"]["error_code"], json!("webhook_unverified"));
        assert_eq!(r["messages"][0]["text"], json!("the proof does not match"));
        for v in [c, r] {
            meclaw_core::validate_ubf_body(&v).expect("a UBF body");
        }
    }
}
