//! #1275 -- the quick voice speaks before the route stamp, the curator
//! decides at the stamp.
//!
//! WHY this file exists: a `simple` turn waited for the route stamp (the route
//! model) before the quick voice was even asked, so the stamp's latency stood
//! in front of every fast answer. With `quick_speculative` on, the collector
//! sends the SAME curate of the turn early (`hop.spec = "1"`, no path), the
//! curator asks the quick voice at once and HOLDS its answer; at the normal
//! curate (the stamp) it keeps or drops it. A held answer never reaches the
//! splitter, the wall or the history unless it is kept, a dropped one never
//! at all, and a turn never gets two answers or none. Off (the default), a
//! spec curate is parked and nothing else changes.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

fn spec_hive(spec_tools: &str) -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "spec_tools", json!(spec_tools)),
    ]);
    h.check_emits = true;
    h
}

/// The early curate of the collector: the same turn, `spec: "1"`, no path.
fn spec_curate(h: &mut Hive, turn: &str, body: Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, body);
}

fn spec_call(h: &mut Hive, turn: &str, text: &str) -> Msg {
    spec_curate(h, turn, json!({"messages": [user(text)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    assert_eq!(quick[0].hop["spec"], json!("1"), "the spec call says so");
    assert!(h.routed("brain").is_empty());
    quick[0].clone()
}

/// The normal seam of the same turn, at the stamp (or at the expiry: `path` "").
fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
}

/// The quick voice's answer to a spec call, as talky's `./quick -> ./curator`
/// edge hands it in: the call's context with `spec`, lane `in_spec_llm`.
fn spec_answer(h: &mut Hive, call: &Msg, finish: &str, messages: Value) {
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        json!({"finish_reason": finish, "model": "test-model", "tokens_prompt": 10}),
        json!({"messages": messages}),
    );
}

/// No model call and no answer left the hive.
fn silent(h: &Hive) -> bool {
    ["quick", "brain", "spec_release"]
        .iter()
        .all(|r| h.routed(r).is_empty())
}

fn spec_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'quick_spec' ORDER BY seq")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("JSON"))
        .collect()
}

fn in_blocks(h: &Hive, needle: &str) -> i64 {
    h.rows(&format!(
        "SELECT count(*) FROM blocks WHERE body LIKE '%{needle}%'"
    ))[0][0]
        .as_i64()
        .unwrap()
}

fn anywhere_out(h: &Hive, needle: &str) -> bool {
    h.out
        .iter()
        .any(|m| sj::to_string(&m.body).unwrap().contains(needle))
}

#[test]
fn spec_quick_dropped_on_talk() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("");
    let call = spec_call(&mut h, "t1", "how are you?");
    spec_answer(&mut h, &call, "stop", json!([said("SPECTEXT fine")]));
    assert!(h.out.is_empty(), "a held answer goes nowhere: {:?}", h.out);
    seam(&mut h, "t1", "talk", "", "how are you?");
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(h.routed("spec_release").is_empty());
    assert!(!anywhere_out(&h, "SPECTEXT"), "{:?}", h.out);
    assert_eq!(in_blocks(&h, "SPECTEXT"), 0, "the wall never holds it");
    let m = spec_marks(&h);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["verdict"], json!("dropped"));
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn spec_quick_kept_on_simple_with_tool_call() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web");
    let call = spec_call(&mut h, "t1", "news of today?");
    let spec_id = call.hop["curator_call"].as_str().unwrap().to_string();
    spec_answer(
        &mut h,
        &call,
        "tool_calls",
        json!([tool_call("c1", "web_search")]),
    );
    assert!(h.out.is_empty(), "{:?}", h.out);
    seam(&mut h, "t1", "simple", "tool", "news of today?");
    assert!(
        h.routed("quick").is_empty(),
        "no second quick call: {:?}",
        h.out
    );
    assert!(h.routed("brain").is_empty());
    let rel = h.routed("spec_release");
    assert_eq!(rel.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(rel[0].hop["curator_call"], json!(spec_id));
    assert_eq!(rel[0].hop["finish_reason"], json!("tool_calls"));
    assert_eq!(rel[0].hop["turn_id"], json!("t1"));
    assert_eq!(rel[0].messages()[0]["id"], json!("c1"));
    // The tap ran on the released answer like on a quick answer.
    let model = h.rows(&format!(
        "SELECT model FROM calls WHERE call_id = '{spec_id}'"
    ));
    assert_eq!(model[0][0], json!("test-model"), "the tap booked it");
    let m = spec_marks(&h);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["verdict"], json!("kept"));
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn spec_quick_without_tool_dropped_when_needs_tool() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web");
    let call = spec_call(&mut h, "t1", "weather in Berlin?");
    spec_answer(&mut h, &call, "stop", json!([said("SPECGUESS sunny")]));
    seam(&mut h, "t1", "simple", "tool", "weather in Berlin?");
    let quick = h.routed("quick");
    assert!(
        quick
            .iter()
            .any(|q| q.hop.get("interim") == Some(&json!("tool"))
                && q.body["tool_scope"]["require"] == json!(true)),
        "the tool round starts: {quick:?}"
    );
    assert!(h.routed("spec_release").is_empty());
    assert!(!anywhere_out(&h, "SPECGUESS"));
    assert_eq!(in_blocks(&h, "SPECGUESS"), 0);
    assert_eq!(spec_marks(&h)[0]["verdict"], json!("dropped"));
}

