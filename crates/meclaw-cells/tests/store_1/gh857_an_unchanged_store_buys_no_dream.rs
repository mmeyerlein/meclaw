//! GH #857 -- the nightly dream run buys no model call over an unchanged store.
//!
//! Measured before the fix: the nightly run (`clock` 03:00 -> `dream-glue`)
//! guarded only its idempotence (same run id, same window), and `scope` read
//! every open fact up to `delta_to` and stopped only on an EMPTY store. A store
//! nothing had touched since the last night therefore bought the dreamer every
//! night, with the whole store as its payload.
//!
//! The repair is a deterministic question in front of the model: did a fact get
//! recorded, or did one expire, since `delta_from`? Two `limit 1` selects answer
//! it, and "no" closes the run `skipped: unchanged` with `llm_calls 0` while the
//! deterministic steps of the night (embedding backfill, scratch sweep) still
//! run and the window still advances. Three nights may never skip: the first
//! one (nothing to compare with), a crash recovery (the run is already half
//! written), and a night after one that left pages of an over-cap axis for
//! later (the judge owes those pages whether or not a fact moved).
//!
//! Everything runs the REAL `params.script_inline` against injected store
//! replies, so no model is called.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};

const GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";
const HIVE: &str = "../../templates/memory-hive/config.json";

const RUN: &str = "r2";
const FROM: &str = "2026-09-24T03:00:00Z";
const TO: &str = "2026-09-25T03:00:00Z";

fn glue() -> String {
    shipped_script(GLUE)
}

/// A store reply as the store -> dream-glue edge delivers it: the phase and the
/// run keys in the context (the dream-glue -> store edge put them there), the
/// operation on the hop, the rows in the body.
fn store_reply(phase: &str, operation: &str, dream_from: &str, rows: Value) -> Value {
    json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO, "dream_from": dream_from},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).unwrap_or(Value::Null)
}

fn routes(msgs: &[Value]) -> Vec<String> {
    msgs.iter()
        .map(|m| m["header"]["route"].as_str().unwrap_or("").to_string())
        .collect()
}

