//! Wave 13 -- event time reaches the drain over a UBF-valid body (GitHub #135).
//!
//! `memory-drain`'s `turns_of` reads `happened_at` off each turn, so that a
//! replay which KNOWS when something was said keeps the bi-temporal split
//! instead of having the writer stamp its own clock over the whole import. The
//! branch worked at script level and described a route that could not exist:
//! `TurnObject` is closed (`additionalProperties: false`), so a turn carrying
//! the field died as `invalid_ubf_body` before it reached any cell. Found while
//! building `examples/never-forgets`, which worked around it by speaking to the
//! episode port directly with the event time in the header.
//!
//! The header cannot be the answer in general: it carries ONE time per message,
//! and a batch of replayed turns carries a different one per turn. So the slot
//! is opened on the turn -- by name, not by opening the object:
//!
//! 1. a turn MAY carry `happened_at`, and the body validates;
//! 2. every OTHER extra field is still `invalid_ubf_body` -- the door did not
//!    open, one named slot did;
//! 3. a turn without the field is unchanged, which is what makes the extension
//!    additive rather than a wire break;
//! 4. and the field survives the trip: the drain's episode carries the caller's
//!    instant out of a body that is UBF-valid on the way in.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};
use meclaw_core::validate_ubf_body;

const DRAIN_CONFIG: &str = "../../templates/memory-drain/drain/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs at boot.
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

fn drain_script() -> String {
    let raw = std::fs::read_to_string(DRAIN_CONFIG).expect("memory-drain drain config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
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

fn emit(doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(
        &drain_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "drain exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn args(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("op text"))
        .expect("op args")
}

/// The body half of a message, which is what the UBF validator judges.
fn body(turns: Value) -> Value {
    json!({"messages": turns})
}

#[test]
fn a_turn_may_carry_its_event_time() {
    let b = body(json!([
        {"origin": "user", "type": "text", "text": "we moved in April",
         "happened_at": "2020-04-01T00:00:00.000000Z"}
    ]));
    validate_ubf_body(&b).expect("a turn may state when it happened");
}

#[test]
fn one_named_slot_is_not_an_open_door() {
    // The point of the closed turn object stands: structural extras belong in
    // the header. `happened_at` is an exception with a reason, not a precedent.
    let b = body(json!([
        {"origin": "assistant", "type": "tool_call", "id": "c1",
         "tool_name": "search"}
    ]));
    let err = validate_ubf_body(&b).expect_err("an unknown field is still a closed door");
    assert!(
        err.contains("tool_name") || err.contains("additional"),
        "the rejection names the offending field: {err}"
    );
}

#[test]
fn a_turn_without_the_field_is_unchanged() {
    // Additivity, stated as a test: every body that was valid before the slot
    // existed is valid after it.
    let b = body(json!([
        {"origin": "user", "type": "text", "text": "hello"},
        {"origin": "assistant", "type": "text", "text": "hi"}
    ]));
    validate_ubf_body(&b).expect("an old body stays valid");
}

#[test]
fn the_event_time_reaches_the_drains_episode_over_a_valid_body() {
    // Both halves in one run: the body passes the wire gate, and the branch the
    // issue called unreachable now fires on the far side of it.
    let turns = json!([
        {"origin": "user", "type": "text", "text": "we moved in April",
         "happened_at": "2020-04-01T00:00:00.000000Z"}
    ]);
    validate_ubf_body(&body(turns.clone())).expect("the import body is UBF-valid");

    // The provenance of the round travels with the batch (#244/#269): a replay
    // out of an archive knows who was present just as a live close does, and
    // the drain refuses a batch that does not say.
    let doc = json!({
        "header": {"hop": {"route": "in_batch", "session_id": "s1"},
                   "context": {"session_id": "s1",
                               "audience_set": "[\"member:alex\",\"agent:scribe\"]",
                               "channel": "tg:4711"}},
        "messages": turns
    });
    let parked: Value = meclaw_core::serde_json::from_str(
        args(&emit(doc)[0])["row"]["payload"]
            .as_str()
            .expect("payload"),
    )
    .expect("payload json");
    assert_eq!(
        parked[0]["happened_at"], "2020-04-01T00:00:00.000000Z",
        "the parked day keeps the caller's instant: {parked}"
    );

    let rows = json!([{"id": "2026-08-15-aa", "kind": "batch",
                       "payload": parked.to_string(), "drained_upto": 0}]);
    let out = emit(json!({
        "header": {"hop": {"route": "lstore", "operation": "select"},
                   "context": {"session_id": "s1", "drain_phase": "probe"}},
        "messages": [{"origin": "user", "type": "tool_result", "id": "d-probe",
                      "text": rows.to_string()}]
    }));
    let episode = out
        .iter()
        .find(|m| m["header"]["route"] == "episode")
        .unwrap_or_else(|| panic!("the drain fires an episode: {out:?}"));
    assert_eq!(
        episode["header"]["happened_at"], "2020-04-01T00:00:00.000000Z",
        "the episode travels under the event time, not the drain's clock"
    );
}
