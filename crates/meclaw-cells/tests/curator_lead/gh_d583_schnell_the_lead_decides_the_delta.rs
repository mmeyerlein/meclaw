//! Wave d58-3, strand `schnell` (GH #1529, contract point 3) -- the lead
//! decides whether a delta call is reused or fired anew; the curator only
//! carries the decision out.
//!
//! WHY this file exists: since GH #1447 the lead holds every choice of the
//! switch (R-58-9: the switch moves out) -- way, target, fallback, what
//! becomes of the turn's speculation. One choice stayed in the curator's
//! policy: whether the spec curate of a turn reuses the call fired from the
//! route's delta stamp (`delta_speculation`, GH #1279) or fires anew. The
//! policy compared the turn's heard text, normalised, with the stamp's
//! `text_norm` itself, beside the lead's flags (`delta.ok`, `delta.memory`).
//! Now the policy sends the normalised text of the turn with its question to
//! the lead (`lead_ask`, field `text_norm`), the lead's verdict says
//! `delta.verdict` -- `reused` or `refired` -- and `delta.why` (empty, or the
//! reason, e.g. `memory`), and the policy executes that word: the cut, the
//! executor and the marks stay with the curator, which owns the window
//! (R-WM-1).
//!
//! Runs the pure half of the shipped lead (`cell_scope`) and the shipped
//! curator hive with the lead wired as talky wires it
//! (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The turn's words and their normalised form (the form the route stamps a
/// delta with and the policy's `normalize` gives them).
const ASK: &str = "How warm is it?";
const ASK_NORM: &str = "how warm is it";

/// The lead's knobs for a speculating voice with deltas, `recall_to_fallback`
/// as given (d58-3 namen, GH #1525: the old `memory_to_brain`); the need
/// names of `recall_needs` are the composition's, wired by `cell_scope`
/// (`namen_wiring`: `["memory"]`), so the reason is the need name `memory`.
fn delta_knobs(recall_to_fallback: &str) -> Value {
    ack_knobs(json!({"spec_ahead": "1", "spec_tools": "",
                     "delta_speculation": "1", "recall_to_fallback": recall_to_fallback}))
}

/// The verdict the policy builds the window on (`lead_verdict` with the
/// lead's `spec_window` beside it, as the lead's `ask` phase sends it) for
/// the spec curate of turn `t1` whose heard text is `heard`, with a delta
/// stamp of the turn that says `text_norm` and `needs`.
fn delta_verdict(recall_to_fallback: &str, heard: &str, text_norm: &str, needs: Value) -> Value {
    let head = ack_head(json!({"spec": "1", "text_norm": heard}));
    let state = json!({"escalated": "", "consult_pending": {}, "spec_held": {},
                       "spec": {"call": "c-t1", "state": "fired"}, "called": false});
    let stamp = json!({"turn_id": "t1", "path": "simple", "p": 0.9, "intent": "",
                       "needs": needs, "families": [], "duration": 5,
                       "text_norm": text_norm});
    let raw = json!({"delta:t1": canonical(&json!({"stamp": stamp}))});
    let (v, err) = cell_scope(
        "lead",
        delta_knobs(recall_to_fallback),
        "(lambda v: (v.update(spec_window(v, ARGS[1], ARGS[3], ARGS[2])), v)[1])\
         (lead_verdict(ARGS[0], ARGS[1], ARGS[2]))",
        json!([{"path": "", "needs": [], "families": []}, head, state, raw]),
    );
    assert!(v.is_object(), "a verdict: {v} {err}");
    v
}

/// Pin: the lead names the delta's fate -- the same text reuses it, another
/// text fires anew, and a turn that leaves on the fallback for its memory
/// fires anew with the reason `memory`.
#[test]
fn gh_d583_schnell_the_lead_decides_reuse_or_refire() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let same = delta_verdict("0", ASK_NORM, ASK_NORM, json!(["tool"]));
    assert_eq!(
        same["delta"]["verdict"],
        json!("reused"),
        "the same text: the delta call is the turn's call -- {}",
        same["delta"]
    );
    assert_eq!(same["delta"]["why"], json!(""), "{}", same["delta"]);
    let other = delta_verdict("0", "how cold is it", ASK_NORM, json!(["tool"]));
    assert_eq!(
        other["delta"]["verdict"],
        json!("refired"),
        "another text: the turn is fired anew -- {}",
        other["delta"]
    );
    let memory = delta_verdict("1", ASK_NORM, ASK_NORM, json!(["memory"]));
    assert_eq!(
        memory["delta"]["verdict"],
        json!("refired"),
        "a turn that leaves on the fallback for its memory -- {}",
        memory["delta"]
    );
    assert_eq!(
        memory["delta"]["why"],
        json!("memory"),
        "{}",
        memory["delta"]
    );
}

/// The marks of kind `kind`, oldest first.
fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

/// talky's voice path with deltas (`delta_speculation` with
/// `spec_ahead`, GH #1279).
fn delta_hive() -> Hive {
    Hive::with(&[
        ("lead", "spec_ahead", json!("1")),
        ("lead", "spec_tools", json!("")),
        ("lead", "delta_speculation", json!("1")),
    ])
    .reporting_composite("talky")
}

