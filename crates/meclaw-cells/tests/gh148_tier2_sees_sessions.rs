//! GH #148 — tier 2 gets the candidates grouped by session, not just ranked.
//!
//! The measurement that started this: 50 stratified LongMemEval questions
//! against the 0.9.0 tree returned 96 % R@5 and 58 % end accuracy, and the
//! `multi-session` class was the sharp one — **100 % R@5 against 30.8 %
//! accuracy**. In 19 of 21 wrong answers the retrieval had delivered the gold
//! session. The evidence was in the bundle and the synthesis did not combine
//! it.
//!
//! Which is not surprising once you look at the shape it arrived in: one flat
//! list ordered by RRF rank, with nothing saying which candidates came from the
//! same conversation. A question that counts, compares, or asks what changed is
//! an aggregation over sessions, and it was being asked of a structure that had
//! thrown the session axis away.
//!
//! So tier 2 now also receives `sessions`: the same candidates, grouped by the
//! conversation they came from, oldest first. Nothing extra is retrieved — this
//! is a shape change, and the tests below are about the shape.
//!
//! What this file does NOT claim: that accuracy improved. That is a benchmark
//! run, not a unit test, and it is the honest gate for this issue. These tests
//! pin the structure the run would be measuring.

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

fn episode(id: &str, session: &str, content: &str, happened_at: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": session, "sender": "user",
                       "content": content, "happened_at": happened_at,
                       "recorded_at": happened_at})
}

/// A fact row as the hydration select returns it — including the `session_id`
/// column, which has always been selected.
fn fact(
    id: &str,
    session: &str,
    subject: &str,
    claim: &str,
    valid_from: &str,
) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": session, "subject": subject,
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

/// The `t1-emit` document of a **tier 2** request: same scratch shape as tier 1,
/// `memory_tier` is what routes it to the dialectic.
fn emit_doc(
    candidates: serde_json::Value,
    eps: serde_json::Value,
    facts: serde_json::Value,
) -> serde_json::Value {
    let fused = serde_json::json!({
        "candidates": candidates,
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": 5, "semantic": 0, "graph": 0, "temporal": 0},
        "semantic_degraded": true
    });
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": eps.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "2",
                        "recall_query": "how often did I change editors?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The message that goes to the dialectic cell, and the JSON it carries.
fn dialectic_payload(msgs: &[serde_json::Value]) -> serde_json::Value {
    let m = msgs
        .iter()
        .find(|m| m["header"]["route"] == "dialectic")
        .expect("a tier-2 request emits a dialectic message");
    let text = m["messages"][0]["text"].as_str().expect("payload text");
    serde_json::from_str(text).expect("payload json")
}

/// Three sessions, out of chronological order in the ranking — which is the
/// normal case, because RRF ranks by relevance and relevance is not time.
fn three_sessions() -> (serde_json::Value, serde_json::Value, serde_json::Value) {
    let candidates = serde_json::json!([
        {"kind": "episode", "id": "e-mid", "score": 0.09, "legs": ["keyword"]},
        {"kind": "fact", "id": "f-late", "score": 0.08, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-early", "score": 0.07, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-mid-2", "score": 0.06, "legs": ["keyword"]}
    ]);
    let eps = serde_json::json!([
        episode(
            "e-early",
            "s-jan",
            "I use vim",
            "2026-01-05T09:00:00.000000Z"
        ),
        episode(
            "e-mid",
            "s-apr",
            "switched to helix",
            "2026-04-05T09:00:00.000000Z"
        ),
        episode(
            "e-mid-2",
            "s-apr",
            "helix is nice",
            "2026-04-05T09:30:00.000000Z"
        ),
    ]);
    let facts = serde_json::json!([fact(
        "f-late",
        "s-aug",
        "person:example",
        "zed",
        "2026-08-01T09:00:00.000000Z"
    )]);
    (candidates, eps, facts)
}

#[test]
fn the_dialectic_payload_carries_the_sessions_view() {
    let (c, e, f) = three_sessions();
    let payload = dialectic_payload(&emit(emit_doc(c, e, f)));

    let sessions = payload["sessions"]
        .as_array()
        .expect("`sessions` travels with the tier-2 payload");
    let ids: Vec<&str> = sessions
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["s-jan", "s-apr", "s-aug"],
        "sessions come oldest first — the model reads this as a timeline"
    );

    // The flat ranking is still there. `sessions` is an additional view, not a
    // replacement: the ranking answers "what is most relevant", the grouping
    // answers "what belongs together", and a spanning question needs both.
    assert_eq!(
        payload["candidates"].as_array().map(|c| c.len()),
        Some(4),
        "the ranked list travels unchanged next to the grouping"
    );
}

