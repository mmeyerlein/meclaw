//! #1398 (V1, #1275 goal 3) -- the quick voice has two siblings: the
//! speculation and the bridge no longer queue behind the one `./quick`.
//!
//! WHY this file exists: the speculation (#1275), the bridge (#1248) and the
//! tool round (#1260) all ran on the one `./quick`, a stateful `llm` cell that
//! runs one `handle()` at a time (llm/factory.rs, cell_task.rs). The bridge
//! waited behind a spec in flight, the tool call behind the bridge (about
//! 700 ms), and a drop the stamp already knew waited for the spec's end
//! (+1.3 s p50, t1 breakdown). A spec call cannot be cancelled inside the llm
//! cell, so it gets a cell of its own instead: `./quick_spec` takes every call
//! with `hop.spec` "1", `./quick_bridge` the bridge of a speculating talky
//! (`hop.spec` "bridge", set by the curator only with `quick_speculative` on),
//! and `./quick` everything else. `./quick_spec` has no road to the splitter:
//! a spec answer only ever speaks through the curator's `spec_release`.
//!
//! The claims are routing claims over the SHIPPED talky edges, evaluated by the
//! colony's own router (`apply_edges`), fed with the calls the SHIPPED curator
//! emits (the curator-hive harness runs its scripts synchronously). Which cell
//! a call lands on is what decides whether it queues; no clock is involved,
//! so there is no wait point for a paused clock to hold (OR-d53-2.v1.1).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};
use sha2::{Digest, Sha256};

const TALKY: &str = "templates/talky/config.json";
const BASE: &str = "/t";

fn spec_hive(spec_tools: &str) -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "spec_tools", json!(spec_tools)),
    ]);
    h.check_emits = true;
    h
}

fn plain_hive() -> Hive {
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    h.check_emits = true;
    h
}

/// The collector's early curate (`hop.spec` "1", no path); the spec call.
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

/// The normal curate of the turn at its route stamp.
fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, families: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": families});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let system = json!({"instructions": {"mode": {"text": "Be brief."},
                                         "sidecar": {"text": "End with a sidecar block."}}});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(text)], "system": system}),
    );
}

/// A spec call's end as talky's `./quick_spec -> ./curator` edge hands it in.
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

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("JSON"))
    .collect()
}

/// Every message that answers or asks a model: calls and released answers.
fn answers(h: &Hive) -> usize {
    ["quick", "brain", "spec_release"]
        .iter()
        .map(|r| h.routed(r).len())
        .sum()
}

fn is_bridge(m: &Msg) -> bool {
    m.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .ends_with("-bridge")
}

