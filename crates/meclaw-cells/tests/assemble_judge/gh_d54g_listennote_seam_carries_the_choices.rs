//! D54g listennote (GH #1538) -- the route stamp's choices ride on the seam
//! to the curator.
//!
//! WHY this file exists: the lead's gate `listen_sections` reads the enum
//! values the route model judged (`choices`, GH #1304: `depth`, `risk`, ...).
//! The collector is the stamp's only way to the curator, and it parked the
//! choices with the turn's route leg and handed none of them on: the lead
//! never saw one. Now the seam carries them as `hop.choices`, the JSON text
//! of the object (hop values are text, like `needs_yes` and `trail`), and
//! only when the stamp carries them: a stamp without choices leaves the seam
//! of before, byte for byte.
//!
//! The collector's shipped script and params; no model.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{assemble, bundle_reply};
use serde_json::{Value, json};

const TURN: &str = "c-1";
const FRESH: &str = "2999-01-01T00:00:00.000000Z";
const TEXT: &str = "My knee has hurt since yesterday.";

fn waiting() -> Vec<(&'static str, Value)> {
    vec![("memory_tier", json!("1")), ("route_wait_ms", json!(150))]
}

fn leg(role: &str, payload: Value) -> Value {
    json!({"turn_id": TURN, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

/// The seam of the turn whose route leg holds `stamp`.
fn seam_of(stamp: Value) -> Value {
    let rows = json!([
        leg(
            "leg-window",
            json!({"turns": [{"role": "user", "text": TEXT, "consult_id": ""}],
                   "deferred": 0, "deferred_turns": []})
        ),
        leg("leg-memory", json!({"system": {}, "messages": []})),
        leg("leg-route", stamp)
    ]);
    let out = assemble(
        &waiting(),
        bundle_reply("collect", TURN, &[("c-collect-read", rows)]),
    );
    out.iter()
        .find(|m| m["header"]["route"].as_str() == Some("curate"))
        .cloned()
        .unwrap_or_else(|| panic!("every leg stands: {out:?}"))
}

fn plain() -> Value {
    json!({"path": "talk", "p": 0.9, "intent": "chat", "needs": [], "families": [],
           "duration": 0})
}

#[test]
fn gh_d54g_listennote_seam_carries_the_choices_only_when_the_stamp_does() {
    let chosen = json!({"depth": {"value": "important", "p": 0.9, "source": "model"},
                        "risk": {"value": "strained", "p": 0.8, "source": "model"}});
    let mut stamp = plain();
    stamp["choices"] = chosen.clone();
    let with = seam_of(stamp);
    let rode: Value = serde_json::from_str(
        with["header"]["choices"]
            .as_str()
            .unwrap_or_else(|| panic!("the choices ride as text: {with}")),
    )
    .expect("the choices are JSON text");
    assert_eq!(rode, chosen, "{with}");

    // Without choices: no key, and the seam is the one of before -- the
    // choices add their own key and nothing else.
    let without = seam_of(plain());
    assert!(
        without["header"].get("choices").is_none(),
        "a stamp without choices adds no key: {without}"
    );
    let mut stripped = with.clone();
    stripped["header"]
        .as_object_mut()
        .expect("a header")
        .remove("choices");
    assert_eq!(
        serde_json::to_string(&stripped).expect("json"),
        serde_json::to_string(&without).expect("json"),
        "the seam of a stamp with choices is the seam without them plus the one key"
    );
    // An empty object is no choice: the seam of before.
    let mut empty = plain();
    empty["choices"] = json!({});
    assert_eq!(
        serde_json::to_string(&seam_of(empty)).expect("json"),
        serde_json::to_string(&without).expect("json")
    );
}
