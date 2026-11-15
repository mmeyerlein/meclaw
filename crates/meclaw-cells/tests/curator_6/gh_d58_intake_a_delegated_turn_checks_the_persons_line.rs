//! GH #1498 (D58, OR-58-184) -- the intake checks a delegated turn's quote
//! against the person's line of that turn.
//!
//! WHY this file exists: on a phone call the voice handed a turn to the
//! backend as a delegation. The answer round of a delegation runs under the
//! call id as its session (`<label>~<deadline_ms>~<hex8>`, `delegation_id`,
//! `live`), while the person's line of the turn stands on the wall under the
//! phone session and the turn's label (`phone-...`, `<call>#3`). The rule
//! `source` read the person's rows only under the answer round's own session
//! and turn, found none, refused the model's correct wellbeing fact with
//! `quote_not_in_source` and, under `mode` "drop", dropped it: the follow-up
//! got `facts: []` and no note was set (live run, rows 1101/1146/1169/1420 of
//! the call blob). The same sentence on a normal turn was kept. Now a
//! delegation round also reads the person's rows of the turn it answers --
//! its label -- wherever the audience gate lets the round see them; a quote
//! that is not in that line is still refused, on delegated and normal turns.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The rule a talky stamps when `memory` checks its facts at their source.
const RULE: &str = r#"{"items":"facts","mode":"drop","quote":"source"}"#;
const CALL: &str = "call-8ed7";
const PHONE: &str = "phone-2026-11-12T15:51:54Z";
/// A deadline no run of this test reaches (2100-01-01).
const OPEN_UNTIL: u64 = 4_102_444_800_000;
const SAID: &str = "Since this morning I have been so dizzy and I am alone at home";
const EARLIER: &str = "My neighbour waters the plants on Sundays";

fn hive() -> Hive {
    Hive::with(&[(
        "intake",
        "sections",
        json!({"memory": {"lane": "sidecar", "mark": "topic"}}),
    )])
}

fn label(n: u32) -> String {
    format!("{CALL}#{n}")
}

/// The person's turns of the phone session, as the voice's own path writes
/// them: the person's line and the voice's answer under the turn's label.
fn phone_turns(h: &mut Hive) {
    turn(h, PHONE, &label(2), EARLIER, "How kind of them.", json!({}));
    turn(h, PHONE, &label(3), SAID, "Oh, that", json!({}));
}

/// The delegation round of turn #3: its own session (the call id), keyed
/// `<label>~<deadline>~<hex8>`, with `delegation_id` and the handle `live`.
/// The frame carries only the start of what was heard, as live.
fn delegation_round(h: &mut Hive) -> Msg {
    h.out.clear();
    let round = format!("{}~{OPEN_UNTIL}~ab12cd34", label(3));
    let hop = json!({"session_id": CALL, "turn_id": round, "iter": "0", "phase": "",
                     "path": "", "needs": "", "families": "",
                     "delegation_id": "item_d1", "live": CALL});
    let ctx = json!({"session_id": CALL, "turn_id": round, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "",
                     "delegation_id": "item_d1", "live": CALL});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(
            "[delegation item_d1 from the voice channel: the caller expects backend \
             work; last heard:  Since this morning I \u{2026}]"
        )]}),
    );
    let calls: Vec<Msg> = h
        .routed("brain")
        .into_iter()
        .chain(h.routed("quick"))
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the delegation makes its call: {:?}",
        h.stderr
    );
    calls[0].clone()
}

/// The `memory` section of the answer to `call`, as the talky's splitter
/// hands it to `in_section`: the answer's context, the hop's turn its label
/// (`turn`), the rule stamped beside it.
fn section_in(h: &mut Hive, call: &Msg, turn: &str, facts: Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    for k in ["delegation_id", "live"] {
        if let Some(v) = call.hop.get(k) {
            ctx.insert(k.into(), v.clone());
        }
    }
    ctx.insert("episode_turn_id".into(), json!(""));
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "memory", "turn_id": turn, "sidecar_verify": RULE}),
        json!({"messages": [], "section": "memory",
               "payload": {"facts": facts, "nothing_new": false}, "verify_answer": ""}),
    );
}

fn fact(quote: &str) -> Value {
    json!({"claim": "the person is dizzy and alone", "quote": quote,
           "source": "person", "wellbeing": true})
}

/// The quotes of the facts that passed, each with its `verified`.
fn passed(h: &Hive) -> Vec<(String, Value)> {
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?} {:?}", h.out, h.stderr);
    sides[0].body["payload"]["facts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|f| {
            (
                f["quote"].as_str().unwrap_or("").to_string(),
                f["verified"].clone(),
            )
        })
        .collect()
}

fn reasons(h: &Hive) -> Vec<String> {
    h.routed("reject")
        .iter()
        .map(|m| m.hop["reject_reason"].as_str().unwrap_or("").to_string())
        .collect()
}

fn reject_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'verify_reject' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("")).unwrap_or(Value::Null))
        .collect()
}

fn q(s: &str) -> (String, Value) {
    (s.to_string(), json!(["quote"]))
}

fn refusal(item: u64) -> Value {
    json!({"section": "memory", "field": "quote", "reason": "quote_not_in_source",
           "source": "person", "item": item})
}

