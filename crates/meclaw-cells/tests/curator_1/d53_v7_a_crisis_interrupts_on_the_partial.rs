//! #1404 -- a crisis heard on a partial interrupts before the turn ends.
//!
//! WHY this file exists: on the phone the crisis question came 2.0 to 3.7 s
//! after the person's sentence ended (BAT r2c, P4: the guard's `interrupt`
//! 1.0 to 1.8 s after the sentence, the first spoken word 0.4 to 1.9 s after
//! that), against 1.5 s. The route already judges every partial of a voice
//! turn (`in_delta_stamp`), so the crisis value `choices.risk` is known while
//! the person still speaks. With `interrupt_on_delta` on, the curator sends
//! the `interrupt` advise AT ONCE on the first partial whose `choices.risk`
//! value has a question in `interrupt_questions` (the keys ARE the crisis
//! values, the words are the set's): ONE `live_feed` with `hop.section`
//! `interrupt`, no `hop.at_turn_end`, addressed by `call_id`, `session_id`
//! and `turn_id`, the question as the assistant's text. At most one per turn
//! (a claimed state row); a value without a question, a fallback stamp and a
//! `no_text` turn never interrupt -- the judgement is the route's, the core
//! only looks the value up (R-108). The turn's end sends no second one; it
//! books `interrupt_early {delta_n, ms_before_turn_end}`. `0` (the default)
//! moves no byte.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const QUESTION: &str = "Are you safe right now?";

fn hive() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("lead", "interrupt_on_delta", json!("1")),
        (
            "lead",
            "interrupt_questions",
            json!(json!({"acute": QUESTION}).to_string()),
        ),
    ]);
    h.check_emits = true;
    h
}

/// A partial's stamp as the route sends it: `choices.risk` as the model
/// chose it, `hop.fallback` where the stamp is a fallback. A `call_id` in
/// `hop_extra` moves the call from the context onto the hop (as the route's
/// `delta_stamp` carries it); `null` there leaves the partial without one.
fn delta(h: &mut Hive, turn: &str, text_norm: &str, risk: &str, hop_extra: Value) {
    h.out.clear();
    let mut hop = json!({"turn_id": turn, "path": "talk"});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "delta": "1", "call_id": "call-9",
                         "audience_set": ROUND_E});
    for (k, v) in obj(hop_extra) {
        if k == "call_id" {
            ctx.as_object_mut().expect("ctx").remove("call_id");
            if v.is_null() {
                continue;
            }
        }
        hop[k] = v;
    }
    let source = if risk == "none" { "fallback" } else { "model" };
    h.lane(
        "in_delta_stamp",
        ctx,
        hop,
        json!({"messages": [], "stamp": {"turn_id": turn, "path": "talk", "p": 0.9,
                                         "intent": "", "needs": [], "families": [],
                                         "duration": 5, "text_norm": text_norm,
                                         "choices": {"risk": {"value": risk, "p": 0.9,
                                                              "source": source}},
                                         "turn_notes": []}}),
    );
}

fn interrupts(h: &Hive) -> Vec<Msg> {
    h.routed("live_feed")
        .into_iter()
        .filter(|m| m.hop.get("section") == Some(&json!("interrupt")))
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

/// The turn's end: its normal curate (the seam) and the answer on the tap.
fn turn_end(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "",
                     "engine": "duplex", "call_id": "call-9"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("I do not want to go on")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let mut seen = h.out.clone();
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("I am here.")]));
    seen.append(&mut h.out);
    h.out = seen;
}

/// Pin 1: the first partial whose risk value has a question sends ONE
/// interrupt, before the turn ends, addressed like the guard's.
#[test]
fn a_crisis_on_a_partial_interrupts_before_the_turn_ends() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    delta(&mut h, "s1#2", "i do not want", "acute", json!({}));
    let sent = interrupts(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let f = &sent[0];
    assert!(
        f.hop.get("at_turn_end").is_none(),
        "at once, not at the turn's end: {:?}",
        f.hop
    );
    assert_eq!(f.hop["call_id"], json!("call-9"));
    assert_eq!(f.hop["session_id"], json!("s1"));
    assert_eq!(f.hop["turn_id"], json!("s1#2"));
    assert_eq!(
        f.messages(),
        vec![json!({"origin": "assistant", "type": "text", "text": QUESTION})]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // A later partial of the same turn, still acute: no second one.
    delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
    assert!(interrupts(&h).is_empty(), "one per turn: {:?}", h.out);
}

/// Pin 2: a value without a question (`strained`), a fallback value, or a
/// partial of no call never interrupts.
#[test]
fn a_non_crisis_partial_never_interrupts() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    delta(&mut h, "s1#2", "i am tired", "strained", json!({}));
    assert!(interrupts(&h).is_empty(), "{:?}", h.out);
    delta(&mut h, "s1#2", "i am tired", "none", json!({}));
    assert!(interrupts(&h).is_empty(), "{:?}", h.out);
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'pending:interrupt:%'")
            .is_empty(),
        "nothing claimed"
    );
    // The same turn turns acute later: now it goes.
    delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
    assert_eq!(interrupts(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);

    // o8.7: a partial of no call never interrupts -- one ledger line says
    // why, and nothing is claimed for the turn.
    delta(
        &mut h,
        "s1#4",
        "i do not want to go on",
        "acute",
        json!({"call_id": null}),
    );
    assert!(interrupts(&h).is_empty(), "{:?}", h.out);
    assert_eq!(
        marks(&h, "interrupt_skipped"),
        vec![json!({"reason": "no_call_id"})],
        "{:?}",
        h.stderr
    );
    assert!(
        h.rows("SELECT key FROM state WHERE key = 'pending:interrupt:s1#4'")
            .is_empty()
    );
    // The call on the hop, as the route's delta stamp carries it, is enough.
    delta(
        &mut h,
        "s1#6",
        "i do not want to go on",
        "acute",
        json!({"call_id": "call-hop"}),
    );
    let sent = interrupts(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(sent[0].hop["call_id"], json!("call-hop"));
}

/// Pin 3: a `no_text` turn (the greeting, a continuation: FX3A1C) never
/// interrupts, whatever its choices carry.
#[test]
fn a_no_text_turn_never_interrupts() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    delta(
        &mut h,
        "s1#0",
        "hello",
        "acute",
        json!({"fallback": "no_text"}),
    );
    assert!(interrupts(&h).is_empty(), "{:?}", h.out);
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'pending:interrupt:%'")
            .is_empty()
    );
}

