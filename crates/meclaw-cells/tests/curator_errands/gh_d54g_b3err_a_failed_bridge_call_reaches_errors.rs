//! GH #1536 (d54g b3err) -- a failed `./quick_bridge` call reaches `./errors`
//! with the delegation id of its turn.
//!
//! WHY this file exists: an errand the voice model hands the backend
//! (`context.delegation_id`, welle-live R-25-4) is answered by a curator
//! call. When that call goes to `./quick_bridge` and fails, talky's edge
//! `./quick_bridge -> ./errors` stays shut: it leaves every main call of the
//! curator (`context.curator_call` set, no `side` kind) to the curator. The
//! curator files the wall row and the `errand_outcome` mark (GH #1469), but
//! nothing reaches `./errors`, so the duplex cell that delegated the turn
//! waits for an answer that never comes (found live on the soul2me set,
//! `cbae9834b`). A failed brain call reaches `./errors` already, since its
//! edge has no curator exclusion. The pins run the errand through the
//! shipped curator hive and the shipped talky edges, evaluated by the
//! colony's own CEL:
//!
//! * a failed bridge call of a delegated turn yields exactly one message to
//!   `./errors`, carrying the turn's `delegation_id`;
//! * the curator still gets the failure on `in_llm` and files its wall row
//!   and its `errand_outcome` mark, once each;
//! * a successful bridge call yields no message to `./errors`;
//! * a failed bridge call of a turn without a delegation stays the curator's,
//!   as before.
//!
//! The drain `./errors` names the cell that failed in `error_source`
//! (K ruling on b3err): the soul2me set's edge asks for `quick_bridge`, and
//! the drain stamped every model failure `brain`. A cell knows no topology,
//! so talky's edge into the drain stamps the name (`hop.failed_cell`) and
//! the drain reports it; a brain failure stays `brain`.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_errand.rs"]
#[allow(unused_imports)]
use crate::curator_errand; // harness_fold: mod curator_errand;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use chrono::Duration;
use curator_errand::*;
use curator_hive::*;
use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const TALKY: &str = "templates/talky/config.json";

/// One message an edge carries: its target, its context and its hop.
type Route = (String, Map<String, Value>, Map<String, Value>);

/// The shipped talky edges leaving `from`, evaluated against one message:
/// every edge whose condition holds, with its modifier applied, as
/// `(to, context, hop)`. An edge whose condition errs is skipped, as the
/// colony skips it.
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

/// The call of the curator `call`, sent to the target `quick_bridge` through
/// talky's shipped `./curator -> ./quick_bridge` edge: the context the
/// bridge cell carries.
fn at_the_bridge(call: &Msg) -> Map<String, Value> {
    let mut hop = call.hop.clone();
    hop.insert("target".into(), json!("quick_bridge"));
    let routes: Vec<_> = talky_routes("./curator", &call.context, &hop)
        .into_iter()
        .filter(|(to, _, _)| to == "./quick_bridge")
        .collect();
    assert_eq!(
        routes.len(),
        1,
        "the call reaches ./quick_bridge on one edge: {hop:?} {:?}",
        call.context
    );
    routes[0].1.clone()
}

/// What `./quick_bridge` says back with `finish`, as `(to, context, hop)`
/// over every shipped talky edge it takes.
fn bridge_says(ctx: &Map<String, Value>, finish: &str, extra: Value) -> Vec<Route> {
    let mut hop = obj(json!({"finish_reason": finish, "model": "test-model"}));
    for (k, v) in obj(extra) {
        hop.insert(k, v);
    }
    talky_routes("./quick_bridge", ctx, &hop)
}

/// What the shipped drain `./errors` reports for one message talky routed
/// into it: the header of its one emission.
fn drain_reports(ctx: &Map<String, Value>, hop: &Map<String, Value>) -> Map<String, Value> {
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
    let list = emitted.as_array().cloned().unwrap_or_default();
    assert_eq!(list.len(), 1, "the drain emits one message: {emitted}");
    obj(list[0]["header"].clone())
}

fn to_errors(routes: &[Route]) -> Vec<Route> {
    routes
        .iter()
        .filter(|(to, _, _)| to == "./errors")
        .cloned()
        .collect()
}

