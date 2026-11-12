//! D58 note (GH #1477): the shared steps of the pins of a note that stands
//! once and answers its sender (`plans/d58-2026-11-10/plan-parts/u-note.md`).
//!
//! A note of scope `once` stands in the calls of exactly one turn, or in the
//! start window of exactly one running session with its renewals, and then
//! nowhere. A note that carries `receipt` (a value of its sender the curator
//! does not read) and was shown answers on the route `note_receipt`; every
//! field of that answer stands in the head, because edges read only the head.
//! Behind the knob `note_once` on `curator/intake`, `curator/policy` and
//! `curator/history`, shipped "0".
//!
//! The hive runs in one process (`support/curator_hive.rs`), the shipped
//! scripts under python3; the speculation steps are the ones of
//! `curator_2/gh1382_a_failed_speculation_answers_once.rs`.
#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The knob of the contract (point 8), on the three cells that read it.
pub const KNOB: &str = "note_once";
pub const KNOB_CELLS: [&str; 3] = ["intake", "policy", "history"];

/// The route of the answer to the sender (contract point 5).
pub const RECEIPT_ROUTE: &str = "note_receipt";

/// The head fields of a receipt (contract point 5, OR-d58-2.note.1): the
/// note's source, id and receipt, the session, the turn, the target, the time.
pub const RECEIPT_KEYS: [&str; 7] = [
    "note_source",
    "note_id",
    "receipt",
    "session_id",
    "turn_id",
    "target",
    "at",
];

/// The share of the window a notes block may take in these pins: the value
/// the example set `talky` carries (`support/curator_profiles.rs`).
pub const NOTES_SHARE: f64 = 0.02;

/// The window profile that gives notes their share -- what a target reports
/// on `in_catalog` (D58 target, contract point 13: a profile, no role).
pub fn notes_profile() -> Value {
    json!({"notes_share": NOTES_SHARE})
}

/// A curator whose main target reported [`notes_profile`] (the form the
/// tests of D58 target use, `Hive::with(..).reporting(profile)`), with
/// `over` params on it -- the hive of a pin that compares against the
/// curator of before (`note_once` as shipped).
pub fn share_hive(over: &[(&str, &str, Value)]) -> Hive {
    Hive::with(over).reporting(notes_profile())
}

/// The params `note_once` "1" on the three cells, `extra` over them.
fn knob_on<'a>(extra: &[(&'a str, &'a str, Value)]) -> Vec<(&'a str, &'a str, Value)> {
    let mut over: Vec<(&str, &str, Value)> = Vec::new();
    for cell in KNOB_CELLS {
        over.push((cell, KNOB, json!("1")));
    }
    over.extend(extra.iter().cloned());
    over
}

/// A curator whose main target gives notes a share of the window
/// ([`share_hive`]) with `note_once` "1" on the three cells, and `extra`
/// params over it.
pub fn once_hive(extra: &[(&str, &str, Value)]) -> Hive {
    let mut h = share_hive(&knob_on(extra));
    h.check_emits = true;
    h
}

/// The same with the speculation of the quick voice on, in talky's
/// composite as `gh1382` runs it (every model cell reports its shipped
/// profile); the quick targets ship no share of notes, so their profiles get
/// [`notes_profile`] -- the speculation's window and the quick fallback held
/// behind it must be able to show the note (contract point 3).
pub fn once_spec_hive() -> Hive {
    let over = knob_on(&[
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!("")),
    ]);
    let share: Vec<(&str, Value)> = ["quick", "quick_spec", "quick_bridge"]
        .into_iter()
        .map(|cell| (cell, notes_profile()))
        .collect();
    let mut h = Hive::with(&over).reporting_composite_with("talky", &share);
    h.check_emits = true;
    h
}

/// A note of scope `once` from the source `probe`, with a receipt or none.
pub fn once(id: &str, text: &str, receipt: Option<&str>) -> Value {
    let mut n = json!({"id": id, "source": "probe", "text": text, "scope": "once"});
    if let Some(r) = receipt {
        n["receipt"] = json!(r);
    }
    n
}

