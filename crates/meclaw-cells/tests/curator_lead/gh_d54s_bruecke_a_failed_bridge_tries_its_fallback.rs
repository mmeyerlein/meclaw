//! d54s bruecke (GH #1586, OR-54s-P-6) -- a bridge that fails goes once
//! more, to the target the lead names as its fallback.
//!
//! WHY this file exists: the bridge of a consult turn was one model call to
//! one pinned host and nothing behind it. Measured (reader 22, receipts
//! r038 B and r039 A): the side call to the fast voice got no first byte and
//! ended after 5 s in its timeout (`wire_attempts` 1); no second try, no
//! other voice, so the first word the person heard came from the brain after
//! the advice, 40 to 47 s into the turn. The planner's ruling: the bridge
//! gets a second target (`bridge_fallback` of the lead), never a fixed
//! sentence (R-TR-27).
//!
//! The contract these pins hold:
//! 1. a side call that ends in an error or its timeout goes exactly once
//!    more, to the lead's `bridge_fallback`; without the knob nothing goes;
//! 2. the second try is the same window under the same kind -- no fixed text;
//! 3. a second try that fails too reaches `./errors` with its own cell's
//!    name, and nothing goes a third time.
//!
//! The hive runs in one process (`support/curator_hive.rs`), the shipped
//! scripts under python3 and the shipped talky edges under the colony's CEL;
//! the model cells are stand-ins the test answers (no model is called), the
//! timeout is the error the `llm` cell emits for it (`docs/cell-types.md`
//! § `llm`: `finish_reason` `error`, `error_code` `timeout`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const TALKY: &str = "templates/talky/config.json";

/// The fallback the planner names (OR-54s-P-6): talky's backend voice.
const FALLBACK: &str = "quick_bridge";

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

/// The context the model cell `cell` holds for the call `call`, through
/// talky's shipped `./curator -> ./<cell>` edge (exactly one).
fn at_cell(call: &Msg, cell: &str) -> Map<String, Value> {
    let routes: Vec<_> = talky_routes("./curator", &call.context, &call.hop)
        .into_iter()
        .filter(|(to, _, _)| *to == format!("./{cell}"))
        .collect();
    assert_eq!(
        routes.len(),
        1,
        "the call reaches ./{cell} on one edge: {:?} {:?}",
        call.hop,
        call.context
    );
    routes[0].1.clone()
}

/// What the model cell `cell` says back with `finish`, over every shipped
/// talky edge it takes.
fn cell_says(cell: &str, ctx: &Map<String, Value>, finish: &str, extra: Value) -> Vec<Route> {
    let mut hop = obj(json!({"finish_reason": finish, "model": "test-model"}));
    for (k, v) in obj(extra) {
        hop.insert(k, v);
    }
    talky_routes(&format!("./{cell}"), ctx, &hop)
}

fn to_cell(routes: &[Route], to: &str) -> Vec<Route> {
    routes.iter().filter(|(t, _, _)| t == to).cloned().collect()
}

/// The `error_source` the shipped drain `./errors` reports for a message
/// talky routed into it.
fn drain_source(ctx: &Map<String, Value>, hop: &Map<String, Value>) -> Value {
    let script = read_json(&repo("templates/talky/errors/config.json"))["params"]["script_inline"]
        .as_str()
        .expect("the drain's script")
        .to_string();
    let doc = json!({
        "body": {"messages": []},
        "envelope": {"header": {"context": ctx, "hop": hop}},
    });
    let out = run_python(&script, &doc.to_string());
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let emitted: Value = sj::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "the drain emits one JSON list ({e}): {stdout} {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    emitted[0]["header"]["error_source"].clone()
}

/// The curator as talky wires it, the lead's `bridge_fallback` set when
/// `fallback` names one.
fn bridge_hive(fallback: Option<&str>) -> Hive {
    let over: Vec<(&str, &str, Value)> = fallback
        .map(|f| vec![("lead", "bridge_fallback", json!(f))])
        .unwrap_or_default();
    moveout_hive(&over)
}

