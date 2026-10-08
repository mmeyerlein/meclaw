//! GH #1044 — a recall the model asks for itself, and the ask of a role
//! without a recall push, are sized by the same model package as the push.
//!
//! Since GH #1040 the memory hive sizes its tier-1 bundle by the asker's
//! usable window (`recall_input_soft`): the curator's `./policy` keeps the
//! answering model's `input_soft` -- the catalog row, cut to the model's
//! window alone since GH #1085 (no role's `quality_cap` any more) -- and
//! `./push` hands it over on the ask it enriches. Two roads
//! still arrived without it and got the old sizes (2000 tokens, 20 items, 20
//! per leg): the model's own `memory_recall` tool call -- roughly a fifth of
//! all brain calls in a measured benchmark run -- and the collector's ask of a
//! role whose push is off.
//!
//! The locks:
//!   * a tool call of the model walks the shipped edges (curator -> brain ->
//!     dispatcher -> assistant -> member -> memory hive -> tool -> recall) with
//!     the curator's value, and the recall cell's fan asks the store as deep as
//!     that value allows (talky under luna: 125 per leg, 12 500 tokens);
//!   * without a package every road keeps the old sizes (20 per leg);
//!   * the composite that sets the key clears it on every exit, so it never
//!     leaves the member beside the tool call (GH #494/#823);
//!   * an ask the push hands on untouched (`recall_push` off) carries the value
//!     too.
//!
//! The scripts are the shipped `params.script_inline` programs, the edges the
//! shipped graphs under the colony's own CEL evaluator. No colony, no model.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const LUNA: &str =
    r#"{"input_soft": 250000, "input_hard": 1000000, "cost_in": 10.0, "cost_cached_in": 1.0}"#;

fn graph(template: &str) -> Vec<Value> {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn holds(edge: &Value, h: &Headers) -> bool {
    let Some(c) = edge["condition"].as_str() else {
        return true;
    };
    let cond = parse_condition(c).expect("the condition compiles");
    evaluate_condition(&cond, &h.context, &h.hop).unwrap_or(false)
}

/// The message `h` crosses the shipped edge `from -> to` of `template`, with
/// the substrate's default rule: a `default` edge fires only when no other
/// edge out of `from` takes the message.
fn cross(template: &str, from: &str, to: &str, h: &Headers) -> Headers {
    let edges = graph(template);
    let taken: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == json!(from) && holds(e, h))
        .collect();
    let any_plain = taken.iter().any(|e| e["default"] != json!(true));
    let hit: Vec<&&Value> = taken
        .iter()
        .filter(|e| e["to"] == json!(to) && (!any_plain || e["default"] != json!(true)))
        .collect();
    assert_eq!(
        hit.len(),
        1,
        "{template}: exactly one edge {from} -> {to} takes {h:?}: {hit:?}"
    );
    if hit[0]["modifier"].is_null() {
        return h.clone();
    }
    let spec: ModifierSpec = sj::from_value(hit[0]["modifier"].clone()).expect("modifier spec");
    let m = parse_modifier(&spec).expect("the modifier compiles");
    apply_modifier(&m, h).expect("the modifier evaluates")
}

/// The model call the curator of `role` builds for a turn, after the model
/// answered the turn before with `usage` (`None`: no answer yet).
fn brain_call(role: &str, usage: Option<Value>) -> Msg {
    let mut h = Hive::with(&[("policy", "role", json!(role))]);
    if let Some(usage) = usage {
        let call = h.curate("s1", "t1", 0, json!([user("Who is my son?")]), mode(""));
        h.tap(&call, "stop", usage, json!([said("noted")]));
    }
    h.curate(
        "s1",
        "t2",
        0,
        json!([user("And what does he do?")]),
        mode(""),
    )
}

