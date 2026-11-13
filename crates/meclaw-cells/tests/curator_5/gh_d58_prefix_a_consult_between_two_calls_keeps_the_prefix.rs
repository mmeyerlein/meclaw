//! GH #1497 (D58 prefix, contract point 7) -- a consult between two calls
//! keeps the prefix.
//!
//! WHY this file exists: while a consult runs, the speaking model is told so
//! (`consult.pending`, #1198). The note stood in the system part: it joined
//! the tree with the first call after the consult started and left it with
//! the call that brought the advice -- two changes of the front of the
//! window without a rebuild, each a cache write of the whole window.
//! Measured in the hive harness (probe-10 of the strand, no model): the call
//! that brought the advice counted `prefix_break` 1 with `curate_mark`
//! "none"; the call after the start counted 0 only because the watch did not
//! look -- the call before it, the consult itself, sent no system part, and
//! the watch compared system parts only when both calls listed one. The note
//! now stands where the other notes of a turn stand, in the notes block that
//! opens the person's newest message (the tail, behind the cached prefix),
//! and the watch compares the system part the target holds on every call,
//! whether the call before it listed one or not.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The words the pending note opens with (`pending_text` of the policy).
const NOTE: &str = "A consult is pending (id ";

/// One turn of session `s1` on the way `path` with `system` as the
/// collector's system part; the brain call it routed, if any.
fn ask(
    h: &mut Hive,
    turn: &str,
    path: &str,
    extra: Value,
    words: &str,
    system: Value,
) -> Option<Msg> {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        ctx[k] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(words)], "system": system}),
    );
    h.routed("brain").first().cloned()
}

/// The verdict of the policy's own watch on `call` (`window_est:<call>`),
/// read before the tap of its answer takes the row down.
fn prefix_break(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    let est: Value = meclaw_core::serde_json::from_str(&h.state(&format!("window_est:{id}")))
        .unwrap_or(Value::Null);
    est["prefix_break"].clone()
}

fn sent(call: &Msg, turn: &str) -> Sent {
    Sent {
        call: call.clone(),
        turn: turn.to_string(),
        stable: call.messages().len().saturating_sub(1),
    }
}

/// The calls of the conversation: a talk turn, a complex turn that starts a
/// consult (no brain call), a talk turn while it runs, the turn that brings
/// the advice. `t3_system` is the collector's system part of the third turn.
struct Run {
    h: Hive,
    c1: Msg,
    c3: Msg,
    c4: Msg,
    cid: String,
    /// `prefix_break` of the third and the fourth call.
    breaks: (Value, Value),
}

fn run(t3_system: Value) -> Run {
    clear_of_midnight();
    // A day of `consult_pending_ms`: the mark outlives the test.
    let mut h = Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        ("lead", "consult_pending_ms", json!(86_400_000)),
    ])
    .reporting_composite("talky");
    let c1 = ask(
        &mut h,
        "t1",
        "talk",
        json!({}),
        "how are you?",
        mode("Be brief."),
    )
    .expect("a brain call for the first talk turn");
    h.tap(&c1, "stop", warm_usage(), json!([said("Fine.")]));
    let none = ask(
        &mut h,
        "t2",
        "complex",
        json!({}),
        "plan my week",
        mode("Be brief."),
    );
    assert!(
        none.is_none(),
        "check of the setup: the complex turn goes to the consult"
    );
    let errand = h.routed("consult");
    assert_eq!(
        errand.len(),
        1,
        "check of the setup: one consult: {:?}",
        h.out
    );
    let cid = errand[0].hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !cid.is_empty(),
        "check of the setup: the consult has its id"
    );
    let c3 = ask(
        &mut h,
        "t3",
        "talk",
        json!({}),
        "and what about tomorrow?",
        t3_system,
    )
    .expect("a brain call while the consult runs");
    let b3 = prefix_break(&h, &c3);
    h.tap(
        &c3,
        "stop",
        warm_usage(),
        json!([said("It is being looked into.")]),
    );
    let c4 = ask(
        &mut h,
        "t4",
        "complex",
        json!({"consult_id": cid.clone()}),
        "the advice",
        mode("Be brief."),
    )
    .expect("the advice is spoken by the brain");
    let b4 = prefix_break(&h, &c4);
    h.tap(
        &c4,
        "stop",
        warm_usage(),
        json!([said("Here is the plan.")]),
    );
    assert_eq!(
        cut_epoch(&h, &c1),
        cut_epoch(&h, &c4),
        "check of the setup: no rebuild lay between the calls"
    );
    Run {
        h,
        c1,
        c3,
        c4,
        cid,
        breaks: (b3, b4),
    }
}

/// Whether any message of `call` as the model cell got it holds the note.
fn note_in_messages(call: &Msg) -> bool {
    call.messages()
        .iter()
        .any(|m| m["text"].as_str().is_some_and(|t| t.contains(NOTE)))
}

/// Whether the system tree the model holds for `call` holds the note.
fn note_in_system(h: &Hive, call: &Msg) -> bool {
    window_of(h, call)
        .leaves
        .iter()
        .any(|(path, text)| path.ends_with(".pending") || text.contains(NOTE))
}

