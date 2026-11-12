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
//! `interrupt`, no `hop.at_turn_end`, addressed by `live` (the handle of the
//! running session, D58 live), `session_id` and `turn_id`, the question as
//! the assistant's text. At most one per turn
//! (a claimed state row); a value without a question, a fallback stamp and a
//! `no_text` turn never interrupt -- the judgement is the route's, the core
//! only looks the value up (R-108). The turn's end sends no second one; it
//! books `interrupt_early {delta_n, ms_before_turn_end}`. `0` (the default)
//! moves no byte. Since D58 feed (GH #1481, contract point 6) the interrupt
//! takes the curator's one road of orders: its claim is
//! `pending:feed:<live>#<turn>#interrupt` (was `pending:interrupt:<turn>`)
//! and it books the mark `feed` (OR-d58-2.feed.6). Since D58 moveout
//! (GH #1476, contract points 8 and 9) the lead ORDERS it on the curator's
//! lane `in_feed` (the composite opens it: `feed_in` "1", as talky ships it);
//! a question without a handle is booked by the one road as not sent
//! (`feed {reason: no_live}`, was `interrupt_skipped {reason: no_live}`), the
//! reason of a fallback stamp and how early the interrupt went are the lead's
//! row `interrupt:<session>#<turn>` (was `interrupt_skipped {reason:
//! fallback:<value>}` and `interrupt_early {delta_n, ms_before_turn_end}` in
//! the curator's ledger).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const QUESTION: &str = "Are you safe right now?";

fn hive() -> Hive {
    let mut h = Hive::with(&[
        // D58 moveout: the lead's order arrives on the lane from outside.
        ("policy", "feed_in", json!("1")),
        ("lead", "interrupt_on_delta", json!("1")),
        (
            "lead",
            "interrupt_questions",
            json!(json!({"acute": QUESTION}).to_string()),
        ),
    ])
    .reporting_composite("talky");
    h.check_emits = true;
    h
}

