//! GH #691 — duplicate episodes fold in the query legs.
//!
//! Measured on a live hive (trace `01a09af4`): the question had been asked ten
//! times, so it lay in the store as ten episodes — and those ten were its own
//! nearest neighbours. The semantic leg, the one leg that does not care which
//! language the question and the fact were written in, spent its whole page of
//! twenty on episodes, ten of them byte-identical copies of the question, and
//! not one fact reached it.
//!
//! Three rules are pinned here:
//!
//! 1. a query leg spends ONE rank position per normal form of an episode — the
//!    semantic leg and the keyword episode page alike — and the semantic leg
//!    over-fetches `2 x LEG_LIMIT` so the folding makes room for something else
//!    (a derived constant, not a knob);
//! 2. the folded slot belongs to the NEWEST copy and its siblings travel as
//!    `copies`, so the `(seen: N)` count of GH #15 stays true — also across legs;
//! 3. an episode without content has no normal form and never folds.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents, hop by hop. No colony, no store, no model, nothing spent.

use meclaw_core::serde_json::{self, Value, json};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const RID: &str = "r-691";
const AUDIENCE: &str = r#"["agent:aide", "member:alex"]"#;
const CHANNEL: &str = "tg:private";
/// The question of the measured turn. Its words are never read by an assertion
/// here — it is the content every copy carries, nothing else.
const QUESTION: &str = "Where did I park the car?";

fn run(doc: &Value) -> Vec<Value> {
    meclaw_testing::emit_all(&meclaw_testing::shipped_script(RECALL_CONFIG), doc)
}

