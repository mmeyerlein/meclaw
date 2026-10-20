//! GH #1260 (OR-WG-6) -- the seam carries the route stamp's `needs`.
//!
//! WHY this file exists: the curator requires the tool call of a `simple`
//! turn whose stamp needs a `tool`, but the seam carried only the stamp's
//! path and families: in a lab colony the stamp said `needs: ["tool"]` (row
//! 9465 of a lab run) and the curator's call on `./quick` held no
//! `needs` at all (row 9497). `hop.needs` rides beside `hop.families`, a comma
//! list in stamp order, "" without a stamp.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "route-channel#0001";
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

fn seam_of(stamp: Value) -> Value {
    let window = leg(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "how is the weather in Potsdam",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
    );
    let out = assemble(
        &[("memory_tier", json!("1")), ("route_wait_ms", json!(150))],
        bundle_reply(
            "collect",
            TURN,
            &[("c-collect-read", json!([window, leg("leg-route", stamp)]))],
        ),
    );
    out.iter()
        .find(|m| m["header"]["route"] == "curate")
        .cloned()
        .unwrap_or_else(|| panic!("no seam: {out:?}"))
}

#[test]
fn the_seam_carries_the_stamps_needs() {
    let s = seam_of(json!({"path": "simple", "needs": ["tool", "reasoning"],
                           "families": ["web"]}));
    assert_eq!(hop_str(&s, "needs"), "tool,reasoning");
    assert_eq!(hop_str(&s, "families"), "web");
    let s = seam_of(json!({"path": "simple"}));
    assert_eq!(
        s["header"]["needs"],
        json!(""),
        "present without needs, like every key of the head: {s}"
    );
}

/// Fix round 1 (review M-1): `hop.needs` is declared beside `hop.families`,
/// so the build-time locality check sees what the curator reads.
#[test]
fn the_collector_declares_the_needs() {
    let hop = &config_of(ASSEMBLE)["contract"]["emits"]["hop"];
    assert_eq!(hop["needs"]["type"], json!("string"), "{hop}");
    assert_eq!(hop["needs"]["required"], json!(false), "{hop}");
}