#[test]
fn spec_quick_scope_is_spec_tools_only() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web");
    spec_curate(
        &mut h,
        "t1",
        json!({"messages": [user("hi")],
               "tool_scope": {"allow": ["web", "orga", "weather"], "deny": ["bash"]}}),
    );
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        quick[0].body["tool_scope"],
        json!({"allow": ["web"], "deny": ["bash"]}),
        "spec_tools only: no orga, no quick_tools, no require, no hint"
    );
    let call = quick[0].hop["curator_call"].as_str().unwrap().to_string();
    assert!(
        h.state(&format!("pending:{call}-esc")).is_empty(),
        "no -esc copy"
    );
    assert!(
        h.rows("SELECT seq FROM marks WHERE kind = 'route'")
            .is_empty(),
        "no route mark"
    );

    let mut none = spec_hive("");
    spec_curate(&mut none, "t1", json!({"messages": [user("hi")]}));
    assert_eq!(
        none.routed("quick")[0].body["tool_scope"],
        json!({"allow": []}),
        "spec_tools empty: no tool at all"
    );
}

#[test]
fn spec_write_tool_never_runs_on_talk() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web");
    let call = spec_call(&mut h, "t1", "note that I called mum");
    spec_answer(
        &mut h,
        &call,
        "tool_calls",
        json!([tool_call("cw1", "orga_write")]),
    );
    assert!(h.out.is_empty());
    seam(&mut h, "t1", "talk", "", "note that I called mum");
    assert_eq!(h.routed("brain").len(), 1);
    assert!(h.routed("spec_release").is_empty());
    assert!(!anywhere_out(&h, "cw1"), "{:?}", h.out);
    assert!(!anywhere_out(&h, "orga_write"));
}

#[test]
fn spec_quick_dropped_when_stamp_never_comes() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("");
    let call = spec_call(&mut h, "t1", "hello");
    spec_answer(&mut h, &call, "stop", json!([said("SPECLATE hi")]));
    seam(&mut h, "t1", "", "", "hello");
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(h.routed("spec_release").is_empty());
    assert_eq!(in_blocks(&h, "SPECLATE"), 0);
    assert_eq!(spec_marks(&h)[0]["verdict"], json!("dropped"));
}

#[test]
fn spec_await_releases_on_arrival() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("");
    let call = spec_call(&mut h, "t1", "hello");
    seam(&mut h, "t1", "simple", "", "hello");
    assert!(silent(&h), "the stamp came first: nothing yet: {:?}", h.out);
    spec_answer(&mut h, &call, "stop", json!([said("SPECHI there")]));
    let rel = h.routed("spec_release");
    assert_eq!(rel.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(h.routed("quick").is_empty() && h.routed("brain").is_empty());
    assert_eq!(rel[0].messages()[0]["text"], json!("SPECHI there"));
    assert_eq!(spec_marks(&h)[0]["verdict"], json!("kept"));
    assert!(in_blocks(&h, "SPECHI") >= 1, "a kept answer is on the wall");
}