#[test]
fn candidates_of_one_session_are_grouped_and_counted() {
    let (c, e, f) = three_sessions();
    let payload = dialectic_payload(&emit(emit_doc(c, e, f)));
    let sessions = payload["sessions"].as_array().unwrap();

    let apr = sessions
        .iter()
        .find(|s| s["session_id"] == "s-apr")
        .expect("s-apr");
    assert_eq!(
        apr["n"], 2,
        "the two April candidates are counted as one session, not two occurrences"
    );
    let members: Vec<&str> = apr["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        members.contains(&"e-mid") && members.contains(&"e-mid-2"),
        "both April candidates belong to their session: {members:?}"
    );
    assert_eq!(
        apr["first"], "2026-04-05T09:00:00.000000Z",
        "the group spans from its earliest instant"
    );
    assert_eq!(apr["last"], "2026-04-05T09:30:00.000000Z", "to its latest");
}

#[test]
fn a_fact_is_grouped_by_the_session_it_was_said_in() {
    // The fact branch of the hydration used to drop `session_id` even though the
    // column was selected — so a fact could never be grouped with the
    // conversation it came from, which is most of what a multi-session answer
    // is made of.
    let (c, e, f) = three_sessions();
    let payload = dialectic_payload(&emit(emit_doc(c, e, f)));
    let sessions = payload["sessions"].as_array().unwrap();

    let aug = sessions
        .iter()
        .find(|s| s["session_id"] == "s-aug")
        .expect("the fact's session is a group of its own");
    let members: Vec<&str> = aug["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(members, vec!["f-late"], "the fact sits in its own session");
}

#[test]
fn a_candidate_without_a_session_is_named_rather_than_dropped() {
    // A row written before the column existed has no session. Silently dropping
    // it would delete evidence; folding it into a neighbour would invent a
    // shared origin. It gets its own bucket and says what it is.
    let candidates = serde_json::json!([
        {"kind": "episode", "id": "e-known", "score": 0.09, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-orphan", "score": 0.08, "legs": ["keyword"]}
    ]);
    let eps = serde_json::json!([
        episode("e-known", "s-jan", "I use vim", "2026-01-05T09:00:00.000000Z"),
        {"id": "e-orphan", "session_id": serde_json::Value::Null, "sender": "user",
         "content": "an old row", "happened_at": "2026-01-06T09:00:00.000000Z",
         "recorded_at": "2026-01-06T09:00:00.000000Z"}
    ]);
    let payload = dialectic_payload(&emit(emit_doc(candidates, eps, serde_json::json!([]))));
    let sessions = payload["sessions"].as_array().unwrap();

    let orphan = sessions
        .iter()
        .find(|s| s["session_id"] == "unattributed")
        .expect("a candidate with no session lands in `unattributed`");
    assert_eq!(orphan["n"], 1);
    let total: u64 = sessions.iter().map(|s| s["n"].as_u64().unwrap()).sum();
    assert_eq!(
        total, 2,
        "every candidate is in exactly one group — nothing is lost to the grouping"
    );
}

#[test]
fn the_prompt_tells_the_model_what_to_do_with_the_grouping() {
    // A structure the instructions never mention is a structure the model is
    // free to ignore, and this one exists precisely because it was being
    // ignored.
    let (c, e, f) = three_sessions();
    let msgs = emit(emit_doc(c, e, f));
    let m = msgs
        .iter()
        .find(|m| m["header"]["route"] == "dialectic")
        .unwrap();
    let prompt = m["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");

    assert!(
        prompt.contains("sessions"),
        "the prompt names the structure it was given"
    );
    for phrase in ["Counting means counting the SESSIONS", "CHANGE OVER TIME"] {
        assert!(
            prompt.contains(phrase),
            "the prompt says how to aggregate ({phrase:?} missing)"
        );
    }
}
