//! GH #296 (precondition half) — the diagnostic record leaves the payload lane.
//!
//! The tier-1 bundle is read by a model. Everything in it costs prompt budget
//! and everything in it can mislead — a leg size, a rank, a fused score say
//! nothing to a reader answering a question, and say everything to whoever has
//! to explain afterwards why THAT candidate came back.
//!
//! So the two audiences get two slots. `system.memory.bundle` stays the
//! payload; `recall_diagnostic` is a top-level body slot carrying the full
//! internal record, and it is neither `system` nor a message: the collector's
//! `in_bundle` lane keeps `system` and `messages` and drops the rest, so the
//! trace lands in the message log and never in the next prompt.
//!
//! That is the precondition every later removal in this wave stands on:
//! nothing becomes harder to debug, because whatever leaves the payload is
//! still here.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

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

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the shipped script against a real stdin document; return the emissions.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &recall_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A fact row as the hydration select returns it.
fn fact(id: &str, subject: &str, claim: &str, valid_from: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-1", "subject": subject,
                       "predicate": "prefers", "claim": claim,
                       "canonical_subject": subject, "canonical_predicate": "prefers",
                       "canonical_claim": claim, "valid_from": valid_from,
                       "valid_until": serde_json::Value::Null,
                       "recorded_at": valid_from,
                       "expired_at": serde_json::Value::Null,
                       "superseded_by": serde_json::Value::Null,
                       "episode_id": format!("ep-of-{id}"),
                       "fact_kind": "state", "confidence": 90})
}

fn episode(id: &str, content: &str, happened_at: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-1", "sender": "user",
                       "content": content, "happened_at": happened_at,
                       "recorded_at": happened_at})
}

/// The `fused` scratch payload this fixture hands the emit phase — two legs
/// present, sizes and caps deliberately distinct so the diagnostic can be
/// checked against something other than a default.
fn fused() -> serde_json::Value {
    serde_json::json!({
        "candidates": [
            {"kind": "fact", "id": "f-1", "score": 0.09, "legs": ["keyword"]},
            {"kind": "episode", "id": "e-1", "score": 0.075,
             "legs": ["keyword", "semantic"]},
            {"kind": "fact", "id": "f-2", "score": 0.06, "legs": ["semantic"]}
        ],
        "legs_present": ["keyword", "semantic"],
        "leg_sizes": {"keyword": 2, "semantic": 2, "graph": 0, "temporal": 0},
        "leg_capped": {"semantic": 20},
        "semantic_degraded": false
    })
}

/// The episode of the three-candidate fixture.
fn one_episode() -> serde_json::Value {
    serde_json::json!([episode(
        "e-1",
        "we talked about editors",
        "2026-01-02T09:00:00.000000Z"
    )])
}

/// The two facts of the three-candidate fixture, on two axes.
fn two_facts() -> serde_json::Value {
    serde_json::json!([
        fact(
            "f-1",
            "person:example",
            "helix",
            "2026-01-05T09:00:00.000000Z"
        ),
        fact(
            "f-2",
            "person:other",
            "kakoune",
            "2026-01-06T09:00:00.000000Z"
        )
    ])
}

