//! R-39-1 point 1 (GitHub #1322): a correction of the person works from the very
//! turn it is said in.
//!
//! Until this file, only the close pass parked a replacement window (ruling Q9,
//! GitHub #300), so a `replaces` on the per-turn inline lane met an empty window
//! and was discarded: "not Anna, but Anne" stood next to "Anna" until the session
//! ended. The owner's ruling R-39-1 (2026-10-26) changed Q9 and kept Q2's guard
//! rails:
//!
//! * the hive parks the window ITSELF -- `recall` parks the open facts it hands
//!   the speaking model for a turn, under a key built from the context's
//!   `session_id` and `turn_id` (edge truth, never a body claim);
//! * the inline block of the same turn checks `replaces` against exactly that
//!   window, with every rail of the close pass unchanged (only what was shown,
//!   `expired_at is_null`, close and never delete, no crossing of sources, the
//!   direction guard of GH #71);
//! * no model call is added: the window is a store row the recall writes next to
//!   the bundle it emits anyway.
//!
//! Everything here runs the REAL `params.script_inline` of `recall` and
//! `extract-glue` against injected store replies -- no model, no cost.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const EXTRACT_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

const SESSION: &str = "s-1";
const TURN: &str = "T-1~1790731886237~09af3e6c";
const ANNA: &str = "3fa2b1c9-0000-4000-8000-00000000a001";
const LISBON: &str = "7c11e0aa-0000-4000-8000-00000000a002";
const CLOSED: &str = "9d0f00e1-0000-4000-8000-00000000a003";

fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str) -> String {
    let raw = std::fs::read_to_string(path).expect("template config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// The program travels on stdin (a script near 128 KiB does not fit one argv
/// string), the document is put under `sys.stdin` before it runs.
fn run(script: &str, doc: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let stdin_doc = meclaw_testing::code_stdin(&doc).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
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
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "script exited non-zero: {err}");
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (msgs, err)
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

fn store_args(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter(|m| {
            let r = m["header"]["route"].as_str().unwrap_or("");
            r == "rstore" || r == "xstore"
        })
        .map(args_of)
        .collect()
}

// ------------------------------------------------------------------ recall

/// A fact row as recall's hydration select returns it.
fn fact(id: &str, predicate: &str, claim: &str, valid_from: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-0", "subject": "user",
                       "predicate": predicate, "claim": claim,
                       "canonical_subject": "user", "canonical_predicate": predicate,
                       "canonical_claim": claim, "valid_from": valid_from,
                       "valid_until": serde_json::Value::Null,
                       "recorded_at": valid_from,
                       "expired_at": serde_json::Value::Null,
                       "superseded_by": serde_json::Value::Null,
                       "episode_id": format!("ep-of-{id}"), "source": "",
                       "fact_kind": "world", "confidence": 90})
}

fn recall_facts() -> serde_json::Value {
    let mut closed = fact(
        CLOSED,
        "favorite_color",
        "favorite color is green",
        "2026-01-01T09:00:00.000000Z",
    );
    closed["expired_at"] = serde_json::json!("2026-02-01T09:00:00.000000Z");
    closed["superseded_by"] = serde_json::json!("somebody");
    serde_json::json!([
        fact(
            ANNA,
            "has_child",
            "has a daughter named Anna",
            "2026-01-05T09:00:00.000000Z"
        ),
        fact(
            LISBON,
            "lives_in",
            "lives in Lisbon",
            "2026-01-06T09:00:00.000000Z"
        ),
        closed
    ])
}