/// A partial's stamp as the route sends it: `choices.risk` as the model
/// chose it, `hop.fallback` where the stamp is a fallback. A `live` in
/// `hop_extra` moves the handle from the context onto the hop (as the route's
/// `delta_stamp` carries it); `null` there leaves the partial without one.
fn delta(h: &mut Hive, turn: &str, text_norm: &str, risk: &str, hop_extra: Value) {
    h.out.clear();
    let mut hop = json!({"turn_id": turn, "path": "talk"});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "delta": "1", "live": "call-9",
                         "audience_set": ROUND_E});
    for (k, v) in obj(hop_extra) {
        if k == "live" {
            ctx.as_object_mut().expect("ctx").remove("live");
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

/// The claims of the one road of feeds the ledger holds (D58 feed), sorted;
/// `like` narrows them (a SQL `LIKE` pattern after the prefix).
fn claims(h: &Hive, like: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT key FROM state WHERE key LIKE 'pending:feed:{like}' ORDER BY key"
    ))
    .iter()
    .filter_map(|r| r[0].as_str().map(str::to_string))
    .collect()
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

/// The marks `feed` of the section `interrupt` (D58 feed, D58 moveout).
fn interrupt_marks(h: &Hive) -> Vec<Value> {
    marks(h, "feed")
        .into_iter()
        .filter(|v| v["section"] == json!("interrupt"))
        .collect()
}

/// D58 moveout (contract point 9): the lead's row of the interrupt of
/// `turn` in session `s1` -- its reason, or when, on how many words and how
/// long before the turn's end it went; `None` when the lead keeps none.
fn lead_row(h: &Hive, turn: &str) -> Option<Value> {
    h.lead_rows(&format!(
        "SELECT value FROM state WHERE key = 'interrupt:s1#{turn}'"
    ))
    .first()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the row is JSON"))
}

/// The turn's end: its normal curate (the seam) and the answer on the tap.
fn turn_end(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": "",
                     "live": "call-9"});
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
    assert_eq!(f.hop["live"], json!("call-9"));
    assert_eq!(f.hop["session_id"], json!("s1"));
    assert_eq!(f.hop["turn_id"], json!("s1#2"));
    assert_eq!(
        f.hop["audience_set"],
        json!(ROUND_E),
        "the round: {:?}",
        f.hop
    );
    assert_eq!(
        f.messages(),
        vec![json!({"origin": "assistant", "type": "text", "text": QUESTION})]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
    assert_eq!(
        claims(&h, "%"),
        vec!["pending:feed:call-9#s1#2#interrupt".to_string()],
        "the turn's claim on the one road"
    );
    let booked = interrupt_marks(&h);
    assert_eq!(booked.len(), 1, "the interrupt books `feed`: {booked:?}");
    assert_eq!(
        booked[0]["mark"]["delta_n"],
        json!(4),
        "the words of the partial ride the mark: {booked:?}"
    );

    // A later partial of the same turn, still acute: no second one.
    delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
    assert!(interrupts(&h).is_empty(), "one per turn: {:?}", h.out);
    assert_eq!(claims(&h, "%").len(), 1, "no second claim");

    // The round rides in the order's head (contract point 8, review B M-3):
    // the order the shipped lead builds -- the round from the event, the
    // stamp's context without one -- delivered on `in_feed` with a context
    // that holds no round (`audience_set` null) still reaches the voice
    // with it; the curator's fallback to the context cannot carry it here.
    let (order, err) = cell_scope(
        "lead",
        json!({"interrupt_on_delta": "1",
               "interrupt_questions": json!({"acute": QUESTION}).to_string()}),
        "early_interrupt(ARGS[0], ARGS[1], ARGS[2], ARGS[3], ARGS[4])",
        json!([{"text_norm": "i do not want to go on",
                "choices": {"risk": {"value": "acute", "p": 0.9, "source": "model"}}},
               "s1#5", {}, {"session_id": "s1", "live": "call-9"}, ROUND_E]),
    );
    assert_eq!(
        order["header"]["audience_set"],
        json!(ROUND_E),
        "the lead's order names its round: {order} {err}"
    );
    let mut h = hive();
    h.out.clear();
    let mut body = order.clone();
    body.as_object_mut().expect("an order").remove("header");
    h.lane(
        "in_feed",
        json!({"session_id": "s1", "audience_set": null}),
        order["header"].clone(),
        body,
    );
    let sent = interrupts(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        sent[0].hop["audience_set"],
        json!(ROUND_E),
        "a context without a round: the head's round reaches the voice: {:?}",
        sent[0].hop
    );
}

