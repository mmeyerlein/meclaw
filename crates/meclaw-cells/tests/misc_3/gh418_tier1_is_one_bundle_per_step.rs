//! GH #418 — the tier-1 recall chain, counted.
//!
//! Ruling R1 of the perf/structure wave asks for a NUMBER: "≤6 store messages
//! per tier-1 recall". A budget is not kept by a shape assertion, it is kept by
//! a count — so every test in this file counts store messages, and the ones
//! that name `tool_call_id`s do it to prove that the ops which used to be N
//! messages really are N calls of ONE message rather than N messages with new
//! names.
//!
//! The chain, phase by phase, and what each one costs the store:
//!
//! | step | phase | store messages |
//! |---|---|---|
//! | S1 | `t1-fan` | 1 (plus the embed ask, which is not a store message) |
//! | S2 | `t1-park` | 1 (the fan's half of the rendezvous) |
//! | Sq | `t1-qvec-park` | 1 (the embedder's half -- the one that carries on) |
//! | S3 | `t1-join` | 1 |
//! | S4 | `t1-legs` | 1 |
//! | S4a | `t1-graph` | 1, and ONLY when the graph walk reached a node (GH #520) |
//! | S5 | `t1-emit` | 1 (park, hydration, axis page, verdicts, read-back) |
//!
//! S4a is the one conditional message of the chain: the nodes a walk passed
//! through are known only after the walk, so the facts they are about cost a
//! bundle that a request with no anchors — or one whose walk came back empty —
//! never sends. Every fixture in this file answers the walk with `{"paths":
//! []}`, so what it counts is the six below; the seventh is counted in
//! `gh520_from_an_entity_to_a_fact.rs`.
//!
//! **Six.** The reply to S5 renders and emits the answer; it asks the store
//! nothing more. Tier 2 costs a seventh, and it is deliberately NOT on the
//! critical path: the rendered bundle leaves as one parked row BESIDE the
//! provider call, so a provider error has a single row to read back instead of
//! a projection to run twice.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const AUDIENCE: &str = r#"["user"]"#;
const CHANNEL: &str = "c1";
const RID: &str = "req-1";

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
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts are within a few KB of that).
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

/// The `tool_call_id`s of one emitted message, in call order.
fn ids_of(msg: &serde_json::Value) -> Vec<&str> {
    msg["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter(|t| t["type"] == "tool_call")
        .map(|t| t["id"].as_str().expect("tool_call id"))
        .collect()
}

/// How many of these emissions go to the store.
fn store_messages(msgs: &[serde_json::Value]) -> usize {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "rstore")
        .count()
}

/// The tier-1 context every phase of one request carries.
fn ctx(phase: &str) -> serde_json::Value {
    serde_json::json!({"mem_phase": phase, "recall_id": RID, "memory_tier": "1",
                       "recall_query": "what does the user eat",
                       "audience_now": AUDIENCE, "channel": CHANNEL})
}

/// A store BUNDLE reply (#295): N `tool_result` turns plus the top-level
/// `results[]` slot, correlated by `tool_call_id`.
fn bundle_reply(phase: &str, legs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": ctx(phase),
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| serde_json::json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| serde_json::json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

/// One `recall_scratch` row as the store answers it.
fn scratch(leg: &str, payload: &str) -> serde_json::Value {
    serde_json::json!({"request_id": RID, "leg": leg, "payload": payload, "fired": 0})
}

/// The reply that reaches the FUSION: the `t1-legs` bundle, whose read-back
/// carries the parked `legs` row and whose other calls carry the walk and the
/// semantic companion.
///
/// There is no fusion PHASE any more -- the last hop before the hydration fuses
/// out of its own reply, which is the whole point of the chain.
fn fuse_doc() -> serde_json::Value {
    bundle_reply(
        "t1-legs",
        &[
            ("r-legs-graph", serde_json::json!({"paths": []})),
            ("r-legs-sem-aud", serde_json::json!([])),
            (
                "r-legs-read",
                serde_json::json!([
                    scratch(
                        "legs",
                        &serde_json::json!({
                            "kw-ep": [{"kind": "episode", "id": "e1"}],
                            "kw-fact": [{"kind": "fact", "id": "f1"}],
                            "temporal": [], "beliefs": [], "anchors": [],
                            // The axis map the fan parked: the canonical keys of
                            // the fact the keyword leg nominated. Deriving the
                            // axis page from THIS instead of from the hydration
                            // rows is what lets it ride in the same bundle.
                            "axis": {"f1": ["user", "eats"]},
                            "model": {"model_id": "m", "dim": 1024}
                        })
                        .to_string()
                    ),
                    scratch("sem", "[]"),
                ]),
            ),
        ],
    )
}

/// The tier-1 request as it leaves the door.
fn request_doc(tier: &str, query: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "", "recall_id": RID, "memory_tier": tier,
                        "recall_query": query, "session_id": "s1",
                        "audience_now": AUDIENCE, "channel": CHANNEL},
            "hop": {"phase": "recall"}
        },
        "messages": []
    })
}

// ---------------------------------------------------------------------------

/// The fused ranking, its two hydration pages and the read-back are ONE store
/// message. Not "fewer messages": exactly one, because a count is the only
/// assertion a round-trip budget can be made of (GH #418, ruling R1).
#[test]
fn the_hydration_round_is_one_bundle() {
    let out = emit(fuse_doc());
    assert_eq!(
        store_messages(&out),
        1,
        "the fusion must emit ONE store message: {out:#?}"
    );
    assert_eq!(
        ids_of(&out[0]),
        [
            "r-hyd-fused",
            "r-hyd-fact",
            "r-hyd-ep",
            "r-hyd-axis",
            "r-hyd-card",
            "r-hyd-read"
        ],
        "park, fact page, episode page, axis page, verdicts, read-back: {out:#?}"
    );
}

