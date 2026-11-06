//! #1382 -- a failed speculation still gives the turn exactly one answer.
//!
//! WHY this file exists: in lab run 2 of rollout R034 the quick voice's spec
//! call ended with `finish_reason error` after 3 001 ms (a provider timeout),
//! and the person got nothing; the answer surfaced minutes later as a dead
//! letter. A failed spec falls back to the stored normal quick call (FU1,
//! #1275); when that call fails as well, the round must go to the brain once
//! (#1352), as a plain quick call does. These pins hold every path of a
//! speculating turn to exactly one answer: spec ok and kept, spec ok and
//! dropped, spec error with the quick call ok, spec error with the quick call
//! failing (a spec timeout is that `error`, `error_code timeout`), and the
//! stored quick call failing late, after the session's next turn built its
//! call. Pin E holds the same late error for a plain quick call with
//! `quick_speculative` "0".

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn spec_hive() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "spec_tools", json!("")),
    ]);
    h.check_emits = true;
    h
}

/// The collector's early curate of the turn (`hop.spec = "1"`, no path);
/// returns the spec call that left for the quick voice.
fn spec_call(h: &mut Hive, turn: &str, text: &str) -> Msg {
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

/// The normal curate of the same turn at the route stamp.
fn seam(h: &mut Hive, turn: &str, path: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
}

/// The spec call's end as talky's `./quick -> ./curator in_spec_llm` edge
/// hands it in (every finish reason, `context.spec = "1"`).
fn spec_end(h: &mut Hive, call: &Msg, finish: &str, extra: Value, messages: Value) {
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

fn escalations(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate' ORDER BY seq")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("JSON"))
        .collect()
}

fn spec_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'quick_spec' ORDER BY seq")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("JSON"))
        .collect()
}

/// Every message the hive sent that answers or asks again: model calls and
/// released spec answers.
fn answers(h: &Hive) -> usize {
    ["quick", "brain", "spec_release"]
        .iter()
        .map(|r| h.routed(r).len())
        .sum()
}

const TIMEOUT: &str = r#"{"error_code": "timeout", "error_source": "wire"}"#;

/// The R034 lab run: the spec call fails after 3 s, the stored quick call
/// fails as well -- one answer through the brain, in both orders of stamp
/// and spec end.
#[test]
fn gh1382_a_failed_speculation_still_yields_one_answer_via_brain() {
    if !shipped() {
        return;
    }
    for stamp_first in [true, false] {
        let mut h = spec_hive();
        let spec = spec_call(&mut h, "t1", "please call my daughter");
        let err: Value = sj::from_str(TIMEOUT).unwrap();
        if stamp_first {
            seam(&mut h, "t1", "simple", "please call my daughter");
            assert_eq!(answers(&h), 0, "the stamp waits for the spec: {:?}", h.out);
            spec_end(&mut h, &spec, "error", err.clone(), json!([]));
        } else {
            spec_end(&mut h, &spec, "error", err.clone(), json!([]));
            assert_eq!(
                answers(&h),
                0,
                "a spec end alone sends nothing: {:?}",
                h.out
            );
            seam(&mut h, "t1", "simple", "please call my daughter");
        }
        let quick = h.routed("quick");
        let brain = h.routed("brain");
        let fallback = if let Some(q) = quick.first() {
            assert_eq!(quick.len(), 1, "one fallback: {:?}", h.out);
            assert!(brain.is_empty(), "not quick AND brain: {:?}", h.out);
            assert!(q.hop.get("spec").is_none());
            Some(q.clone())
        } else {
            None
        };
        if let Some(fb) = fallback {
            h.out.clear();
            h.tap(&fb, "error", err.clone(), json!([]));
        }
        let brain = h.routed("brain");
        assert_eq!(
            brain.len(),
            1,
            "stamp_first={stamp_first}: the failed turn goes to the brain: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert_eq!(answers(&h), 1, "exactly one: {:?}", h.out);
        let shown = sj::to_string(&brain[0].body["messages"]).unwrap();
        assert!(shown.contains("call my daughter"), "{shown}");
        let marks = escalations(&h);
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert_eq!(marks[0]["reason"], json!("no_answer"));
        let sm = spec_marks(&h);
        assert_eq!(sm.len(), 1, "{sm:?}");
        // The ledger says why the spec was dropped: its finish and error code.
        assert_eq!(sm[0]["verdict"], json!("dropped"));
        assert_eq!(
            sm[0]["spec_error"],
            json!({"finish": "error", "error_code": "timeout"}),
            "{sm:?}"
        );
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }
}

/// The stored quick call of a failed spec, and the turn's normal curate that
/// stored it (stamp first, then the spec's error).
fn stored_fallback(h: &mut Hive, turn: &str, text: &str) -> Msg {
    let spec = spec_call(h, turn, text);
    seam(h, turn, "simple", text);
    let err: Value = sj::from_str(TIMEOUT).unwrap();
    spec_end(h, &spec, "error", err, json!([]));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "the stored call goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    quick[0].clone()
}

/// Pin B: the spec fails, the stored quick call answers -- that one answer,
/// no second call, no brain, and its `-esc` copy goes with it.
#[test]
fn gh1382_b_failed_speculation_falls_back_to_the_stored_quick_call_once() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let fb = stored_fallback(&mut h, "t1", "what time is it");
    let call = fb.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !h.state(&format!("pending:{call}-esc")).is_empty(),
        "win kept the copy"
    );
    h.out.clear();
    h.tap(&fb, "stop", json!({}), json!([said("It is noon.")]));
    assert_eq!(answers(&h), 0, "nothing asks again: {:?}", h.out);
    assert!(escalations(&h).is_empty(), "{:?}", escalations(&h));
    assert_eq!(h.state(&format!("pending:{call}-esc")), "", "the copy goes");
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// The `./quick` edges of talky 6.13.0, byte for byte (Review Focus 2: a turn
/// without speculation is untouched by #1382).
const QUICK_EDGES_6_13_0: [(&str, &str); 5] = [
    (
        "./splitter",
        "has(hop.finish_reason) && (hop.finish_reason == 'stop' || hop.finish_reason == 'tool_calls' || hop.finish_reason == 'length') && !(has(context.spec) && context.spec == '1')",
    ),
    (
        "./curator",
        "has(hop.finish_reason) && !has(hop.refused_subscriber) && !(has(context.spec) && context.spec == '1')",
    ),
    (
        "./curator",
        "has(hop.finish_reason) && !has(hop.refused_subscriber) && has(context.spec) && context.spec == '1'",
    ),
    (
        "./errors",
        "has(hop.finish_reason) && (hop.finish_reason == 'error' || hop.finish_reason == 'content_filter') && !has(hop.refused_subscriber) && !(has(context.interim) && context.interim == 'tool') && !(has(context.curator_call) && context.curator_call != '' && !context.curator_call.endsWith('-bridge')) && !(has(context.spec) && context.spec == '1')",
    ),
    (".", "has(hop.refused_subscriber)"),
];

fn quick_edges(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            if m.get("from") == Some(&json!("./quick")) && m.contains_key("to") {
                out.push((
                    m["to"].as_str().unwrap_or_default().to_string(),
                    m.get("condition")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ));
            }
            m.values().for_each(|x| quick_edges(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| quick_edges(x, out)),
        _ => {}
    }
}

/// Pin C: a kept speculation is unchanged -- the spec answer is released, no
/// call of its own, no escalation; and the `./quick` edges are 6.13.0's.
#[test]
fn gh1382_c_a_kept_speculation_is_unchanged() {
    if !shipped() {
        return;
    }
    let mut edges = Vec::new();
    quick_edges(&read_json(&repo("templates/talky/config.json")), &mut edges);
    // GH #1411 came after 6.13.0: the rim edge that hands a streamed sentence
    // (`partial`) out of talky. It is not part of the lock, but it must keep
    // the promise the lock stands for -- a speculation sends nothing out.
    let partial = |c: &str| c.starts_with("has(hop.route) && hop.route == 'partial'");
    let rim: Vec<(String, String)> = edges.iter().filter(|(_, c)| partial(c)).cloned().collect();
    assert_eq!(rim.len(), 1, "one partial rim edge out of ./quick: {rim:?}");
    assert_eq!(rim[0].0, ".", "{rim:?}");
    assert!(
        rim[0]
            .1
            .ends_with("!(has(context.spec) && context.spec == '1')")
            && rim[0].1.contains("!has(hop.finish_reason)"),
        "a speculation never streams out: {rim:?}"
    );
    edges.retain(|(_, c)| !partial(c));
    let want: Vec<(String, String)> = QUICK_EDGES_6_13_0
        .iter()
        .map(|(t, c)| (t.to_string(), c.to_string()))
        .collect();
    assert_eq!(edges, want, "the ./quick edges moved");
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1", "hello there");
    spec_end(&mut h, &spec, "stop", json!({}), json!([said("Hello!")]));
    seam(&mut h, "t1", "simple", "hello there");
    assert_eq!(
        h.routed("spec_release").len(),
        1,
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        answers(&h),
        1,
        "the spec answer and nothing else: {:?}",
        h.out
    );
    let marks = spec_marks(&h);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["verdict"], json!("kept"));
    assert!(
        marks[0].get("spec_error").is_none(),
        "a kept spec names no error: {marks:?}"
    );
    assert!(escalations(&h).is_empty());
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin D (R034 lab run 2, hole 1; the plan's `…_a_speculation_timeout_is_an_error_not_a_hang`):
/// not the spec's timeout (that is pin A, `error_code timeout`) but the
/// STORED quick call's: it comes back with a timeout `error` only after the
/// session's next turn built its call, which took down the stored call's
/// projection (`f-est-old`). The late `error` still sends the old turn to the
/// brain, once, from its `-esc` copy; before, `tap` read no way and the turn
/// ended in silence.
#[test]
fn gh1382_d_a_stored_calls_late_timeout_still_escalates_once() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let fb = stored_fallback(&mut h, "t1", "please call my daughter");
    let call = fb.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let _next = spec_call(&mut h, "t2", "what is the weather");
    assert_eq!(
        h.state(&format!("window_est:{call}")),
        "",
        "the next call took the projection"
    );
    h.out.clear();
    let err: Value = sj::from_str(TIMEOUT).unwrap();
    h.tap(&fb, "error", err, json!([]));
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "the late error goes to the brain: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(answers(&h), 1, "exactly one: {:?}", h.out);
    assert_eq!(brain[0].hop["turn_id"], json!("t1"), "its own turn");
    let shown = sj::to_string(&brain[0].body["messages"]).unwrap();
    assert!(shown.contains("call my daughter"), "{shown}");
    let marks = escalations(&h);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["reason"], json!("no_answer"));
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin E (review I1): the `tap` switch of #1382 asks no speculation -- it is
/// the one effect of this fix while a set runs with `quick_speculative` "0".
/// A plain quick call whose projection fell (here: the session's next turn
/// built its call, `f-est-old`) and that then ends in an `error` goes to the
/// brain once, from its `-esc` copy; before #1382 `tap` read no way and the
/// turn ended in silence. With an answer it stays quick and its copy goes.
#[test]
fn gh1382_e_a_plain_quick_call_whose_projection_fell_still_escalates_once() {
    if !shipped() {
        return;
    }
    for (finish, extra, said_text) in [
        ("error", sj::from_str::<Value>(TIMEOUT).unwrap(), None),
        ("stop", json!({}), Some("Who should I call?")),
    ] {
        let mut h = Hive::with(&[
            ("policy", "role", json!("talky")),
            ("policy", "quick_speculative", json!("0")),
        ]);
        h.check_emits = true;
        seam(&mut h, "t1", "simple", "please call my daughter");
        let quick = h.routed("quick");
        assert_eq!(quick.len(), 1, "one quick call: {:?} {:?}", h.out, h.stderr);
        assert!(quick[0].hop.get("spec").is_none(), "no speculation");
        let first = quick[0].clone();
        let call = first.hop["curator_call"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        seam(&mut h, "t2", "simple", "what is the weather");
        assert_eq!(
            h.state(&format!("window_est:{call}")),
            "",
            "the next call took the projection: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert!(
            !h.state(&format!("pending:{call}-esc")).is_empty(),
            "the copy outlives the projection"
        );
        h.out.clear();
        let messages = match said_text {
            Some(t) => json!([said(t)]),
            None => json!([]),
        };
        h.tap(&first, finish, extra, messages);
        let brain = h.routed("brain");
        if finish == "error" {
            assert_eq!(
                brain.len(),
                1,
                "the late error goes to the brain: {:?} {:?}",
                h.out,
                h.stderr
            );
            assert_eq!(answers(&h), 1, "exactly one: {:?}", h.out);
            assert_eq!(brain[0].hop["turn_id"], json!("t1"), "its own turn");
            let marks = escalations(&h);
            assert_eq!(marks.len(), 1, "{marks:?}");
            assert_eq!(marks[0]["reason"], json!("no_answer"));
        } else {
            assert_eq!(answers(&h), 0, "nothing asks again: {:?}", h.out);
            assert!(escalations(&h).is_empty(), "{:?}", escalations(&h));
        }
        assert_eq!(h.state(&format!("pending:{call}-esc")), "", "the copy goes");
        assert!(
            spec_marks(&h).is_empty(),
            "no speculation, no quick_spec mark"
        );
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }
}
