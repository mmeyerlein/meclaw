//! #1432 -- a speculation sentence already shown is never repeated.
//!
//! WHY this file exists: in round 4 of the Telegram battery (turns 2 and 5)
//! the first sentence of a speculation went out as a partial (#1411: the
//! turn awaited the speculation and needed no tool), then the spec call ran
//! into its external timeout (`outcome="timeout" handle_ms=6502
//! first_partial_ms=4833 partials=2`), finished `error`, and `spec_verdict`
//! dropped it as `no_answer`. The stored normal call went and wrote the
//! answer anew and in other words ("for four portions" instead of "for four
//! people"); the Telegram rim cuts only an answer that begins with the shown
//! sentence, so the new answer went whole -- the content twice. In turn 5 the
//! stored call failed as well and the round went to the brain (#1352), which
//! also wrote anew.
//!
//! The gate of #1411 stays: under `await`, the row's own call and no `tool`
//! in the stored `needs`, `spec_verdict` can drop only for `no_answer`, which
//! no gate can see before the finish (pin 1 holds that). The fix is in the
//! fallback: the curator keeps the text of the first partial it passed
//! (`spec_shown:<session>#<turn>`), and the call that answers instead --
//! the stored call, and the brain call of its escalation -- carries that
//! text as its own last message, already delivered, so the model writes on
//! from there (the llm cell turns a trailing assistant text into the note
//! `TRAILING_ASSISTANT_NOTE` on a Claude wire, GH #1282).
//!
//! The claims run the SHIPPED curator scripts in the curator-hive harness;
//! the spec rows come from the shipped flow. No clock is involved.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const TIMEOUT: &str = r#"{"error_code": "timeout", "error_source": "wire"}"#;
const ASK: &str = "how much rice do I need for four people";
const S: &str = "For four people you need about 300 grams of rice.";
const S2: &str = "For four people you need about 300 grams of rice. Rinse it first.";

fn spec_hive() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!("web,weather")),
    ]);
    h.check_emits = true;
    h
}

/// The collector's early curate (`hop.spec` "1", no path); the spec call.
fn spec_call(h: &mut Hive, turn: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ASK)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].hop["spec"], json!("1"));
    quick[0].clone()
}

/// The normal curate of the turn at its route stamp: no tool, family `web`
/// (inside `spec_tools`) -- the row moves to `await`.
fn seam(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "simple", "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ASK)]}));
    assert_eq!(row_state(h, turn), "await", "fixture: {:?}", h.stderr);
}

fn spec_ctx(call: &Msg) -> Map<String, Value> {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    ctx
}

/// One partial of the spec call on `in_spec_partial`, as talky's edge hands
/// it in; returns the routes that left the hive.
fn spec_partial(h: &mut Hive, call: &Msg, seq: u64, text: &str) -> Vec<String> {
    h.out.clear();
    h.stderr.clear();
    let hop = json!({"route": "partial", "seq": seq, "final": false,
                     "partial_of": "0190aaaa-0000-7000-8000-000000000001"});
    h.lane(
        "in_spec_partial",
        Value::Object(spec_ctx(call)),
        hop,
        json!({"messages": [said(text)]}),
    );
    h.out.iter().map(|m| m.route().to_string()).collect()
}

/// The spec call's end on `in_spec_llm`.
fn spec_end(h: &mut Hive, call: &Msg, finish: &str, extra: Value, messages: Value) {
    h.out.clear();
    h.stderr.clear();
    let mut hop = obj(json!({"finish_reason": finish, "model": "test-model",
                             "tokens_prompt": 10}));
    for (k, v) in obj(extra) {
        hop.insert(k, v);
    }
    h.lane(
        "in_spec_llm",
        Value::Object(spec_ctx(call)),
        Value::Object(hop),
        json!({"messages": messages}),
    );
}

fn row_state(h: &Hive, turn: &str) -> String {
    let raw = h.lead_state(&format!("spec:s1#{turn}"));
    let row: Value = sj::from_str(&raw).unwrap_or(Value::Null);
    row["state"].as_str().unwrap_or_default().to_string()
}

fn shown_row(h: &Hive, turn: &str) -> String {
    h.lead_state(&format!("spec_shown:s1#{turn}"))
}

fn err() -> Value {
    sj::from_str(TIMEOUT).unwrap()
}

