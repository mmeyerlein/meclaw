//! GH #1399 (d53-2 V2): a delegation opens its round beside an open one.
//!
//! On the telephone a duplex voice cell hands the backend an errand
//! (`in_delegation`) while the round of an earlier turn of the same session is
//! still open: its brain call has a tool call out, so `c-open-round` finds an
//! `assistant` row that has not fired. With `defer_turns` "1" (shipped, GH
//! #103) the delegation was deferred like a spoken turn and rode the next round
//! the session opened -- the backend call started only after the round before
//! it had closed (measured on the telephone battery: fact after a delegation
//! p50 3 679 ms).
//!
//! `delegation_priority` "1" takes a delegation, and nothing else, into the
//! path `defer_turns` "0" already has (GH #894): the round opens beside the
//! open one, under its own key, and carries only its own rows -- a text turn
//! deferred behind the open round keeps its place. "0" (shipped) is the
//! behaviour of before, byte for byte.
//!
//! The assemble scenarios run the shipped script once per hop as a child
//! process, so time is the rows' `recorded_at`, not a paused tokio clock
//! (development-rules § 11: no paused clock across child processes). The open
//! round is stamped inside its idle window, as a round two seconds old is.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

/// The open turn the voice cell names on the delegation's hop.
const OPEN_TURN: &str = "s1#3";
/// The round of that turn, still open: its tool call is out.
const OPEN_ROUND: &str = "s1#3";
/// A text turn the person said while that round was open, deferred behind it.
const WAITING_TURN: &str = "s1#4";
/// Inside every idle and stale window (the round is "two seconds old").
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

/// The telephone's set: the delegation takes its turn's tool stamp (#1279).
fn phone(priority: Option<&str>) -> Vec<(&'static str, Value)> {
    let mut v = vec![
        ("route_wait_ms", json!(150)),
        ("delegation_inherits_stamp", json!("1")),
    ];
    if let Some(p) = priority {
        v.push(("delegation_priority", json!(p)));
    }
    v
}

fn delegation() -> Value {
    lane(
        "in_delegation",
        json!({"turn_id": OPEN_TURN}),
        json!({"delegation_id": "dlg-1", "engine": "duplex"}),
        json!([{"origin": "user", "type": "text", "text": "when is my appointment?"}]),
    )
}

fn tool_stamp() -> Value {
    json!({"path": "simple", "families": ["calendar"], "needs": ["tool"]})
}

fn open_round_rows() -> Value {
    json!([{"turn_id": OPEN_ROUND, "iter": 0, "recorded_at": FRESH, "session_id": SESSION}])
}

/// The turn-open reply of a round keyed `key` whose own row has `role`, with
/// the open round of the session and the text turn deferred behind it.
fn opened(key: &str, role: &str, text: &str, route: Option<Value>) -> Value {
    let mut legs = vec![
        ("c-open-round", open_round_rows()),
        (
            "c-open-win",
            json!([
                {"id": "r1", "session_id": SESSION, "turn_id": WAITING_TURN,
                 "role": "user", "content": "and the weather?",
                 "recorded_at": FRESH, "deferred": 1, "consult_id": ""},
                {"id": "r2", "session_id": SESSION, "turn_id": key,
                 "role": role, "content": text,
                 "recorded_at": FRESH, "deferred": 0,
                 "consult_id": if role == "delegation" { "dlg-1" } else { "" }}
            ]),
        ),
        ("c-open-scope", json!([])),
        ("c-open-roster", json!([])),
    ];
    if let Some(found) = route {
        legs.push(("c-open-route", found));
    }
    bundle_reply("turn-open", key, &legs)
}

fn deferred(out: &[Value]) -> bool {
    out.iter()
        .any(|m| m["header"]["phase"].as_str() == Some("defer-w"))
}

fn collect_of(out: &[Value]) -> Option<&Value> {
    out.iter().find(|m| {
        m["header"]["route"].as_str() == Some("cstore")
            && m["header"]["phase"].as_str() == Some("collect")
    })
}

/// The window leg the collect bundle parks, as the round will read it.
fn window_of(collect: &Value) -> Value {
    let row = call(collect, "c-collect-row")["row"].clone();
    assert_eq!(row["role"], "leg-window", "{row}");
    serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json")
}