/// The `t1-emit` document of a tier-1 request, with or without the round's
/// session and turn on the context.
fn recall_doc(facts: serde_json::Value, ids: &[&str], turn: bool) -> serde_json::Value {
    let cands: Vec<serde_json::Value> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            serde_json::json!({"kind": "fact", "id": id, "score": 0.09 - 0.01 * i as f64,
                               "legs": ["keyword"]})
        })
        .collect();
    let fused = serde_json::json!({"candidates": cands, "legs_present": ["keyword"],
                                   "leg_sizes": {"keyword": ids.len()},
                                   "leg_capped": {}, "semantic_degraded": false});
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": "[]", "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    let mut ctx = serde_json::json!({"mem_phase": "t1-emit", "recall_id": "r1",
                                     "memory_tier": "1",
                                     "recall_query": "what about my daughter?",
                                     "recall_as_of": "2026-08-12T00:00:00Z",
                                     "recall_window_from": "", "recall_window_to": ""});
    if turn {
        ctx["session_id"] = serde_json::json!(SESSION);
        ctx["turn_id"] = serde_json::json!(TURN);
    }
    serde_json::json!({
        "header": {"context": ctx, "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn bundle_text(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle")["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string()
}

/// The window op recall parked, if any: `(key, axes)`.
fn parked_window(msgs: &[serde_json::Value]) -> Option<(String, serde_json::Value)> {
    store_args(msgs)
        .into_iter()
        .find(|a| {
            a["operation"] == "insert" && a["table"] == "scratch" && a["row"]["kind"] == "window"
        })
        .map(|a| {
            (
                a["row"]["key"].as_str().expect("key").to_string(),
                serde_json::from_str(a["row"]["payload"].as_str().expect("payload"))
                    .expect("window payload"),
            )
        })
}

fn shown_turn() -> (String, serde_json::Value, String) {
    let (msgs, _) = run(
        &script_of(RECALL_CONFIG),
        recall_doc(recall_facts(), &[ANNA, LISBON, CLOSED], true),
    );
    let (key, axes) = parked_window(&msgs).expect("recall parks the window of the turn");
    (key, axes, bundle_text(&msgs))
}

#[test]
fn recall_parks_the_open_facts_it_shows_as_the_window_of_the_turn() {
    let (key, axes, text) = shown_turn();
    assert_eq!(
        key,
        format!("turn-window:{SESSION}|{TURN}"),
        "the key is the round's session and turn, read off the context the edges \
         stamped -- nothing in a body names it"
    );
    let axes = axes.as_array().expect("axes").clone();
    let ids: Vec<&str> = axes
        .iter()
        .flat_map(|a| a["statements"].as_array().unwrap().iter())
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&ANNA) && ids.contains(&LISBON),
        "both open facts: {axes:?}"
    );
    assert!(
        !ids.contains(&CLOSED),
        "a closed statement is not closable and not part of the window: {axes:?}"
    );
    let anna = axes
        .iter()
        .find(|a| a["predicate"] == "has_child")
        .expect("the has_child axis");
    assert_eq!(anna["subject"], "user", "the CANONICAL subject: {anna}");
    let s = &anna["statements"][0];
    assert_eq!(s["claim"], "has a daughter named Anna");
    assert_eq!(s["ref"], "3fa2b1c9", "the short id the model reads: {s}");
    assert_eq!(s["since"], "2026-01-05T09:00:00.000000Z");
    assert_eq!(s["last_asserted"], "2026-01-05T09:00:00.000000Z");
    assert!(
        text.contains("has a daughter named Anna") && text.contains("id 3fa2b1c9"),
        "the model reads the id next to the fact it may replace: {text}"
    );
    assert!(
        !text.contains("id 9d0f00e1"),
        "a fact outside the window shows no id: {text}"
    );
}

#[test]
fn a_recall_without_a_turn_parks_nothing_and_shows_no_id() {
    // An outside caller, an object's facts question: no round, no window -- and
    // the bundle stays byte for byte what it was, ids included (GH #296).
    let (msgs, _) = run(
        &script_of(RECALL_CONFIG),
        recall_doc(recall_facts(), &[ANNA, LISBON], false),
    );
    assert!(parked_window(&msgs).is_none(), "{msgs:?}");
    let text = bundle_text(&msgs);
    assert!(!text.contains("id 3fa2b1c9"), "{text}");
    assert!(!text.contains(ANNA), "{text}");
}