fn last_message(m: &Msg) -> Value {
    m.messages().last().cloned().unwrap_or(Value::Null)
}

/// The turn up to the spec's failure: spec call, stamp, the partials given
/// (each passes), then the spec call ends `error`. Returns the stored call
/// that went.
fn failed_after(h: &mut Hive, partials: &[&str]) -> Msg {
    let spec = spec_call(h, "t1");
    seam(h, "t1");
    for (i, p) in partials.iter().enumerate() {
        let routes = spec_partial(h, &spec, i as u64 + 1, p);
        assert_eq!(routes, vec!["partial".to_string()], "{:?}", h.stderr);
    }
    spec_end(h, &spec, "error", err(), json!([]));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "the stored call goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(quick[0].hop.get("spec").is_none());
    quick[0].clone()
}

/// Pin 1 (characterization): under `await`, the row's own call and no
/// `tool` the verdict drops only an answer that is none -- a partial went
/// out, the spec call then ends `error`, the answer is dropped as
/// `no_answer` and the stored call goes. No gate sees that before the finish.
#[test]
fn gh1432_a_partial_passed_then_the_spec_ends_without_an_answer() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1");
    seam(&mut h, "t1");
    assert_eq!(
        spec_partial(&mut h, &spec, 1, S),
        vec!["partial".to_string()]
    );
    spec_end(&mut h, &spec, "error", err(), json!([]));
    let dropped: Vec<&String> = h
        .stderr
        .iter()
        .filter(|l| l.contains("dropped (no_answer) -- the stored call goes"))
        .collect();
    assert_eq!(dropped.len(), 1, "{:?}", h.stderr);
    assert_eq!(h.routed("quick").len(), 1, "{:?}", h.out);
    assert!(h.routed("spec_release").is_empty(), "{:?}", h.out);
    assert_eq!(row_state(&h, "t1"), "", "the decided row went");
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 2: the stored call of a failed speculation whose sentence was shown
/// carries that sentence as its own last message, already delivered.
#[test]
fn gh1432_the_stored_call_carries_the_shown_sentence() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[S]);
    assert_eq!(last_message(&fb), said(S), "{:?}", fb.messages());
    let shown: Vec<Value> = fb
        .messages()
        .into_iter()
        .filter(|m| *m == said(S))
        .collect();
    assert_eq!(shown.len(), 1, "once: {:?}", fb.messages());
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 3: the stored call fails as well -- the round goes to the brain
/// (#1352), and the brain call carries the shown sentence the same way. The
/// row ends with the turn's answer.
#[test]
fn gh1432_the_escalated_brain_call_carries_the_shown_sentence() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[S]);
    h.out.clear();
    h.stderr.clear();
    h.tap(&fb, "error", err(), json!([]));
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "to the brain: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        last_message(&brain[0]),
        said(S),
        "{:?}",
        brain[0].messages()
    );
    assert!(
        !shown_row(&h, "t1").is_empty(),
        "the row stands until the answer"
    );
    let b = brain[0].clone();
    h.out.clear();
    h.tap(
        &b,
        "stop",
        json!({}),
        json!([said("Rinse it first, then cook it.")]),
    );
    assert_eq!(
        shown_row(&h, "t1"),
        "",
        "the row ends with the turn's answer"
    );
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 4: the sentence is the FIRST partial passed (the rim shows that one);
/// a later partial does not overwrite it. A kept speculation appends nothing
/// and its row goes with the decision; a stored call that answers ends it.
#[test]
fn gh1432_the_first_partial_is_the_shown_sentence_and_the_row_ends() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[S, S2]);
    assert_eq!(last_message(&fb), said(S), "{:?}", fb.messages());
    h.out.clear();
    h.tap(&fb, "stop", json!({}), json!([said("Rinse it first.")]));
    assert_eq!(shown_row(&h, "t1"), "", "the stored call's answer ends it");

    // Kept: the row goes with the decision, the release is the answer.
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1");
    seam(&mut h, "t1");
    assert_eq!(
        spec_partial(&mut h, &spec, 1, S),
        vec!["partial".to_string()]
    );
    assert!(
        !shown_row(&h, "t1").is_empty(),
        "the passed sentence is kept"
    );
    spec_end(&mut h, &spec, "stop", json!({}), json!([said(S2)]));
    let rel = h.routed("spec_release");
    assert_eq!(rel.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(rel[0].messages(), vec![said(S2)], "nothing appended");
    assert!(h.routed("quick").is_empty());
    assert_eq!(
        shown_row(&h, "t1"),
        "",
        "kept: the row goes with the decision"
    );
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 4b: no partial passed (held back before the stamp) -- the stored
/// call is the one of a turn without any partial, message for message.
#[test]
fn gh1432_without_a_passed_partial_the_stored_call_is_as_before() {
    if !shipped() {
        return;
    }
    let reference = {
        let mut h = spec_hive();
        failed_after(&mut h, &[]).messages()
    };
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1");
    assert!(
        spec_partial(&mut h, &spec, 1, S).is_empty(),
        "held back before the stamp"
    );
    seam(&mut h, "t1");
    spec_end(&mut h, &spec, "error", err(), json!([]));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].messages(), reference);
    assert_eq!(shown_row(&h, "t1"), "");
}