/// Pin 1 and pin 2: the failed bridge call of an errand reaches `./errors`
/// once with the errand's `delegation_id`, and the curator still files the
/// wall row and the `errand_outcome` mark of that failure.
#[test]
fn a_failed_bridge_call_of_an_errand_reaches_errors_once() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p01", "bbbb2222", 5);
    assert_eq!(
        e.call.context.get("delegation_id"),
        Some(&json!(e.id)),
        "the errand's call carries its delegation id: {:?}",
        e.call.context
    );
    let ctx = at_the_bridge(&e.call);
    assert!(
        ctx.get("curator_call")
            .and_then(Value::as_str)
            .is_some_and(|c| !c.is_empty()),
        "the bridge call is a call of the curator: {ctx:?}"
    );
    let routes = bridge_says(&ctx, "error", json!({"error_code": "upstream_error"}));

    // Pin 1: exactly one message to ./errors, with the turn's delegation id.
    let errors = to_errors(&routes);
    assert_eq!(
        errors.len(),
        1,
        "a failed bridge call of a delegated turn reaches ./errors exactly once: {routes:?}"
    );
    assert_eq!(
        errors[0].1.get("delegation_id"),
        Some(&json!(e.id)),
        "the message to ./errors carries the turn's delegation id: {:?}",
        errors[0]
    );
    let report = drain_reports(&errors[0].1, &errors[0].2);
    assert_eq!(
        report.get("error_source"),
        Some(&json!("quick_bridge")),
        "the drain names the cell that failed: {report:?}"
    );

    // Pin 2: the curator gets the failure on in_llm and files its outcome.
    let to_curator: Vec<_> = routes
        .iter()
        .filter(|(to, _, hop)| to == "./curator" && hop.get("route") == Some(&json!("in_llm")))
        .cloned()
        .collect();
    assert_eq!(
        to_curator.len(),
        1,
        "the failure still reaches the curator once on in_llm: {routes:?}"
    );
    let (_, c, hp) = to_curator[0].clone();
    h.set_clock((start() + Duration::seconds(20)).timestamp_micros());
    h.lane(
        "in_llm",
        Value::Object(c),
        Value::Object(hp),
        json!({"messages": []}),
    );
    let call = ask(&mut h, start() + Duration::minutes(1), "t2");
    let rows = outcome_rows(&h, &e);
    assert_eq!(
        rows.len(),
        1,
        "the curator files one wall row for the failed errand: {rows:?} {:?}",
        h.stderr
    );
    assert!(rows[0].contains(FAILED), "the row says failed: {rows:?}");
    let marks = outcome_marks(&h, &e);
    assert_eq!(
        marks.len(),
        1,
        "one errand_outcome mark names the errand: {marks:?}"
    );
    assert_eq!(
        outcome_lines(&call, &e).len(),
        1,
        "the next window says the outcome once: {:?}",
        texts(&call)
    );
}

/// Pin 3: a successful bridge call of an errand reaches no `./errors`.
#[test]
fn a_successful_bridge_call_of_an_errand_reaches_no_errors() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p02", "cccc3333", 5);
    let ctx = at_the_bridge(&e.call);
    for finish in ["stop", "tool_calls", "length"] {
        let routes = bridge_says(&ctx, finish, json!({}));
        assert!(
            to_errors(&routes).is_empty(),
            "a bridge call that ends with {finish} reaches no ./errors: {routes:?}"
        );
    }
}

/// Unchanged: a failed bridge call of the curator for a turn without a
/// delegation stays the curator's, and a failed side call reaches
/// `./errors` once, as before.
#[test]
fn a_failed_bridge_call_without_a_delegation_is_unchanged() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let call = h.curate(
        "s-plain",
        "t1",
        0,
        json!([user("Hello there.")]),
        mode("Be brief."),
    );
    assert!(
        call.context.get("delegation_id").is_none(),
        "a plain turn carries no delegation id: {:?}",
        call.context
    );
    let ctx = at_the_bridge(&call);
    let routes = bridge_says(&ctx, "error", json!({"error_code": "upstream_error"}));
    assert!(
        to_errors(&routes).is_empty(),
        "a failed curator call without a delegation stays the curator's: {routes:?}"
    );
    let mut side = ctx.clone();
    side.insert("call_kind".into(), json!("side"));
    let routes = bridge_says(&side, "error", json!({"error_code": "upstream_error"}));
    assert_eq!(
        to_errors(&routes).len(),
        1,
        "a failed side call reaches ./errors once, as before: {routes:?}"
    );
}

/// The counter-check of the drain's stamp: a failed brain call of the same
/// errand still reports `brain`, and a failed quick call that reaches the
/// drain reports `quick` -- the name comes from the edge, not a fixed value.
#[test]
fn the_drain_names_the_model_cell_that_failed() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p03", "dddd4444", 5);
    let ctx = at_the_bridge(&e.call);
    let failed = obj(
        json!({"finish_reason": "error", "error_code": "upstream_error",
                            "error_source": "wire", "model": "test-model"}),
    );
    let mut sources = Vec::new();
    for (cell, c) in [
        ("brain", ctx.clone()),
        ("quick", Map::new()),
        ("quick_bridge", ctx),
    ] {
        let errors = to_errors(&talky_routes(&format!("./{cell}"), &c, &failed));
        assert_eq!(
            errors.len(),
            1,
            "a failed {cell} call reaches ./errors once: {errors:?}"
        );
        let report = drain_reports(&errors[0].1, &errors[0].2);
        sources.push((cell, report.get("error_source").cloned()));
    }
    assert_eq!(
        sources,
        vec![
            ("brain", Some(json!("brain"))),
            ("quick", Some(json!("quick"))),
            ("quick_bridge", Some(json!("quick_bridge"))),
        ],
        "the drain reports the cell that failed"
    );
}