/// The memory hive's tool cell on the call as the hive's door delivers it.
fn tool_cell(h: &Headers) -> Headers {
    let doc = json!({
        "header": {"context": Value::Object(h.context.clone()), "hop": Value::Object(h.hop.clone())},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                      "text": "{\"query\": \"What are my sons called?\"}"}],
        "params": {}
    });
    let script = meclaw_testing::shipped_script("../../templates/memory-hive/tool/config.json");
    let out =
        meclaw_testing::run_shipped_script(&script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = sj::from_slice(&out.stdout).expect("stdout is json");
    let first = match v {
        Value::Array(a) => a.into_iter().next().expect("one emission"),
        other => other,
    };
    assert_eq!(first["header"]["route"], "ask", "{first}");
    Headers::from_parts(h.context.clone(), map(first["header"].clone()))
}

/// The limit of every call of the recall cell's tier-1 fan, by id.
fn fan(h: &Headers) -> Value {
    let mut ctx = map(json!({"recall_id": "", "mem_phase": ""}));
    ctx.extend(h.context.clone());
    let doc = json!({
        "header": {"context": Value::Object(ctx), "hop": {"route": "in_query", "phase": "recall"}},
        "messages": [],
        "params": {}
    });
    let script = meclaw_testing::shipped_script("../../templates/memory-hive/recall/config.json");
    let out =
        meclaw_testing::run_shipped_script(&script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = sj::from_slice(&out.stdout).expect("stdout is json");
    let msgs = match v {
        Value::Array(a) => a,
        other => vec![other],
    };
    let fan = msgs
        .iter()
        .find(|m| m["header"]["phase"] == "t1-fan")
        .unwrap_or_else(|| panic!("no t1-fan bundle in {msgs:?}"));
    let mut limits = Map::new();
    for t in fan["messages"].as_array().expect("messages") {
        if t["type"] == "tool_call" {
            let args: Value = sj::from_str(t["text"].as_str().expect("text")).expect("args");
            limits.insert(t["id"].as_str().expect("id").into(), args["limit"].clone());
        }
    }
    Value::Object(limits)
}

/// One `memory_recall` of the model of `core` (`talky`/`cogny`, its curator
/// in `role`), from the curator's model call to the recall cell's fan. Returns
/// the context the recall cell sees and the fan's limits.
fn tool_recall(core: &str, role: &str, usage: Option<Value>) -> (Headers, Value) {
    let call = brain_call(role, usage);
    // The member's round rides from the channel in.
    let outer = map(json!({"audience_set": ROUND_E, "channel": "test", "session_id": "s1"}));
    let h = Headers::from_parts(outer, call.hop.clone());
    let h = cross(core, "./curator", "./brain", &h);
    // The model answers with a tool call; the dispatcher hands it on with the
    // context of the call (the llm cell, the splitter and the dispatcher write
    // a new hop, the context rides).
    let h = Headers::from_parts(
        h.context.clone(),
        map(
            json!({"route": "tool", "tool_name": "memory_recall", "tool_call_id": "call-1",
                   "turn_id": "t2"}),
        ),
    );
    let h = cross(core, "./dispatcher", ".", &h);
    assert!(
        !h.context.contains_key("recall_input_soft"),
        "{core}: the key leaves the composite on the hop, never in the context: {h:?}"
    );
    let h = cross("assistant", &format!("./{core}"), ".", &h);
    let h = cross("member", "./assistants", "./memory-hive", &h);
    let h = cross("memory-hive", ".", "./tool", &h);
    let h = tool_cell(&h);
    let h = cross("memory-hive", "./tool", "./recall", &h);
    let limits = fan(&h);
    (h, limits)
}

fn soft(h: &Headers) -> String {
    h.context
        .get("recall_input_soft")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

// ═════════════════════════════════════════ 1. the model's own memory_recall

#[test]
fn a_tool_recall_of_talky_under_luna_asks_as_deep_as_the_push() {
    let (h, limits) = tool_recall("talky", "talky", Some(sj::from_str(LUNA).unwrap()));
    // luna's catalog row, input_soft 250 000 (GH #1085: no role cuts it);
    // 5 % of it is 12 500 tokens, 6.25 times the old 2000 -- 125 per leg
    // instead of 20.
    assert_eq!(soft(&h), "250000", "{h:?}");
    assert_eq!(limits["r-fan-kw-ep"], 125, "{limits}");
    assert_eq!(limits["r-fan-kw-fact"], 125, "{limits}");
    assert_eq!(limits["r-fan-self"], 125, "{limits}");
}

#[test]
fn a_tool_recall_of_cogny_is_sized_by_the_same_row() {
    // consult under luna: the same row, no role's window of its own
    // (GH #1085; `quality_cap` 80 000 made it 40 per leg before).
    let (h, limits) = tool_recall("cogny", "consult", Some(sj::from_str(LUNA).unwrap()));
    assert_eq!(soft(&h), "250000", "{h:?}");
    assert_eq!(limits["r-fan-kw-ep"], 125, "{limits}");
}

#[test]
fn a_tool_recall_without_a_package_keeps_the_old_sizes() {
    for core in ["talky", "cogny"] {
        let (h, limits) = tool_recall(core, "talky", None);
        assert_eq!(soft(&h), "", "{core}: {h:?}");
        assert_eq!(limits["r-fan-kw-ep"], 20, "{core}: {limits}");
        assert_eq!(limits["r-fan-self"], 20, "{core}: {limits}");
    }
}

// ═════════════════════════════════════════ 2. the key never leaves the member

/// The composite that sets the key on its model call clears it on every one of
/// its exits: the value leaves on the hop of the memory call alone.
#[test]
fn every_exit_of_a_core_clears_the_key() {
    for core in ["talky", "cogny"] {
        let edges = graph(core);
        let sets = edges.iter().any(|e| {
            e["to"] != json!(".") && !e["modifier"]["set_context"]["recall_input_soft"].is_null()
        });
        assert!(sets, "{core}: its model call carries the key in");
        for e in edges.iter().filter(|e| e["to"] == json!(".")) {
            let cleared = e["modifier"]["delete_context"]
                .as_array()
                .is_some_and(|d| d.iter().any(|k| k == "recall_input_soft"));
            assert!(cleared, "{core}: an exit lets the key out: {e}");
        }
    }
}

/// A tool call that is not a memory question carries no value: the tool exit
/// stamps the key empty, and the context leaves without it.
#[test]
fn another_tool_call_carries_no_package() {
    for core in ["talky", "cogny"] {
        let ctx = map(json!({"recall_input_soft": "250000", "curator_call": "c"}));
        let h = Headers::from_parts(
            ctx,
            map(json!({"route": "tool", "tool_name": "file_read", "tool_call_id": "x"})),
        );
        let out = cross(core, "./dispatcher", ".", &h);
        assert_eq!(out.hop["recall_input_soft"], "", "{core}: {out:?}");
        assert!(
            !out.context.contains_key("recall_input_soft"),
            "{core}: {out:?}"
        );
    }
}

// ═════════════════════════════════════════ 3. a role without a push

fn ask(h: &mut Hive, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": "s1", "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

#[test]
fn an_ask_handed_on_untouched_carries_the_package_too() {
    let mut h = Hive::with(&[("policy", "role", json!("consult"))]);
    let first = ask(&mut h, "t1", "Who is my son?");
    assert_eq!(first.len(), 1, "{:?}", h.stderr);
    assert_eq!(first[0].hop["recall_input_soft"], "", "no answer yet");
    let call = h.curate("s1", "t1", 0, json!([user("Who is my son?")]), mode(""));
    h.tap(
        &call,
        "stop",
        sj::from_str(LUNA).unwrap(),
        json!([said("noted")]),
    );
    let asks = ask(&mut h, "t2", "And what does he do?");
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    assert_eq!(
        asks[0].hop["recall_input_soft"], "250000",
        "{:?}",
        asks[0].hop
    );
    // Untouched otherwise: the collector's question as it came.
    assert_eq!(asks[0].hop["recall_query"], "And what does he do?");
    assert_eq!(asks[0].messages(), vec![user("And what does he do?")]);
}