#[test]
fn spec_await_sends_the_stored_fallback() {
    if !shipped() {
        return;
    }
    // Needs a tool, the answer has none: the stored normal call goes.
    let mut h = spec_hive("web");
    let call = spec_call(&mut h, "t1", "weather?");
    seam(&mut h, "t1", "simple", "tool", "weather?");
    assert!(silent(&h), "{:?}", h.out);
    spec_answer(&mut h, &call, "stop", json!([said("SPECNOPE")]));
    assert!(h.routed("spec_release").is_empty());
    let quick = h.routed("quick");
    assert!(
        quick
            .iter()
            .any(|q| q.body["tool_scope"]["require"] == json!(true)),
        "the stored tool round goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(!anywhere_out(&h, "SPECNOPE"));
    assert_eq!(spec_marks(&h)[0]["verdict"], json!("dropped"));

    // An answer that failed is no answer: the fallback goes as well.
    let mut e = spec_hive("");
    let call = spec_call(&mut e, "t1", "hello");
    seam(&mut e, "t1", "simple", "", "hello");
    spec_answer(&mut e, &call, "error", json!([]));
    assert_eq!(e.routed("quick").len(), 1, "{:?} {:?}", e.out, e.stderr);
    assert!(e.routed("quick")[0].hop.get("spec").is_none());
    assert!(e.routed("spec_release").is_empty());
}

#[test]
fn spec_two_turns_keep_their_own_answers() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("");
    let a = spec_call(&mut h, "t1", "first");
    seam(&mut h, "t1", "talk", "", "first");
    assert_eq!(h.routed("brain").len(), 1);
    let b = spec_call(&mut h, "t2", "second");
    spec_answer(&mut h, &a, "stop", json!([said("SPECOLD")]));
    assert!(h.out.is_empty(), "a late spec of an old turn is dropped");
    spec_answer(&mut h, &b, "stop", json!([said("SPECNEW")]));
    seam(&mut h, "t2", "simple", "", "second");
    let rel = h.routed("spec_release");
    assert_eq!(rel.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(rel[0].hop["curator_call"], b.hop["curator_call"]);
    assert_eq!(rel[0].hop["turn_id"], json!("t2"));
    assert!(!anywhere_out(&h, "SPECOLD"));
    assert_eq!(in_blocks(&h, "SPECOLD"), 0);
}

#[test]
fn spec_off_parks_a_spec_curate() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    for (k, v) in [("quick_speculative", "0"), ("spec_tools", "")] {
        assert_eq!(policy["params"][k], json!(v), "{k}");
        assert_eq!(
            policy["contract"]["settings"][k]["default"],
            json!(v),
            "{k}"
        );
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    spec_curate(&mut h, "t1", json!({"messages": [user("hi")]}));
    assert!(
        h.routed("quick").is_empty() && h.routed("brain").is_empty(),
        "parked, never a call: {:?}",
        h.out
    );
    assert!(
        h.stderr.iter().any(|s| s.contains("spec")),
        "{:?}",
        h.stderr
    );
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'pending:%'")
            .is_empty(),
        "nothing left parked"
    );
    // On, a turn without a spec curate goes its way as before.
    let mut on = spec_hive("");
    seam(&mut on, "t1", "simple", "", "hi");
    assert_eq!(on.routed("quick").len(), 1, "{:?} {:?}", on.out, on.stderr);
}

