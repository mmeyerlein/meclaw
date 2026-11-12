//! D58 live (GH #1482): the handle of a running session, `live`, as the pins
//! of the strand drive it -- written once here for the nine pins instead of
//! once per file.
//!
//! The curator holds a session of its own (start window, renewal, live feed,
//! a fact from a tool, a parked delegation, the topic's age) by ONE neutral
//! key: `live`. It carries the value of the voice's `call_id` exactly when
//! the voice runs `duplex`, and it is translated at the edges of the
//! compositions, in both directions; no script of the curator or of the lead
//! reads `call_id` or `engine` (plan `u-live.md`, contract points 1 to 8).
//!
//! Two kinds of helper: the hive in one process (`support/curator_hive.rs`)
//! driven on a turn or a seed ask under given handle keys, and the shipped
//! edges of a composition evaluated by the colony's own CEL -- the same
//! `parse_condition` / `apply_modifier` the hive harness routes with, on the
//! manifest as it ships, never a copy of it.
#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

/// The value of the handle a running session carries (the voice's call id).
pub const LIVE: &str = "call-live-7";

/// The session the voice's own messages are stamped with -- never the handle,
/// so a pin can tell a key built from the handle from one built from the
/// session.
pub const VOICE_SESSION: &str = "voice-session-3";

/// The hive the pins run: the live feed on, `over` on top, the profiles of
/// the composite talky reported (D58 target, contract point 13: the former
/// role talky).
pub fn live_hive(over: &[(&str, &str, Value)]) -> Hive {
    let mut all: Vec<(&str, &str, Value)> = vec![("policy", "live_feed", json!("1"))];
    all.extend(over.iter().cloned());
    Hive::with(&all).reporting_composite("talky")
}

/// The handle keys of the new form: `live` in hop and context.
pub fn live_keys(live: &str) -> Value {
    json!({"live": live})
}

/// The keys of before: the voice's `engine` and `call_id`, no handle.
pub fn call_keys(call: &str) -> Value {
    json!({"engine": "duplex", "call_id": call})
}

/// One `in_curate` of `turn` in `session`, `keys` set in hop AND context
/// (the context carries a handle over several cells, contract point 1).
pub fn seam(h: &mut Hive, session: &str, turn: &str, path: &str, keys: &Value, extra: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                         "path": path, "needs": "", "families": ""});
    let mut ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in obj(keys.clone()) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    for (k, v) in obj(extra) {
        hop[k.as_str()] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("and then?")]}),
    );
}

/// A talk turn of `session` under `keys`, answered and closed (`stop`).
pub fn closed_turn(h: &mut Hive, session: &str, turn: &str, keys: &Value) {
    seam(h, session, turn, "talk", keys, json!({}));
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("fine")]));
}

/// The voice's ask for a start window (`in_seed_ask`), through the hive's
/// own edges: `keys` in hop and context, the voice's session, `reason`.
/// Returns every `seed` that left the hive.
pub fn ask_seed(h: &mut Hive, keys: &Value, reason: &str) -> Vec<Msg> {
    h.out.clear();
    let mut hop = json!({"session_id": VOICE_SESSION, "reason": reason, "renewal_n": 0});
    let mut ctx = json!({"audience_set": ROUND_E});
    for (k, v) in obj(keys.clone()) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    h.lane("in_seed_ask", ctx, hop, json!({"messages": []}));
    h.routed("seed")
}

/// The text of a seed's first line (the facts).
pub fn first_line(seed: &Msg) -> String {
    seed.messages()
        .first()
        .and_then(|m| m["text"].as_str())
        .unwrap_or("")
        .to_string()
}

/// A `topic` mark's value as `./intake` writes it.
pub fn topic_value(name: &str, movement: &str) -> String {
    sj::to_string(&json!({"movement": movement, "name": name})).unwrap()
}

/// The keys of the ledger's `state` rows that start with `prefix`.
pub fn state_keys(h: &Hive, prefix: &str) -> Vec<String> {
    h.rows("SELECT key FROM state ORDER BY key")
        .iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .filter(|k| k.starts_with(prefix))
        .collect()
}

/// A header key's value as a non-empty string; None when absent or empty.
pub fn named(m: &Map<String, Value>, key: &str) -> Option<String> {
    m.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// One message after one edge of a composition.
#[derive(Debug, Clone)]
pub struct Passed {
    pub edge: usize,
    pub to: String,
    pub context: Map<String, Value>,
    pub hop: Map<String, Value>,
}

/// The edges of the shipped template `template` (`templates/<t>/config.json`).
pub fn edges_of(template: &str) -> Vec<Value> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("templates/{template}/config.json has no params.graph.edges"))
}

/// A message with `context` and `hop` leaving `from` inside the shipped
/// template `template`: every edge whose condition holds, its modifier
/// applied -- the colony's own CEL. A condition that does not compile or a
/// modifier that does not apply fails the pin where it happens.
pub fn through(template: &str, from: &str, context: Value, hop: Value) -> Vec<Passed> {
    let ctx = obj(context);
    let hp = obj(hop);
    let mut out = Vec::new();
    for (i, e) in edges_of(template).iter().enumerate() {
        if e["from"].as_str() != Some(from) {
            continue;
        }
        if let Some(c) = e["condition"].as_str() {
            let cond = parse_condition(c)
                .unwrap_or_else(|x| panic!("templates/{template} edge {i}: {c}: {x}"));
            if !matches!(evaluate_condition(&cond, &ctx, &hp), Ok(true)) {
                continue;
            }
        }
        let (mut c2, mut h2) = (ctx.clone(), hp.clone());
        if e["modifier"].is_object() {
            let spec: ModifierSpec = sj::from_value(e["modifier"].clone())
                .unwrap_or_else(|x| panic!("templates/{template} edge {i}: {x}"));
            let m = parse_modifier(&spec)
                .unwrap_or_else(|x| panic!("templates/{template} edge {i}: {x:?}"));
            let h = apply_modifier(&m, &Headers::from_parts(ctx.clone(), hp.clone()))
                .unwrap_or_else(|x| panic!("templates/{template} edge {i}: {x}"));
            c2 = h.context;
            h2 = h.hop;
        }
        out.push(Passed {
            edge: i,
            to: e["to"].as_str().unwrap_or("").to_string(),
            context: c2,
            hop: h2,
        });
    }
    out
}

/// The messages of [`through`] that reach `to`.
pub fn reaching(template: &str, from: &str, to: &str, context: Value, hop: Value) -> Vec<Passed> {
    through(template, from, context, hop)
        .into_iter()
        .filter(|p| p.to == to)
        .collect()
}