fn leg(key: &str, role: &str, payload: Value) -> Value {
    json!({"turn_id": key, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

/// Run a delegation through its turn-open: (round key, reply out, stderr).
fn open_delegation(over: &[(&str, Value)]) -> (String, Vec<Value>, String) {
    let out = assemble(over, delegation());
    let key = hop_str(in_phase(&out, "turn-open"), "turn_id");
    let (out, err) = run_cell(
        ASSEMBLE,
        over,
        opened(
            &key,
            "delegation",
            "when is my appointment?",
            Some(json!([{"turn": tool_stamp().to_string()}])),
        ),
    );
    (key, out, err)
}

/// The store drop of a round's rows (`drop_round`), wherever it rides.
fn drops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("cstore"))
        .flat_map(calls_of)
        .filter(|(id, _)| id == "c-drop-turns" || id == "c-drop-round")
        .map(|(_, a)| a)
        .collect()
}

#[test]
fn a_delegation_with_a_tool_stamp_opens_its_round_beside_an_open_one() {
    let on = phone(Some("1"));
    // Hop 1: the arrival writes its row and asks the turn-open bundle.
    let (key, out, err) = open_delegation(&on);
    assert!(
        key.starts_with("s1#3~"),
        "the round is keyed by its turn: {key}"
    );
    assert_ne!(key, OPEN_ROUND, "its own key, beside the open round");
    // Hop 2: the reply finds the round of s1#3 open -- and does not defer.
    assert!(!deferred(&out), "a delegation is never deferred: {out:?}");
    let collect = collect_of(&out).unwrap_or_else(|| panic!("the round opens now: {out:?}"));
    assert_eq!(hop_str(collect, "turn_id"), key);
    assert!(
        calls_of(collect)
            .iter()
            .any(|(id, _)| id == "c-collect-route"),
        "the tool stamp parks beside the window leg: {collect}"
    );
    assert!(
        err.contains("delegation_round_opened") && err.contains(&key) && err.contains(OPEN_ROUND),
        "the ledger line names the round and the one it opened beside: {err}"
    );
    // Hop 3: the fan-in completes and the curate leaves -- at t + three hops,
    // with the round of s1#3 still open; no answer of it was waited for.
    let rows = json!([
        leg(&key, "leg-window", window_of(collect)),
        leg(&key, "leg-route", tool_stamp())
    ]);
    let out = assemble(
        &on,
        bundle_reply("collect", &key, &[("c-collect-read", rows)]),
    );
    let seam = on_route(&out, "curate");
    assert_eq!(hop_str(seam, "turn_id"), key);
    assert_eq!(hop_str(seam, "path"), "simple");
    assert_eq!(hop_str(seam, "needs"), "tool");
}

#[test]
fn text_turns_still_wait_behind_an_open_round() {
    let on = phone(Some("1"));
    // A spoken turn that arrives while the round is open defers, as before.
    let (out, err) = run_cell(
        ASSEMBLE,
        &on,
        opened(WAITING_TURN, "user", "and the weather?", None),
    );
    assert!(
        deferred(&out),
        "a text turn waits for the next round: {out:?}"
    );
    assert!(collect_of(&out).is_none(), "it opens no round: {out:?}");
    assert!(!err.contains("delegation_round_opened"), "{err}");
    // The delegation round beside carries ONLY its own row: the waiting text
    // turn keeps its place and rides the next round, so the answers come in
    // the order they did.
    let (_, out, _) = open_delegation(&on);
    let collect = collect_of(&out).unwrap_or_else(|| panic!("{out:?}"));
    let win = window_of(collect);
    assert_eq!(
        win["turns"],
        json!([{"role": "delegation", "text": "when is my appointment?", "consult_id": "dlg-1"}]),
        "no deferred row rides the delegation round: {win}"
    );
    assert_eq!(win["deferred"], json!(0), "{win}");
    assert_eq!(win["deferred_turns"], json!([]), "{win}");
}

#[test]
fn two_rounds_of_one_session_close_independently() {
    let on = phone(Some("1"));
    let (key, out, _) = open_delegation(&on);
    assert!(
        collect_of(&out).is_some(),
        "the delegation round stands: {out:?}"
    );
    // The delegation's answer is final and drops its own round only.
    let gone = drops(&assemble(&on, answer_in(&key, "Tuesday at ten.")));
    assert!(!gone.is_empty(), "the answer ends its round");
    for op in &gone {
        assert_eq!(op["where"]["turn_id"], json!(key), "{op}");
    }
    // The round of s1#3 ends with its own answer, and only it.
    let gone = drops(&assemble(&on, answer_in(OPEN_ROUND, "Done.")));
    assert!(!gone.is_empty(), "the old answer ends its round");
    for op in &gone {
        assert_eq!(op["where"]["turn_id"], json!(OPEN_ROUND), "{op}");
    }
}

#[test]
fn default_zero_behaves_as_before() {
    // The shipped value is "0" and says so in all three places.
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["delegation_priority"], json!("0"));
    assert_eq!(
        cfg["contract"]["settings"]["delegation_priority"]["default"],
        json!("0")
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"delegation_priority\", \"0\")"),
        "the script's fallback"
    );
    // Golden: under the shipped value a delegation behind an open round is
    // deferred, as it was, and "0" spelled out is the same bytes.
    // One fixed round key, as hop 1 mints it, so the two runs compare.
    let key = "s1#3~99999999999999~abcd1234";
    let reply = |over: &[(&str, Value)]| {
        let doc = opened(
            key,
            "delegation",
            "when is my appointment?",
            Some(json!([{"turn": tool_stamp().to_string()}])),
        );
        let parsed = assemble(over, doc.clone());
        let (bytes, err) = run_cell_stdout(ASSEMBLE, over, doc);
        (parsed, bytes, err)
    };
    let (shipped, shipped_bytes, err) = reply(&phone(None));
    assert!(
        deferred(&shipped),
        "shipped: the delegation waits: {shipped:?}"
    );
    assert!(collect_of(&shipped).is_none(), "{shipped:?}");
    assert!(!err.contains("delegation_round_opened"), "{err}");
    let (_, zero_bytes, _) = reply(&phone(Some("0")));
    assert_eq!(zero_bytes, shipped_bytes, "\"0\" is the bytes of before");
}