/// The route's stamp of a delta of turn `t1` saying `text_norm`.
fn delta_stamp(h: &mut Hive, text_norm: &str) {
    h.out.clear();
    h.lane(
        "in_delta_stamp",
        json!({"session_id": "s1", "turn_id": "t1", "delta": "1"}),
        json!({"turn_id": "t1", "path": "simple"}),
        json!({"messages": [], "stamp": {"turn_id": "t1", "path": "simple", "p": 0.9,
                                         "intent": "", "needs": ["tool"], "families": [],
                                         "duration": 5, "text_norm": text_norm}}),
    );
}

/// The collector's early curate of turn `t1` (`spec` "1", no path yet).
fn spec_curate(h: &mut Hive) {
    spec_curate_with(h, json!([user(ASK)]));
}

/// [`spec_curate`] with the round's elements `messages`.
fn spec_curate_with(h: &mut Hive, messages: Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

/// The spec curate of a turn with a delta stamp saying `text_norm`, every
/// verdict of the lead for this call held on its way home and its delta
/// decision replaced by `word` before the policy reads it; the `delta` marks
/// after. Every one, not the first (OR-583.schnell.4): the spec curate is
/// the session's first and goes over `./handover`, and the policy asks the
/// lead anew after it (`handover-done`, GH #1371) -- an unheld second
/// answer would decide with the lead's own word.
fn executed(text_norm: &str, word: &str) -> Vec<Value> {
    marks(&executed_in(text_norm, word), "delta")
}

/// [`executed`], the hive after it.
fn executed_in(text_norm: &str, word: &str) -> Hive {
    let mut h = delta_hive();
    delta_stamp(&mut h, text_norm);
    h.hold_lane = Some(VERDICT_LANE.to_string());
    spec_curate(&mut h);
    let (mut seen, mut rounds) = (0, 0);
    while !h.held.is_empty() {
        rounds += 1;
        assert!(
            rounds <= 8,
            "check of the setup: the lead is asked a bounded number of times: {:?}",
            h.held
        );
        for m in h.held.iter_mut() {
            if m.body.get("verdict").is_some_and(Value::is_object) {
                m.body["verdict"]["delta"]["verdict"] = json!(word);
                m.body["verdict"]["delta"]["why"] = json!("");
                seen += 1;
            }
        }
        h.deliver_held();
    }
    h.hold_lane = None;
    assert!(
        seen >= 1,
        "check of the setup: the lead's verdict was held: {:?}",
        h.stderr
    );
    h
}

/// Pin: the curator executes the lead's word, whatever the texts say -- the
/// same text with the word `refired` fires anew, another text with the word
/// `reused` reuses the delta call.
#[test]
fn gh_d583_schnell_the_curator_only_executes_the_delta() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let refired = executed(ASK_NORM, "refired");
    assert_eq!(refired.len(), 1, "{refired:?}");
    assert_eq!(
        refired[0]["verdict"],
        json!("refired"),
        "the lead said refired, the same text does not overrule it: {refired:?}"
    );
    let reused = executed("how cold is it", "reused");
    assert_eq!(reused.len(), 1, "{reused:?}");
    assert_eq!(
        reused[0]["verdict"],
        json!("reused"),
        "the lead said reused, another text does not overrule it: {reused:?}"
    );

    // The question carries what the lead decides on: the turn's text,
    // normalised. Read where it leaves the curator (no bridge to a lead).
    let mut h = delta_hive();
    h.wired = false;
    spec_curate(&mut h);
    let asks: Vec<Msg> = to_lead(&h)
        .into_iter()
        .filter(|m| m.route() == "lead_ask")
        .collect();
    assert_eq!(asks.len(), 1, "one question to the lead: {:?}", h.out);
    assert_eq!(
        asks[0].body["ask"]["text_norm"],
        json!(ASK_NORM),
        "the question names the turn's normalised text: {:?}",
        asks[0].body["ask"]
    );
}

/// Pin (review M-1 of the strand): a verdict without a word -- the lead
/// holds no claim on the delta (`delta.verdict` "") -- decides nothing: the
/// curator writes no `delta` mark and ends the delta (`delta_end`, the lead
/// takes its row down); the spec curate runs as #1275 says.
#[test]
fn gh_d583_schnell_a_verdict_without_a_word_ends_the_delta_unmarked() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let h = executed_in(ASK_NORM, "");
    assert!(
        marks(&h, "delta").is_empty(),
        "no word, no delta mark: {:?}",
        marks(&h, "delta")
    );
    assert_eq!(
        h.lead_state("delta:t1"),
        "",
        "the delta ends: the lead's row of it is taken down -- {:?}",
        h.stderr
    );
}

/// Pin (review M-3 of the strand): the turn's text the lead compares is the
/// person's words alone, sorted the way the wall sorts a round
/// (`kinds_of`): a framed text in the same round -- an advice, a
/// delegation, an app or a timer -- is nobody's words and does not turn a
/// delta of the same words into a new call.
#[test]
fn gh_d583_schnell_a_framed_text_beside_the_words_keeps_the_delta() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let mut h = delta_hive();
    delta_stamp(&mut h, ASK_NORM);
    spec_curate_with(
        &mut h,
        json!([
            user(ASK),
            user(
                "[advice from your reasoning core -- not said by the person]\nThe garden is south-facing."
            )
        ]),
    );
    let marked = marks(&h, "delta");
    assert_eq!(
        marked.len(),
        1,
        "check of the setup: one delta mark {marked:?} {:?}",
        h.stderr
    );
    assert_eq!(
        marked[0]["verdict"],
        json!("reused"),
        "the person's words are the stamp's: the framed text does not count -- {marked:?}"
    );
}