/// The `t1-emit` document of a **tier 1** request — the tier is what keeps the
/// run on the emit path instead of routing it to the dialectic.
fn doc_of(
    fused: serde_json::Value,
    eps: serde_json::Value,
    facts: serde_json::Value,
    axis: serde_json::Value,
) -> serde_json::Value {
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": eps.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": axis.to_string(), "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The three-candidate document itself.
fn emit_doc() -> serde_json::Value {
    doc_of(fused(), one_episode(), two_facts(), serde_json::json!([]))
}

/// The tier-1 emission itself — the whole message, because this test is about
/// which SLOT of it carries what.
fn bundle_message(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")
        .clone()
}

#[test]
fn the_full_candidate_record_travels_in_the_diagnostic_slot() {
    // The record is the ranking as the loop built it: every candidate, with the
    // rank it holds, the fused score that put it there and the legs that
    // nominated it — plus the bookkeeping of the run those numbers only mean
    // something against.
    let msg = bundle_message(&emit(emit_doc()));
    let diag = &msg["recall_diagnostic"];
    assert!(
        diag.is_object(),
        "the bundle message carries a `recall_diagnostic` slot: {msg}"
    );

    assert_eq!(diag["recall_id"], "r1", "the run this record belongs to");
    assert_eq!(diag["query"], "what do I prefer?");
    assert_eq!(diag["as_of"], "2026-08-12T00:00:00Z");
    assert_eq!(diag["tier"], 1);

    assert_eq!(
        diag["legs_present"],
        serde_json::json!(["keyword", "semantic"]),
        "the legs the fusion reported, unchanged: {diag}"
    );
    assert_eq!(
        diag["leg_sizes"],
        serde_json::json!({"keyword": 2, "semantic": 2, "graph": 0, "temporal": 0}),
        "and their sizes, unchanged: {diag}"
    );
    assert_eq!(
        diag["leg_capped"],
        serde_json::json!({"semantic": 20}),
        "the caps the PAGES reported travel with the sizes — the two answer \
         different questions and the record keeps both: {diag}"
    );
    assert_eq!(
        diag["semantic_degraded"], false,
        "the semantic leg ran: {diag}"
    );

    let cands = diag["candidates"].as_array().expect("candidates");
    assert_eq!(
        cands.len(),
        3,
        "two facts and one episode, none of them collapsed: {diag}"
    );
    let seen: Vec<(&str, i64, f64, serde_json::Value)> = cands
        .iter()
        .map(|c| {
            (
                c["id"].as_str().expect("id"),
                c["rank"].as_i64().expect("rank"),
                c["score"].as_f64().expect("score"),
                c["legs"].clone(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            ("f-1", 1, 0.09, serde_json::json!(["keyword"])),
            ("e-1", 2, 0.075, serde_json::json!(["keyword", "semantic"])),
            ("f-2", 3, 0.06, serde_json::json!(["semantic"])),
        ],
        "every candidate keeps the id, rank, score and legs the fused payload \
         named: {diag}"
    );

    // Not a summary of the ranking — the ranking. A candidate in the record
    // carries the fields the loop put on it, so a later package may take one
    // out of the payload without taking it out of the trace.
    assert_eq!(
        cands[0]["text"], "helix",
        "the record is the full candidate, not a stub: {}",
        cands[0]
    );
}

#[test]
fn the_diagnostic_slot_is_not_system_and_not_a_message() {
    // This is the pin that keeps the trace out of the prompt. `system` is
    // durable state for the next turn and `messages` is what the model reads —
    // a top-level slot is neither, so the collector's `in_bundle` lane drops it
    // and only the message log keeps it.
    let msg = bundle_message(&emit(emit_doc()));

    assert!(
        msg["system"].get("recall_diagnostic").is_none(),
        "the trace is not system state: {}",
        msg["system"]
    );
    for m in msg["messages"].as_array().expect("messages") {
        assert!(
            !m.to_string().contains("recall_diagnostic"),
            "the trace never reaches the rendered turn: {m}"
        );
    }
    // And it is really there — otherwise the two assertions above pass on an
    // empty message and prove nothing.
    assert!(
        msg["recall_diagnostic"]["candidates"].is_array(),
        "the slot the two assertions above are about: {msg}"
    );
}

// ============================================================ the payload half
// With the record safe one slot over, the bundle may finally be what it is read
// as: an answer. A row id, a fused score, a rank and a leg name say nothing to
// a reader answering a question — they cost prompt budget and they invite a
// model to reason about numbers it cannot interpret. Everything the four tests
// below miss in the payload is asserted to be in `recall_diagnostic`, because
// "nothing becomes harder to debug" is the scope sentence of #296 and not a
// nicety.

/// The bundle JSON exactly as it travels — the STRING, because half of what
/// this package is about is how long it is.
fn payload_text(msg: &serde_json::Value) -> String {
    msg["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle text")
        .to_string()
}

fn payload(msg: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&payload_text(msg)).expect("bundle json")
}

#[test]
fn no_uuid_no_score_no_rank_no_leg_name_reaches_the_payload() {
    // The fixture reports no capped leg, and that is deliberate: `complete_reason`
    // NAMES the leg a cap ended (#280), which is a sentence written for the
    // reader and the one legitimate place a leg name belongs in the payload.
    // This test is about the candidate records around it.
    let mut uncapped = fused();
    uncapped["leg_capped"] = serde_json::json!({});
    let msgs = emit(doc_of(
        uncapped,
        one_episode(),
        two_facts(),
        serde_json::json!([]),
    ));
    let msg = bundle_message(&msgs);
    let text = payload_text(&msg);

    for id in ["f-1", "f-2", "e-1", "ep-of-f-1", "s-1"] {
        assert!(
            !text.contains(id),
            "a row id names a row and answers nothing — {id} is in the payload: {text}"
        );
    }
    for leg in ["keyword", "semantic", "graph", "temporal"] {
        assert!(
            !text.contains(leg),
            "which leg found a candidate is retrieval bookkeeping — {leg} is in \
             the payload: {text}"
        );
    }
    for word in ["score", "rank", "session_id"] {
        assert!(
            !text.contains(word),
            "{word} is plumbing and travels in the record: {text}"
        );
    }

    // The positive half: what is left is still an answer.
    let cands = payload(&msg);
    let cands = cands["candidates"].as_array().expect("candidates").clone();
    assert_eq!(cands.len(), 3, "two facts and one episode: {cands:?}");
    for c in &cands {
        assert!(c["kind"].is_string(), "a candidate says what it is: {c}");
        assert!(c["text"].is_string(), "and what it says: {c}");
    }
    for c in cands.iter().filter(|c| c["kind"] == "fact") {
        for key in ["subject", "predicate", "since"] {
            assert!(
                c.get(key).is_some(),
                "a fact says WHICH question it answers and since when — {key} \
                 is missing: {c}"
            );
        }
    }

    // And the plumbing is where it was promised to be.
    let record = &msg["recall_diagnostic"]["candidates"][0];
    assert_eq!(record["id"], "f-1", "the record keeps the id: {record}");
    assert_eq!(record["rank"], 1, "and the rank: {record}");
    assert_eq!(record["score"], 0.09, "and the score: {record}");
    assert_eq!(
        record["legs"],
        serde_json::json!(["keyword"]),
        "and the leg that nominated it: {record}"
    );
}

#[test]
fn time_arrives_as_a_date_not_as_a_microsecond_instant() {
    // A microsecond instant is six characters of budget nobody asked for, and a
    // day is the resolution a "since when?" question is answered at — the same
    // resolution the rendered line has used since O-5, out of the same `fmt_day`.
    const INSTANT: &str = "2026-08-16T09:41:12.481900Z";
    let cands = serde_json::json!([
        {"kind": "fact", "id": "f-1", "score": 0.09, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-1", "score": 0.07, "legs": ["keyword"]}
    ]);
    let mut f = fused();
    f["candidates"] = cands;
    f["leg_capped"] = serde_json::json!({});
    let msgs = emit(doc_of(
        f,
        serde_json::json!([episode("e-1", "we talked about editors", INSTANT)]),
        serde_json::json!([fact("f-1", "person:example", "helix", INSTANT)]),
        serde_json::json!([]),
    ));
    let msg = bundle_message(&msgs);
    let b = payload(&msg);

    assert_eq!(
        b["candidates"][0]["since"], "2026-08-16",
        "the day the fact started being true: {}",
        b["candidates"][0]
    );
    assert_eq!(
        b["candidates"][1]["when"], "2026-08-16",
        "the day the turn happened: {}",
        b["candidates"][1]
    );
    assert!(
        !payload_text(&msg).contains(INSTANT),
        "the instant itself never travels in the payload: {}",
        payload_text(&msg)
    );
    assert_eq!(
        msg["recall_diagnostic"]["candidates"][0]["valid_from"], INSTANT,
        "and it is still in the record, to the microsecond: {}",
        msg["recall_diagnostic"]["candidates"][0]
    );
}

/// One axis, four versions, each closed in favour of the next — the shape a
/// projection turns into a three-entry history.
fn four_versions() -> serde_json::Value {
    let ids = ["f-a", "f-b", "f-c", "f-d"];
    let claims = ["vim", "emacs", "kakoune", "helix"];
    let starts = [
        "2026-01-01T00:00:00.000000Z",
        "2026-02-01T00:00:00.000000Z",
        "2026-03-01T00:00:00.000000Z",
        "2026-04-01T00:00:00.000000Z",
    ];
    let mut out = Vec::new();
    for i in 0..4 {
        let mut row = fact(ids[i], "person:example", claims[i], starts[i]);
        if i < 3 {
            row["expired_at"] = serde_json::json!(starts[i + 1]);
            row["superseded_by"] = serde_json::json!(ids[i + 1]);
        }
        out.push(row);
    }
    serde_json::json!(out)
}

#[test]
fn a_superseded_claim_still_announces_itself_without_its_chain() {
    // "And what was it before?" is answered by the claim immediately before this
    // one. Everything older is the history OF that history: it answers a
    // question nobody asked this round, and it is the single biggest field a
    // fact candidate carries.
    let mut f = fused();
    f["candidates"] = serde_json::json!([
        {"kind": "fact", "id": "f-d", "score": 0.09, "legs": ["keyword"]}
    ]);
    f["leg_capped"] = serde_json::json!({});
    let msgs = emit(doc_of(
        f,
        serde_json::json!([]),
        serde_json::json!([fact(
            "f-d",
            "person:example",
            "helix",
            "2026-04-01T00:00:00.000000Z"
        )]),
        four_versions(),
    ));
    let msg = bundle_message(&msgs);
    let b = payload(&msg);
    let cand = &b["candidates"][0];

    assert_eq!(
        cand["previously"],
        serde_json::json!([{"claim": "kakoune", "until": "2026-04-01"}]),
        "one predecessor, with the day it ended: {cand}"
    );
    let text = payload_text(&msg);
    for gone in ["vim", "emacs"] {
        assert!(
            !text.contains(gone),
            "the older claims stay out of the payload — {gone} is in it: {text}"
        );
    }
    assert_eq!(
        msg["recall_diagnostic"]["candidates"][0]["history"]
            .as_array()
            .expect("history")
            .len(),
        3,
        "and the whole chain is still in the record: {}",
        msg["recall_diagnostic"]["candidates"][0]
    );
}

/// A uuid-shaped id — the ids a real store hands out, which is what makes the
/// measurement below the measurement of #296 rather than of a short fixture.
fn uid(a: usize, b: usize) -> String {
    format!("{a:08x}-{b:04x}-4000-8000-{:012x}", a * 100 + b)
}

/// The measurement shape of #296: eighteen candidates as a real recall returns
/// them — twelve facts on twelve axes, each with the versions its axis went
/// through, and six turns. Every id is a uuid, every fact knows its session,
/// every hit knows its score and its legs.
fn measurement_doc() -> serde_json::Value {
    let claims = ["vim", "emacs", "nano", "kakoune", "helix"];
    let starts: Vec<String> = (1..=6)
        .map(|m| format!("2026-{m:02}-01T00:00:00.000000Z"))
        .collect();
    let (mut cands, mut hyd, mut axis, mut eps) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..12 {
        let subject = format!("person:{i:02}");
        for j in 0..5 {
            let mut row = fact(&uid(i, j), &subject, claims[j], &starts[j]);
            row["session_id"] = serde_json::json!(uid(900, i));
            if j < 4 {
                row["expired_at"] = serde_json::json!(starts[j + 1]);
                row["superseded_by"] = serde_json::json!(uid(i, j + 1));
            }
            axis.push(row.clone());
            if j == 4 {
                hyd.push(row);
                cands.push(serde_json::json!({"kind": "fact", "id": uid(i, j),
                                              "score": 0.09 - (i as f64) / 1000.0,
                                              "legs": ["keyword", "semantic"]}));
            }
        }
    }
    for i in 0..6 {
        let happened = format!("2026-05-{:02}T09:00:00.000000Z", i + 1);
        let mut ep = episode(
            &uid(800, i),
            &format!("we talked about editors, part {i}"),
            &happened,
        );
        ep["session_id"] = serde_json::json!(uid(900, i));
        eps.push(ep);
        cands.push(serde_json::json!({"kind": "episode", "id": uid(800, i),
                                      "score": 0.05 - (i as f64) / 1000.0,
                                      "legs": ["keyword"]}));
    }
    let f = serde_json::json!({
        "candidates": cands,
        "legs_present": ["keyword", "semantic"],
        "leg_sizes": {"keyword": 12, "semantic": 6, "graph": 0, "temporal": 0},
        "leg_capped": {},
        "semantic_degraded": false
    });
    doc_of(
        f,
        serde_json::json!(eps),
        serde_json::json!(hyd),
        serde_json::json!(axis),
    )
}

#[test]
fn the_payload_is_a_quarter_of_the_plumbing() {
    // The acceptance line of #296 — "roughly a quarter of the tokens" — as a
    // ratio a fixture can carry. Both halves describe the same eighteen
    // candidates; the difference is entirely what a reader can do with them.
    let msg = bundle_message(&emit(measurement_doc()));
    let bundle = payload_text(&msg);
    let record = msg["recall_diagnostic"].to_string();

    assert_eq!(
        payload(&msg)["candidates"]
            .as_array()
            .expect("candidates")
            .len(),
        18,
        "nothing was cut — the comparison is over the same eighteen candidates"
    );
    assert_eq!(
        msg["recall_diagnostic"]["candidates"]
            .as_array()
            .expect("record candidates")
            .len(),
        18,
        "and the record holds all eighteen too"
    );
    assert!(
        bundle.len() * 4 < record.len(),
        "the payload is {} chars against {} of record ({:.2}x, wanted > 4x): {bundle}",
        bundle.len(),
        record.len(),
        record.len() as f64 / bundle.len() as f64
    );
}