fn ctx(phase: &str) -> Value {
    json!({"mem_phase": phase, "recall_id": RID, "memory_tier": "1",
           "recall_query": QUESTION, "audience_now": AUDIENCE, "channel": CHANNEL})
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

/// The tool_call arguments of every emitted message, in call order.
fn calls(out: &[Value]) -> Vec<Value> {
    out.iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter(|t| t["type"] == "tool_call")
        .map(|t| serde_json::from_str(t["text"].as_str().expect("text")).expect("args"))
        .collect()
}

fn parked(out: &[Value], leg: &str) -> Value {
    calls(out)
        .into_iter()
        .find(|a| a["table"] == "recall_scratch" && a["row"]["leg"] == leg)
        .map(|a| serde_json::from_str(a["row"]["payload"].as_str().unwrap()).unwrap())
        .unwrap_or_else(|| panic!("no parked {leg} in {out:#?}"))
}

fn hits_of(payload: &Value) -> Vec<Value> {
    payload
        .get("hits")
        .cloned()
        .unwrap_or_else(|| payload.clone())
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// An episode row as the store answers the companion select: the gate columns
/// plus what the fold reads (content, event time, ingest time).
fn episode(id: &str, content: &str, hour: u32) -> Value {
    json!({"id": id, "content": content, "sender": "member:alex", "session_id": format!("s-{id}"),
           "happened_at": format!("2026-09-13T{hour:02}:00:00.000000Z"),
           "recorded_at": format!("2026-09-13T{hour:02}:00:00.000000Z"),
           "channel": CHANNEL, "audience_set": AUDIENCE})
}

/// The measured semantic page, one row longer than a leg: ten copies of the
/// question (`e-q0`..`e-q9`, the NEWEST is `e-q4`), ten other episodes, and the
/// fact that carries the answer on rank 21.
fn the_semantic_page() -> Value {
    let mut rows = Vec::new();
    for i in 0..10 {
        rows.push(json!({"owner_id": format!("e-q{i}"), "model_id": "m-1",
                         "owner_table": "episodes", "distance": 10 + i}));
    }
    for i in 0..10 {
        rows.push(json!({"owner_id": format!("e-o{i}"), "model_id": "m-1",
                         "owner_table": "episodes", "distance": 30 + i}));
    }
    rows.push(json!({"owner_id": "f-sons", "model_id": "m-1",
                     "owner_table": "facts", "distance": 50}));
    json!(rows)
}

/// The companion rows of that page. `question` is the content every copy
/// carries — the empty string is the "no normal form" case.
fn the_companions(question: &str) -> (Value, Value) {
    let mut eps = Vec::new();
    for i in 0..10u32 {
        // e-q4 is the newest copy: neither the first nor the last of the page.
        let hour = if i == 4 { 20 } else { 8 + i };
        eps.push(episode(&format!("e-q{i}"), question, hour));
    }
    for i in 0..10u32 {
        eps.push(episode(&format!("e-o{i}"), &format!("another turn {i}"), 7));
    }
    let facts = json!([{"id": "f-sons", "channel": CHANNEL, "audience_set": AUDIENCE,
                        "canonical_subject": "user", "canonical_predicate": "has_sons_named",
                        "subject": "user", "predicate": "has_sons_named"}]);
    (json!(eps), facts)
}

/// The `legs` row the fan parked — every leg but the semantic one empty unless
/// the caller hands a keyword episode page.
fn fan_legs(kw_ep: Value) -> Value {
    json!({"kw-ep": kw_ep, "kw-fact": [], "temporal": [], "self": [], "beliefs": [],
           "anchors": [], "axis": {}, "model": {"model_id": "m-1", "dim": 1024}})
}

/// Drive `t1-join` (the `similar` reply) and then `t1-legs` (the companions and
/// the read-back), and return the parked `fused` document.
fn fuse(question: &str, kw_ep: Value) -> Value {
    let join = run(&bundle_reply(
        "t1-join",
        &[
            ("r-join-anchor", json!([])),
            ("r-join-sem", the_semantic_page()),
        ],
    ));
    let sem = parked(&join, "sem");
    let (eps, facts) = the_companions(question);
    let out = run(&bundle_reply(
        "t1-legs",
        &[
            ("r-legs-sem-aud", facts),
            ("r-legs-sem-aud-ep", eps),
            (
                "r-legs-read",
                json!([scratch("legs", &fan_legs(kw_ep)), scratch("sem", &sem)]),
            ),
        ],
    ));
    parked(&out, "fused")
}

fn fused_ids(fused: &Value) -> Vec<String> {
    fused["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_semantic_page_is_not_spent_on_copies_of_the_question() {
    // 1. the leg ASKS for twice its own length: folding frees rank positions only
    //    if there is something behind the page to move up into them.
    let rendezvous = bundle_reply(
        "t1-qvec-park",
        &[(
            "r-t1-qvec-park-read",
            json!([
                scratch(
                    "legs",
                    &json!({"model": {"model_id": "m-1", "dim": 1024},
                                        "anchors": []})
                ),
                scratch("qvec", &json!({"vector": [1, 2, 3], "degraded": false})),
            ]),
        )],
    );
    let similar = calls(&run(&rendezvous))
        .into_iter()
        .find(|a| a["operation"] == "similar")
        .expect("the similar call");
    assert_eq!(
        similar["limit"], 40,
        "the semantic leg over-fetches 2 x LEG_LIMIT: {similar}"
    );

    // 2. the companion page carries what the fold reads, at the same length
    let join = run(&bundle_reply(
        "t1-join",
        &[
            ("r-join-anchor", json!([])),
            ("r-join-sem", the_semantic_page()),
        ],
    ));
    let ep_aud = calls(&join)
        .into_iter()
        .find(|a| a["table"] == "episodes" && a["operation"] == "select")
        .expect("the episode companion page");
    assert_eq!(
        ep_aud["columns"],
        json!([
            "id",
            "channel",
            "audience_set",
            "content",
            "happened_at",
            "recorded_at"
        ]),
        "{ep_aud}"
    );
    assert_eq!(ep_aud["limit"], 40, "{ep_aud}");

    // 3. the fusion: ten copies are one rank position, and the fact on page
    //    rank 21 is a semantic candidate
    let fused = fuse(QUESTION, json!([]));
    let ids = fused_ids(&fused);
    assert_eq!(
        ids.iter().filter(|i| i.starts_with("e-q")).count(),
        1,
        "the ten copies take one rank position: {ids:?}"
    );
    let sons = fused["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "f-sons")
        .unwrap_or_else(|| panic!("the carrying fact reached the semantic leg: {ids:?}"));
    assert_eq!(sons["legs"], json!(["semantic"]));
    assert!(
        fused["leg_sizes"]["semantic"].as_u64().unwrap() <= 20,
        "the leg still votes with at most LEG_LIMIT hits: {fused}"
    );
}

#[test]
fn the_newest_copy_owns_the_folded_slot_and_counts_its_siblings() {
    // The keyword page saw two MORE copies the semantic page did not (e-q10,
    // e-q11; e-q11 is the newest of all twelve), folded on its own page.
    let kw_rows: Vec<Value> = (0..12u32)
        .map(|i| {
            let hour = match i {
                11 => 22,
                10 => 6,
                4 => 20,
                _ => 8 + i,
            };
            episode(&format!("e-q{i}"), QUESTION, hour)
        })
        .collect();
    let fan = run(&bundle_reply(
        "t1-fan",
        &[
            ("r-fan-kw-ep", json!(kw_rows.clone())),
            ("r-fan-kw-fact", json!([])),
            ("r-fan-temporal", json!([])),
            (
                "r-fan-model",
                json!([{"model_id": "m-1", "dim": 1024, "active": 1}]),
            ),
        ],
    ));
    let kw_ep = parked(&fan, "legs")["kw-ep"].clone();
    let kw_hits = hits_of(&kw_ep);
    assert_eq!(kw_hits.len(), 1, "the keyword page folds too: {kw_ep}");
    assert_eq!(
        kw_hits[0]["id"], "e-q11",
        "the newest copy represents: {kw_ep}"
    );
    assert_eq!(kw_hits[0]["copies"].as_array().map(Vec::len), Some(12));

    // the semantic representative is e-q4 (the newest ITS page saw)
    let fused = fuse(QUESTION, kw_ep);
    let rep = fused["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "e-q4")
        .cloned()
        .unwrap_or_else(|| panic!("the semantic representative: {fused}"));
    assert_eq!(
        rep["copies"].as_array().map(Vec::len),
        Some(10),
        "its siblings travel with it: {rep}"
    );

    // t1-emit: one line, the newest copy of all, counted over BOTH legs
    let fused_c = fused["candidates"].as_array().unwrap().clone();
    let ep_rows: Vec<Value> = kw_rows
        .iter()
        .filter(|r| {
            fused_c
                .iter()
                .any(|c| c["id"] == r["id"] && c["kind"] == "episode")
        })
        .cloned()
        .chain((0..10u32).map(|i| episode(&format!("e-o{i}"), &format!("another turn {i}"), 7)))
        .collect();
    let emit = run(&bundle_reply(
        "t1-emit",
        &[
            ("r-hyd-fact", json!([])),
            ("r-hyd-ep", json!(ep_rows)),
            ("r-hyd-read", json!([scratch("fused", &fused)])),
        ],
    ));
    let msg = emit
        .iter()
        .find(|m| m.get("recall_diagnostic").is_some())
        .unwrap_or_else(|| panic!("no bundle in {emit:#?}"));
    let lines: Vec<&Value> = msg["recall_diagnostic"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["text"] == QUESTION)
        .collect();
    assert_eq!(lines.len(), 1, "one line for the question: {lines:?}");
    assert_eq!(lines[0]["id"], "e-q11", "the newest copy of all owns it");
    assert_eq!(
        lines[0]["seen"], 12,
        "the union of both legs' copies, each counted once"
    );
    let payload: Value =
        serde_json::from_str(msg["system"]["memory"]["bundle"]["text"].as_str().unwrap()).unwrap();
    assert!(
        payload["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("copies").is_none()),
        "`copies` is bookkeeping and never reaches the payload: {payload}"
    );
    assert!(
        lines[0].get("copies").is_none(),
        "…nor the diagnostic record, which carries the count"
    );
}

#[test]
fn an_episode_without_content_never_folds() {
    // An empty normal form is no identity at all (#15): ten empty rows are ten
    // rows, and the fact on rank 21 stays outside the leg.
    let fused = fuse("", json!([]));
    let ids = fused_ids(&fused);
    assert_eq!(
        ids.iter().filter(|i| i.starts_with("e-q")).count(),
        10,
        "{ids:?}"
    );
    assert!(!ids.contains(&"f-sons".to_string()), "{ids:?}");
    assert!(
        fused["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("copies").is_none()),
        "{fused}"
    );
}

/// A semantic page of `n` DISTINCT episodes (nothing folds) and its companions.
fn distinct_page(n: u32) -> (Value, Value) {
    let page: Vec<Value> = (0..n)
        .map(|i| {
            json!({"owner_id": format!("e-d{i}"), "model_id": "m-1",
                   "owner_table": "episodes", "distance": 10 + i})
        })
        .collect();
    let eps: Vec<Value> = (0..n)
        .map(|i| episode(&format!("e-d{i}"), &format!("distinct turn {i}"), 8))
        .collect();
    (json!(page), json!(eps))
}

/// Drive `t1-join` + `t1-legs` over an arbitrary semantic page; return the
/// parked `sem` payload and the parked `fused` document.
fn fuse_page(page: Value, eps: Value) -> (Value, Value) {
    let join = run(&bundle_reply(
        "t1-join",
        &[("r-join-anchor", json!([])), ("r-join-sem", page)],
    ));
    let sem = parked(&join, "sem");
    let out = run(&bundle_reply(
        "t1-legs",
        &[
            ("r-legs-sem-aud", json!([])),
            ("r-legs-sem-aud-ep", eps),
            (
                "r-legs-read",
                json!([scratch("legs", &fan_legs(json!([]))), scratch("sem", &sem)]),
            ),
        ],
    ));
    (sem, parked(&out, "fused"))
}

#[test]
fn a_full_over_fetched_page_reports_its_cap_at_sem_fetch() {
    // #280 item 3 at the new depth: the page is fetched SEM_FETCH = 40 deep, so
    // a page of exactly 40 is the full page and reports `capped_at: 40`.
    let (page, eps) = distinct_page(40);
    let (sem, fused) = fuse_page(page, eps);
    assert_eq!(
        sem["capped_at"], 40,
        "a full semantic page reports SEM_FETCH: {sem}"
    );
    assert_eq!(
        fused["leg_capped"]["semantic"], 40,
        "{}",
        fused["leg_capped"]
    );
}

#[test]
fn the_cut_to_leg_limit_after_the_fold_is_reported_as_a_cap() {
    // Review T6 finding 1: a page of 30 distinct episodes comes back SHORT of
    // SEM_FETCH, so the page reports no cap -- but nothing folds, and the cut
    // to LEG_LIMIT then drops ten rows. Before #691 the same store answered a
    // page of 20 that counted as capped; a silent cut here would take the
    // "possibly more exist" hedge away from exactly the run that lost rows.
    let (page, eps) = distinct_page(30);
    let (sem, fused) = fuse_page(page, eps.clone());
    assert!(
        sem.get("capped_at").is_none(),
        "the page itself is short: {sem}"
    );
    assert!(
        fused["leg_sizes"]["semantic"].as_u64().unwrap() <= 20,
        "{}",
        fused["leg_sizes"]
    );
    assert_eq!(
        fused["leg_capped"]["semantic"], 20,
        "the cut to LEG_LIMIT is a cap: {}",
        fused["leg_capped"]
    );

    // ... and t1-emit reads it as one: the bundle is not complete.
    let emit = run(&bundle_reply(
        "t1-emit",
        &[
            ("r-hyd-fact", json!([])),
            ("r-hyd-ep", eps),
            ("r-hyd-read", json!([scratch("fused", &fused)])),
        ],
    ));
    let msg = emit
        .iter()
        .find(|m| m.get("recall_diagnostic").is_some())
        .unwrap_or_else(|| panic!("no bundle in {emit:#?}"));
    let text = msg["system"]["memory"]["bundle"]["text"].as_str().unwrap();
    let payload: Value = serde_json::from_str(text).unwrap();
    assert_eq!(payload["complete"], false, "{payload}");
    assert!(text.contains("semantic at 20 rows"), "{text}");
}

#[test]
fn a_fold_that_leaves_the_leg_within_its_limit_reports_no_cap() {
    // Guard: 21 rows, ten of them copies -> twelve after the fold. Nothing is
    // cut, so nothing is reported.
    let fused = fuse(QUESTION, json!([]));
    assert!(
        fused["leg_capped"].get("semantic").is_none(),
        "{}",
        fused["leg_capped"]
    );
}
