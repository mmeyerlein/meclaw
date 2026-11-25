//! GH #1619 (w1 fehlerwort) -- a failed model call of a person's turn says
//! one sentence, if the composite is told which.
//!
//! WHY this file exists: a model cell of talky (`./brain`, `./quick`,
//! `./quick_bridge`) that fails reaches `./errors`, which normalises the
//! failure to ONE report on route `error` and nothing else. The report goes
//! up through every rim to the colony's drain, and the person whose turn it
//! was hears nothing at all (w1 fix2, `senken.md` § 5: 12 provider errors and
//! one timeout in run b18-6, each a turn without an answer; the owner's
//! "never silence", R-54-1/R-54-3). The knob `error_reply` of `talky/errors`
//! names the sentence; the drain then emits it beside its report, on route
//! `answer`, through the one edge `./errors -> .` that carries it. The pins:
//!
//! * `error_reply` empty (shipped) or absent: the drain's output is the same
//!   bytes as before, for every lane that fans in;
//! * set, a failed brain, quick or quick_bridge call of a person's turn: the
//!   report unchanged, plus exactly one answer with the sentence, and the
//!   shipped edges carry the report on `error` and the answer on `answer`,
//!   each exactly once;
//! * set, a failure where nobody waits for this sentence -- a delegated
//!   errand (the duplex cell hears the failure itself, GH #1536), a side call
//!   (the turn's answer still comes from the brain), an inner turn, a duplex
//!   advice round, a self summary, a store refusal, a refused quote, an
//!   unknown failure: the report alone.

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

const TALKY: &str = "templates/talky/config.json";
const ERRORS: &str = "templates/talky/errors/config.json";
const SENTENCE: &str = "Sorry, something went wrong on my side. Please say that again.";

/// One message an edge carries: its target, its context and its hop.
type Route = (String, Map<String, Value>, Map<String, Value>);

/// The shipped talky edges leaving `from`, evaluated against one message,
/// with their modifiers applied, as `(to, context, hop)`.
fn talky_routes(from: &str, context: &Map<String, Value>, hop: &Map<String, Value>) -> Vec<Route> {
    let talky = read_json(&repo(TALKY));
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{TALKY}: params.graph.edges"))
        .clone();
    let mut out = Vec::new();
    for e in edges.iter().filter(|e| e["from"] == json!(from)) {
        if let Some(src) = e["condition"].as_str() {
            let cond = parse_condition(src).unwrap_or_else(|x| panic!("{src}: {x}"));
            if !matches!(evaluate_condition(&cond, context, hop), Ok(true)) {
                continue;
            }
        }
        let (mut ctx, mut h) = (context.clone(), hop.clone());
        if e["modifier"].is_object() {
            let spec: ModifierSpec =
                sj::from_value(e["modifier"].clone()).expect("a modifier spec");
            let m = parse_modifier(&spec).expect("a modifier");
            match apply_modifier(&m, &Headers::from_parts(ctx.clone(), h.clone())) {
                Ok(out_h) => {
                    ctx = out_h.context;
                    h = out_h.hop;
                }
                Err(_) => continue,
            }
        }
        out.push((e["to"].as_str().unwrap_or("").to_string(), ctx, h));
    }
    out
}

/// The raw stdout of the shipped drain for one message, with `params` as the
/// code cell hands them to its script (`docs/cell-types.md` § `code`), or
/// none at all.
fn drain_stdout(ctx: &Value, hop: &Value, params: Option<Value>) -> String {
    let script = read_json(&repo(ERRORS))["params"]["script_inline"]
        .as_str()
        .expect("the drain's script")
        .to_string();
    let mut doc = json!({
        "body": {"messages": []},
        "envelope": {"header": {"context": ctx, "hop": hop}},
    });
    if let Some(p) = params {
        doc["params"] = p;
    }
    let out = run_python(&script, &doc.to_string());
    assert!(
        out.status.success(),
        "the drain runs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn drain(ctx: &Value, hop: &Value, params: Option<Value>) -> Vec<Value> {
    let stdout = drain_stdout(ctx, hop, params);
    let v: Value = sj::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("the drain emits one JSON list ({e}): {stdout}"));
    v.as_array().cloned().unwrap_or_default()
}

