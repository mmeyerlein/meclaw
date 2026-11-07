//! GH #1447 (D38 C1, R-38-4 Nr. 4) -- the lead names the way and the target.
//!
//! WHY this file exists: the switch of the curator -- which way a call leaves
//! on (`path_routes`, `way_of`), the memory rule (`memory_to_brain`), the
//! speculation's verdict, the escalation of the quick voice, the clarify
//! turn, the bridge and the live feed -- lived spread over ~1 830 lines of
//! `curator/policy`, interleaved with the window it shapes (`win` 7005 ->
//! 7612). R-38-4 takes it out into a cell of its own, `curator/lead`, with
//! a narrow contract: in, the route stamp (`path`, `needs`, `families`) and
//! the turn's head (channel, engine, session, call id, turn, tool iteration,
//! consult id, spec, escalation); out, one verdict
//! `{way, target, reason, spec, bridge, clarify, live}`. This table pins the
//! verdict of the pure decision `lead_verdict(stamp, head, state)` of the
//! shipped lead script for every way the C0 corpus records -- the same cases
//! the policy decided before the move, so the move changes no way.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The seven fields of the verdict the table pins, in this order.
const FIELDS: [&str; 7] = [
    "way", "target", "reason", "spec", "bridge", "clarify", "live",
];

fn head(extra: Value) -> Value {
    let mut h = json!({"channel": "text", "engine": "", "session": "s1", "call_id": "",
                       "turn": "t1", "iter": 0, "consult_id": "", "consult_class": "",
                       "spec": "", "escalate": ""});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        h[k] = v;
    }
    h
}

fn stamp(path: &str, needs: Value, families: Value) -> Value {
    json!({"path": path, "needs": needs, "families": families})
}