/// Pin 4: the turn's end sends no second interrupt; it books how early the
/// first one went.
#[test]
fn the_turn_end_sends_no_second_interrupt() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
    assert_eq!(interrupts(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);
    turn_end(&mut h, "s1#2");
    assert!(interrupts(&h).is_empty(), "{:?}", h.out);
    let m = marks(&h, "interrupt_early");
    assert_eq!(m.len(), 1, "{:?} {:?}", m, h.stderr);
    assert_eq!(m[0]["delta_n"], json!(7), "the words of the partial");
    assert!(
        m[0]["ms_before_turn_end"]
            .as_i64()
            .is_some_and(|ms| ms >= 0),
        "{m:?}"
    );
    // A turn without an early interrupt books nothing.
    turn_end(&mut h, "s1#3");
    assert_eq!(marks(&h, "interrupt_early").len(), 1);
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

/// Default lock: `interrupt_on_delta` "0" and `interrupt_questions` "{}"
/// shipped; off (or absent), a crisis partial moves no byte -- no emit, no row.
#[test]
fn interrupt_on_delta_off_moves_no_byte() {
    if !shipped() {
        return;
    }
    // The knobs live in the lead since GH #1447.
    let policy = cell_config("lead");
    for (k, v) in [
        ("interrupt_on_delta", json!("0")),
        ("interrupt_questions", json!("{}")),
    ] {
        assert_eq!(policy["params"][k], v, "{k}");
        assert_eq!(policy["contract"]["settings"][k]["default"], v, "{k}");
    }
    let run = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over);
        delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
        let out: Vec<(String, Value)> = h
            .out
            .iter()
            .map(|m| (m.route().to_string(), Value::Object(m.body.clone())))
            .collect();
        turn_end(&mut h, "s1#2");
        let state = h.rows("SELECT key FROM state WHERE key LIKE 'pending:interrupt:%'");
        // The call ids are fresh per run (uuid v4); the lines around them are
        // what the lock compares.
        let said: Vec<String> = h
            .stderr
            .iter()
            .map(|l| {
                l.split(' ')
                    .map(|w| {
                        if w.len() == 36 && w.matches('-').count() == 4 {
                            "<call>"
                        } else {
                            w
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        (out, state, h.ledger_ops.len(), said)
    };
    let plain = run(&[("policy", "role", json!("talky"))]);
    let off = run(&[
        ("policy", "role", json!("talky")),
        ("lead", "interrupt_on_delta", json!("0")),
        (
            "lead",
            "interrupt_questions",
            json!(json!({"acute": QUESTION}).to_string()),
        ),
    ]);
    assert_eq!(plain, off);
    assert!(off.0.is_empty(), "{:?}", off.0);
    assert!(off.1.is_empty());
}

/// #1407 (P4-F1): `talky/route` marks an unsure PATH with `hop.fallback`
/// "unsure" and leaves the choices as the model chose them (#1304). The
/// phone battery's crisis partial (20-P4 turn 5: `risk acute p 1.0 source
/// model`) carried exactly that and was dropped silently -- the interrupt
/// waited for the turn's end (+1 505 ms after the sentence instead of about
/// +300). An unsure path interrupts at once like a sure one.
#[test]
fn a_crisis_delta_stamp_on_an_unsure_path_interrupts_early() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    delta(
        &mut h,
        "s1#5",
        "i do not really want to go on anymore",
        "acute",
        json!({"fallback": "unsure"}),
    );
    let sent = interrupts(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(sent[0].hop["turn_id"], json!("s1#5"));
    assert!(marks(&h, "interrupt_skipped").is_empty());
}

/// #1407 lock: the fallbacks that are no judgement of the text -- `no_text`
/// (the greeting, a continuation: FX3A1C), `error`, `late` -- still never
/// interrupt; a crisis value they carry is booked once as skipped, with the
/// fallback as its reason, and nothing is claimed.
#[test]
fn a_no_text_turn_still_never_interrupts() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    for (n, fb) in ["no_text", "error", "late"].iter().enumerate() {
        let turn = format!("s1#{n}");
        delta(&mut h, &turn, "hello", "acute", json!({"fallback": fb}));
        assert!(interrupts(&h).is_empty(), "{fb}: {:?}", h.out);
    }
    assert_eq!(
        marks(&h, "interrupt_skipped"),
        vec![
            json!({"reason": "fallback:no_text"}),
            json!({"reason": "fallback:error"}),
            json!({"reason": "fallback:late"}),
        ],
        "{:?}",
        h.stderr
    );
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'pending:interrupt:%'")
            .is_empty()
    );
    // A blocked stamp without a crisis value books nothing.
    delta(
        &mut h,
        "s1#9",
        "hello",
        "strained",
        json!({"fallback": "no_text"}),
    );
    assert_eq!(marks(&h, "interrupt_skipped").len(), 3);
}