fn on() -> Option<Value> {
    Some(json!({"error_reply": SENTENCE}))
}

fn person() -> Value {
    json!({"session_id": "s-1", "turn_id": "s-1#u-3", "channel": "telegram:42",
           "channel_node": "/member/telegram", "curator_call": "c-9"})
}

fn failed(cell: &str) -> Value {
    json!({"finish_reason": "error", "error_code": "upstream_error",
           "failed_cell": cell, "model": "test-model"})
}

fn with(base: Value, extra: Value) -> Value {
    let mut o = obj(base);
    for (k, v) in obj(extra) {
        o.insert(k, v);
    }
    Value::Object(o)
}

/// Every lane that fans into the drain, as `(name, context, hop)`.
fn lanes() -> Vec<(&'static str, Value, Value)> {
    vec![
        ("brain", person(), failed("brain")),
        ("quick", person(), failed("quick")),
        ("quick_bridge", person(), failed("quick_bridge")),
        (
            "no stamp",
            person(),
            json!({"finish_reason": "content_filter"}),
        ),
        (
            "keeper",
            person(),
            json!({"route": "reject", "store_error": "query_timeout"}),
        ),
        (
            "quote",
            person(),
            json!({"route": "reject", "reject_reason": "quote_not_in_source",
                                    "section": "memory", "field": "facts"}),
        ),
        ("unknown", person(), json!({"route": "whatever"})),
    ]
}

/// Pin 1: off is byte-identical -- absent and empty give the same stdout.
#[test]
fn an_empty_error_reply_is_byte_identical() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo(ERRORS));
    assert_eq!(
        cfg["params"]["error_reply"],
        json!(""),
        "the knob ships empty: {}",
        cfg["params"]
    );
    assert_eq!(
        cfg["contract"]["settings"]["error_reply"]["default"],
        json!(""),
        "the knob is declared with its default: {}",
        cfg["contract"]["settings"]
    );
    for (name, ctx, hop) in lanes() {
        let bare = drain_stdout(&ctx, &hop, None);
        for p in [
            json!({}),
            json!({"error_reply": ""}),
            json!({"error_reply": "  "}),
        ] {
            assert_eq!(
                drain_stdout(&ctx, &hop, Some(p.clone())),
                bare,
                "{name}: params {p} change the drain's bytes"
            );
        }
        assert_eq!(drain(&ctx, &hop, None).len(), 1, "{name}: one report");
    }
}

