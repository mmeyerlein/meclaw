//! GH #146 -- when recall fuses without the semantic leg, its warn line names
//! the REASON the embedder gave.
//!
//! Wave #144 made the moment loud: `t1-semfire` is the one place that knows the
//! fan is about to run on three legs, and it writes a stderr line the substrate
//! turns into a warn row. The line was already reaching for the embedder's own
//! wording (`qvec.get("error")`) -- but the reason never got that far: the
//! `t1-qvec` hop stored `vector` and `degraded` and dropped `error` on the
//! floor, so every cause in the world collapsed into "no query vector".
//!
//! That is the one thing an operator cannot act on. A timeout after two attempts,
//! an HTTP 401, and a model generation that has no active row are three
//! different jobs; the log has to tell them apart.
//!
//! The REAL `params.script_inline` runs here against a stub stdin -- the same
//! probe pattern as `q2_recall_query_guard.rs`, and nothing costs anything.

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
    let v: Value = serde_json::from_str(&raw).expect("recall config json");
    resolve_vars(v["params"]["script_inline"].as_str().expect("script"))
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` the empty string --
/// the same substitution the colony performs at instantiation.
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

struct Run {
    msgs: Vec<Value>,
    stderr: String,
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

/// Feeds one hop into the recall cell: `context` carries the phase and the
/// request id, `hop` whatever the store reported, and `text` is the incoming
/// message payload.
fn run_phase(phase: &str, hop: Value, text: String) -> Run {
    run_stdin(json!({
        "envelope": {"header": {"context": {"mem_phase": phase, "recall_id": "rid-1",
                                            "memory_tier": "1", "recall_query": "what do I eat"},
                                "hop": hop}},
        "body": {"messages": [{"origin": "tool", "type": "tool_result", "id": "in",
                               "text": text}]},
        "params": {},
    }))
}

/// The rendezvous, as the store answers it.
///
/// Since GH #418 the five legs that need no query vector park as ONE row and
/// the query vector parks as the other; both halves read the parking place back
/// in their own bundle, and whichever is second carries on. So the reply is a
/// BUNDLE whose trailing `r-t1-park-read` turn carries the parked rows. What the
/// two tests below measure is untouched: the warn line the fan writes when it is
/// short a leg, and the reason inside it.
fn run_rendezvous(rows: Value) -> Run {
    run_stdin(json!({
        "envelope": {"header": {"context": {"mem_phase": "t1-park", "recall_id": "rid-1",
                                            "memory_tier": "1",
                                            "recall_query": "what do I eat"},
                                "hop": {"operation": "bundle", "rows_affected": 1,
                                        "bundle_errors": 0}}},
        "body": {"messages": [{"origin": "tool", "type": "tool_result",
                               "id": "r-t1-park-read", "text": rows.to_string()}],
                 "results": [{"tool_call_id": "r-t1-park-read", "operation": "select",
                              "rows_affected": 1, "duration_ms": 0}]},
        "params": {},
    }))
}

fn run_stdin(doc: Value) -> Run {
    let stdin = doc.to_string();

    let out = run_script_on_stdin(&recall_script(), &stdin);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "recall must not die: {stderr}");
    let msgs: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a multi-send ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    Run { msgs, stderr }
}

/// The scratch payload the emitted insert would write.
fn inserted_payload(run: &Run) -> Value {
    // #418: the hop parks its half and reads the join back in ONE message, so
    // the insert is the FIRST CALL of that message rather than the message.
    assert_eq!(
        run.msgs.len(),
        1,
        "one store message per hop: {:?}",
        run.msgs
    );
    let args: Value = serde_json::from_str(run.msgs[0]["messages"][0]["text"].as_str().unwrap())
        .expect("store args json");
    assert_eq!(args["operation"], "insert");
    serde_json::from_str(args["row"]["payload"].as_str().expect("payload string"))
        .expect("payload json")
}

/// Half one: the reason survives the hop into the scratch table. Without this
/// the warn line downstream has nothing to say.
#[test]
fn the_embedders_reason_is_carried_into_the_scratch_row() {
    let answer = json!({"vector": null, "degraded": true, "model_id": "m",
                        "dim": 1024, "recall_id": "rid-1",
                        "error": "endpoint unreachable (timeout) (after 2 attempt(s))"});
    let run = run_phase("t1-qvec", json!({}), answer.to_string());
    let payload = inserted_payload(&run);

    assert_eq!(payload["degraded"], true);
    assert_eq!(
        payload["error"], "endpoint unreachable (timeout) (after 2 attempt(s))",
        "the embedder's own wording travels with its verdict: {payload}"
    );
}

/// A healthy hop is unchanged apart from the (absent) reason -- the field is
/// carried, not invented.
#[test]
fn a_healthy_query_vector_carries_no_reason() {
    let answer = json!({"vector": "oA==", "degraded": false, "model_id": "m",
                        "dim": 1024, "recall_id": "rid-1"});
    let run = run_phase("t1-qvec", json!({}), answer.to_string());
    let payload = inserted_payload(&run);

    assert_eq!(payload["degraded"], false);
    assert_eq!(payload["vector"], "oA==");
    assert!(
        payload["error"].is_null(),
        "no failure, no reason: {payload}"
    );
}

/// Half two: at the fan the reason is what the warn line says. This is the row
/// an operator greps when a colony answers on three legs.
#[test]
fn the_warn_line_at_the_fan_names_that_reason() {
    let rows = json!([
        {"request_id": "rid-1", "leg": "legs", "fired": 0,
         "payload": json!({"model": {"model_id": "qwen/qwen3-embedding-8b", "dim": 1024},
                           "anchors": []}).to_string()},
        {"request_id": "rid-1", "leg": "qvec", "fired": 1,
         "payload": json!({"vector": null, "degraded": true,
                           "error": "endpoint returned HTTP 500 (after 2 attempt(s))"}).to_string()},
    ]);
    let run = run_rendezvous(rows);

    assert!(
        run.stderr
            .contains("semantic leg skipped, fusing three legs"),
        "the fan still says out loud that it is short a leg: {:?}",
        run.stderr
    );
    assert!(
        run.stderr.contains("HTTP 500") && run.stderr.contains("2 attempt"),
        "and it says WHY, in the embedder's words: {:?}",
        run.stderr
    );
}

/// No reason available (an embedder that never answered at all, or a store with
/// no active model row) still produces a line -- the fallback wording is the
/// floor, not the norm.
#[test]
fn without_a_reason_the_line_falls_back_instead_of_falling_silent() {
    let rows = json!([
        {"request_id": "rid-1", "leg": "legs", "fired": 0,
         "payload": json!({"model": {"model_id": "qwen/qwen3-embedding-8b", "dim": 1024},
                           "anchors": []}).to_string()},
        {"request_id": "rid-1", "leg": "qvec", "fired": 1,
         "payload": json!({"vector": null, "degraded": true}).to_string()},
    ]);
    let run = run_rendezvous(rows);

    assert!(
        run.stderr
            .contains("semantic leg skipped, fusing three legs"),
        "silence here is the defect #144 closed: {:?}",
        run.stderr
    );
    assert!(
        run.stderr.contains("no query vector"),
        "the fallback wording still names what was missing: {:?}",
        run.stderr
    );
}