/// A note of any other scope from `probe`.
pub fn note(id: &str, scope: &str, text: &str) -> Value {
    json!({"id": id, "source": "probe", "text": text, "scope": scope})
}

/// The notes on `in_note`, in the standard round, for session `s1`.
pub fn post(h: &mut Hive, notes: Value) {
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": notes}),
    );
}

/// `used_turn` of the note `id` of `probe`; panics when there is no row.
pub fn used(h: &Hive, id: &str) -> String {
    let rows = h.rows(&format!(
        "SELECT used_turn FROM notes WHERE source = 'probe' AND note_id = '{id}'"
    ));
    assert_eq!(
        rows.len(),
        1,
        "one row of note {id}: {rows:?} {:?}",
        h.stderr
    );
    rows[0][0].as_str().unwrap_or("").to_string()
}

/// `(note_id, scope, used_turn, text)` of every `notes` row, by id.
pub fn note_rows(h: &Hive) -> Vec<Vec<Value>> {
    h.rows("SELECT note_id, scope, used_turn, text FROM notes ORDER BY source, note_id")
}

/// The receipts that left the hive since `out` was last cleared.
pub fn receipts(h: &Hive) -> Vec<Msg> {
    h.routed(RECEIPT_ROUTE)
}

/// The receipts of the note `id`.
pub fn receipts_of(h: &Hive, id: &str) -> Vec<Msg> {
    receipts(h)
        .into_iter()
        .filter(|m| m.hop.get("note_id") == Some(&json!(id)))
        .collect()
}

/// A head field of a message as text, "" when absent.
pub fn field(m: &Msg, k: &str) -> String {
    m.hop
        .get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Whether a call's messages carry `text` anywhere.
pub fn shows(call: &Msg, text: &str) -> bool {
    texts(call).iter().any(|t| t.contains(text))
}

/// The `notes` marks the windows left, parsed.
pub fn note_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'notes' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("a notes mark is JSON"))
        .collect()
}

/// The normal curate of a turn at its route stamp, in session `session`.
pub fn seam_in(h: &mut Hive, session: &str, turn: &str, path: &str, needs: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": ""});
    let ctx = json!({"session_id": session, "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
}

/// The same in session `s1`.
pub fn seam(h: &mut Hive, turn: &str, path: &str, text: &str) {
    seam_in(h, "s1", turn, path, "", text);
}

/// The collector's early curate of a turn (`hop.spec` "1", no path);
/// returns the spec call that left for the quick voice.
pub fn spec_call(h: &mut Hive, turn: &str, text: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].hop["spec"], json!("1"));
    quick[0].clone()
}

/// The spec call's end as talky's `./quick -> ./curator in_spec_llm` edge
/// hands it in (`context.spec` "1").
pub fn spec_end(h: &mut Hive, call: &Msg, finish: &str, extra: Value, messages: Value) {
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    let mut hop = obj(json!({"finish_reason": finish, "model": "test-model",
                             "tokens_prompt": 10}));
    for (k, v) in obj(extra) {
        hop.insert(k, v);
    }
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        Value::Object(hop),
        json!({"messages": messages}),
    );
}

/// The `seed` of the running session `call` (its handle `live`, D58 live
/// GH #1482) for `reason` (`start`, `renew`), through the hive's own edge to
/// `./history`; the receipts it sent stay in `out`.
pub fn seed(h: &mut Hive, call: &str, reason: &str, renewal_n: u64) -> Msg {
    h.out.clear();
    h.lane(
        "in_seed_ask",
        json!({"live": call}),
        json!({"live": call, "session_id": call, "reason": reason,
               "renewal_n": renewal_n}),
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    seeds[0].clone()
}

/// The texts of a seed, one per message.
pub fn seed_texts(s: &Msg) -> Vec<String> {
    s.messages()
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect()
}