/// Fault injection: the store refuses the read of the shown sentence -- no
/// crash, the stored call goes as before, one stderr line names the leg.
#[test]
fn gh1432_a_refused_read_of_the_shown_sentence_sends_as_before() {
    if !shipped() {
        return;
    }
    let reference = {
        let mut h = spec_hive();
        failed_after(&mut h, &[]).messages()
    };
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1");
    seam(&mut h, "t1");
    assert_eq!(
        spec_partial(&mut h, &spec, 1, S),
        vec!["partial".to_string()]
    );
    // The shown sentence lives in the lead's store since GH #1447.
    h.lead_refuse_legs = vec![("h-shown".to_string(), "store_error".to_string())];
    spec_end(&mut h, &spec, "error", err(), json!([]));
    assert!(h.lead_refuse_legs.is_empty(), "the refusal struck");
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].messages(), reference);
    let lines: Vec<&String> = h.stderr.iter().filter(|l| l.contains("h-shown")).collect();
    assert_eq!(lines.len(), 1, "{:?}", h.stderr);
}

/// The answer texts the wall holds for turn `turn`, in `seq` order.
fn wall_answers(h: &Hive, turn: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.turn_id = '{turn}' AND w.kind = 'assistant' ORDER BY w.seq"
    ))
    .into_iter()
    .map(|r| {
        let el: Value = sj::from_str(r[0].as_str().unwrap_or_default()).unwrap_or(Value::Null);
        el["text"].as_str().unwrap_or_default().to_string()
    })
    .collect()
}

/// Pin 6 (review I-1): the person saw S and then the continuation, so the
/// wall -- and with it the episodes and the next window -- holds the same:
/// S in front of the fallback's answer (stored call and brain call alike).
/// An answer that already begins with S keeps it once; a turn without a
/// shown sentence writes its answer as before. The rim is untouched (pin 5).
#[test]
fn gh1432_p6_the_wall_keeps_the_shown_sentence_in_front_of_the_answer() {
    if !shipped() {
        return;
    }
    let rest = "Rinse it first.";
    // The stored call answers with the continuation only.
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[S]);
    h.out.clear();
    h.tap(&fb, "stop", json!({}), json!([said(rest)]));
    assert_eq!(
        wall_answers(&h, "t1"),
        vec![format!("{S} {rest}")],
        "{:?}",
        h.stderr
    );

    // The brain of the escalation answers with the continuation only.
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[S]);
    h.out.clear();
    h.tap(&fb, "error", err(), json!([]));
    let b = h.routed("brain")[0].clone();
    h.out.clear();
    h.tap(&b, "stop", json!({}), json!([said(rest)]));
    assert_eq!(wall_answers(&h, "t1"), vec![format!("{S} {rest}")]);

    // The model said S again: S stands once.
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[S]);
    h.out.clear();
    h.tap(&fb, "stop", json!({}), json!([said(S2)]));
    assert_eq!(wall_answers(&h, "t1"), vec![S2.to_string()]);

    // No shown sentence: the answer as it came.
    let mut h = spec_hive();
    let fb = failed_after(&mut h, &[]);
    h.out.clear();
    h.tap(&fb, "stop", json!({}), json!([said(rest)]));
    assert_eq!(wall_answers(&h, "t1"), vec![rest.to_string()]);
    assert_eq!(h.emit_violations, Vec::<String>::new());
}