/// Pin: the delegated turn's person fact with the caller's literal words is
/// verified and kept -- its line stands under the phone session.
#[test]
fn a_delegated_turn_keeps_the_persons_literal_words() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    phone_turns(&mut h);
    let call = delegation_round(&mut h);
    section_in(&mut h, &call, &label(3), json!([fact(SAID)]));
    assert_eq!(reasons(&h), Vec::<String>::new(), "{:?}", h.stderr);
    assert_eq!(passed(&h), [q(SAID)], "{:?}", h.stderr);
    assert!(reject_marks(&h).is_empty(), "{:?}", reject_marks(&h));
}

/// Counter-pin: on the delegated turn a quote that is not in the person's
/// line of THAT turn is still refused -- words never said, words of the
/// delegation frame, and the person's words of an earlier turn of the call.
#[test]
fn a_delegated_turn_still_refuses_a_quote_not_in_the_persons_line() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    phone_turns(&mut h);
    let call = delegation_round(&mut h);
    section_in(
        &mut h,
        &call,
        &label(3),
        json!([
            fact("I fell down the stairs this morning"),
            fact("the caller expects backend work"),
            fact(EARLIER),
            fact(SAID),
        ]),
    );
    assert_eq!(passed(&h), [q(SAID)], "{:?}", h.stderr);
    assert_eq!(reasons(&h), ["quote_not_in_source"; 3]);
    assert_eq!(reject_marks(&h), [refusal(0), refusal(1), refusal(2)]);
}

/// Counter-pin: on a normal turn of the phone session nothing changed -- the
/// person's words pass, words never said are refused.
#[test]
fn a_normal_turn_checks_as_before() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        PHONE,
        &label(2),
        EARLIER,
        "How kind of them.",
        json!({}),
    );
    let call = turn(&mut h, PHONE, &label(3), SAID, "Oh, that", json!({}));
    section_in(
        &mut h,
        &call,
        &label(3),
        json!([
            fact("I fell down the stairs this morning"),
            fact(EARLIER),
            fact(SAID)
        ]),
    );
    assert_eq!(passed(&h), [q(SAID)], "{:?}", h.stderr);
    assert_eq!(reasons(&h), ["quote_not_in_source"; 2]);
    assert_eq!(reject_marks(&h), [refusal(0), refusal(1)]);
}

/// Sets the audience of every wall row of the phone session to `audience`
/// (the JSON text of the column): the person's line under another round.
fn phone_rows_under(h: &Hive, audience: &str) {
    let n =
        h.db.execute(
            &format!("UPDATE wall SET audience_set = '{audience}' WHERE session_id = '{PHONE}'"),
            [],
        )
        .expect("the wall rows of the phone session");
    assert!(n > 0, "check of the setup: the phone session has rows");
}

/// Gate (review M1 of the strand): the person's line of the turn stands
/// under an audience the delegation round does not cover -- the label read
/// goes through the round's audience gate like every read, so the line stays
/// closed and the quote is refused, even with the caller's literal words.
#[test]
fn a_line_under_an_audience_the_round_does_not_cover_stays_closed() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    phone_turns(&mut h);
    phone_rows_under(&h, r#"["member:z"]"#);
    let call = delegation_round(&mut h);
    section_in(&mut h, &call, &label(3), json!([fact(SAID)]));
    assert_eq!(passed(&h), Vec::<(String, Value)>::new(), "{:?}", h.stderr);
    assert_eq!(reasons(&h), ["quote_not_in_source"]);
    assert_eq!(reject_marks(&h), [refusal(0)]);
}

/// Gate: a section without a declared round reads only rows of its own
/// session (the delegation's, the call id) that declare no round either --
/// the person's line under the phone session is not read, though it declares
/// none, and the quote is refused.
#[test]
fn without_a_round_the_label_read_stays_in_the_rounds_own_session() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    phone_turns(&mut h);
    phone_rows_under(&h, "[]");
    let mut call = delegation_round(&mut h);
    call.context.remove("audience_set");
    section_in(&mut h, &call, &label(3), json!([fact(SAID)]));
    assert_eq!(passed(&h), Vec::<(String, Value)>::new(), "{:?}", h.stderr);
    assert_eq!(reasons(&h), ["quote_not_in_source"]);
    assert_eq!(reject_marks(&h), [refusal(0)]);
}

/// Gate: the label read refused by the store is `store_error`, never a
/// missing quote.
#[test]
fn a_failed_label_read_is_store_error() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    phone_turns(&mut h);
    let call = delegation_round(&mut h);
    h.ledger_may_refuse = true;
    h.refuse_legs
        .push(("a-dsrc".to_string(), "store_error".to_string()));
    section_in(&mut h, &call, &label(3), json!([fact(SAID)]));
    assert!(
        h.refuse_legs.is_empty(),
        "check of the setup: the read was refused"
    );
    assert_eq!(passed(&h), Vec::<(String, Value)>::new(), "{:?}", h.stderr);
    assert_eq!(reasons(&h), ["store_error"], "{:?}", h.stderr);
    let marks = reject_marks(&h);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["reason"], "store_error", "{marks:?}");
}