fn no_state() -> Value {
    json!({"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}})
}

fn state(extra: Value) -> Value {
    let mut s = no_state();
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        s[k] = v;
    }
    s
}

fn verdict(way: &str, target: &str, reason: &str, spec: &str, bridge: &str) -> Value {
    json!({"way": way, "target": target, "reason": reason, "spec": spec, "bridge": bridge,
           "clarify": false, "live": false})
}

fn with(mut v: Value, k: &str, x: Value) -> Value {
    v[k] = x;
    v
}

/// The verdict of the shipped lead under the params `over`, cut to [`FIELDS`].
fn lead(over: Value, stamp: Value, head: Value, state: Value) -> Value {
    assert!(
        lead_shipped(),
        "templates/curator/lead/config.json is missing: the lead cell is not shipped (GH #1447)"
    );
    let (got, err) = cell_scope(
        "lead",
        over,
        "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
        json!([stamp, head, state]),
    );
    let mut cut = json!({});
    for f in FIELDS {
        assert!(
            got.get(f).is_some(),
            "the verdict names `{f}`: {got} ({err})"
        );
        cut[f] = got[f].clone();
    }
    cut
}

#[test]
fn the_lead_names_way_and_target_for_every_way_of_the_corpus() {
    if !shipped() {
        return;
    }
    let none = json!([]);
    let spec_on = json!({"quick_speculative": "1"});
    let fired = json!({"spec": {"call": "c-other", "state": "fired"}});
    // (case, params, stamp, head, state, verdict)
    let table: Vec<(&str, Value, Value, Value, Value, Value)> = vec![
        (
            "talk is the brain's",
            json!({}),
            stamp("talk", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("brain", "brain", "path", "", ""),
        ),
        (
            "simple is the quick voice's",
            json!({}),
            stamp("simple", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("quick", "quick", "path", "", ""),
        ),
        (
            "a simple turn that needs a tool gets the tool round and its bridge",
            json!({}),
            stamp("simple", json!(["tool"]), json!(["weather"])),
            head(json!({})),
            no_state(),
            verdict("quick", "quick", "path", "", "tool"),
        ),
        (
            "complex is a consult with its bridge",
            json!({}),
            stamp("complex", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("consult", "backend", "path", "", "consult"),
        ),
        (
            "research is an errand with its bridge",
            json!({}),
            stamp("research", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("research", "backend", "path", "", "research"),
        ),
        (
            "no stamp keeps the brain",
            json!({}),
            stamp("", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("brain", "brain", "no_path", "", ""),
        ),
        (
            "a path path_routes does not name keeps the brain",
            json!({}),
            stamp("dance", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("brain", "brain", "no_route", "", ""),
        ),
        (
            "a way path_routes names but nobody knows is the brain",
            json!({"path_routes": "simple:fly"}),
            stamp("simple", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            verdict("brain", "brain", "unknown_way", "", ""),
        ),
        (
            "the answer of a consult is spoken, never consulted again",
            json!({}),
            stamp("complex", none.clone(), none.clone()),
            head(json!({"consult_id": "k-c0"})),
            no_state(),
            verdict("brain", "brain", "answer", "", ""),
        ),
        (
            "a clarify turn is the brain's and asks",
            json!({}),
            stamp("clarify", none.clone(), none.clone()),
            head(json!({})),
            no_state(),
            with(
                verdict("brain", "brain", "clarify", "", ""),
                "clarify",
                json!(true),
            ),
        ),
        (
            "memory_to_brain sends a simple memory turn to the brain",
            json!({"memory_to_brain": "1"}),
            stamp("simple", json!(["memory"]), none.clone()),
            head(json!({})),
            no_state(),
            verdict("brain", "brain", "memory", "", ""),
        ),
        (
            "off, the memory turn stays quick",
            json!({}),
            stamp("simple", json!(["memory"]), none.clone()),
            head(json!({})),
            no_state(),
            verdict("quick", "quick", "path", "", ""),
        ),
        (
            "a spec curate rides the quick way to its own cell",
            spec_on.clone(),
            stamp("", none.clone(), none.clone()),
            head(json!({"spec": "1"})),
            no_state(),
            verdict("quick", "quick_spec", "spec", "call", ""),
        ),
        (
            "a tool round that came back without a call escalates",
            json!({}),
            stamp("simple", json!(["tool"]), none.clone()),
            head(json!({"iter": 1, "escalate": "no_tool_call"})),
            no_state(),
            verdict("brain", "brain", "escalate:no_tool_call", "", ""),
        ),
        (
            "an escalated turn stays with the brain in its next iteration",
            json!({}),
            stamp("simple", json!(["tool"]), none.clone()),
            head(json!({"iter": 1})),
            state(json!({"escalated": "t1"})),
            verdict("brain", "brain", "escalated", "", ""),
        ),
        (
            "past quick_max_iter the brain speaks",
            json!({}),
            stamp("simple", json!(["tool"]), none.clone()),
            head(json!({"iter": 3})),
            no_state(),
            verdict("brain", "brain", "iterations", "", ""),
        ),
        (
            "a duplex call's turn feeds the live line",
            json!({"live_feed": "1"}),
            stamp("talk", none.clone(), none.clone()),
            head(json!({"channel": "phone", "engine": "duplex", "call_id": "call-1"})),
            no_state(),
            with(
                verdict("brain", "brain", "path", "", ""),
                "live",
                json!(true),
            ),
        ),
        (
            "a phone turn takes the way of its stamp",
            json!({}),
            stamp("simple", none.clone(), none.clone()),
            head(json!({"channel": "phone"})),
            no_state(),
            verdict("quick", "quick", "path", "", ""),
        ),
        (
            "a talk stamp drops the speculation",
            spec_on.clone(),
            stamp("talk", none.clone(), none.clone()),
            head(json!({})),
            state(fired.clone()),
            verdict("brain", "brain", "path", "dropped:path:talk", ""),
        ),
        (
            "a memory turn drops the speculation",
            json!({"quick_speculative": "1", "memory_to_brain": "1"}),
            stamp("simple", json!(["memory"]), none.clone()),
            head(json!({})),
            state(fired.clone()),
            verdict("brain", "brain", "memory", "dropped:memory", ""),
        ),
        (
            "a quick turn waits for the spec answer",
            spec_on.clone(),
            stamp("simple", none.clone(), none.clone()),
            head(json!({})),
            state(fired.clone()),
            verdict("quick", "quick", "path", "wait", ""),
        ),
        (
            "a held spec answer is kept and no call goes",
            spec_on.clone(),
            stamp("simple", none.clone(), none.clone()),
            head(json!({})),
            state(json!({"spec": {"call": "c-other", "state": "held"},
                      "spec_held": {"call": "c-other", "answer": "Sunny.", "finish": "stop"}})),
            verdict("quick", "held", "path", "kept", ""),
        ),
        (
            "a family the spec call could not reach dooms it",
            spec_on.clone(),
            stamp("simple", json!(["tool"]), json!(["orga"])),
            head(json!({})),
            state(fired.clone()),
            verdict("quick", "quick", "path", "dropped:family:orga", "tool"),
        ),
    ];
    let mut wrong = Vec::new();
    for (case, over, st, hd, sta, want) in table {
        let got = lead(over, st, hd, sta);
        if got != want {
            wrong.push(format!("{case}:\n  want {want}\n  got  {got}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the lead names another way:\n{}",
        wrong.join("\n")
    );
}