/// talky's own edges, evaluated by the colony's CEL.
fn talky_edges(from: &str, to: &str) -> Vec<Value> {
    let cfg = read_json(&repo("templates/talky/config.json"));
    cfg["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["from"] == from && e["to"] == to)
        .cloned()
        .collect()
}

/// `(passes, context, hop)` of one edge for a message.
type EdgeOutcome = (bool, Map<String, Value>, Map<String, Value>);

/// `(passes, context, hop)` of each edge `from -> to` for a message.
fn through(from: &str, to: &str, ctx: Value, hop: Value) -> Vec<EdgeOutcome> {
    let (ctx, hop) = (obj(ctx), obj(hop));
    talky_edges(from, to)
        .iter()
        .map(|e| {
            let pass = e["condition"].as_str().is_none_or(|c| {
                matches!(
                    evaluate_condition(&parse_condition(c).unwrap(), &ctx, &hop),
                    Ok(true)
                )
            });
            if !pass || !e["modifier"].is_object() {
                return (pass, ctx.clone(), hop.clone());
            }
            let spec: ModifierSpec = sj::from_value(e["modifier"].clone()).unwrap();
            let h = apply_modifier(
                &parse_modifier(&spec).unwrap(),
                &Headers::from_parts(ctx.clone(), hop.clone()),
            )
            .expect("the modifier applies");
            (true, h.context, h.hop)
        })
        .collect()
}

fn passing(v: &[EdgeOutcome]) -> usize {
    v.iter().filter(|x| x.0).count()
}

#[test]
fn spec_talky_edges_keep_a_spec_answer_inside() {
    if !shipped() {
        return;
    }
    let spec = json!({"curator_call": "c1", "spec": "1"});
    let plain = json!({"curator_call": "c1"});
    let empty = json!({"curator_call": "c1", "spec": ""});
    for finish in ["stop", "tool_calls", "length"] {
        let hop = json!({"finish_reason": finish});
        assert_eq!(
            passing(&through("./quick", "./splitter", spec.clone(), hop.clone())),
            0
        );
        assert_eq!(
            passing(&through(
                "./quick",
                "./splitter",
                plain.clone(),
                hop.clone()
            )),
            1
        );
        assert_eq!(
            passing(&through(
                "./quick",
                "./splitter",
                empty.clone(),
                hop.clone()
            )),
            1
        );
    }
    let stop = json!({"finish_reason": "stop"});
    let to_cur = through("./quick", "./curator", spec.clone(), stop.clone());
    assert_eq!(passing(&to_cur), 1, "only the spec lane");
    let lane = to_cur.iter().find(|x| x.0).unwrap();
    assert_eq!(lane.2["route"], json!("in_spec_llm"));
    let plain_cur = through("./quick", "./curator", plain.clone(), stop.clone());
    assert_eq!(passing(&plain_cur), 1);
    assert_eq!(
        plain_cur.iter().find(|x| x.0).unwrap().2["route"],
        json!("in_llm")
    );
    let err = json!({"finish_reason": "error"});
    assert_eq!(
        passing(&through("./quick", "./errors", spec.clone(), err.clone())),
        0
    );
    let err_ctx = json!({"curator_call": "c1-bridge"});
    assert_eq!(
        passing(&through("./quick", "./errors", err_ctx, err)),
        1,
        "unchanged without spec"
    );

    // The curator's quick call lifts `hop.spec`; a brain call clears it.
    let call = |extra: Value| {
        let mut h = json!({"route": "quick", "iter": "0", "curator_call": "c1",
                           "turn_id": "t1", "session_id": "s1"});
        for (k, v) in obj(extra) {
            h[k] = v;
        }
        h
    };
    for e in through(
        "./curator",
        "./quick",
        json!({}),
        call(json!({"spec": "1"})),
    ) {
        if e.0 {
            assert_eq!(e.1["spec"], json!("1"));
        }
    }
    for e in through("./curator", "./quick", json!({}), call(json!({}))) {
        if e.0 {
            assert_eq!(e.1["spec"], json!(""));
        }
    }
    let mut brain = call(json!({}));
    brain["route"] = json!("brain");
    for e in through("./curator", "./brain", json!({"spec": "1"}), brain) {
        if e.0 {
            assert_eq!(e.1["spec"], json!(""));
        }
    }
    // The release reaches the splitter with the call's own context.
    let rel = through(
        "./curator",
        "./splitter",
        json!({"spec": "1", "interim": "tool"}),
        json!({"route": "spec_release", "finish_reason": "stop", "curator_call": "c1",
               "turn_id": "t1", "session_id": "s1", "iter": "0", "episode_turn_id": "e1"}),
    );
    assert_eq!(passing(&rel), 1);
    let (_, c, _) = rel.iter().find(|x| x.0).unwrap();
    assert_eq!(c["curator_call"], json!("c1"));
    assert_eq!(c["turn_id"], json!("t1"));
    assert_eq!(c["episode_turn_id"], json!("e1"));
    assert_eq!(c["interim"], json!(""));
    assert_eq!(c["spec"], json!(""));
}

/// The normal seam of a turn whose stamp names its tool `families`.
fn seam_families(h: &mut Hive, turn: &str, needs: &str, families: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "simple", "needs": needs, "families": families});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
}

/// The normal quick round of the turn went (not the spec call), and its scope
/// holds the stamped family the spec call could not reach.
fn normal_round_with(h: &Hive, family: &str) -> bool {
    h.routed("quick").iter().any(|q| {
        q.hop.get("spec").is_none()
            && q.body["tool_scope"]["allow"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(family)))
    })
}