#[test]
fn two_ids_with_one_prefix_are_shown_long_enough_to_differ() {
    let twin = "3fa2b1c9-ffff-4000-8000-00000000b001";
    let facts = serde_json::json!([
        fact(
            ANNA,
            "has_child",
            "has a daughter named Anna",
            "2026-01-05T09:00:00.000000Z"
        ),
        fact(twin, "owns", "owns a bike", "2026-01-06T09:00:00.000000Z")
    ]);
    let (msgs, _) = run(
        &script_of(RECALL_CONFIG),
        recall_doc(facts, &[ANNA, twin], true),
    );
    let (_, axes) = parked_window(&msgs).expect("window");
    let refs: Vec<String> = axes
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["statements"].as_array().unwrap().clone())
        .map(|s| s["ref"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(refs.len(), 2);
    assert_ne!(refs[0], refs[1], "one short id names one fact: {refs:?}");
}

// ------------------------------------------------------------ extract-glue

const BATCH: &str = "inline:b1";
const CHANNEL: &str = "c-r39";
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

fn x_ctx(phase: &str, close_pass: bool) -> serde_json::Value {
    let mut ctx = serde_json::json!({"store_origin": "extract", "mem_phase": phase,
                                     "batch_id": BATCH, "session_id": SESSION,
                                     "turn_id": TURN, "channel": CHANNEL,
                                     "audience_set": AUDIENCE});
    if close_pass {
        ctx["close_pass"] = serde_json::json!("1");
    }
    ctx
}

fn x_reply(
    phase: &str,
    operation: &str,
    rows: serde_json::Value,
    close: bool,
) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": x_ctx(phase, close),
                   "hop": {"operation": operation, "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

#[test]
fn the_inline_lane_reads_the_window_of_its_own_turn() {
    let (key, _, _) = shown_turn();
    let (msgs, _) = run(
        &script_of(EXTRACT_CONFIG),
        x_reply("merge", "insert", serde_json::json!([]), false),
    );
    let sel = store_args(&msgs)
        .into_iter()
        .find(|a| a["operation"] == "select" && a["table"] == "scratch")
        .expect("the meeting read");
    assert_eq!(
        sel["where"]["key"],
        serde_json::json!({"in": [BATCH, key]}),
        "the batch's own rows AND the window recall parked for this turn -- the \
         same key, built by both cells out of the same two context keys"
    );
}

#[test]
fn the_close_pass_reads_its_own_window_only() {
    let (msgs, _) = run(
        &script_of(EXTRACT_CONFIG),
        x_reply("merge", "insert", serde_json::json!([]), true),
    );
    let sel = store_args(&msgs)
        .into_iter()
        .find(|a| a["operation"] == "select" && a["table"] == "scratch")
        .expect("the meeting read");
    assert_eq!(
        sel["where"]["key"], BATCH,
        "the close pass parks what it rendered under its own key, as before"
    );
}

fn staged(claim: &str, replaces: &str) -> serde_json::Value {
    serde_json::json!({
        "episode_id": "e9", "subject": "user", "predicate": "has_child", "claim": claim,
        "fact_kind": "world", "valid_from": "", "valid_until": null, "confidence": 90,
        "replaces": replaces, "claim_hash": claim.replace(' ', "_")
    })
}

fn apply_with_turn_window(fact: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let (key, axes, _) = shown_turn();
    let payload = serde_json::json!({"facts": [fact], "entities": [], "edges": []});
    let episodes = serde_json::json!({"e9": {"happened_at": "2026-08-12T10:00:00Z",
                                             "session_id": SESSION,
                                             "audience_set": "[\"member:user\"]"}});
    let rows = serde_json::json!([
        {"key": BATCH, "kind": "payload", "payload": payload.to_string()},
        {"key": BATCH, "kind": "known", "payload": "[]"},
        {"key": BATCH, "kind": "episodes", "payload": episodes.to_string()},
        {"key": key, "kind": "window", "payload": axes.to_string()}
    ]);
    run(
        &script_of(EXTRACT_CONFIG),
        x_reply("apply", "select", rows, false),
    )
}

fn fact_ops(msgs: &[serde_json::Value], operation: &str) -> Vec<serde_json::Value> {
    store_args(msgs)
        .into_iter()
        .filter(|a| a["table"] == "facts" && a["operation"] == operation)
        .collect()
}

#[test]
fn not_anna_but_anne_closes_the_anna_statement_in_the_same_turn() {
    // "not Anna, but Anne" on an axis that ENUMERATES (has_child): the
    // prompt allows `replaces` there for a correction only, and the ingress
    // writes exactly the closure the close pass and the judge write.
    let (msgs, _) = apply_with_turn_window(staged("has a daughter named Anne", "3fa2b1c9"));
    let inserted = fact_ops(&msgs, "insert");
    assert_eq!(inserted.len(), 1, "Anne is written: {msgs:?}");
    let new_id = inserted[0]["row"]["id"].as_str().unwrap().to_string();
    let closures = fact_ops(&msgs, "update");
    assert_eq!(closures.len(), 1, "Anna is closed in this turn: {msgs:?}");
    assert_eq!(
        closures[0]["set"],
        serde_json::json!({"expired_at": "2026-08-12T10:00:00Z",
                           "superseded_by": new_id,
                           "closure_source": format!("extract:{BATCH}")})
    );
    assert_eq!(
        closures[0]["where"],
        serde_json::json!({"id": ANNA, "canonical_subject": "user",
                           "canonical_predicate": "has_child",
                           "expired_at": {"is_null": true},
                           "source": {"or_null": {"eq": ""}}}),
        "the full id from the window, never the short one; a judged closure is \
         never overwritten (`expired_at is_null`), the source is pinned"
    );
}

#[test]
fn an_id_the_turn_did_not_show_is_discarded_and_says_so() {
    let (msgs, err) = apply_with_turn_window(staged("has a daughter named Anne", "9d0f00e1"));
    assert_eq!(
        fact_ops(&msgs, "insert").len(),
        1,
        "the fact is still written"
    );
    assert!(fact_ops(&msgs, "update").is_empty(), "{msgs:?}");
    assert!(
        err.contains("outside the extraction window"),
        "the guard rail is loud: {err:?}"
    );
}

#[test]
fn a_full_id_of_a_shown_fact_is_accepted_too() {
    let (msgs, _) = apply_with_turn_window(staged("has a daughter named Anne", ANNA));
    assert_eq!(fact_ops(&msgs, "update").len(), 1, "{msgs:?}");
}

#[test]
fn a_block_that_claims_a_window_parks_nothing() {
    // Being shown is edge truth: a per-turn block that carries `shown` itself
    // is parsed as before and parks no window row.
    let block = serde_json::json!({"episode_id": "e9",
        "shown": [{"subject": "user", "predicate": "has_child",
                   "statements": [{"id": ANNA, "claim": "x", "since": ""}]}],
        "facts": [{"episode_id": "e9", "subject": "user", "predicate": "has_child",
                   "claim": "has a daughter named Anne", "fact_kind": "world",
                   "replaces": ANNA}]});
    let doc = serde_json::json!({
        "header": {"context": {"store_origin": "inline", "mem_phase": "inline",
                               "session_id": SESSION, "turn_id": TURN,
                               "channel": CHANNEL, "audience_set": AUDIENCE},
                   "hop": {}},
        "messages": [{"origin": "user", "type": "text", "text": block.to_string()}]
    });
    let (msgs, _) = run(&script_of(EXTRACT_CONFIG), doc);
    assert!(
        !store_args(&msgs)
            .iter()
            .any(|a| a["table"] == "scratch" && a["row"]["kind"] == "window"),
        "{msgs:?}"
    );
}

// ------------------------------------------------- who asked (review I-2)

fn recall_doc_asked_by(caller: &str) -> serde_json::Value {
    let mut doc = recall_doc(recall_facts(), &[ANNA, LISBON, CLOSED], true);
    doc["header"]["context"]["recall_caller"] = serde_json::json!(caller);
    doc
}

#[test]
fn an_ask_for_someone_else_parks_no_window_even_with_a_turn_on_its_context() {
    // Ruling Q2: only what the speaking model was provably shown. The member's
    // outside door (`outside`) and an object's facts (`objects`, its own
    // audience) answer an asker whose model is not the one of this round, so a
    // turn id riding along on their context must not open a window.
    for caller in ["outside", "objects", "resident"] {
        let (msgs, _) = run(&script_of(RECALL_CONFIG), recall_doc_asked_by(caller));
        assert!(
            parked_window(&msgs).is_none(),
            "an ask by `{caller}` parks no window: {msgs:?}"
        );
        assert!(
            !bundle_text(&msgs).contains("   id "),
            "and its bundle renders no id"
        );
    }
}

#[test]
fn the_speakers_own_ask_still_parks_its_window() {
    for caller in ["talky", "talky-chat", "cogny", "round", ""] {
        let (msgs, _) = run(&script_of(RECALL_CONFIG), recall_doc_asked_by(caller));
        assert!(
            parked_window(&msgs).is_some(),
            "the speaker's ask (`{caller}`) parks the window of its turn"
        );
    }
}

// ------------------------------------- the shipped edges (review I-1)

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;

fn obj(v: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn shipped_edges(config: &str) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(config).expect("template config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    v["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

/// The context the router hands `to` after the one shipped edge `from -> to`
/// that holds for this header -- condition and modifier as the colony runs them.
fn crossed(
    edges: &[serde_json::Value],
    from: &str,
    to: &str,
    context: &serde_json::Value,
    hop: &serde_json::Value,
) -> serde_json::Value {
    let (c, h) = (obj(context), obj(hop));
    let holding: Vec<&serde_json::Value> = edges
        .iter()
        .filter(|e| {
            e["from"] == serde_json::json!(from)
                && e["to"] == serde_json::json!(to)
                && e["condition"].as_str().is_some_and(|x| {
                    let cond = parse_condition(x).unwrap_or_else(|err| panic!("{x}: {err}"));
                    matches!(evaluate_condition(&cond, &c, &h), Ok(true))
                })
        })
        .collect();
    assert_eq!(
        holding.len(),
        1,
        "one shipped edge {from} -> {to} holds for {hop}"
    );
    let spec: ModifierSpec =
        serde_json::from_value(holding[0]["modifier"].clone()).expect("a modifier spec");
    let m = parse_modifier(&spec).expect("a modifier");
    let out =
        apply_modifier(&m, &meclaw_core::Headers::from_parts(c, h)).expect("the modifier applies");
    serde_json::Value::Object(out.context)
}

/// The round as the collector minted it (`disarm(...)[:128]` turns the
/// channel's `~` into `_`) and the raw channel id the context still carries.
const ROUND: &str = "chat_raw_1790731886237";
const RAW: &str = "chat~raw~1790731886237";

fn both_halves_meet_through(config: &str, asker: &str) {
    let edges = shipped_edges(config);
    let round_ctx = serde_json::json!({"session_id": SESSION, "turn_id": RAW,
                                       "channel": CHANNEL, "audience_set": AUDIENCE});
    // The ask: the collector's `recall` head carries the round on the hop.
    let ask = crossed(
        &edges,
        asker,
        "./memory-hive",
        &round_ctx,
        &serde_json::json!({"route": "recall", "turn_id": ROUND, "memory_tier": "1",
                            "recall_query": "what about my daughter?"}),
    );
    // The answer's `memory` section: the splitter stamps the round on its hop.
    let block = crossed(
        &edges,
        asker,
        "./memory-hive",
        &round_ctx,
        &serde_json::json!({"route": "sidecar", "section": "memory", "turn_id": ROUND}),
    );
    assert_eq!(
        ask["turn_id"],
        serde_json::json!(ROUND),
        "{config}: the ask's round"
    );
    assert_eq!(
        block["turn_id"], ask["turn_id"],
        "{config}: the `memory` section must name the round the recall ask named, \
         or the window of the turn is never found"
    );

    let mut doc = recall_doc(recall_facts(), &[ANNA, LISBON, CLOSED], false);
    for k in ["session_id", "turn_id"] {
        doc["header"]["context"][k] = ask[k].clone();
    }
    let (msgs, _) = run(&script_of(RECALL_CONFIG), doc);
    let (key, _) = parked_window(&msgs).expect("the ask parks the window of its round");

    let mut reply = x_reply("merge", "insert", serde_json::json!([]), false);
    for k in ["session_id", "turn_id"] {
        reply["header"]["context"][k] = block[k].clone();
    }
    let (msgs, _) = run(&script_of(EXTRACT_CONFIG), reply);
    let sel = store_args(&msgs)
        .into_iter()
        .find(|a| a["operation"] == "select" && a["table"] == "scratch")
        .expect("the meeting read");
    assert_eq!(
        sel["where"]["key"],
        serde_json::json!({"in": [BATCH, key]}),
        "{config}: the inline block reads exactly the window its turn's recall parked"
    );
}

#[test]
fn the_member_edges_key_ask_and_block_of_one_turn_alike() {
    both_halves_meet_through("../../templates/member/config.json", "./assistants");
}

#[test]
fn the_simple_agent_edges_key_ask_and_block_of_one_turn_alike() {
    both_halves_meet_through("../../templates/simple-agent/config.json", "./curator");
}
