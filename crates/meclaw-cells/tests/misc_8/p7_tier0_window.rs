//! 0.2.0 P7 -- tier 0 says that it ignored a recall window (GitHub #16).
//!
//! A HALF window is a caller bug and leaves through the reject port at request
//! entry, for every tier (P15 ruling R5). A COMPLETE window on a tier-0 request
//! was the one silent deviation the window feature left behind: tier 0 has fixed
//! legs (session episodes, active beliefs, open foresight) and no temporal leg to
//! cut, so the range was dropped without a word and the caller got a correct
//! bundle that did not answer the question asked.
//!
//! The ruling is the smallest honest form: a `window_ignored` marker in the
//! bundle. No reject (that would break existing callers) and no auto-upgrade to
//! tier 1 (that would change the cost of a request unannounced).
//!
//! Everything here runs the shipped `params.script_inline` against real stdin
//! documents, so nothing is mocked and nothing is spent.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
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

/// Run the real script against a real stdin document and return the emitted messages.
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

const FROM: &str = "2026-08-01T00:00:00Z";
const TO: &str = "2026-08-09T00:00:00Z";

/// The request context. `audience_now`/`channel` are the asking round the read
/// path is fail-closed on since #244 -- one person asking one agent in one
/// room, which is the whole cast of this file. Not `["*"]`: the reject this
/// file pins sits at request entry, next to the audience check, and a
/// universal round would hide a read path that stopped gating.
fn context(from: &str, to: &str, phase: &str) -> serde_json::Value {
    serde_json::json!({"mem_phase": phase, "recall_id": "r1", "memory_tier": "0",
                       "recall_query": "what do you know about the editor?",
                       "recall_as_of": "",
                       "audience_now": r#"["member:user","agent:assistant"]"#,
                       "channel": "c-p7",
                       "recall_window_from": from, "recall_window_to": to})
}

/// The store's answer to a tier-0 request: ONE bundle carrying all three fixed
/// legs (GH #295). Schema-pure `tool_result` turns in call order, the per-leg
/// metadata beside them in the body's top-level `results[]` slot, correlated by
/// `tool_call_id` — the shape `store` really builds for N ops.
fn bundle_doc(from: &str, to: &str) -> serde_json::Value {
    let episodes = serde_json::json!([
        {"id": "e1", "session_id": "s1", "sender": "user",
         "content": "The editor of choice is vscode.",
         "happened_at": "2026-08-09T09:00:00.000000Z",
         "recorded_at": "2026-08-09T09:00:01.000000Z",
         "channel": "c-p7", "audience_set": r#"["member:user","agent:assistant"]"#}
    ]);
    serde_json::json!({
        "header": {"context": context(from, to, "legs"),
                   "hop": {"operation": "bundle", "rows_affected": 1, "duration_ms": 2,
                           "bundle_errors": 0}},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-leg-episodes",
             "text": episodes.to_string()},
            {"origin": "tool", "type": "tool_result", "id": "r-leg-beliefs", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-leg-foresight", "text": "[]"}
        ],
        "results": [
            {"tool_call_id": "r-leg-episodes", "operation": "select", "rows_affected": 1,
             "duration_ms": 1},
            {"tool_call_id": "r-leg-beliefs", "operation": "select", "rows_affected": 0,
             "duration_ms": 1},
            {"tool_call_id": "r-leg-foresight", "operation": "select", "rows_affected": 0,
             "duration_ms": 1}
        ]
    })
}

/// A fresh request as the port edge delivers it: no phase, no store operation.
fn request_doc(from: &str, to: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": context(from, to, ""), "hop": {}},
        "messages": [{"origin": "user", "type": "text", "id": "q",
                      "text": "what do you know about the editor?"}]
    })
}

fn bundle_of(msgs: &[serde_json::Value]) -> serde_json::Value {
    let text = msgs[0]["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle text");
    serde_json::from_str(text).expect("bundle json")
}

fn rendered_of(msgs: &[serde_json::Value]) -> String {
    msgs[0]["messages"][0]["text"]
        .as_str()
        .expect("rendered text")
        .to_string()
}

fn phases(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .map(|m| {
            m["header"]["phase"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The done-when of #16: the range the tier could not honour is stated, in the
/// JSON body, in the text the model reads, and on the hop for a router.
#[test]
fn a_complete_window_on_tier_0_is_named_in_the_bundle() {
    let msgs = emit(bundle_doc(FROM, TO));
    let bundle = bundle_of(&msgs);
    assert_eq!(
        bundle["window_ignored"],
        serde_json::json!({"from": FROM, "until": TO}),
        "bundle: {bundle}"
    );
    assert_eq!(msgs[0]["header"]["window_ignored"], "1");
    let rendered = rendered_of(&msgs);
    assert!(
        rendered.contains(&format!("window_ignored: {FROM} -> {TO}")),
        "rendered: {rendered}"
    );
    // the bundle itself is unchanged otherwise -- the marker is honesty, not a
    // second answer.
    assert_eq!(bundle["episodes"].as_array().expect("episodes").len(), 1);
}

/// Nothing new is noisy: without a window the tier-0 bundle carries no marker at
/// all, in neither half of the message.
#[test]
fn a_request_without_a_window_carries_no_marker() {
    let msgs = emit(bundle_doc("", ""));
    let bundle = bundle_of(&msgs);
    assert!(bundle.get("window_ignored").is_none(), "bundle: {bundle}");
    assert!(msgs[0]["header"].get("window_ignored").is_none());
    assert!(
        !rendered_of(&msgs).contains("window_ignored"),
        "rendered: {}",
        rendered_of(&msgs)
    );
}

/// The marker does not replace the reject: a HALF window is still a caller bug
/// and still leaves at request entry, before any leg runs. A COMPLETE window is
/// answered with the ordinary tier-0 fan -- no reject, no upgrade.
#[test]
fn the_half_window_reject_survives_the_marker() {
    let half = emit(request_doc(FROM, ""));
    assert_eq!(half.len(), 1);
    assert_eq!(half[0]["header"]["route"], "reject");
    assert_eq!(half[0]["header"]["phase"], "request");

    let complete = emit(request_doc(FROM, TO));
    // Since GH #295 the "ordinary tier-0 fan" is ONE message holding the three
    // fixed legs, not three messages holding one each.
    assert_eq!(phases(&complete), vec!["legs"]);
}