// #1366 (OR-LV-2): the stamp needs `orga` (a family outside `spec_tools`), the
// spec call ran `weather` -- the PIN-B repro. Before the fix the answer was
// kept because it called SOME tool, and the needed orga tool never ran.
#[test]
fn spec_quick_dropped_when_a_stamped_family_lies_outside_spec_tools() {
    if !shipped() {
        return;
    }
    // Held first, then the stamp.
    let mut h = spec_hive("web,weather,story_read");
    let call = spec_call(&mut h, "t1", "move my dentist to friday");
    spec_answer(
        &mut h,
        &call,
        "tool_calls",
        json!([tool_call("cw", "weather")]),
    );
    seam_families(&mut h, "t1", "tool", "orga", "move my dentist to friday");
    assert!(
        h.routed("spec_release").is_empty(),
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(normal_round_with(&h, "orga"), "{:?} {:?}", h.out, h.stderr);
    let m = spec_marks(&h);
    assert_eq!(m[0]["verdict"], json!("dropped"), "{m:?}");
    assert_eq!(m[0]["reason"], json!("family:orga"), "{m:?}");

    // The stamp first, the answer after it. Until #1398 the stored call waited
    // for the spec's end here (the stamp was silent). The drop is known at the
    // stamp -- `orga` lies outside `spec_tools` -- so the stored call and its
    // bridge go at once and the late answer sends nothing (#1398, plan
    // T2-core-voice § V1, pin 3: the wait cost +1.3 s p50 in the t1 breakdown).
    let mut a = spec_hive("web,weather,story_read");
    let call = spec_call(&mut a, "t1", "move my dentist to friday");
    seam_families(&mut a, "t1", "tool", "orga", "move my dentist to friday");
    assert!(normal_round_with(&a, "orga"), "{:?} {:?}", a.out, a.stderr);
    assert!(
        a.routed("quick")
            .iter()
            .any(|q| q.hop.get("spec") == Some(&json!("bridge"))),
        "the bridge goes with the call: {:?}",
        a.out
    );
    assert!(a.routed("spec_release").is_empty(), "{:?}", a.out);
    assert_eq!(spec_marks(&a)[0]["reason"], json!("family:orga"));
    spec_answer(
        &mut a,
        &call,
        "tool_calls",
        json!([tool_call("cw", "weather")]),
    );
    assert!(
        silent(&a),
        "the late answer sends nothing: {:?} {:?}",
        a.out,
        a.stderr
    );
    assert_eq!(spec_marks(&a).len(), 1, "one decision per turn");
    assert_eq!(
        a.rows("SELECT count(*) FROM marks WHERE kind = 'late_spec_discarded'")[0][0],
        json!(1),
        "the late answer is discarded once"
    );
}

// #1366: a stamped family inside `spec_tools` that the spec call did not call
// is the same gap -- the tool the turn needs never ran.
#[test]
fn spec_quick_dropped_when_a_stamped_family_was_not_called() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather,story_read");
    let call = spec_call(&mut h, "t1", "weather and news?");
    spec_answer(
        &mut h,
        &call,
        "tool_calls",
        json!([tool_call("cw", "weather")]),
    );
    seam_families(&mut h, "t1", "tool", "weather,web", "weather and news?");
    assert!(
        h.routed("spec_release").is_empty(),
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(normal_round_with(&h, "web"), "{:?} {:?}", h.out, h.stderr);
    assert_eq!(spec_marks(&h)[0]["reason"], json!("family:web"));
}

// #1366, the counter case: the stamped family is the one the spec call ran.
#[test]
fn spec_quick_kept_when_it_called_the_stamped_family() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather,story_read");
    let call = spec_call(&mut h, "t1", "weather in Berlin?");
    spec_answer(
        &mut h,
        &call,
        "tool_calls",
        json!([tool_call("cw", "weather")]),
    );
    seam_families(&mut h, "t1", "tool", "weather", "weather in Berlin?");
    assert!(h.routed("quick").is_empty(), "{:?}", h.out);
    assert_eq!(
        h.routed("spec_release").len(),
        1,
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(spec_marks(&h)[0]["verdict"], json!("kept"));
}

// #1371 (lab_zk): the spec curate of a turn's first call claims the turn's row
// (`fired`) and goes through `./handover` (a new session); the normal curate
// (stamp simple, needs tool, families orga) comes in while it is there, builds
// its call, stores it as the fallback and moves the row to `await`. The spec
// curate, back from the handover, saw the stored call's `calls` row, took it
// for a decided turn ("after its call -- no call") and dropped the row: the
// stored call never went and no quick round ran at all (PIN-B lab, trace
// 01a18e1c-24a1). The turn gets exactly one normal quick round.
#[test]
fn spec_curate_back_from_handover_leaves_the_turn_one_normal_round() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather,story_read");
    let text = "move my dentist to friday";
    h.lane_at_phase(
        "handover-done",
        "in_curate",
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
               "channel": "test", "audience_set": ROUND_E, "path": "simple"}),
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
               "path": "simple", "needs": "tool", "families": "orga"}),
        json!({"messages": [user(text)]}),
    );
    // Until #1398 the stored call waited here for the spec call's answer.
    // The stamp's `orga` lies outside `spec_tools`, so the drop is known at the
    // stamp: the normal call and its bridge go in this very step, and the spec
    // curate back from the handover finds a decided turn and asks nothing
    // (#1398, plan T2-core-voice § V1, pin 3). Still one normal round, never none.
    let mut quick = h.routed("quick");
    spec_curate(&mut h, "t1", json!({"messages": [user(text)]}));
    quick.extend(h.routed("quick"));
    assert!(
        quick.iter().all(|q| q.hop.get("spec") != Some(&json!("1"))),
        "no spec call for a decided turn: {quick:?}"
    );
    let mut rounds: Vec<String> = quick
        .iter()
        .map(|q| {
            let c = q.hop["curator_call"].as_str().unwrap_or("");
            c.trim_end_matches("-bridge").to_string()
        })
        .collect();
    rounds.dedup();
    assert_eq!(
        rounds.len(),
        1,
        "one normal quick round: {quick:?} {:?}",
        h.stderr
    );
    assert!(
        quick.iter().any(|q| q.hop.get("spec").is_none()
            && q.body["tool_scope"]["allow"]
                .as_array()
                .is_some_and(|a| a.contains(&json!("orga")))),
        "{quick:?}"
    );
    assert!(h.routed("spec_release").is_empty(), "{:?}", h.out);
    assert_eq!(spec_marks(&h)[0]["reason"], json!("family:orga"));
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'spec%'")
            .is_empty(),
        "no spec row left behind"
    );
}

