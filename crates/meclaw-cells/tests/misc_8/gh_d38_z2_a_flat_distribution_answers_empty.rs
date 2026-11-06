//! GH #1441 (D38 Z2) -- a flat score distribution answers with an empty bundle.
//!
//! WHY this file exists: RK-38-7 and OR-38-2 (memory in the window: little and
//! on purpose). The fusion always ranks: a question nothing in the memory
//! answers still gets its TOPK candidates, and a model handed them reads them
//! as an answer. The SHAPE of the fused scores tells the two apart -- one hit
//! clearly above the rest, or a flat field where the best is barely above the
//! middle. The new recall knob `flat_ratio` (0 shipped = off): when the best
//! fused score over the median of the first TOPK is at or below it, the recall
//! answers with an empty bundle (`answers` "none", `recall_empty` "1") and says
//! why in `recall_diagnostic.flat` = true. A single candidate has no
//! distribution and is never flat. 0 is byte for byte the bundle of before.
//!
//! Same full-document harness as `gh297_a_leg_may_come_back_short.rs`: the
//! REAL `params.script_inline` runs over a `t1-emit` document whose `fused`
//! leg carries the scores; no store, no model.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn recall_script() -> String {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    let v: Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

fn recall_config() -> Value {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    serde_json::from_str(&raw).unwrap()
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` the empty string --
/// the substitution the colony performs at instantiation.
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

/// The program goes to python3 on stdin: one argv string is capped at 128 KiB
/// and the recall script is larger.
fn run_python(src: &str) -> std::process::Output {
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

fn emit(doc: Value) -> Vec<Value> {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        serde_json::to_string(&meclaw_testing::code_stdin(&doc).to_string()).unwrap(),
    );
    let out = run_python(&src);
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

/// A `t1-emit` document over the scratch legs, with the recall's params.
fn emit_doc(params: Value, legs: &[(&str, Value)]) -> Value {
    let rows: Vec<Value> = legs
        .iter()
        .map(|(name, payload)| {
            json!({"request_id": "r1", "leg": name, "payload": payload.to_string(), "fired": 1})
        })
        .collect();
    json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what did I do at the lake?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "select"}
        },
        "params": params,
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": Value::Array(rows).to_string()}]
    })
}

/// A fused record of episodes `e0..` with these scores, best first.
fn fused(scores: &[f64]) -> Value {
    let n = scores.len();
    json!({
        "candidates": scores.iter().enumerate().map(|(i, s)| json!(
            {"kind": "episode", "id": format!("e{i}"), "score": s,
             "legs": ["keyword"], "agreement": 1})).collect::<Vec<_>>(),
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": n, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_sizes_raw": {"keyword": n, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_capped": {},
        "semantic_degraded": true
    })
}

/// The hydrated episodes behind `fused`: one turn each.
fn episodes(n: usize) -> Value {
    json!(
        (0..n)
            .map(
                |i| json!({"id": format!("e{i}"), "session_id": "s-1", "sender": "user",
                            "content": format!("lake story {i}"),
                            "happened_at": "2026-01-01T09:00:00Z",
                            "recorded_at": "2026-01-01T09:00:00Z"})
            )
            .collect::<Vec<_>>()
    )
}

fn run(params: Value, scores: &[f64]) -> (Value, Value) {
    let msgs = emit(emit_doc(
        params,
        &[
            ("fused", fused(scores)),
            ("hyd-fact", json!([])),
            ("hyd-ep", episodes(scores.len())),
            ("hyd-axis", json!([])),
        ],
    ));
    let msg = msgs
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")
        .clone();
    let bundle: Value = serde_json::from_str(
        msg["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle text"),
    )
    .expect("bundle json");
    (msg, bundle)
}

/// Five candidates whose best is barely above the middle (1.012 x the median).
const FLAT: [f64; 5] = [0.0164, 0.0163, 0.0162, 0.0161, 0.0160];
/// One clear hit above four of the field (3.09 x the median).
const PEAKED: [f64; 5] = [0.05, 0.0163, 0.0162, 0.0161, 0.0160];

#[test]
fn a_flat_distribution_answers_with_an_empty_bundle() {
    let (msg, bundle) = run(json!({"flat_ratio": 1.05}), &FLAT);
    assert_eq!(bundle["answers"], json!("none"), "{bundle}");
    assert!(
        bundle["candidates"].as_array().expect("a list").is_empty(),
        "no candidate of a flat field reaches the window: {bundle}"
    );
    assert_eq!(
        msg["header"]["recall_empty"],
        json!("1"),
        "{}",
        msg["header"]
    );
    assert_eq!(
        msg["recall_diagnostic"]["flat"],
        json!(true),
        "the diagnostic says why: {}",
        msg["recall_diagnostic"]
    );
    let text = msg["messages"][0]["text"].as_str().expect("rendered turn");
    assert!(
        !text.contains("lake story"),
        "the rendered turn carries none of the field: {text:?}"
    );
}

#[test]
fn a_clear_hit_answers_as_before_with_the_knob_on() {
    let (msg, bundle) = run(json!({"flat_ratio": 1.05}), &PEAKED);
    assert_eq!(bundle["answers"], json!("direct"), "{bundle}");
    assert_eq!(
        bundle["candidates"].as_array().expect("a list").len(),
        5,
        "{bundle}"
    );
    assert!(
        msg["recall_diagnostic"].get("flat").is_none(),
        "absent unless true: {}",
        msg["recall_diagnostic"]
    );
}

/// One candidate has no distribution: never flat, whatever the knob says.
#[test]
fn a_single_candidate_is_never_flat() {
    let (msg, bundle) = run(json!({"flat_ratio": 5.0}), &[0.0164]);
    assert_eq!(bundle["answers"], json!("direct"), "{bundle}");
    assert!(msg["recall_diagnostic"].get("flat").is_none());
}

/// The knob ships 0 in params and setting, and 0 is the bundle of before: the
/// flat field is answered with its rows, no `flat` key anywhere.
#[test]
fn the_knob_ships_off_and_off_is_byte_identical() {
    let cfg = recall_config();
    assert_eq!(cfg["params"]["flat_ratio"], json!(0), "{}", cfg["params"]);
    assert_eq!(
        cfg["contract"]["settings"]["flat_ratio"]["default"],
        json!(0),
        "the setting says the same"
    );
    for params in [json!({}), json!({"flat_ratio": 0})] {
        let (shipped_msg, shipped) = run(params, &FLAT);
        assert_eq!(shipped["answers"], json!("direct"), "{shipped}");
        assert_eq!(
            shipped["candidates"].as_array().expect("a list").len(),
            5,
            "{shipped}"
        );
        assert!(shipped_msg["recall_diagnostic"].get("flat").is_none());
        assert!(shipped_msg["header"].get("recall_empty").is_none());
    }
}