/// The read-back is LAST, and that position is the whole election mechanism:
/// a select in front of the park would read the request's parking place as it
/// was before this hop wrote to it (pinned in
/// `gh418_a_bundle_sees_its_own_writes.rs`).
#[test]
fn the_read_back_is_the_last_call_of_its_bundle() {
    let out = emit(fuse_doc());
    let ids = ids_of(&out[0]);
    assert!(
        ids.last().expect("at least one call").ends_with("-read"),
        "the read-back must close the bundle: {ids:?}"
    );
}

/// The tier-1 request costs ONE store message. The embed request beside it is
/// not one -- it goes to the embedder, and it is the reason the semantic leg
/// waits at all (GH #418, ruling R1).
#[test]
fn the_tier1_request_is_one_store_message_plus_the_embed_ask() {
    let out = emit(request_doc("1", "what does the user eat"));
    assert_eq!(out.len(), 2, "{out:#?}");
    assert_eq!(store_messages(&out), 1, "{out:#?}");
    assert_eq!(out[0]["header"]["phase"], "t1-fan");
    assert_eq!(out[1]["header"]["route"], "embed");
}

/// The whole tier-1 chain, counted: fan, rendezvous, join, legs, hydration,
/// emit. Six store messages, and the count is the acceptance criterion of
/// ruling R1.
#[test]
fn a_tier1_recall_costs_six_store_messages() {
    let mut n = 0;
    let mut named: Vec<String> = Vec::new();
    for doc in tier1_chain() {
        let out = emit(doc);
        for m in &out {
            if m["header"]["route"] == "rstore" {
                n += 1;
                named.push(m["header"]["phase"].as_str().unwrap_or("?").to_string());
            }
        }
    }
    // Both halves of the rendezvous cost a message -- the fan's park and the
    // vector's park -- and only the second of them carries on. The budget is
    // therefore fan + park + qvec-park + join + legs + emit.
    assert_eq!(
        n, 6,
        "tier 1 must cost six store messages, not {n} ({named:?})"
    );
}

/// The six replies of one tier-1 request, in the order the store answers them.
///
/// Every document is what the PREVIOUS step's message comes back as, so the
/// chain is walked rather than asserted about: the request, the fan reply, the
/// rendezvous (the embedder is second, so the vector's park is the one that
/// completes), the join, the legs, the hydration and the emit.
fn tier1_chain() -> Vec<serde_json::Value> {
    let store_legs = serde_json::json!({
        "kw-ep": [], "kw-fact": [], "temporal": [], "beliefs": [],
        "anchors": ["Berlin"], "axis": {},
        "model": {"model_id": "m", "dim": 1024}
    });
    let qvec = serde_json::json!({"vector": "AAEC", "degraded": false, "error": null});
    vec![
        // S1: the request itself.
        request_doc("1", "what does the user eat"),
        // S2: the fan comes back; the legs park and read the rendezvous.
        bundle_reply(
            "t1-fan",
            &[
                ("r-fan-kw-ep", serde_json::json!([])),
                ("r-fan-kw-fact", serde_json::json!([])),
                ("r-fan-temporal", serde_json::json!([])),
                (
                    "r-fan-model",
                    serde_json::json!([{"model_id": "m", "dim": 1024}]),
                ),
            ],
        ),
        // Sq: the embedder answers. Its half parks and reads the parking place
        // back in ONE message -- this is the message the budget must count.
        serde_json::json!({
            "header": {"context": ctx("t1-qvec"),
                       "hop": {"route": "embed", "rows_affected": 1}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": qvec.to_string()}]
        }),
        // ... and that park reads a COMPLETE set, because the vector is second
        // as it always is: the store answers in milliseconds, the embedder in
        // seconds.
        bundle_reply(
            "t1-qvec-park",
            &[(
                "r-t1-qvec-park-read",
                serde_json::json!([
                    scratch("legs", &store_legs.to_string()),
                    scratch("qvec", &qvec.to_string()),
                ]),
            )],
        ),
        // S3: the anchors and the vector neighbours come back together.
        bundle_reply(
            "t1-join",
            &[
                (
                    "r-join-anchor",
                    serde_json::json!([{"id": "e-b",
                                                      "canonical_name": "Berlin"}]),
                ),
                ("r-join-sem", serde_json::json!([])),
            ],
        ),
        // S4: the walk, the companion and the read-back that carries the rest.
        bundle_reply(
            "t1-legs",
            &[
                ("r-legs-graph", serde_json::json!({"paths": []})),
                (
                    "r-legs-read",
                    serde_json::json!([
                        scratch("legs", &store_legs.to_string()),
                        scratch("sem", "[]"),
                    ]),
                ),
            ],
        ),
        // S5: the hydration round, which is also the last one -- the axis page
        // and the judged verdicts ride in it. Its reply renders and emits, and
        // asks the store nothing more, which is what makes the budget six.
        bundle_reply(
            "t1-emit",
            &[
                ("r-hyd-fused", serde_json::json!([])),
                ("r-hyd-ep", serde_json::json!([])),
                (
                    "r-hyd-read",
                    serde_json::json!([scratch("fused", r#"{"candidates":[],"legs_present":[]}"#)]),
                ),
            ],
        ),
    ]
}