fn phases(msgs: &[Value]) -> Vec<String> {
    msgs.iter()
        .map(|m| m["header"]["phase"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The last done run, as `window-eval` reads it out of `consolidation_log`.
fn done_run(verdicts: Value) -> Value {
    json!({"run_id": "r1", "delta_from": "", "delta_to": FROM, "status": "done",
           "verdicts": verdicts.to_string()})
}

/// `window-eval` over the given log rows; the one op it emits.
fn window_eval(log: Value) -> Value {
    let out = emit_all(&glue(), &store_reply("window-eval", "select", "", log));
    assert_eq!(out.len(), 1, "window-eval emits one op: {out:?}");
    out[0].clone()
}

#[test]
fn the_window_carries_its_lower_bound_to_the_store_and_back() {
    let claim = window_eval(json!([done_run(json!({}))]));
    assert_eq!(args_of(&claim)["table"], "consolidation_log");
    assert_eq!(args_of(&claim)["row"]["delta_from"], FROM);
    assert_eq!(
        claim["header"]["dream_from"], FROM,
        "the lower bound does not ride the hop of the claim: {claim}"
    );
    // The dream-glue -> store edge is what turns the hop key into context, and a
    // modifier that reads a hop key the message does not carry fails the whole
    // edge -- so the edge sets it and every store op carries it.
    let hive: Value =
        serde_json::from_str(&std::fs::read_to_string(HIVE).expect("hive")).expect("json");
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    let to_store = edges
        .iter()
        .find(|e| e["from"] == "./dream-glue" && e["to"] == "./store")
        .expect("dream-glue -> store edge");
    assert_eq!(
        to_store["modifier"]["set_context"]["dream_from"], "hop.dream_from",
        "the store edge does not carry dream_from: {to_store}"
    );
    // docs/development-rules.md § 8c: a working key promoted into context is
    // deleted on every exit edge of the hive that deletes its two siblings.
    for e in edges {
        let del = &e["modifier"]["delete_context"];
        let Some(list) = del.as_array() else { continue };
        if list.iter().any(|k| k == "dream_to") {
            assert!(
                list.iter().any(|k| k == "dream_from"),
                "an exit edge deletes dream_to but not dream_from: {e}"
            );
        }
    }
}

#[test]
fn an_unchanged_store_closes_the_night_without_a_model_call() {
    let claim = window_eval(json!([done_run(json!({}))]));
    let gated = claim["header"]["phase"]
        .as_str()
        .expect("phase")
        .to_string();
    // The claim echo asks the store whether anything was recorded since.
    let first = emit_all(&glue(), &store_reply(&gated, "insert", FROM, json!([])));
    assert_eq!(first.len(), 1, "one count question: {first:?}");
    let q1 = args_of(&first[0]);
    assert_eq!(q1["table"], "facts");
    assert_eq!(q1["where"]["recorded_at"]["gt"], FROM, "{q1}");
    assert_eq!(q1["limit"], 1, "a count, not a payload: {q1}");
    assert_eq!(q1["columns"], json!(["id"]), "a count, not a payload: {q1}");
    // Nothing new -> the second half: anything EXPIRED since?
    let ph1 = first[0]["header"]["phase"].as_str().expect("phase");
    let second = emit_all(&glue(), &store_reply(ph1, "select", FROM, json!([])));
    assert_eq!(second.len(), 1, "one count question: {second:?}");
    let q2 = args_of(&second[0]);
    assert_eq!(q2["where"]["expired_at"]["gt"], FROM, "{q2}");
    assert_eq!(q2["limit"], 1);
    // Nothing expired either -> the night closes without the dreamer.
    let ph2 = second[0]["header"]["phase"].as_str().expect("phase");
    let end = emit_all(&glue(), &store_reply(ph2, "select", FROM, json!([])));
    let r = routes(&end);
    assert!(
        !r.iter().any(|x| x == "dream" || x == "judge"),
        "an unchanged store still reached a model: {r:?}"
    );
    let close = end
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run closes");
    assert_eq!(close["operation"], "update");
    assert_eq!(close["where"]["run_id"], RUN);
    assert_eq!(close["set"]["status"], "done", "{close}");
    assert_eq!(close["set"]["llm_calls"], 0, "{close}");
    // The window advances: the run is booked done up to delta_to, which is the
    // delta_from the next night derives.
    assert_eq!(close["set"]["finished_at"], TO, "{close}");
    let verdicts: Value =
        serde_json::from_str(close["set"]["verdicts"].as_str().expect("verdicts")).expect("json");
    assert_eq!(verdicts, json!({"skipped": "unchanged", "since": FROM}));
    // The deterministic steps of the night still run.
    let ops: Vec<Value> = end.iter().map(args_of).collect();
    assert!(
        ops.iter()
            .any(|a| a["table"] == "embeddings" && a["where"]["status"] == "queued"),
        "the embedding backfill did not run on a skipped night: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|a| a["operation"] == "delete" && a["table"] == "scratch"),
        "the scratch sweep did not run on a skipped night: {ops:?}"
    );
    // And the next night (a run of its own) starts where this one ended.
    let mut next_night = store_reply(
        "window-eval",
        "select",
        "",
        json!([
            done_run(json!({})),
            {"run_id": RUN, "delta_from": FROM, "delta_to": TO, "status": "done",
             "verdicts": verdicts.to_string()}
        ]),
    );
    next_night["header"]["context"]["dream_run"] = json!("r3");
    next_night["header"]["context"]["dream_to"] = json!("2026-09-26T03:00:00Z");
    let next = emit_all(&glue(), &next_night);
    assert_eq!(next.len(), 1, "{next:?}");
    assert_eq!(args_of(&next[0])["row"]["delta_from"], TO);
}

fn gated_phases() -> (String, String) {
    let claim = window_eval(json!([done_run(json!({}))]));
    let gated = claim["header"]["phase"]
        .as_str()
        .expect("phase")
        .to_string();
    let first = emit_all(&glue(), &store_reply(&gated, "insert", FROM, json!([])));
    let ph1 = first[0]["header"]["phase"]
        .as_str()
        .expect("phase")
        .to_string();
    (gated, ph1)
}

#[test]
fn a_new_fact_buys_the_dreamer() {
    let (_gated, ph1) = gated_phases();
    let out = emit_all(
        &glue(),
        &store_reply(&ph1, "select", FROM, json!([{"id": "f9"}])),
    );
    assert_eq!(phases(&out), vec!["scope".to_string()], "{out:?}");
    let scope = args_of(&out[0]);
    assert_eq!(scope["table"], "facts");
    assert_eq!(scope["where"]["recorded_at"]["lte"], TO);
}

#[test]
fn an_expired_fact_buys_the_dreamer() {
    let (_gated, ph1) = gated_phases();
    let second = emit_all(&glue(), &store_reply(&ph1, "select", FROM, json!([])));
    let ph2 = second[0]["header"]["phase"]
        .as_str()
        .expect("phase")
        .to_string();
    let out = emit_all(
        &glue(),
        &store_reply(&ph2, "select", FROM, json!([{"id": "f3"}])),
    );
    assert_eq!(phases(&out), vec!["scope".to_string()], "{out:?}");
}

#[test]
fn the_first_night_never_skips() {
    let claim = window_eval(json!([]));
    assert_eq!(args_of(&claim)["row"]["delta_from"], "");
    let ph = claim["header"]["phase"].as_str().expect("phase");
    let out = emit_all(&glue(), &store_reply(ph, "insert", "", json!([])));
    assert_eq!(
        phases(&out),
        vec!["scope".to_string()],
        "the first night asked a change question with nothing to compare against: {out:?}"
    );
}

#[test]
fn open_pages_of_the_last_night_are_never_skipped() {
    let pages = json!({"pages": {"over_cap": 3, "triaged": 3, "enumerating": 1,
                                 "undecided": 0, "functional": 2,
                                 "paged": [{"subject": "s", "predicate": "p",
                                            "open_statements": 20, "shown": 6,
                                            "remaining": 14}]}});
    let claim = window_eval(json!([done_run(pages)]));
    let ph = claim["header"]["phase"].as_str().expect("phase");
    let out = emit_all(&glue(), &store_reply(ph, "insert", FROM, json!([])));
    assert_eq!(
        phases(&out),
        vec!["scope".to_string()],
        "a night with pages still owed was gated: {out:?}"
    );
    // The same receipt with every page shown leaves nothing owed, so the gate
    // applies again.
    let done = json!({"pages": {"over_cap": 1, "triaged": 1, "enumerating": 0,
                                "undecided": 0, "functional": 1,
                                "paged": [{"subject": "s", "predicate": "p",
                                           "open_statements": 6, "shown": 6,
                                           "remaining": 0}]}});
    let claim = window_eval(json!([done_run(done)]));
    let ph = claim["header"]["phase"].as_str().expect("phase");
    let out = emit_all(&glue(), &store_reply(ph, "insert", FROM, json!([])));
    assert_ne!(phases(&out), vec!["scope".to_string()], "{out:?}");
}

#[test]
fn a_crash_recovery_never_skips() {
    let rows = json!([
        done_run(json!({})),
        {"run_id": RUN, "delta_from": FROM, "delta_to": TO, "status": "running",
         "verdicts": null}
    ]);
    let out = emit_all(&glue(), &store_reply("window-eval", "select", "", rows));
    assert_eq!(phases(&out), vec!["scope".to_string()], "{out:?}");
}

// ------------------------------------------------------------- fix round 1

/// Review finding M2: OR-SN.N.4 says an unreadable receipt owes pages (never a
/// skip); the first cut answered `False` on a JSON error and gated the night.
#[test]
fn an_unreadable_last_receipt_is_never_skipped() {
    let rows = json!([{"run_id": "r1", "delta_from": "", "delta_to": FROM, "status": "done",
                       "verdicts": "{not json"}]);
    let claim = window_eval(rows);
    let ph = claim["header"]["phase"].as_str().expect("phase");
    let out = emit_all(&glue(), &store_reply(ph, "insert", FROM, json!([])));
    assert_eq!(
        phases(&out),
        vec!["scope".to_string()],
        "a receipt nobody could read was taken as owing nothing: {out:?}"
    );
}

/// Review finding M3: the canonicalisation round is capped per night (alias
/// pairs, cardinality questions). A night that hit a cap left questions for the
/// next one, and a store nothing touched would have deferred them until the
/// next write.
#[test]
fn a_night_that_left_canon_questions_is_never_skipped() {
    for backlog in [json!({"pairs": 12}), json!({"cardinality": 3})] {
        let claim = window_eval(json!([done_run(json!({"backlog": backlog}))]));
        let ph = claim["header"]["phase"].as_str().expect("phase");
        let out = emit_all(&glue(), &store_reply(ph, "insert", FROM, json!([])));
        assert_eq!(
            phases(&out),
            vec!["scope".to_string()],
            "{backlog}: a capped canon round was gated: {out:?}"
        );
    }
}

fn canon_ask(pairs: usize, cardinality: usize) -> Vec<Value> {
    let pairs: Vec<Value> = (0..pairs)
        .map(|i| json!({"left": format!("l{i}"), "right": format!("r{i}"), "score": 90}))
        .collect();
    let card: Vec<Value> = (0..cardinality)
        .map(|i| json!({"predicate": format!("p{i}"), "values": ["a", "b"]}))
        .collect();
    let scan = json!({"predicates": {}, "context": {}, "axes": [], "cardinality": card});
    let rows = json!([
        {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
        {"key": RUN, "kind": "canon-pairs", "payload": Value::Array(pairs).to_string()}
    ]);
    emit_all(&glue(), &store_reply("canon-ask", "select", FROM, rows))
}

fn parked_backlog(out: &[Value]) -> Option<Value> {
    out.iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "canon-backlog")
        .map(|a| serde_json::from_str(a["row"]["payload"].as_str().expect("p")).expect("json"))
}

#[test]
fn the_canon_round_receipts_what_its_caps_left_for_later() {
    let full = parked_backlog(&canon_ask(12, 9)).expect("a capped round parks its backlog");
    assert_eq!(full["pairs"], 12, "{full}");
    assert_eq!(full["cardinality"], 1, "{full}");
    assert_eq!(
        parked_backlog(&canon_ask(3, 2)),
        None,
        "a round under its caps parks nothing"
    );
    // And the receipt of the run carries it, where the gate of the next night
    // reads it.
    let scratch = json!([
        {"kind": "verdicts", "payload": "{}"},
        {"kind": "beliefs", "payload": "[]"},
        {"kind": "canon-backlog", "payload": full.to_string()}
    ]);
    let out = emit_all(&glue(), &store_reply("apply-run", "select", FROM, scratch));
    let close = out
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run closes");
    let receipt: Value =
        serde_json::from_str(close["set"]["verdicts"].as_str().expect("v")).expect("json");
    assert_eq!(receipt["backlog"], full, "{receipt}");
}