/// A consult turn: its one bridge to the fast voice.
fn first_bridge(h: &mut Hive) -> Msg {
    seam(
        h,
        "t1",
        "0",
        errand_stamp("complex"),
        json!({"messages": [user("plan my week around the move")]}),
    );
    let sides = side_calls(h);
    assert_eq!(
        sides.len(),
        1,
        "a consult turn sends one bridge: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(sides[0].hop["target"], json!("quick"), "{:?}", sides[0].hop);
    sides[0].clone()
}

/// Contract points 1 and 2, the pin of the plan: the first bridge runs into
/// its timeout, the second try goes to the fallback with the same window,
/// answers, and its words are the first the turn says.
#[test]
fn gh_d54s_bruecke_a_bridge_that_times_out_goes_once_more_to_its_fallback() {
    if !shipped() {
        return;
    }
    let mut h = bridge_hive(Some(FALLBACK));
    let first = first_bridge(&mut h);
    answer_call(
        &mut h,
        &first,
        "error",
        json!({"error_code": "timeout"}),
        json!([]),
    );
    let again = side_calls(&h);
    assert_eq!(
        again.len(),
        1,
        "a bridge that timed out goes once more: {:?} {:?}",
        h.out,
        h.stderr
    );
    let retry = again[0].clone();
    assert_eq!(retry.hop["target"], json!(FALLBACK), "{:?}", retry.hop);
    assert_ne!(
        retry.hop["curator_call"], first.hop["curator_call"],
        "the second try is a call of its own"
    );
    assert_eq!(
        side_text(&retry),
        side_text(&first),
        "the same instruction, no fixed text"
    );
    assert_eq!(retry.messages(), first.messages(), "the same window");
    assert_eq!(retry.body["tool_scope"], json!({"allow": []}), "no tool");
    assert_eq!(
        trigger_of(&h, &retry),
        SIDE,
        "its own calls row, trigger `side`"
    );
    assert!(
        main_calls(&h).is_empty(),
        "no main call goes again: {:?}",
        h.out
    );

    // The fallback answers: its words take talky's answer path, and nothing
    // goes a third time.
    let ctx = at_cell(&retry, FALLBACK);
    let routes = cell_says(FALLBACK, &ctx, "stop", json!({}));
    assert_eq!(
        to_cell(&routes, "./splitter").len(),
        1,
        "the fallback's bridge reaches the answer path: {routes:?}"
    );
    answer_call(
        &mut h,
        &retry,
        "stop",
        json!({}),
        json!([said("One moment, I am on it.")]),
    );
    assert!(
        side_calls(&h).is_empty(),
        "a bridge that answered goes no more: {:?}",
        h.out
    );
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'side_answer'");
    let id = retry.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        marks
            .iter()
            .any(|r| r[0].as_str().unwrap_or_default().contains(&id)),
        "the fallback's words are booked as the turn's bridge: {marks:?} {:?}",
        h.stderr
    );
}

/// Contract point 3, error injection: the second try fails too. Its error
/// reaches `./errors` under its own cell's name, the first one's under
/// `quick`, and nothing goes a third time.
#[test]
fn gh_d54s_bruecke_a_fallback_that_fails_too_reaches_errors_and_goes_no_more() {
    if !shipped() {
        return;
    }
    let mut h = bridge_hive(Some(FALLBACK));
    let first = first_bridge(&mut h);
    let ctx = at_cell(&first, "quick");
    let failed = to_cell(
        &cell_says("quick", &ctx, "error", json!({"error_code": "timeout"})),
        "./errors",
    );
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(drain_source(&failed[0].1, &failed[0].2), json!("quick"));
    answer_call(
        &mut h,
        &first,
        "error",
        json!({"error_code": "timeout"}),
        json!([]),
    );
    let retry = side_calls(&h);
    assert_eq!(retry.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let retry = retry[0].clone();

    let ctx = at_cell(&retry, FALLBACK);
    let routes = cell_says(
        FALLBACK,
        &ctx,
        "error",
        json!({"error_code": "provider_error"}),
    );
    let failed = to_cell(&routes, "./errors");
    assert_eq!(
        failed.len(),
        1,
        "the failed second try reaches ./errors once: {routes:?}"
    );
    assert_eq!(
        drain_source(&failed[0].1, &failed[0].2),
        json!(FALLBACK),
        "the drain names the cell that failed"
    );
    answer_call(
        &mut h,
        &retry,
        "error",
        json!({"error_code": "provider_error"}),
        json!([]),
    );
    assert!(
        side_calls(&h).is_empty() && main_calls(&h).is_empty(),
        "a second try that failed goes no third time: {:?}",
        h.out
    );
}

/// Contract point 1: without `bridge_fallback` a failed bridge stays as it
/// was -- no second try, and the verdict's entry names no fallback.
#[test]
fn gh_d54s_bruecke_without_a_fallback_a_failed_bridge_goes_no_more() {
    if !shipped() {
        return;
    }
    let v = verdict(lead_knobs(json!({})), "complex", &[], &["web"], LIVE_H);
    assert!(
        sides_of(&v, EXTERNAL)[0].get("fallback").is_none(),
        "without the knob the entry names no fallback: {v}"
    );
    let v = verdict(
        lead_knobs(json!({"bridge_fallback": FALLBACK})),
        "complex",
        &[],
        &["web"],
        LIVE_H,
    );
    assert_eq!(
        sides_of(&v, EXTERNAL)[0]["fallback"],
        json!(FALLBACK),
        "{v}"
    );

    let mut h = bridge_hive(None);
    let first = first_bridge(&mut h);
    answer_call(
        &mut h,
        &first,
        "error",
        json!({"error_code": "timeout"}),
        json!([]),
    );
    assert!(
        side_calls(&h).is_empty() && main_calls(&h).is_empty(),
        "without a fallback nothing goes again: {:?}",
        h.out
    );
}