/// talky's shipped edges as the colony's router holds them, rooted at `/t`.
fn talky_table() -> EdgeTable {
    let params = read_json(&repo(TALKY))["params"].clone();
    let hp: HiveParams = sj::from_value(params).expect("talky params");
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let abs = |ep: &str| match ep {
            "." => BASE.to_string(),
            other => format!("{BASE}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

/// The cells one message reaches from `from` (`/t/<cell>`), with each hop.
fn reach(
    t: &EdgeTable,
    from: &str,
    ctx: &Map<String, Value>,
    hop: &Map<String, Value>,
) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(ctx.clone(), hop.clone());
    apply_edges(t, &Path::new(&format!("{BASE}/{from}")), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out))
        .collect()
}

/// The model cells a curator call lands on (the curator's out hop, its context).
fn cells_of(t: &EdgeTable, call: &Msg) -> Vec<String> {
    let mut hop = call.hop.clone();
    hop.insert("iter".into(), json!("0"));
    reach(t, "curator", &call.context, &hop)
        .into_iter()
        .map(|(to, _)| to)
        .collect()
}

/// A model cell's answer to `call`, as it leaves the cell: the context the
/// curator edge promoted, `hop.finish_reason`.
fn answer_reach(t: &EdgeTable, cell: &str, call: &Msg, finish: &str) -> Vec<(String, Headers)> {
    let mut hop = call.hop.clone();
    hop.insert("iter".into(), json!("0"));
    let into = reach(t, "curator", &call.context, &hop);
    let (_, h) = into
        .iter()
        .find(|(to, _)| to == &format!("{BASE}/{cell}"))
        .unwrap_or_else(|| panic!("{cell} takes the call: {into:?}"));
    reach(
        t,
        cell,
        &h.context,
        &obj(json!({"finish_reason": finish, "model": "test-model"})),
    )
}

fn targets(v: &[(String, Headers)]) -> Vec<String> {
    let mut t: Vec<String> = v.iter().map(|(to, _)| to.clone()).collect();
    t.sort();
    t
}

/// Pin 1 (#1275 goal 3): a spec call is in flight on its own cell when the
/// stamp says `complex`; the bridge goes at the stamp onto `./quick_bridge`,
/// never onto the cell that still runs the spec (one `handle()` at a time),
/// and its answer speaks through the splitter like before.
#[test]
fn gh1275_the_bridge_never_waits_behind_a_speculation() {
    if !shipped() {
        return;
    }
    let t = talky_table();
    let mut h = spec_hive("web,weather");
    let spec = spec_call(&mut h, "t1", "plan my trip to Rome");
    assert_eq!(cells_of(&t, &spec), vec!["/t/quick_spec".to_string()]);
    seam(&mut h, "t1", "complex", "", "web", "plan my trip to Rome");
    let bridges: Vec<Msg> = h.routed("quick").into_iter().filter(is_bridge).collect();
    assert_eq!(
        bridges.len(),
        1,
        "the bridge goes at the stamp: {:?} {:?}",
        h.out,
        h.stderr
    );
    let cells = cells_of(&t, &bridges[0]);
    assert_eq!(
        cells,
        vec!["/t/quick_bridge".to_string()],
        "{:?}",
        bridges[0].hop
    );
    assert!(!cells.contains(&"/t/quick_spec".to_string()));
    let said = targets(&answer_reach(&t, "quick_bridge", &bridges[0], "stop"));
    assert_eq!(
        said,
        vec!["/t/curator".to_string(), "/t/splitter".to_string()]
    );
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 2 (#1275 goal 3): the tool round of a speculating talky -- the required
/// call and its bridge leave in ONE emission (the same tick), the call onto
/// `./quick`, the bridge onto `./quick_bridge`, so neither waits for the other.
#[test]
fn gh1275_the_tool_round_starts_beside_the_bridge() {
    if !shipped() {
        return;
    }
    let t = talky_table();
    let mut h = spec_hive("web,weather");
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "orga",
        "move my dentist to friday",
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "call and bridge together: {:?} {:?}",
        h.out,
        h.stderr
    );
    let (bridge, call): (Vec<Msg>, Vec<Msg>) = quick.into_iter().partition(is_bridge);
    assert_eq!(bridge.len(), 1);
    assert_eq!(
        call[0].body["tool_scope"]["require"],
        json!(true),
        "the tool round"
    );
    assert_eq!(cells_of(&t, &call[0]), vec!["/t/quick".to_string()]);
    assert_eq!(
        cells_of(&t, &bridge[0]),
        vec!["/t/quick_bridge".to_string()]
    );
    // The bridge's sentence is interim: through the splitter, the tap books it.
    let said = targets(&answer_reach(&t, "quick_bridge", &bridge[0], "stop"));
    assert_eq!(
        said,
        vec!["/t/curator".to_string(), "/t/splitter".to_string()]
    );
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 3: the stamp needs `orga`, outside `spec_tools` -- no spec answer can be
/// kept (`family:orga`), so the drop is known at the stamp. The stored sends
/// leave at once, beside the stamp; the spec's later end is discarded once
/// (`late_spec_discarded`), and it sends nothing a second time.
#[test]
fn gh1275_a_drop_known_at_the_stamp_releases_the_stored_call_at_once() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive("web,weather,story_read");
    let spec = spec_call(&mut h, "t1", "move my dentist to friday");
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "orga",
        "move my dentist to friday",
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "call and bridge at the stamp: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(quick.iter().all(|q| q.hop.get("spec") != Some(&json!("1"))));
    let m = marks(&h, "quick_spec");
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["verdict"], json!("dropped"));
    assert_eq!(m[0]["reason"], json!("family:orga"));
    assert!(
        h.rows("SELECT key FROM state WHERE key LIKE 'spec%'")
            .is_empty(),
        "the turn's spec rows went with the drop"
    );
    spec_end(
        &mut h,
        &spec,
        "tool_calls",
        json!({}),
        json!([tool_call("cw", "weather")]),
    );
    assert_eq!(answers(&h), 0, "no second send: {:?} {:?}", h.out, h.stderr);
    let late = marks(&h, "late_spec_discarded");
    assert_eq!(late.len(), 1, "{late:?}");
    assert_eq!(late[0]["call"], spec.hop["curator_call"]);
    assert_eq!(late[0]["finish"], json!("tool_calls"));
    assert_eq!(marks(&h, "quick_spec").len(), 1, "one decision per turn");
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// The condition of talky's `in_call_end` doors (GH #1369, GH #1401).
const CALL_END: &str = "has(hop.route) && hop.route == 'in_call_end'";

/// The edges of talky before #1398 (6.13.1), hashed in a canonical form
/// (keys sorted, no whitespace): the golden of pin 4.
/// Re-blessed for GH #1440 (the role's self summary on `in_summary_self`, the
/// brain's `summarize` exclusion, the three keys in the exits'
/// `delete_context`) -- a wanted edge change, old
/// `781c0dc7c4464d412a3943e0cd88016848bb11155b9044803c913c68bbc2290a`.
const EDGES_6_13_1_SHA256: &str =
    "c19ec872ed4c7da304f3e6fa5bf8cebe534db03574ad4f7e68f8e1e8172d34f3";

/// The guard #1398 adds to the brain's model door and to `./curator -> ./quick`.
const DOOR_GUARD: &str =
    " || hop.subscriber.endsWith('/quick_spec') || hop.subscriber.endsWith('/quick_bridge')";
const QUICK_GUARD: &str = " && !(has(hop.spec) && (hop.spec == '1' || hop.spec == 'bridge'))";

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), canonical(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Pin 4: with `quick_speculative` "0" (shipped) not one call reaches a
/// sibling -- the tool round, its bridge and a complex turn's bridge all land
/// on `./quick` and carry no `hop.spec` -- and the edges without the new
/// cells' are 6.13.1's byte for byte, but for the one guard each on the
/// brain's model door and the curator's road onto `./quick`.
#[test]
fn gh1275_speculation_off_sends_nothing_to_the_siblings() {
    if !shipped() {
        return;
    }
    let t = talky_table();
    let mut h = plain_hive();
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "orga",
        "move my dentist to friday",
    );
    let mut calls = h.routed("quick");
    assert_eq!(calls.len(), 2, "{:?} {:?}", h.out, h.stderr);
    seam(&mut h, "t2", "complex", "", "web", "plan my trip to Rome");
    calls.extend(h.routed("quick"));
    assert!(calls.iter().any(is_bridge) && calls.len() == 3, "{calls:?}");
    for c in &calls {
        assert!(c.hop.get("spec").is_none(), "no spec mark: {:?}", c.hop);
        assert_eq!(cells_of(&t, c), vec!["/t/quick".to_string()], "{:?}", c.hop);
    }

    let cfg = read_json(&repo(TALKY));
    // GH #1401 came after 6.13.1: the call end's door into
    // `./session-keeper` is not part of the golden either.
    let call_end_keeper =
        |e: &Value| e["from"] == "." && e["to"] == "./session-keeper" && e["condition"] == CALL_END;
    // GH #1411 came after 6.13.1 as well: the two rim edges that hand a
    // streamed sentence (`partial`) out of `./quick` and `./brain`. The one
    // out of `./quick` carries the speculation guard, so the promise of
    // #1275 (speculation off sends nothing) holds; it is pinned in gh1382 C.
    let partial_rim = |e: &Value| {
        e["to"] == "."
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.starts_with("has(hop.route) && hop.route == 'partial'"))
    };
    let sibling = |e: &Value| {
        call_end_keeper(e)
            || partial_rim(e)
            || ["./quick_spec", "./quick_bridge"]
                .iter()
                .any(|s| e["from"] == *s || e["to"] == *s)
    };
    let mut old: Vec<Value> = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| !sibling(e))
        .cloned()
        .collect();
    let mut guarded = 0;
    for e in old.iter_mut() {
        let c = e["condition"].as_str().unwrap_or_default().to_string();
        let bare = if e["to"] == "./brain" && c.contains(DOOR_GUARD) {
            c.replacen(DOOR_GUARD, "", 1)
        } else if e["from"] == "./curator" && e["to"] == "./quick" && c.ends_with(QUICK_GUARD) {
            c[..c.len() - QUICK_GUARD.len()].to_string()
        } else {
            continue;
        };
        e["condition"] = json!(bare);
        guarded += 1;
    }
    assert_eq!(
        guarded, 3,
        "the brain's door and the curator's two roads onto ./quick"
    );
    let text = sj::to_string(&canonical(&Value::Array(old))).expect("JSON");
    let got = format!("{:x}", Sha256::digest(text.as_bytes()));
    assert_eq!(
        got, EDGES_6_13_1_SHA256,
        "talky's edges moved beyond the siblings"
    );
}