// #1371, the await case (review I1 of #1398): with a stamped family INSIDE
// `spec_tools` the drop is not known at the stamp, so the normal curate that
// comes while the spec curate is in `./handover` still stores its call as the
// fallback and moves the row to `await`. The spec curate back from the
// handover must not take that recorded fallback for a decided turn (the
// `state != "await"` guard): its spec call goes, the answer (a weather call,
// not web) settles the turn, and the stored call goes -- one normal round.
// Without the guard the row was dropped and no quick round ran at all (PIN-B
// lab_zk, trace 01a18e1c-24a1). Since #1398 the `orga` test above never
// reaches the guard; this one is its only pin.
#[test]
fn spec_curate_back_from_handover_still_awaits_a_family_inside_spec_tools() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather,story_read");
    let text = "weather and news?";
    h.lane_at_phase(
        "handover-done",
        "in_curate",
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
               "channel": "test", "audience_set": ROUND_E, "path": "simple"}),
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
               "path": "simple", "needs": "tool", "families": "web"}),
        json!({"messages": [user(text)]}),
    );
    assert!(
        h.routed("quick").is_empty(),
        "a family inside spec_tools waits for the spec: {:?}",
        h.out
    );
    spec_curate(&mut h, "t1", json!({"messages": [user(text)]}));
    let mut quick = h.routed("quick");
    let spec: Vec<Msg> = quick
        .iter()
        .filter(|q| q.hop.get("spec") == Some(&json!("1")))
        .cloned()
        .collect();
    assert_eq!(
        spec.len(),
        1,
        "the spec call goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    // Its answer (a weather call, not web) decides: the stored call goes.
    spec_answer(
        &mut h,
        &spec[0],
        "tool_calls",
        json!([tool_call("cw", "weather")]),
    );
    quick.extend(h.routed("quick"));
    let mut rounds: Vec<String> = quick
        .iter()
        .filter(|q| q.hop.get("spec") != Some(&json!("1")))
        .map(|q| {
            let c = q.hop["curator_call"].as_str().unwrap_or("");
            c.trim_end_matches("-bridge").to_string()
        })
        .collect();
    rounds.dedup();
    assert_eq!(
        rounds.len(),
        1,
        "one normal quick round: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(normal_round_with(&h, "web"), "{:?} {:?}", h.out, h.stderr);
    assert!(h.routed("spec_release").is_empty(), "{:?}", h.out);
    assert_eq!(spec_marks(&h)[0]["reason"], json!("family:web"));
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'spec%'")
            .is_empty(),
        "no spec row left behind"
    );
}
