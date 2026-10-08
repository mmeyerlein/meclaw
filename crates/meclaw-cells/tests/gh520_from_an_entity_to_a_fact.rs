//! GH #520 — the graph leg walks to an entity and then reaches the facts that
//! entity is ABOUT.
//!
//! The tier-1 graph leg walked `entity_edges` and threw the nodes away, keeping
//! only the edge's provenance:
//!
//! ```python
//! for r in paths:
//!     eid = (r.get("edge") or {}).get("episode_id")
//!     ...
//!     graph_payload.append({"kind": "episode", "id": eid})
//! ```
//!
//! `kind: "episode"` was the only kind the leg could produce, and the comment
//! above it gave the reason: *the node names live in a different namespace than
//! `facts.subject`*. The first half of that sentence — `entity_edges` carries
//! `episode_id` as its provenance — is true. The second half is not.
//!
//! Measured on a two-week-old hive (28 edges, 55 entities, 182 episodes, 34
//! facts): `src_entity` / `dst_entity` resolve as a row **id** in 0 of 28 cases
//! — they are NAMES — and 15 of the 28 edges have a `dst_entity` that IS a
//! `facts.canonical_subject`, case-insensitively. Not two namespaces: the same
//! names under two normalisations, `entity_edges` keeping the written spelling
//! and `facts` the lower-cased canonical one. The leg was never blocked by the
//! schema. It stopped one join short.
//!
//! Four things are pinned here:
//!
//! 1. the join is ASKED — `facts where canonical_subject in <the walked nodes,
//!    folded>` — and it is asked in its own bundle, because the nodes a walk
//!    passed through are only known after the walk;
//! 2. a fact about a walked entity becomes a graph-leg candidate and travels
//!    all the way into the rendered bundle;
//! 3. the audience gate applies to a graph-leg fact exactly as to a keyword-leg
//!    fact, and a subject no node carries never enters the leg (the red probe);
//! 4. a walk that reached no node costs no extra store message at all.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. No colony, no store, no provider, nothing spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const RID: &str = "r-520";
const AUDIENCE: &str = r#"["member:marcus"]"#;
const CHANNEL: &str = "tg:private";

/// `${VAR:-default}` becomes the default — the same substitution the colony
/// performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str) -> String {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Hand the shipped script to python3 **on stdin**, never in argv (GH #279).
fn run(doc: Value) -> Vec<Value> {
    let script = script_of(RECALL_CONFIG);
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(&script).unwrap(),
        meclaw_core::serde_json::to_string(&meclaw_testing::code_stdin(&doc).to_string()).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "cell exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn ctx(phase: &str) -> Value {
    json!({"mem_phase": phase, "recall_id": RID, "memory_tier": "1",
           "recall_query": "Marcus",
           "audience_now": AUDIENCE, "channel": CHANNEL})
}