/// Pin 5 (#1382 against the sibling topology): the spec fails on
/// `./quick_spec` (timeout `error`) -- its end reaches only the curator's
/// `in_spec_llm`, never `./errors` or the splitter; the stored call goes onto
/// `./quick`, and when it fails too the turn goes to the brain exactly once.
#[test]
fn gh1382_the_sibling_error_path_still_yields_one_answer() {
    if !shipped() {
        return;
    }
    let t = talky_table();
    let mut h = spec_hive("");
    let spec = spec_call(&mut h, "t1", "please call my daughter");
    let ends = answer_reach(&t, "quick_spec", &spec, "error");
    assert_eq!(targets(&ends), vec!["/t/curator".to_string()], "{ends:?}");
    assert_eq!(ends[0].1.hop["route"], json!("in_spec_llm"));
    seam(&mut h, "t1", "simple", "", "web", "please call my daughter");
    assert_eq!(answers(&h), 0, "the stamp waits for the spec: {:?}", h.out);
    let err = json!({"error_code": "timeout", "error_source": "wire"});
    spec_end(&mut h, &spec, "error", err.clone(), json!([]));
    let fb = h.routed("quick");
    assert_eq!(
        fb.len(),
        1,
        "the stored call goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(cells_of(&t, &fb[0]), vec!["/t/quick".to_string()]);
    h.out.clear();
    h.tap(&fb[0], "error", err, json!([]));
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(answers(&h), 1, "exactly one: {:?}", h.out);
    let esc = marks(&h, "escalate");
    assert_eq!(esc.len(), 1, "{esc:?}");
    assert_eq!(esc[0]["reason"], json!("no_answer"));
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

/// Pin 6 (OR-D53-25, hole 2 of #1382; R034 run 2, trace 01a196b4): the turn
/// moved on -- t1's speculation was dropped at its stamp, t2 has begun -- and
/// t1's spec call answers `stop` only now. That answer reaches no splitter
/// (`./quick_spec` has no road there), releases nothing and sends no stored
/// call into t2: before #1398 the stored call waited for exactly this answer
/// and left with it, into the turn that was open by then.
#[test]
fn gh1382_a_late_stop_answer_of_a_dropped_speculation_never_reaches_a_foreign_turn() {
    if !shipped() {
        return;
    }
    let t = talky_table();
    let mut h = spec_hive("web,weather,story_read");
    let spec1 = spec_call(&mut h, "t1", "move my dentist to friday");
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "orga",
        "move my dentist to friday",
    );
    let t1_sent = answers(&h);
    let spec2 = spec_call(&mut h, "t2", "and how is the weather");
    assert_eq!(spec2.hop["turn_id"], json!("t2"));
    let ends = answer_reach(&t, "quick_spec", &spec1, "stop");
    assert!(
        !targets(&ends).iter().any(|to| to.ends_with("/splitter")),
        "a spec answer never speaks on its own: {ends:?}"
    );
    spec_end(
        &mut h,
        &spec1,
        "stop",
        json!({}),
        json!([said("Done, it is moved.")]),
    );
    assert_eq!(
        answers(&h),
        0,
        "t1's late answer sends nothing into t2 (t1 sent {t1_sent} at its stamp): {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(h.routed("spec_release").is_empty());
    assert_eq!(marks(&h, "late_spec_discarded").len(), 1);
    assert_eq!(h.emit_violations, Vec::<String>::new());
}