/// Pin 2: a failed model call of a person's turn says the sentence once, the
/// report beside it unchanged, and the shipped edges carry each exactly once.
#[test]
fn a_failed_model_call_of_a_person_turn_says_the_sentence_once() {
    if !shipped() {
        return;
    }
    for cell in ["brain", "quick", "quick_bridge"] {
        let ctx = person();
        let hop = with(
            failed(cell),
            json!({"finish_reason": if cell == "quick" { "content_filter" } else { "error" }}),
        );
        let off = drain(&ctx, &hop, None);
        let out = drain(&ctx, &hop, on());
        assert_eq!(out.len(), 2, "{cell}: the report and one answer: {out:?}");
        assert_eq!(out[0], off[0], "{cell}: the report is unchanged");
        let head = obj(out[1]["header"].clone());
        assert_eq!(
            head.get("route"),
            Some(&json!("answer")),
            "{cell}: {head:?}"
        );
        assert_eq!(
            head.get("session_id"),
            Some(&json!("s-1")),
            "{cell}: {head:?}"
        );
        assert_eq!(
            head.get("turn_id"),
            Some(&json!("s-1#u-3")),
            "{cell}: {head:?}"
        );
        assert_eq!(
            head.get("error_reply"),
            Some(&json!("1")),
            "{cell}: {head:?}"
        );
        assert_eq!(
            head.get("error_source"),
            Some(&json!(cell)),
            "{cell}: {head:?}"
        );
        assert_eq!(
            out[1]["messages"],
            json!([{"origin": "assistant", "type": "text", "text": SENTENCE}]),
            "{cell}: the sentence, as the person's answer"
        );

        let c = obj(ctx.clone());
        let report = talky_routes("./errors", &c, &obj(out[0]["header"].clone()));
        assert_eq!(
            report.len(),
            1,
            "{cell}: the report leaves on one edge: {report:?}"
        );
        assert_eq!(report[0].0, ".", "{cell}: to the rim");
        assert_eq!(report[0].2.get("route"), Some(&json!("error")));
        let answer = talky_routes("./errors", &c, &head);
        assert_eq!(
            answer.len(),
            1,
            "{cell}: the answer leaves on one edge: {answer:?}"
        );
        assert_eq!(answer[0].0, ".", "{cell}: to the rim");
        assert_eq!(answer[0].2.get("route"), Some(&json!("answer")));
        assert!(
            answer[0].1.get("curator_call").is_none(),
            "{cell}: the exit clears the inner context like the error exit: {:?}",
            answer[0].1
        );
        assert_eq!(
            answer[0].1.get("channel_node"),
            Some(&json!("/member/telegram")),
            "{cell}: the answer keeps the way back to its channel"
        );
    }
}

/// Pin 3: where nobody waits for this sentence, the report goes alone.
#[test]
fn no_sentence_where_nobody_waits_for_it() {
    if !shipped() {
        return;
    }
    let cases = vec![
        (
            "delegated errand",
            with(person(), json!({"delegation_id": "d-1"})),
            failed("quick_bridge"),
        ),
        (
            "side call",
            with(person(), json!({"call_kind": "side"})),
            failed("quick"),
        ),
        (
            "sections call",
            with(person(), json!({"call_kind": "sections"})),
            failed("brain"),
        ),
        (
            "inner turn",
            with(
                person(),
                json!({"channel": "inner:reminder", "inner_source": "reminder"}),
            ),
            failed("brain"),
        ),
        (
            "duplex advice",
            with(person(), json!({"engine": "duplex"})),
            failed("brain"),
        ),
        (
            "self summary",
            with(person(), json!({"summarize": "1"})),
            failed("brain"),
        ),
        (
            "tool interim",
            with(person(), json!({"interim": "tool"})),
            failed("quick"),
        ),
        (
            "keeper",
            person(),
            json!({"route": "reject", "store_error": "query_timeout"}),
        ),
        (
            "quote",
            person(),
            json!({"route": "reject", "reject_reason": "quote_missing"}),
        ),
        ("unknown", person(), json!({"route": "whatever"})),
    ];
    for (name, ctx, hop) in cases {
        let out = drain(&ctx, &hop, on());
        assert_eq!(out.len(), 1, "{name}: the report alone: {out:?}");
        assert_eq!(out[0]["header"]["route"], json!("error"), "{name}: {out:?}");
    }
}

/// Pin 4: the drain declares what it emits, and exactly one shipped edge
/// carries an answer out of it.
#[test]
fn the_answer_has_its_one_exit() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo(ERRORS));
    let routes = cfg["contract"]["emits"]["hop"]["route"]["values"].clone();
    assert!(
        routes
            .as_array()
            .is_some_and(|v| v.contains(&json!("answer")) && v.contains(&json!("error"))),
        "the drain's contract names both routes: {routes}"
    );
    let talky = read_json(&repo(TALKY));
    let exits: Vec<_> = talky["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["from"] == json!("./errors"))
        .cloned()
        .collect();
    assert_eq!(exits.len(), 2, "two exits out of ./errors: {exits:?}");
    assert!(
        exits.iter().all(|e| e["to"] == json!(".")),
        "both to the rim: {exits:?}"
    );
}