/// Pin 2: a value without a question (`strained`), a fallback value, or a
/// partial of no running session never interrupts.
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
    assert!(claims(&h, "%").is_empty(), "nothing claimed");
    // The same turn turns acute later: now it goes -- and claims.
    delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
    assert_eq!(interrupts(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        claims(&h, "%#s1#2#%").len(),
        1,
        "the claim the check above looks for"
    );

    // o8.7: a partial of no running session (no handle `live`) never
    // interrupts -- one ledger line says why (D58 moveout: the lead orders,
    // the one road books it as not sent), and nothing is claimed for the turn.
    delta(
        &mut h,
        "s1#4",
        "i do not want to go on",
        "acute",
        json!({"live": null}),
    );
    assert!(interrupts(&h).is_empty(), "{:?}", h.out);
    let refused: Vec<Value> = interrupt_marks(&h)
        .into_iter()
        .filter(|v| v.get("reason").is_some())
        .collect();
    assert_eq!(refused.len(), 1, "{refused:?} {:?}", h.stderr);
    assert_eq!(refused[0]["reason"], json!("no_live"), "{refused:?}");
    assert!(
        marks(&h, "interrupt_skipped").is_empty(),
        "the old mark is gone"
    );
    assert!(
        claims(&h, "%#s1#4#%").is_empty(),
        "nothing claimed for s1#4"
    );
    // The handle on the hop, as the route's delta stamp carries it, is enough.
    delta(
        &mut h,
        "s1#6",
        "i do not want to go on",
        "acute",
        json!({"live": "call-hop"}),
    );
    let sent = interrupts(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(sent[0].hop["live"], json!("call-hop"));
    assert_eq!(
        claims(&h, "%#s1#6#%"),
        vec!["pending:feed:call-hop#s1#6#interrupt".to_string()],
        "a claimed turn shows under the pattern the s1#4 check reads"
    );
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
    assert!(claims(&h, "%").is_empty(), "nothing claimed");
}

/// Pin 4: the turn's end sends no second interrupt; it books how early the
/// first one went (D58 moveout: in the lead's row, no mark of the curator).
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
    let m = lead_row(&h, "s1#2").expect("the lead keeps the turn's interrupt");
    assert_eq!(m["delta_n"], json!(7), "the words of the partial: {m}");
    assert!(
        m["ms_before_turn_end"].as_i64().is_some_and(|ms| ms >= 0),
        "{m} {:?}",
        h.stderr
    );
    assert!(
        marks(&h, "interrupt_early").is_empty(),
        "the old mark is gone"
    );
    // A turn without an early interrupt books nothing.
    turn_end(&mut h, "s1#3");
    assert!(lead_row(&h, "s1#3").is_none());
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

/// Default lock: `interrupt_on_delta` "0" and `interrupt_questions` "{}"
/// shipped; off (or absent), a crisis partial moves no byte -- no emit, no row.
#[test]
fn interrupt_on_delta_off_moves_no_byte() {
    if !shipped() {
        return;
    }
    // The knobs live in the lead since GH #1447 (its own hive since GH #1474).
    let policy = lead_config("lead");
    for (k, v) in [
        ("interrupt_on_delta", json!("0")),
        ("interrupt_questions", json!("{}")),
    ] {
        assert_eq!(policy["params"][k], v, "{k}");
        assert_eq!(policy["contract"]["settings"][k]["default"], v, "{k}");
    }
    let run = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over).reporting_composite("talky");
        delta(&mut h, "s1#2", "i do not want to go on", "acute", json!({}));
        let out: Vec<(String, Value)> = h
            .out
            .iter()
            .map(|m| (m.route().to_string(), Value::Object(m.body.clone())))
            .collect();
        turn_end(&mut h, "s1#2");
        let state = claims(&h, "%");
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
    let plain = run(&[]);
    let off = run(&[
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
    assert!(lead_row(&h, "s1#5").is_some_and(|r| r.get("skip").is_none()));
}

/// #1407 lock: the fallbacks that are no judgement of the text -- `no_text`
/// (the greeting, a continuation: FX3A1C), `error`, `late` -- still never
/// interrupt; a crisis value they carry is booked once as skipped, with the
/// fallback as its reason (D58 moveout: never ordered, the reason in the
/// lead's row), and nothing is claimed.
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
    let skipped: Vec<Value> = ["s1#0", "s1#1", "s1#2"]
        .iter()
        .map(|t| {
            lead_row(&h, t)
                .map(|r| r["skip"].clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        skipped,
        vec![
            json!("fallback:no_text"),
            json!("fallback:error"),
            json!("fallback:late"),
        ],
        "{:?}",
        h.stderr
    );
    assert!(
        interrupt_marks(&h).is_empty(),
        "nothing booked by the curator"
    );
    assert!(
        marks(&h, "interrupt_skipped").is_empty(),
        "the old mark is gone"
    );
    assert!(claims(&h, "%").is_empty(), "nothing claimed");
    // A blocked stamp without a crisis value books nothing.
    delta(
        &mut h,
        "s1#9",
        "hello",
        "strained",
        json!({"fallback": "no_text"}),
    );
    assert!(lead_row(&h, "s1#9").is_none());
}
