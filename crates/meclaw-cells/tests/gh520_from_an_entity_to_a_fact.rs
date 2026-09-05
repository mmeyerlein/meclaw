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
        {"node": "Marcus", "depth": 1, "weight_sum": 9,
         "edge": tagged(json!({"episode_id": "ep-1"}))},
        {"node": "acme.example", "depth": 2, "weight_sum": 3,
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
    let fact = calls
        .iter()
        .find(|a| a["table"] == "facts")
        .expect("the join select");
    assert_eq!(fact["operation"], "select");
    assert_eq!(
        fact["where"],
        json!({"canonical_subject": {"in": ["marcus", "acme.example"]}}),
        "folded, deduplicated, in walk order: {fact}"
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
