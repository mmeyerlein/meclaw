//! #1400 -- the voice knows when the answer comes (the horizon).
//!
//! WHY this file exists: on the phone the live model keeps the talk going
//! while a tool round or a delegation runs, and without a sense of time it
//! fills the wait blindly (plan d53 T2 § V3, R-D53-2). With `horizon_advise`
//! on, a duplex call's turn that gets a tool round or a delegation sends ONE
//! `live_feed` with `hop.section` `horizon` (the voice's
//! `horizon_text` says it): how many seconds the answer will take -- the
//! `horizon_quantile` (p75) of the rounds of the turn's family booked so far,
//! else `horizon_defaults_ms[family]`, else nothing. The body carries no
//! words: `seconds`, `way`, `family` -- the voice's text is the set's. A turn
//! the voice answers from its seed gets none; every closed round of a horizon
//! turn books its duration; `0` (the default) moves no byte.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const ASK: &str = "What is the weather in Berlin?";

fn hive(defaults: Value) -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "horizon_advise", json!("1")),
        ("policy", "horizon_defaults_ms", json!(defaults.to_string())),
    ]);
    h.check_emits = true;
    h
}

fn duplex() -> Value {
    json!({"engine": "duplex", "call_id": "call-9"})
}

fn seam(h: &mut Hive, turn: &str, iter: &str, hop_extra: Value, body: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter, "phase": "",
                         "path": "", "needs": "", "families": ""});
    for (k, v) in obj(hop_extra) {
        hop[k] = v;
    }
    let path = hop["path"].clone();
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, body);
}

/// The tool stamp of a duplex turn: `simple`, needs a tool, family `web`.
fn tool_stamp() -> Value {
    let mut hop = duplex();
    hop["path"] = json!("simple");
    hop["needs"] = json!("tool");
    hop["families"] = json!("web");
    hop
}

