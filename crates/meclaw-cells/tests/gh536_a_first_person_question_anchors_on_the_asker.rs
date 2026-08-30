//! GH #536 — a question in the first person anchors on whoever is asking it.
//!
//! *"What are my sons called?"* names nobody. The tier-1 graph anchors are built
//! from the query text and from the subjects of the keyword hits, so such a
//! question resolved its own interrogative words against `entities` — measured
//! on a live hive, where the question was asked in German (the anchors below are
//! that run's, verbatim; the case in this file asks the same question in English,
//! and the property is the same one — an interrogative word is not an entity in
//! any language):
//!
//! ```text
//! r-join-anchor  select entities where canonical_name in
//!                ["Heissen","Meine","Soehne","heissen","meine","soehne"]  ->  []
//! fused          leg_sizes    {keyword: 20, semantic: 20, graph: 0, temporal: 20}
//!                candidates   fact: 0   episode: 20
//! ```
//!
//! The hive HELD the answer — two `has_child` facts, both visible to that round —
//! and the bundle carried no `FACTS` section at all, so the model answered out of
//! the episodes it did get, among them its own earlier "nothing about this is
//! stored". Four legs, none of which could reach the row: the keyword leg has no
//! lexical overlap between *sons* and `has_child`, the semantic leg competes with
//! a corpus of episodes, the temporal leg has no vote in point mode (O-4), and the
//! graph leg never started because there was no anchor.
//!
//! The asker was never missing from the request. `audience_now` carries the
//! participant set, and its `member:` tokens are people. Four things are pinned
//! here:
//!
//! 1. the asker's own subjects are ASKED FOR — in the fan's own bundle, so the
//!    leg costs no round trip of its own;
//! 2. the asker leads the graph anchors, in front of everything the query named;
//! 3. a fact about the asker becomes a `self`-leg candidate and reaches the fused
//!    ranking — with every query-driven leg empty, nothing else could carry it;
//! 4. the audience gate applies to a self fact exactly as to any other, and a
//!    fact about somebody else never enters the leg.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. No colony, no store, no provider, nothing spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const RID: &str = "r-536";
/// The asking round, in affinity vocabulary: one agent and one member. The
/// member token is the only identity in the whole request — there is no
/// `memory_holder` and no `user_id`, exactly as a shipped member promotes it.
const AUDIENCE: &str = r#"["agent:aide", "member:alex"]"#;
const CHANNEL: &str = "tg:private";
/// The question of the issue, in the shape that has no anchor: no name, no
/// entity, nothing but the asker's own pronoun. The live measurement asked it in
/// German; the English wording is used here because no assertion in this file
/// reads a token of it — it is the recall query and nothing else — and an
/// exported test carries its literals in English (export rule R8).
const QUESTION: &str = "What are my sons called?";

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
           "recall_query": QUESTION,
           "audience_now": AUDIENCE, "channel": CHANNEL})
}