/// A store BUNDLE reply (#295): N `tool_result` turns plus the `results[]` slot.
fn bundle_reply(phase: &str, legs: &[(&str, Value)]) -> Value {
    json!({
        "header": {"context": ctx(phase),
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

fn scratch(leg: &str, payload: &Value) -> Value {
    json!({"request_id": RID, "leg": leg, "payload": payload.to_string(), "fired": 0})
}

/// A store row as the STORE answers it: the participant set is a JSON list in a
/// text column, the room is its own column. A row without a set is invisible
/// (README § The rule, in the order it is evaluated), so an untagged fixture row
/// would measure the gate instead of the leg under test.
fn tagged(mut row: Value) -> Value {
    row["audience_set"] = json!(AUDIENCE);
    row["channel"] = json!(CHANNEL);
    row
}

/// The tool_call arguments of one emitted message, in call order.
fn calls_of(m: &Value) -> Vec<Value> {
    m["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|t| t["type"] == "tool_call")
        .map(|t| meclaw_core::serde_json::from_str(t["text"].as_str().expect("text")).unwrap())
        .collect()
}

/// The parked `fused` document of a hydration bundle — the ranking the fusion
/// produced, with the leg attribution on every candidate.
fn fused_of(out: &[Value]) -> Value {
    for a in calls_of(&out[0]) {
        if a["table"] == "recall_scratch" && a["row"]["leg"] == "fused" {
            return meclaw_core::serde_json::from_str(a["row"]["payload"].as_str().unwrap())
                .unwrap();
        }
    }
    panic!("no parked fused document in {out:#?}");
}

/// The parked fan, as `t1-fan` wrote it: this file drives the last two hops, so
/// the four legs in front of them are empty on purpose — every candidate that
/// appears below was carried by the graph leg and by nothing else.
fn legs_row() -> Value {
    json!({"kw-ep": [], "kw-fact": [], "temporal": [], "beliefs": [],
           "anchors": ["Marcus"], "axis": {},
           "model": {"model_id": "m-1", "dim": 1024}})
}

/// The seed of every case below: the walk the anchor `Marcus` produced. The
/// spellings are the ones `entity_edges` keeps — capitalised, with a dot — and
/// `facts.canonical_subject` holds the lower-cased ones.
fn walk() -> Value {
    json!({"paths": [
        {"node": "Marcus", "depth": 1, "weight_sum": 9, "path": ["Alex", "Marcus"],
         "edge": tagged(json!({"episode_id": "ep-1"}))},
        {"node": "acme.example", "depth": 2, "weight_sum": 3,
         "path": ["Alex", "Marcus", "acme.example"],
         "edge": tagged(json!({"episode_id": "ep-2"}))}],
        "truncated": false})
}

/// The reply of `t1-legs`: the walk, the semantic companion and the read-back.
fn legs_reply(paths: Value) -> Value {
    bundle_reply(
        "t1-legs",
        &[
            ("r-legs-graph", paths),
            ("r-legs-sem-aud", json!([])),
            (
                "r-legs-read",
                json!([scratch("legs", &legs_row()), scratch("sem", &json!([]))]),
            ),
        ],
    )
}

/// The reply of `t1-graph`: the fact page the join asked for, and the parking
/// place with the rows the join hop wrote into it.
fn graph_reply(out: &[Value], facts: Value) -> Value {
    let mut page = vec![scratch("legs", &legs_row()), scratch("sem", &json!([]))];
    for a in calls_of(&out[0]) {
        if a["table"] == "recall_scratch" && a["operation"] == "insert" {
            page.push(json!({"request_id": RID, "leg": a["row"]["leg"],
                             "payload": a["row"]["payload"], "fired": 0}));
        }
    }
    bundle_reply(
        "t1-graph",
        &[("r-graph-fact", facts), ("r-graph-read", json!(page))],
    )
}

/// `t1-legs` + `t1-graph` in one call: the fusion, driven through the join.
fn fuse(paths: Value, facts: Value) -> Vec<Value> {
    let join = run(legs_reply(paths));
    assert_eq!(
        join[0]["header"]["phase"], "t1-graph",
        "a walk with nodes asks the join first: {join:#?}"
    );
    run(graph_reply(&join, facts))
}

// ════════════════════════════════════════════════════════════ 1. the join is asked

/// The nodes are folded to the column's normalisation on THIS side of the wire:
/// the store's `in` filter is exact, `facts.canonical_subject` is consistently
/// lower-case and the `entity_edges` nodes are not.
#[test]
fn the_walk_asks_the_fact_table_for_the_nodes_it_reached() {
    let out = run(legs_reply(walk()));
    assert_eq!(out.len(), 1, "one store message: {out:#?}");
    assert_eq!(out[0]["header"]["phase"], "t1-graph");
    let calls = calls_of(&out[0]);
    // GH #1057: one page per walked node, newest first -- a single page over
    // all nodes came back in store order, oldest first, and cut the current
    // value before any ranking saw it.
    let pages: Vec<&Value> = calls.iter().filter(|a| a["table"] == "facts").collect();
    assert!(!pages.is_empty(), "the join select");
    let fact = pages[0];
    assert_eq!(fact["operation"], "select");
    let wheres: Vec<Value> = pages.iter().map(|a| a["where"].clone()).collect();
    assert_eq!(
        wheres,
        vec![
            json!({"canonical_subject": "marcus"}),
            json!({"canonical_subject": "acme.example"})
        ],
        "folded, deduplicated, in walk order: {fact}"
    );
    assert_eq!(
        fact["order_by"],
        json!([{"col": "valid_from", "dir": "desc"}])
    );
    // R1: the version chain is the truth and the cache columns are not
    // consulted. A superseded hit is projected onto its successor in `t1-emit`,
    // exactly as a keyword-leg fact is — it is not dropped in a where clause.
    assert!(fact["where"].get("expired_at").is_none(), "{fact}");
    assert!(fact["where"].get("superseded_by").is_none(), "{fact}");
    // A graph fact is in neither the fan's axis map nor the semantic companion,
    // so its own page carries the gate columns AND the canonical axis keys.
    for col in [
        "canonical_subject",
        "canonical_predicate",
        "channel",
        "audience_set",
    ] {
        assert!(
            fact["columns"].as_array().unwrap().iter().any(|c| c == col),
            "the join page must project `{col}`: {fact}"
        );
    }
    // The walk is parked, not recomputed: a traverse cannot be asked twice for
    // the same page without paying for it twice.
    assert!(
        calls.iter().any(|a| a["row"]["leg"] == "graph-walk"),
        "the walk is parked for the hop that fuses: {calls:#?}"
    );
    assert_eq!(
        calls.last().expect("read-back")["operation"],
        "select",
        "the read-back is LAST, so it sees the parks in front of it"
    );
}

/// The seventh message is the ONLY conditional one in the tier-1 chain (#418
/// counts six): a walk that reached no node has nothing to join, and fuses
/// where it always did.
#[test]
fn a_walk_that_reached_no_node_costs_no_seventh_message() {
    let out = run(legs_reply(json!({"paths": []})));
    assert_eq!(out.len(), 1, "{out:#?}");
    assert_eq!(
        out[0]["header"]["phase"], "t1-emit",
        "no node, no join — straight to the hydration: {out:#?}"
    );
}

// ═══════════════════════════════════════════ 2. the fact becomes a candidate

/// The acceptance criterion of the issue, in one assertion: a tier-1 request
/// whose anchors reach an entity that a fact is about returns that fact as a
/// GRAPH-leg candidate. Nothing else could have carried it — the other three
/// legs are empty in `legs_row`.
#[test]
fn a_fact_about_a_walked_entity_is_a_graph_leg_candidate() {
    let out = fuse(
        walk(),
        json!([tagged(
            json!({"id": "f-1", "canonical_subject": "acme.example",
                             "canonical_predicate": "founded_in"})
        )]),
    );
    let fused = fused_of(&out);
    let carried: Vec<(String, String)> = fused["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|c| c["legs"].as_array().unwrap().iter().any(|l| l == "graph"))
        .map(|c| {
            (
                c["kind"].as_str().unwrap().to_string(),
                c["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        carried.contains(&("fact".into(), "f-1".into())),
        "from an entity there is now a path to a fact: {fused}"
    );
    // Walk order IS the leg's ranking (depth asc, weight_sum desc, node asc),
    // so each node's episode comes first and that node's facts follow it.
    assert_eq!(
        carried,
        vec![
            ("episode".to_string(), "ep-1".to_string()),
            ("episode".to_string(), "ep-2".to_string()),
            ("fact".to_string(), "f-1".to_string())
        ],
        "the episodes keep the order they always had, the facts ride behind \
         their own node: {fused}"
    );
    assert_eq!(
        fused["leg_sizes"]["graph"], 3,
        "the leg reports the size it fused with: {fused}"
    );
}

/// …and it is hydrated and rendered like any other fact. The bundle is what the
/// answering model sees, so a candidate that never reaches it never happened.
#[test]
fn the_graph_leg_fact_reaches_the_rendered_bundle() {
    let out = fuse(
        walk(),
        json!([tagged(
            json!({"id": "f-1", "canonical_subject": "acme.example",
                             "canonical_predicate": "founded_in",
                             "subject": "acme.example", "predicate": "founded_in"})
        )]),
    );
    let fused = fused_of(&out);
    // The hydration bundle names the axis of the graph fact too — the join page
    // carried its canonical keys for exactly this reason.
    let axis = calls_of(&out[0])
        .into_iter()
        .find(|a| a["table"] == "facts" && a["where"].get("canonical_predicate").is_some())
        .expect("the axis page");
    assert_eq!(
        axis["where"]["canonical_subject"]["in"],
        json!(["acme.example"])
    );
    assert_eq!(
        axis["where"]["canonical_predicate"]["in"],
        json!(["founded_in"])
    );

    let hyd = json!([tagged(
        json!({"id": "f-1", "claim": "acme.example was founded in 2025",
                                   "subject": "acme.example", "predicate": "founded_in",
                                   "canonical_subject": "acme.example",
                                   "canonical_predicate": "founded_in",
                                   "valid_from": "2025-02-01T00:00:00Z",
                                   "session_id": "s1", "episode_id": "ep-2"})
    )]);
    let rows = json!([
        {"leg": "fused", "payload": fused.to_string()},
        {"leg": "hyd-fact", "payload": hyd.to_string()},
        {"leg": "hyd-axis", "payload": "[]"},
        {"leg": "card", "payload": "{}"},
        {"leg": "hyd-ep", "payload": "[]"}
    ]);
    let emit = run(json!({
        "header": {"context": ctx("t1-emit"),
                   "hop": {"operation": "select", "rows_affected": 5}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r-hyd",
                      "text": rows.to_string()}]
    }));
    let text = emit[0]["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("the tier-1 bundle");
    assert!(
        text.contains("acme.example was founded in 2025"),
        "the graph leg's fact is in the bundle the model reads: {text}"
    );
    let diag = &emit[0]["recall_diagnostic"];
    assert!(
        diag["candidates"][0]["legs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "graph"),
        "…and the diagnostic names the leg that carried it: {diag}"
    );
}

// ══════════════════════════════════════════════ 3. the gate and the red probe

/// The red probe. The `in` filter is a page, not a promise: a store may answer
/// with a row whose subject no node of this walk carries, and a leg that voted
/// for it would be voting on nothing.
#[test]
fn a_fact_whose_subject_no_node_carries_never_enters_the_leg() {
    let out = fuse(
        walk(),
        json!([tagged(
            json!({"id": "f-berlin", "canonical_subject": "berlin"})
        )]),
    );
    let fused = fused_of(&out);
    let ids: Vec<&str> = fused["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["ep-1", "ep-2"],
        "only a subject the walk actually reached becomes a candidate: {fused}"
    );
}

/// The audience gate applies to a graph-leg fact exactly as to a keyword-leg
/// fact, and a row without a participant set is INVISIBLE, not visible (README
/// § The rule, in the order it is evaluated). Fail-closed, before the ranking
/// and therefore before the fusion.
#[test]
fn the_audience_gate_applies_to_a_graph_leg_fact() {
    for row in [
        json!({"id": "f-hidden", "canonical_subject": "marcus"}),
        json!({"id": "f-theirs", "canonical_subject": "marcus",
               "audience_set": ["member:someone-else"], "channel": CHANNEL}),
    ] {
        let fused = fused_of(&fuse(walk(), json!([row.clone()])));
        let ids: Vec<&str> = fused["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["ep-1", "ep-2"],
            "a fact this round may not see must not reach the RRF sum: {row}"
        );
    }
}

// ═══════════════════════════════════════════════════ 4. the surface says so

/// The drift lock for the sentence on the public template surface
/// (development-rules § 2d): the README's four-leg table says what the graph
/// leg yields, and the shipped script must agree.
#[test]
fn the_readme_says_what_the_graph_leg_yields() {
    let readme = std::fs::read_to_string("../../templates/memory-hive/README.md").expect("README");
    let row = readme
        .lines()
        .find(|l| l.starts_with("| graph |"))
        .expect("the graph row of the four-leg table");
    assert!(
        row.contains("facts"),
        "the table still promises an episode-only leg: {row}"
    );
    let script = script_of(RECALL_CONFIG);
    assert!(
        !script.contains("live in a different namespace than facts.subject"),
        "the reason the code gave for the missing join is measured false and must go"
    );
    assert!(
        script.contains("def graph_leg_of("),
        "the leg that joins is what the sentence describes"
    );
    // Params of `./recall` since GH #138, so the README documents them under the
    // name an `override_params` entry has to use.
    for knob in ["tier1_graph_fact_nodes", "tier1_graph_fact_limit"] {
        assert!(
            readme.contains(knob),
            "the join's cap `{knob}` is an undocumented knob"
        );
    }
}

// ═════════════════════ 4. the current first-hand value first (GH #1057, K2-M)
//
// Measured on a six-month boundary test (120 probes, five of them turned red by
// the graph leg): a graph hit ranked an older value or a rumour before the
// current value the person stated themselves. Three mechanisms, one per case
// family below: the stems of a NAME counted as question words (a rumour and an
// old user statement spell the name, the person's own statement says "they"),
// a node the question never named seated its facts over an edge that added no
// question word (a rumour edge "Leo drives an orange Citroen" seated Leo's cars
// for a question about somebody else's car), and the replaced value of an axis
// kept its seat beside the current one. The fixtures carry the shapes of the
// measured rows; names and references are invented.

/// Run one fixture through `t1-legs` and `t1-graph` and return the graph leg's
/// candidates in rank order. The fan legs are empty, so the fusion order IS the
/// graph leg's order. Every path edge and every fact is tagged with the
/// asker's round, so the gate never decides a case here.
fn graph_case(fixture: &str) -> Vec<String> {
    let case: Value = meclaw_core::serde_json::from_str(fixture).expect("fixture json");
    let query = case["query"].as_str().expect("query").to_string();
    let reply = |phase: &str, legs: Vec<(&str, Value)>| {
        json!({
            "header": {"context": {"mem_phase": phase, "recall_id": RID, "memory_tier": "1",
                                   "recall_query": query, "audience_now": AUDIENCE,
                                   "channel": CHANNEL},
                       "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
            "messages": legs.iter().map(|(id, rows)| json!(
                {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
                .collect::<Vec<_>>(),
            "results": legs.iter().map(|(id, _)| json!(
                {"tool_call_id": id, "operation": "select", "rows_affected": 1,
                 "duration_ms": 0})).collect::<Vec<_>>()
        })
    };
    let legs = json!({"kw-ep": [], "kw-fact": [], "temporal": [], "beliefs": [],
                      "anchors": [], "anchor_rank": case["anchor_rank"], "axis": {},
                      "model": {"model_id": "m-1", "dim": 1024}});
    let paths: Vec<Value> = case["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .map(|p| {
            let mut p = p.clone();
            p["edge"] = tagged(p["edge"].clone());
            p
        })
        .collect();
    let facts: Vec<Value> = case["facts"]
        .as_array()
        .expect("facts")
        .iter()
        .cloned()
        .map(tagged)
        .collect();
    let join = run(reply(
        "t1-legs",
        vec![
            ("r-legs-graph", json!({"paths": paths, "truncated": false})),
            ("r-legs-sem-aud", json!([])),
            (
                "r-legs-read",
                json!([scratch("legs", &legs), scratch("sem", &json!([]))]),
            ),
        ],
    ));
    assert_eq!(join[0]["header"]["phase"], "t1-graph", "{join:#?}");
    let mut page = vec![scratch("legs", &legs), scratch("sem", &json!([]))];
    for a in calls_of(&join[0]) {
        if a["table"] == "recall_scratch" && a["operation"] == "insert" {
            page.push(json!({"request_id": RID, "leg": a["row"]["leg"],
                             "payload": a["row"]["payload"], "fired": 0}));
        }
    }
    let out = run(reply(
        "t1-graph",
        vec![
            ("r-graph-fact", json!(facts)),
            ("r-graph-read", json!(page)),
        ],
    ));
    fused_of(&out)["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .filter(|c| c["legs"].as_array().unwrap().iter().any(|l| l == "graph"))
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

fn seat(leg: &[String], id: &str) -> Option<usize> {
    leg.iter().position(|x| x == id)
}

/// Probe shape stale_vs_current-73: "In which city does Rufus live now?" -- the
/// old user statement (Lyon) and two rumours spell the name and ranked first,
/// the person's own current statement (Krakow, "they still live") was cut off
/// the node page.
const CURRENT_RESIDENCE: &str = r#"{
  "query": "In which city does Ruth Rainer live now?",
  "anchor_rank": {"ruth rainer": 0, "cora cole": 1},
  "paths": [
    {"node": "ruth rainer", "depth": 1, "weight_sum": 1, "path": ["cora cole", "ruth rainer"],
     "edge": {"episode_id": "ep-flat",
              "relation": "The member stated that Cora Cole and Ruth Rainer are each other's flatmates."}}],
  "facts": [
    {"id": "f-lyon", "canonical_subject": "ruth rainer", "subject": "Ruth Rainer", "source": "",
     "predicate": "lives_in", "canonical_predicate": "lives_in",
     "claim": "Ruth Rainer lives in Lyon.", "valid_from": "2025-04-04T11:02:00Z"},
    {"id": "f-saab", "canonical_subject": "ruth rainer", "subject": "Ruth Rainer", "source": "bb11cc22",
     "predicate": "reported_car", "canonical_predicate": "reported_car",
     "claim": "Peer bb11cc22 said they think Ruth Rainer drives a black Saab, based on what Carl heard.",
     "valid_from": "2025-05-27T09:00:00Z"},
    {"id": "f-rostock", "canonical_subject": "ruth rainer", "subject": "Ruth Rainer", "source": "bb11cc22",
     "predicate": "reported_move", "canonical_predicate": "reported_move",
     "claim": "Peer bb11cc22 said they heard Ruth Rainer moved to Rostock, attributing the report to what Carl heard.",
     "valid_from": "2025-06-16T09:48:00Z"},
    {"id": "f-moved", "canonical_subject": "ruth rainer", "subject": "peer aa11bb22", "source": "aa11bb22",
     "predicate": "relocation", "canonical_predicate": "relocation",
     "claim": "Peer aa11bb22 stated that they moved from Lyon to Krakow.", "valid_from": "2025-06-13T13:09:00Z"},
    {"id": "f-krakow", "canonical_subject": "ruth rainer", "subject": "peer aa11bb22", "source": "aa11bb22",
     "predicate": "residence", "canonical_predicate": "lives_in",
     "claim": "Peer aa11bb22 stated that they still live in Krakow and denied having moved to Rostock.",
     "valid_from": "2025-06-20T17:34:00Z"}]
}"#;

#[test]
fn the_current_first_hand_value_leads_and_the_replaced_one_and_rumours_get_no_seat() {
    let leg = graph_case(CURRENT_RESIDENCE);
    assert_eq!(
        leg.first().map(String::as_str),
        Some("f-krakow"),
        "the person's own current statement leads the graph leg: {leg:?}"
    );
    for gone in ["f-lyon", "f-rostock", "f-saab"] {
        assert_eq!(
            seat(&leg, gone),
            None,
            "{gone}: a value the current one replaced, or a rumour where the person \
             answered, takes no seat for a question about now: {leg:?}"
        );
    }
    assert_eq!(
        seat(&leg, "f-moved"),
        Some(1),
        "the statement that changed the value follows it: {leg:?}"
    );
    assert_eq!(
        seat(&leg, "ep-flat"),
        None,
        "an edge that shares only the name with the question brings no episode: {leg:?}"
    );
}

/// Probe shape stale_vs_current-74 (same question): the rumour edge "x heard
/// Rufus moved to Rostock" spelled the asked name, ranked first in the walk and
/// put the rumour episode and the reporter's own residence into the leg.
const RUMOUR_EDGE: &str = r#"{
  "query": "In which city does Ruth Rainer live now?",
  "anchor_rank": {"ruth rainer": 0},
  "paths": [
    {"node": "carl holm", "depth": 1, "weight_sum": 1, "path": ["ruth rainer", "carl holm"],
     "edge": {"episode_id": "ep-rumour",
              "relation": "Peer bb11cc22 said they heard Ruth Rainer moved to Rostock, attributing the report to what Carl heard."}}],
  "facts": [
    {"id": "f-ghent", "canonical_subject": "carl holm", "subject": "peer bb11cc22", "source": "bb11cc22",
     "predicate": "residence", "canonical_predicate": "lives_in",
     "claim": "Peer bb11cc22 says they still live in Ghent and denies having moved to Tromso.",
     "valid_from": "2025-06-16T10:00:00Z"}]
}"#;

#[test]
fn an_edge_that_only_spells_the_asked_name_seats_nothing() {
    let leg = graph_case(RUMOUR_EDGE);
    assert!(
        leg.is_empty(),
        "neither the rumour episode nor the reporter's residence answers the question: {leg:?}"
    );
}

/// Probe shape stale_vs_current-62: "What car does Henrik drive now?" -- the
/// rumour (red Fiat) led the person's own word (orange Citroen, not a red Fiat),
/// and a rumour edge about a neighbour's car seated the neighbour's cars.
const OWN_WORD_OVER_RUMOUR: &str = r#"{
  "query": "What car does Hank Fallow drive now?",
  "anchor_rank": {"hank fallow": 0, "jo ahl": 2},
  "paths": [
    {"node": "hank fallow", "depth": 1, "weight_sum": 1, "path": ["jo ahl", "hank fallow"],
     "edge": {"episode_id": "ep-neighbours",
              "relation": "The member stated that Jo Ahl and Hank Fallow are neighbours."}},
    {"node": "leo varn", "depth": 1, "weight_sum": 1, "path": ["jo ahl", "leo varn"],
     "edge": {"episode_id": "ep-citroen",
              "relation": "Peer jj33kk44 said they thought Leo Varn drives an orange Citroen, adding that this was what Jo heard."}}],
  "facts": [
    {"id": "f-fiat", "canonical_subject": "hank fallow", "subject": "Hank Fallow", "source": "nn55oo66",
     "predicate": "car", "canonical_predicate": "car",
     "claim": "Peer nn55oo66 said they think Hank Fallow drives a red Fiat, based on what Nina heard.",
     "valid_from": "2025-04-21T10:19:00Z"},
    {"id": "f-not-fiat", "canonical_subject": "hank fallow", "subject": "peer hh77ii88", "source": "hh77ii88",
     "predicate": "car", "canonical_predicate": "car",
     "claim": "Peer hh77ii88 stated that they drive an orange Citroen, not a red Fiat.",
     "valid_from": "2025-04-21T18:00:00Z"},
    {"id": "f-citroen", "canonical_subject": "hank fallow", "subject": "peer hh77ii88", "source": "hh77ii88",
     "predicate": "drives", "canonical_predicate": "drives",
     "claim": "Peer hh77ii88 stated that they drive an orange Citroen.", "valid_from": "2025-02-24T09:00:00Z"},
    {"id": "f-volvo", "canonical_subject": "leo varn", "subject": "peer ll99mm00", "source": "ll99mm00",
     "predicate": "car", "canonical_predicate": "car",
     "claim": "Peer ll99mm00 said they drive a blue Volvo, not an orange Citroen, and that people confuse them.",
     "valid_from": "2025-06-25T09:00:00Z"}]
}"#;

#[test]
fn a_rumour_never_leads_the_persons_own_word_and_a_neighbours_car_gets_no_seat() {
    let leg = graph_case(OWN_WORD_OVER_RUMOUR);
    assert_eq!(
        leg.first().map(String::as_str),
        Some("f-not-fiat"),
        "the person's own current word leads: {leg:?}"
    );
    assert_eq!(
        seat(&leg, "f-fiat"),
        None,
        "where the person answered, the rumour takes no graph seat: {leg:?}"
    );
    assert_eq!(
        seat(&leg, "f-volvo"),
        None,
        "a node the question never named, reached over an edge that adds no \
         question word, seats nothing: {leg:?}"
    );
    if let Some(ep) = seat(&leg, "ep-citroen") {
        assert!(
            ep > seat(&leg, "f-not-fiat").unwrap(),
            "a path episode (here a rumour about somebody else) rides behind every \
             seated fact: {leg:?}"
        );
    }
}

/// Probe shape multihop-25: "In which city does the flatmate of my grandson
/// live now?" -- the target's replaced value (Bremen) kept its seat beside the
/// current one (Salzburg) and out-voted it in the fusion.
const REPLACED_VALUE_OF_THE_TARGET: &str = r#"{
  "query": "In which city does the flatmate of my grandson live now?",
  "anchor_rank": {"gina grimm": 1},
  "paths": [
    {"node": "fay quast", "depth": 1, "weight_sum": 1, "path": ["gina grimm", "fay quast"],
     "edge": {"episode_id": "ep-flatmates",
              "relation": "Fay Quast and Gina Grimm are each other's flatmates."}}],
  "facts": [
    {"id": "f-bremen", "canonical_subject": "fay quast", "subject": "Fay Quast", "source": "",
     "predicate": "lives_in", "canonical_predicate": "lives_in",
     "claim": "Fay Quast lives in Bremen.", "valid_from": "2025-03-18T10:00:00Z"},
    {"id": "f-relocated", "canonical_subject": "fay quast", "subject": "Fay Quast", "source": "",
     "predicate": "relocated", "canonical_predicate": "relocated",
     "claim": "Fay Quast moved from Bremen to Salzburg.", "valid_from": "2025-04-25T10:00:00Z"},
    {"id": "f-bergen", "canonical_subject": "fay quast", "subject": "Fay Quast", "source": "mm11nn22",
     "predicate": "reported_move", "canonical_predicate": "reported_move",
     "claim": "Peer mm11nn22 said they had heard Fay Quast moved to Bergen, adding that this was what Mia heard.",
     "valid_from": "2025-04-28T09:00:00Z"},
    {"id": "f-salzburg", "canonical_subject": "fay quast", "subject": "peer ff33gg44", "source": "ff33gg44",
     "predicate": "residence", "canonical_predicate": "lives_in",
     "claim": "Peer ff33gg44 stated that they still live in Salzburg and denied having moved to Bergen.",
     "valid_from": "2025-04-28T12:00:00Z"}]
}"#;

#[test]
fn the_replaced_value_of_a_walked_node_gets_no_seat_for_a_question_about_now() {
    let leg = graph_case(REPLACED_VALUE_OF_THE_TARGET);
    let facts: Vec<&str> = leg
        .iter()
        .map(String::as_str)
        .filter(|x| x.starts_with("f-"))
        .collect();
    assert_eq!(
        facts,
        vec!["f-salzburg", "f-relocated"],
        "the current value, then the change that led to it; neither the replaced \
         value nor the rumour the person denied: {leg:?}"
    );
    assert_eq!(
        seat(&leg, "ep-flatmates"),
        Some(2),
        "the hop's episode (the question's 'flatmate') rides behind the facts: {leg:?}"
    );
}

/// Probe shape multihop-34: "What does the business partner of my family
/// doctor work as now?" -- the walk reached nodes over a flatmate edge and over
/// a rumour edge "x heard Val works as a midwife", and their jobs took every
/// seat of the leg; the hop the question asks for is the business partner one.
const THE_HOP_THE_QUESTION_ASKS_FOR: &str = r#"{
  "query": "What does the business partner of my family doctor work as now?",
  "anchor_rank": {"ola ubbel": 1, "gus rasch": 2},
  "paths": [
    {"node": "sue ost", "depth": 1, "weight_sum": 1, "path": ["ola ubbel", "sue ost"],
     "edge": {"episode_id": "ep-partners",
              "relation": "The member stated that Ola Ubbel and Sue Ost are each other's business partners."}},
    {"node": "eda dorn", "depth": 1, "weight_sum": 1, "path": ["gus rasch", "eda dorn"],
     "edge": {"episode_id": "ep-mates",
              "relation": "The member stated that Gus Rasch and Eda Dorn are each other's flatmates."}},
    {"node": "val mert", "depth": 1, "weight_sum": 1, "path": ["ola ubbel", "val mert"],
     "edge": {"episode_id": "ep-midwife",
              "relation": "Peer pp11qq22 said Mara had heard that Val Mert works as a midwife; this was relayed as hearsay."}}],
  "facts": [
    {"id": "f-sue-job", "canonical_subject": "sue ost", "subject": "Sue Ost", "source": "",
     "predicate": "occupation", "canonical_predicate": "occupation",
     "claim": "Sue Ost works as a nurse.", "valid_from": "2025-06-13T10:00:00Z"},
    {"id": "f-eda-job", "canonical_subject": "eda dorn", "subject": "Eda Dorn", "source": "",
     "predicate": "occupation", "canonical_predicate": "occupation",
     "claim": "The member stated that Eda Dorn works as an architect.", "valid_from": "2025-03-13T10:00:00Z"},
    {"id": "f-val-job", "canonical_subject": "val mert", "subject": "Val Mert", "source": "",
     "predicate": "occupation", "canonical_predicate": "occupation",
     "claim": "Val Mert works as a beekeeper.", "valid_from": "2025-04-28T10:00:00Z"}]
}"#;

#[test]
fn only_the_hop_the_question_asks_for_seats_the_answer() {
    let leg = graph_case(THE_HOP_THE_QUESTION_ASKS_FOR);
    let facts: Vec<&str> = leg
        .iter()
        .map(String::as_str)
        .filter(|x| x.starts_with("f-"))
        .collect();
    assert_eq!(
        facts,
        vec!["f-sue-job"],
        "the partner's job; not the job of a node reached over a flatmate edge, \
         nor of one reached over a rumour that already names the job: {leg:?}"
    );
}

/// Deep review of kf89 (N4): the question names no rank-0 anchor ("the
/// flatmate of my grandson"), and the extractor wrote the edge as "lives with"
/// -- the same question word as the answer. The strict `via` rule (a fact
/// takes a seat only where the edge adds a question word) left the hop
/// without an answer; without a rank-0 anchor a first-hand fact with a
/// question word of its own keeps its seat.
const FLATMATE_LIVES_WITH: &str = r#"{
  "query": "In which city does the flatmate of my grandson live now?",
  "anchor_rank": {"jonas kraft": 1},
  "paths": [
    {"node": "lena salz", "depth": 1, "weight_sum": 1, "path": ["jonas kraft", "lena salz"],
     "edge": {"episode_id": "ep-flat", "relation": "Lena Salz lives with Jonas Kraft."}}],
  "facts": [
    {"id": "f-salz", "canonical_subject": "lena salz", "subject": "Lena Salz", "source": "",
     "predicate": "lives_in", "canonical_predicate": "lives_in",
     "claim": "Lena Salz lives in Salzburg now.", "valid_from": "2025-06-01T09:00:00Z"},
    {"id": "f-nurse", "canonical_subject": "lena salz", "subject": "Lena Salz", "source": "",
     "predicate": "works_as", "canonical_predicate": "works_as",
     "claim": "Lena Salz works as a nurse.", "valid_from": "2025-05-01T09:00:00Z"}]
}"#;

#[test]
fn without_a_named_anchor_a_fact_with_its_own_question_word_keeps_its_seat() {
    let leg = graph_case(FLATMATE_LIVES_WITH);
    assert!(
        seat(&leg, "f-salz").is_some(),
        "the answer lost its seat: {leg:?}"
    );
    assert!(
        seat(&leg, "f-nurse").is_none(),
        "no question word, no seat: {leg:?}"
    );
}

/// Review T3: a fact without `valid_from` has an UNKNOWN time, not the oldest
/// one -- a question about now never drops it behind an older value.
const NOW_WITHOUT_A_TIME: &str = r#"{
  "query": "In which city does Ruth Rainer live now?",
  "anchor_rank": {"ruth rainer": 0, "cora cole": 1},
  "paths": [
    {"node": "ruth rainer", "depth": 1, "weight_sum": 1, "path": ["cora cole", "ruth rainer"],
     "edge": {"episode_id": "ep-flat",
              "relation": "The member stated that Cora Cole and Ruth Rainer are each other's flatmates."}}],
  "facts": [
    {"id": "f-old", "canonical_subject": "ruth rainer", "subject": "Ruth Rainer", "source": "",
     "predicate": "lives_in", "canonical_predicate": "lives_in",
     "claim": "Ruth Rainer lives in Lyon.", "valid_from": "2025-03-01T09:00:00Z"},
    {"id": "f-new", "canonical_subject": "ruth rainer", "subject": "Ruth Rainer", "source": "",
     "predicate": "lives_in", "canonical_predicate": "lives_in",
     "claim": "Ruth Rainer lives in Krakow.", "valid_from": ""}]
}"#;

#[test]
fn a_fact_without_a_time_is_never_taken_for_the_oldest() {
    let leg = graph_case(NOW_WITHOUT_A_TIME);
    assert!(
        seat(&leg, "f-new").is_some(),
        "the timeless value was dropped: {leg:?}"
    );
}