fn horizons(h: &Hive) -> Vec<Msg> {
    h.routed("live_feed")
        .into_iter()
        .filter(|m| m.hop.get("section") == Some(&json!("horizon")))
        .collect()
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

/// The required call of the tool round (not its bridge).
fn round_call(h: &Hive) -> Msg {
    let calls: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .filter(|q| {
            !q.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(calls.len(), 1, "one tool round: {:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

/// The tool round of turn `turn` to its end: the required call asks for the
/// tool, the result comes back, the quick voice answers.
fn finish_round(h: &mut Hive, turn: &str) {
    let call = round_call(h);
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "web_search")]),
    );
    let mut hop = tool_stamp();
    hop["needs"] = json!("");
    seam(
        h,
        turn,
        "1",
        hop,
        json!({"messages": [tool_call("c1", "web_search"), tool_result("c1", "12 degrees")]}),
    );
    let answer = h.routed("quick");
    assert_eq!(answer.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let answer = answer[0].clone();
    h.out.clear();
    h.tap(&answer, "stop", json!({}), json!([said("12 degrees.")]));
    assert!(
        horizons(h).is_empty(),
        "one horizon per turn, never one at its end: {:?}",
        h.out
    );
}

/// Pin 1: a duplex turn whose stamp needs a tool gets ONE horizon, its
/// seconds from the table (no round booked yet).
#[test]
fn a_live_turn_with_a_tool_stamp_gets_one_horizon_advise() {
    if !shipped() {
        return;
    }
    let mut h = hive(json!({"web": 4000}));
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp(),
        json!({"messages": [user(ASK)]}),
    );
    let feeds = horizons(&h);
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let f = &feeds[0];
    // o8.5: no `at_turn_end` -- that holds to the caller's NEXT turn, and this
    // one has just ended; the voice holds it while the model speaks.
    assert!(
        f.hop.get("at_turn_end").is_none(),
        "the voice decides when, not the turn: {:?}",
        f.hop
    );
    assert_eq!(f.hop["call_id"], json!("call-9"));
    assert_eq!(f.hop["turn_id"], json!("t1"));
    assert_eq!(f.body["seconds"], json!(4));
    assert_eq!(f.body["family"], json!("web"));
    assert_eq!(f.body["way"], json!("quick"));
    // The round itself still goes: the required call and its bridge.
    assert_eq!(h.routed("quick").len(), 2, "{:?}", h.out);
    assert_eq!(
        marks(&h, "horizon_sent"),
        vec![json!({"family": "web", "seconds": 4})]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    finish_round(&mut h, "t1");
    let hit = marks(&h, "horizon_hit");
    assert_eq!(hit.len(), 1, "{:?}", h.stderr);
    assert_eq!(hit[0]["planned"], json!(4));
    assert!(hit[0]["actual"].is_number(), "{hit:?}");
    // The round is booked under its family.
    let rounds: Value = sj::from_str(&h.state("horizon_rounds")).expect("booked");
    assert_eq!(rounds["web"].as_array().map(Vec::len), Some(1), "{rounds}");
    assert!(
        h.state("pending:horizon:s1#t1").is_empty(),
        "the turn row ends with its round"
    );
}

/// Pin 2: a turn the voice answers from its seed -- a talk turn, a simple
/// turn without a tool, a text turn -- gets no horizon.
#[test]
fn a_turn_answered_from_the_seed_gets_no_horizon() {
    if !shipped() {
        return;
    }
    let mut h = hive(json!({"web": 4000, "brain": 3000, "quick": 2000}));
    let mut talk = duplex();
    talk["path"] = json!("talk");
    seam(
        &mut h,
        "t1",
        "0",
        talk,
        json!({"messages": [user("and then?")]}),
    );
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(horizons(&h).is_empty(), "talk: {:?}", h.out);

    let mut simple = duplex();
    simple["path"] = json!("simple");
    simple["families"] = json!("web");
    seam(
        &mut h,
        "t2",
        "0",
        simple,
        json!({"messages": [user("thanks")]}),
    );
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(horizons(&h).is_empty(), "no tool round: {:?}", h.out);

    let mut text = tool_stamp();
    text.as_object_mut().unwrap().remove("engine");
    text.as_object_mut().unwrap().remove("call_id");
    seam(&mut h, "t3", "0", text, json!({"messages": [user(ASK)]}));
    assert!(horizons(&h).is_empty(), "a text turn: {:?}", h.out);
    assert!(marks(&h, "horizon_sent").is_empty());
}

/// Pin 3: with rounds of the family booked, the horizon is their p75 -- not
/// the table's value; a family neither booked nor in the table gets none.
#[test]
fn the_horizon_uses_the_p75_of_measured_rounds_when_present() {
    if !shipped() {
        return;
    }
    let mut h = hive(json!({"web": 9000}));
    h.db.execute(
        "INSERT INTO state (key, value) VALUES ('horizon_rounds', ?1)",
        [json!({"web": [1000, 5000, 2000, 4000, 3000]}).to_string()],
    )
    .unwrap();
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp(),
        json!({"messages": [user(ASK)]}),
    );
    let feeds = horizons(&h);
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(feeds[0].body["seconds"], json!(4), "p75 of 1..5 s, not 9 s");

    finish_round(&mut h, "t1");
    let rounds: Value = sj::from_str(&h.state("horizon_rounds")).expect("booked");
    assert_eq!(rounds["web"].as_array().map(Vec::len), Some(6), "{rounds}");

    // An unknown family: no table row, no booked round -- nothing is said.
    let mut other = tool_stamp();
    other["families"] = json!("weather");
    seam(&mut h, "t2", "0", other, json!({"messages": [user(ASK)]}));
    assert_eq!(round_call(&h).hop["turn_id"], json!("t2"));
    assert!(horizons(&h).is_empty(), "{:?}", h.out);
}

/// Pin 4 (lock): the horizon carries no words -- `seconds`, `way` and
/// `family` only, no message, no text field.
#[test]
fn the_horizon_body_carries_no_text() {
    if !shipped() {
        return;
    }
    let mut h = hive(json!({"web": 2500}));
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp(),
        json!({"messages": [user(ASK)]}),
    );
    let feeds = horizons(&h);
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let f = &feeds[0];
    assert!(f.messages().is_empty(), "no words: {:?}", f.body);
    for k in f.body.keys() {
        assert!(
            ["messages", "section", "seconds", "way", "family"].contains(&k.as_str()),
            "structure only, never content: {k}"
        );
    }
    assert_eq!(f.body["seconds"], json!(3), "seconds round up");
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

/// Default byte-identical: with `horizon_advise` "0" a duplex tool round
/// sends, writes and reads exactly what it did without the knobs.
#[test]
fn horizon_off_moves_no_byte() {
    if !shipped() {
        return;
    }
    let run = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over);
        seam(
            &mut h,
            "t1",
            "0",
            tool_stamp(),
            json!({"messages": [user(ASK)]}),
        );
        let out: Vec<(String, Value)> = h
            .out
            .iter()
            .map(|m| (m.route().to_string(), Value::Object(m.body.clone())))
            .collect();
        let state = h.rows("SELECT key FROM state WHERE key LIKE 'horizon%'");
        let ops = h.ledger_ops.len();
        (out, state, ops)
    };
    let plain = run(&[("policy", "role", json!("talky"))]);
    let off = run(&[
        ("policy", "role", json!("talky")),
        ("policy", "horizon_advise", json!("0")),
        ("policy", "horizon_defaults_ms", json!("{\"web\": 4000}")),
        ("policy", "horizon_quantile", json!("75")),
    ]);
    assert_eq!(plain.1, off.1);
    assert!(off.1.is_empty());
    assert_eq!(plain.2, off.2, "the same ledger traffic");
    assert_eq!(plain.0.len(), off.0.len());
    assert!(!off.0.iter().any(|(r, _)| r == "live_feed"));
}