#[test]
fn gh_d58_prefix_a_consult_between_two_calls_keeps_the_prefix() {
    if !shipped() {
        return;
    }
    let r = run(mode("Be brief."));
    let h = &r.h;

    // Review I2 of the strand: the menu stays over the consult -- the
    // provider sends it before the system part, so a scope that comes with
    // the consult and goes with the advice breaks the whole prefix twice.
    for (name, call) in [("t3", &r.c3), ("t4", &r.c4)] {
        assert_eq!(
            call.body.get("tool_scope"),
            r.c1.body.get("tool_scope"),
            "the tool menu of {name} is the one of t1"
        );
    }
    // K OR-58-197: the consult tool stays on the menu and is refused at the
    // call instead -- the call carries the refusal for its dispatcher, which
    // answers such a call as a tool error without sending it (no second
    // consult; the model gets one more call and speaks, OR-58-207;
    // `dispatcher_template.rs`).
    let refused = |call: &Msg| -> Vec<String> {
        call.hop
            .get("tools_refused")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split(',')
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect()
    };
    assert!(
        refused(&r.c3).contains(&"consult_cogny".to_string()),
        "while the consult runs, the call refuses the consult tool: {:?}",
        r.c3.hop
    );
    assert!(
        refused(&r.c1).is_empty() && refused(&r.c4).is_empty(),
        "no refusal before the consult nor after its advice: {:?} {:?}",
        r.c1.hop,
        r.c4.hop
    );

    // The start of the consult: the call after it begins with the window
    // before it -- the same system tree, the stable messages byte for byte.
    let (a, b) = (sent(&r.c1, "t1"), sent(&r.c3, "t3"));
    if let Err(why) = begins_with(h, &a, &b) {
        panic!("the start of the consult moved the front of the window: {why}");
    }
    let (m1, m3) = (r.c1.messages(), r.c3.messages());
    assert_eq!(
        m3[..a.stable],
        m1[..a.stable],
        "the messages the model cell got begin with the ones before"
    );
    assert_eq!(
        r.breaks.0,
        json!(0),
        "the watch counts no break at the start"
    );

    // The note stands at the tail: in the person's newest message, never in
    // the system part.
    let newest = m3
        .iter()
        .rev()
        .find(|m| m["origin"] == "user")
        .and_then(|m| m["text"].as_str())
        .unwrap_or_default();
    assert!(
        newest.contains(&format!("{NOTE}{}", r.cid))
            && newest.ends_with("and what about tomorrow?"),
        "the note opens the person's newest message: {newest:?}"
    );
    assert!(
        !note_in_system(h, &r.c3),
        "the note is no leaf of the system part"
    );

    // The advice: the note goes, and the front of the window stays.
    let c = sent(&r.c4, "t4");
    if let Err(why) = begins_with(h, &b, &c) {
        panic!("the advice moved the front of the window: {why}");
    }
    assert_eq!(
        r.c4.messages()[..b.stable],
        m3[..b.stable],
        "the messages after the advice begin with the ones before"
    );
    assert_eq!(
        r.breaks.1,
        json!(0),
        "the watch counts no break at the advice"
    );
    assert!(
        !note_in_messages(&r.c4) && !note_in_system(h, &r.c4),
        "the advice takes the note down: {:?}",
        r.c4.messages()
    );
}

/// The watch compares the system part the target holds on every call: a
/// leaf that joins the tree after a call that listed none (the consult,
/// which sends no system part) is a break without a rebuild -- before, the
/// watch saw only a leaf that went, never one that came.
#[test]
fn gh_d58_prefix_a_leaf_that_joins_the_system_counts_as_a_break() {
    if !shipped() {
        return;
    }
    let joined = json!({"instructions": {"mode": {"text": "Be brief."},
                                         "later": {"text": "A line that joined the system."}}});
    let r = run(joined);
    assert!(
        window_of(&r.h, &r.c3)
            .leaves
            .iter()
            .any(|(path, _)| path == "instructions.later"),
        "check of the setup: the leaf joined the tree the model holds"
    );
    assert_eq!(
        r.breaks.0,
        json!(1),
        "a leaf that joins the system part breaks the prefix"
    );
}

/// Two talk turns of session `s1`, the second with the channel's scope
/// `scope2` (the first: `{"deny": ["weather"]}`); the `prefix_break` the
/// watch counted on the second call.
fn menu_break(scope2: Value) -> Value {
    clear_of_midnight();
    let mut h =
        Hive::with(&[("policy", "snapshot_window", json!("1"))]).reporting_composite("talky");
    let mut calls = Vec::new();
    for (turn, words, scope) in [
        ("t1", "how are you?", json!({"deny": ["weather"]})),
        ("t2", "and tomorrow?", scope2),
    ] {
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": "talk"});
        let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "talk"});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user(words)], "system": mode("Be brief."),
                   "tool_scope": scope}),
        );
        let call = h
            .routed("brain")
            .first()
            .cloned()
            .expect("a brain call for each talk turn");
        calls.push((call.clone(), prefix_break(&h, &call)));
        h.tap(&call, "stop", warm_usage(), json!([said("Fine.")]));
    }
    assert_eq!(
        cut_epoch(&h, &calls[0].0),
        cut_epoch(&h, &calls[1].0),
        "check of the setup: no rebuild lay between the calls"
    );
    calls[1].1.clone()
}

/// Review I2 of the strand: the provider sends the tool menu before the
/// system part, so the watch judges the menu the target holds as well -- a
/// menu that changes without a rebuild is a break, the same menu is none.
#[test]
fn gh_d58_prefix_a_menu_that_changes_without_a_rebuild_counts_as_a_break() {
    if !shipped() {
        return;
    }
    assert_eq!(
        menu_break(json!({"deny": ["weather"]})),
        json!(0),
        "the same menu on both calls is no break"
    );
    assert_eq!(
        menu_break(json!({"deny": ["weather", "web_search"]})),
        json!(1),
        "a menu that changes without a rebuild breaks the prefix"
    );
}