/// The request as the member's port edge delivers it: `hop.phase == "recall"`
/// and no operation, which is how the echo guard tells a fresh question from an
/// answer of our own (GH #152).
fn request() -> Value {
    json!({
        "header": {"context": {"recall_id": "", "mem_phase": "", "memory_tier": "1",
                               "recall_query": QUESTION,
                               "audience_now": AUDIENCE, "channel": CHANNEL},
                   "hop": {"route": "in_query", "phase": "recall"}},
        "messages": []
    })
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
/// text column, the room is its own column. A row without a set is invisible, so
/// an untagged fixture row would measure the gate instead of the leg under test.
fn tagged(mut row: Value) -> Value {
    row["audience_set"] = json!(AUDIENCE);
    row["channel"] = json!(CHANNEL);
    row
}

/// One of the two rows the live hive held and never returned. The subject is the
/// LEGACY spelling — `user`, written before the extraction lane put person names
/// in that column — because that is where the measured misses were.
fn a_son(id: &str, name: &str) -> Value {
    tagged(json!({"id": id, "episode_id": "ep-old", "subject": "user",
                  "canonical_subject": "user", "predicate": "has_child",
                  "canonical_predicate": "has_child", "claim": name,
                  "fact_kind": "world", "valid_from": "2026-08-19T07:22:16Z",
                  "valid_until": null, "recorded_at": "2026-08-19T07:22:16Z"}))
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

/// The tool_call ids of one emitted message, in call order.
fn ids_of(m: &Value) -> Vec<String> {
    m["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|t| t["type"] == "tool_call")
        .map(|t| t["id"].as_str().expect("id").to_string())
        .collect()
}

/// The store bundle a request emits — the fan. The second message is the embed
/// request, which is not a store message at all.
fn fan_of(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["phase"] == "t1-fan")
        .unwrap_or_else(|| panic!("no t1-fan bundle in {out:#?}"))
        .clone()
}

/// One `(id, args)` pair of a bundle, by call id.
fn call(m: &Value, id: &str) -> Option<Value> {
    ids_of(m)
        .iter()
        .position(|i| i == id)
        .map(|i| calls_of(m)[i].clone())
}

/// Drive the fan's reply and read back the `legs` row it parks — the real one,
/// built by the shipped code out of the pages below, never hand-written.
fn parked_legs(self_rows: Value) -> Value {
    let out = run(bundle_reply(
        "t1-fan",
        &[
            ("r-fan-kw-ep", json!([])),
            ("r-fan-kw-fact", json!([])),
            ("r-fan-self", self_rows),
            ("r-fan-temporal", json!([])),
            (
                "r-fan-model",
                json!([{"model_id": "m-1", "dim": 1024, "active": 1}]),
            ),
        ],
    ));
    assert_eq!(out.len(), 1, "the fan parks in one message: {out:#?}");
    for a in calls_of(&out[0]) {
        if a["table"] == "recall_scratch" && a["row"]["leg"] == "legs" {
            return meclaw_core::serde_json::from_str(a["row"]["payload"].as_str().unwrap())
                .unwrap();
        }
    }
    panic!("no parked legs row in {out:#?}");
}

/// The parked `fused` document of a hydration bundle.
fn fused_of(out: &[Value]) -> Value {
    for a in calls_of(&out[0]) {
        if a["table"] == "recall_scratch" && a["row"]["leg"] == "fused" {
            return meclaw_core::serde_json::from_str(a["row"]["payload"].as_str().unwrap())
                .unwrap();
        }
    }
    panic!("no parked fused document in {out:#?}");
}

/// The fusion, driven out of a REAL parked fan: `t1-legs` with an empty walk and
/// an empty semantic leg, so the only leg that can nominate anything is `self`.
fn fuse(legs_row: &Value) -> Vec<Value> {
    let out = run(bundle_reply(
        "t1-legs",
        &[
            ("r-legs-sem-aud", json!([])),
            (
                "r-legs-read",
                json!([scratch("legs", legs_row), scratch("sem", &json!([]))]),
            ),
        ],
    ));
    assert_eq!(
        out[0]["header"]["phase"], "t1-emit",
        "no walk, no join — straight to the hydration: {out:#?}"
    );
    out
}

// ═════════════════════════════════════════════ 1. the asker's facts are asked for

/// The identity is read off `audience_now` and nothing else has to be promoted
/// for it: `member:alex` is a person, `agent:aide` is a lens on the hive
/// (ADR-0002 E1) and never an identity of its own. The legacy `user` spelling
/// rides with it, because that is what the extraction lane wrote before subjects
/// became person names.
#[test]
fn the_fan_asks_for_the_askers_own_facts() {
    let out = run(request());
    let fan = fan_of(&out);
    let mine = call(&fan, "r-fan-self").expect("the self leg is part of the fan");
    assert_eq!(mine["operation"], "select");
    assert_eq!(mine["table"], "facts");
    let subjects = mine["where"]["canonical_subject"]["in"]
        .as_array()
        .expect("an `in` over the asker's subjects")
        .clone();
    assert!(
        subjects.iter().any(|s| s == "alex"),
        "the member token of the asking round is the asker: {mine}"
    );
    assert!(
        subjects.iter().any(|s| s == "user"),
        "the legacy self-subject is the asker too: {mine}"
    );
    assert!(
        !subjects.iter().any(|s| s == "aide"),
        "an agent is a lens on the hive, never the member it belongs to: {mine}"
    );
    // R1 in one line: the version chain is the truth, the cache column is not
    // consulted — a superseded self fact is projected onto its successor in
    // `t1-emit` exactly as a keyword-leg fact is.
    assert!(mine["where"].get("expired_at").is_none(), "{mine}");
    // Point-mode validity, the same predicate the temporal leg asks with: what
    // holds NOW, with the open end read as open rather than as missing.
    assert!(
        mine["where"]["valid_from"].get("lte").is_some(),
        "the read instant bounds the leg: {mine}"
    );
    assert!(
        mine["where"]["valid_until"]["or_null"].get("gt").is_some(),
        "an open validity is open, not absent: {mine}"
    );
}

/// The leg costs NO round trip of its own: the asker is known at request entry,
/// so the question rides in the same bundle as the four legs that were always
/// there (#295/#418).
#[test]
fn the_self_leg_rides_the_fan_and_adds_no_round_trip() {
    let out = run(request());
    let store: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "rstore")
        .collect();
    assert_eq!(
        store.len(),
        1,
        "one store message leaves a tier-1 request: {out:#?}"
    );
    let ids = ids_of(store[0]);
    assert!(
        ids.contains(&"r-fan-self".to_string()),
        "the self leg is one op of the fan bundle: {ids:?}"
    );
}

// ═════════════════════════════════════ 2. the asker is NOT a graph anchor

/// Tried and rejected on a measurement, which is why it is pinned rather than
/// merely absent. Anchoring the walk on the asker DOES reach a subject spelling
/// the audience token does not carry — but an asker is a HUB, and a hub anchor
/// is true of every question: the walk came back full every time, and the graph
/// leg then voted at full weight for twenty rows nobody asked about. Measured on
/// a live hive, question *"what colour are my eyes"*: `leg_sizes.graph = 20`,
/// and a weather fact about a city in the bundle. A leg whose rank list is the
/// same for every question is a constant, not a retrieval — so the asker lives
/// in a leg with a BUDGET, and the graph leg keeps the one property it has:
/// that the question named where it starts.
#[test]
fn the_asker_does_not_anchor_the_walk() {
    let legs = parked_legs(json!([]));
    let anchors = legs["anchors"].as_array().expect("anchors").clone();
    for name in ["alex", "Alex", "user", "User"] {
        assert!(
            !anchors.iter().any(|a| a == name),
            "the asker is not a graph anchor ({name}): {anchors:?}"
        );
    }
    // What the question named is untouched — this leg is exactly what it was.
    assert!(
        anchors.iter().any(|a| a == "sons"),
        "the query's own anchors are unchanged: {anchors:?}"
    );
}

/// The budget, which is what makes the leg safe to run on every question: a
/// dossier row is seated only while there are slots no query hit is competing
/// for. DOSSIER FLOOD, measured live: seated as an ordinary fact list, twenty
/// dossier rows took every slot the query legs were competing for and two
/// different questions came back with a byte-identical FACTS section.
#[test]
fn the_dossier_gets_a_budget_and_not_the_whole_fact_half() {
    // Twelve dossier rows against a bundle the other two classes can fill on
    // their own: twelve facts the QUESTION found and twelve episodes.
    let mine: Vec<Value> = (0..12)
        .map(|i| a_son(&format!("f-mine-{i}"), "Alex"))
        .collect();
    let mut row = parked_legs(json!(mine));
    row["kw-fact"] = json!(
        (0..12)
            .map(|i| json!({"kind": "fact", "id": format!("f-kw-{i}")}))
            .collect::<Vec<_>>()
    );
    row["kw-ep"] = json!(
        (0..12)
            .map(|i| json!({"kind": "episode", "id": format!("e-{i}")}))
            .collect::<Vec<_>>()
    );
    let fused = fused_of(&fuse(&row));
    let seated: Vec<&str> = fused["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    let dossier = seated.iter().filter(|i| i.starts_with("f-mine-")).count();
    assert_eq!(
        dossier, 6,
        "the dossier gets its budget and not one slot more: {seated:?}"
    );
    assert_eq!(seated.len(), 20, "and the bundle is still full: {seated:?}");
    // The rule the dossier flood broke: what the question found is seated by the
    // question's own competition (episodes keep their own floor), never displaced
    // by a leg that answers the same thing whatever is asked.
    assert!(
        seated.iter().filter(|i| i.starts_with("f-kw-")).count() >= 8,
        "the query-driven facts keep the fact half: {seated:?}"
    );
}

/// ...and the ceiling is a ceiling against COMPETITION, never a cut. A question
/// nothing else answered — the case this leg exists for — is still answered by
/// it, with the slots nobody else claimed.
#[test]
fn an_unclaimed_bundle_falls_back_to_the_dossier() {
    let mine: Vec<Value> = (0..12)
        .map(|i| a_son(&format!("f-mine-{i}"), "Alex"))
        .collect();
    let fused = fused_of(&fuse(&parked_legs(json!(mine))));
    assert_eq!(
        fused["candidates"].as_array().unwrap().len(),
        12,
        "nothing else nominated anything, so the dossier fills the bundle: {fused}"
    );
}

// ═════════════════════════════════════════ 3. the fact reaches the fused ranking

/// The acceptance criterion of the issue, in one assertion: a first-person
/// question that names nobody returns the asker's own facts as candidates. Every
/// query-driven leg is empty in this run — keyword, semantic, graph and temporal
/// all came back with nothing — so `self` is the only leg that could have
/// carried them.
#[test]
fn a_fact_about_the_asker_is_a_self_leg_candidate() {
    let legs = parked_legs(json!([
        a_son("f-elias", "Elias"),
        a_son("f-leroy", "Leroy")
    ]));
    let fused = fused_of(&fuse(&legs));
    let carried: Vec<(String, Vec<String>)> = fused["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap().to_string(),
                c["legs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| l.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        carried,
        vec![
            ("f-elias".to_string(), vec!["self".to_string()]),
            ("f-leroy".to_string(), vec!["self".to_string()]),
        ],
        "both rows the hive held reach the ranking, carried by the self leg: {fused}"
    );
    assert!(
        fused["legs_present"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "self"),
        "leg attribution names the leg that carried them: {fused}"
    );
    assert_eq!(fused["leg_sizes"]["self"], 2, "{fused}");
    // The hydration asks for them, which is what puts them in the bundle's FACTS
    // section rather than only in the ranking.
    let out = fuse(&legs);
    let hyd = call(&out[0], "r-hyd-fact").expect("the fact hydration");
    let ids = hyd["where"]["id"]["in"].as_array().unwrap().clone();
    assert!(
        ids.iter().any(|i| i == "f-elias") && ids.iter().any(|i| i == "f-leroy"),
        "{hyd}"
    );
}

/// And the leg is its OWN list: a dossier can never spend the graph leg's slots,
/// and a wide walk can never spend the dossier's. Two questions, two rank lists.
#[test]
fn the_self_leg_does_not_share_the_graph_legs_slots() {
    let legs = parked_legs(json!([a_son("f-elias", "Elias")]));
    let fused = fused_of(&fuse(&legs));
    assert_eq!(fused["leg_sizes"]["self"], 1, "{fused}");
    assert_eq!(fused["leg_sizes"]["graph"], 0, "{fused}");
}

// ═══════════════════════════════════════════════════════ 4. the gate still rules

/// An anchor decides what is LOOKED UP, never what is disclosed. A self fact
/// recorded in a round this one is not a subset of is invisible here exactly as
/// any other row is — fail-closed, and the ranking never sees it, because a
/// hidden row in the RRF sum would move the ranking of the visible ones.
#[test]
fn a_self_fact_this_round_may_not_see_never_enters_the_leg() {
    // NARROWER than the asking round: recorded when only the member was there,
    // asked now with the agent in the room too. The current round is not a
    // subset of it, so it is not this round's to see.
    let mut hidden = a_son("f-hidden", "Robin");
    hidden["audience_set"] = json!(r#"["member:alex"]"#);
    let legs = parked_legs(json!([a_son("f-elias", "Elias"), hidden]));
    let fused = fused_of(&fuse(&legs));
    let ids: Vec<&str> = fused["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["f-elias"],
        "a row recorded in a narrower round is not this round's to see: {fused}"
    );
    assert_eq!(fused["leg_sizes"]["self"], 1, "{fused}");
}

/// The red probe of the other direction: the leg is the ASKER's facts, not every
/// fact. A row about somebody else never gets there — the `in` filter names the
/// asker's subjects and nothing else.
#[test]
fn a_fact_about_somebody_else_is_not_the_askers() {
    let out = run(request());
    let mine = call(&fan_of(&out), "r-fan-self").expect("the self leg");
    let subjects: Vec<&str> = mine["where"]["canonical_subject"]["in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert!(
        !subjects.iter().any(|s| *s == "sam" || *s == "aide"),
        "only the asker's own spellings are looked up: {subjects:?}"
    );
}
