//! P9 block A — the JSON stdio wire (`--stdio-format json`), end to end.
//!
//! Same fixture shape as `stdio_bridge_demo.rs` (a root hive with a conditional
//! ingress edge and a return edge, plus a synchronous `code` cell), driven
//! through a real `meclaw` process. No HTTP, no mock, no LLM.
//!
//! What these tests are for: the JSON wire is the transport a sub-colony parent
//! speaks. Everything the composition boundary needs — carrying a trace across
//! the process boundary, honouring a decremented TTL, getting the correlation
//! key back — has to be true HERE before a parent-side cell can rely on it.

use std::io::{BufRead as _, Write as _};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Echo cell: mirrors the incoming user turn back as an assistant turn and sets
/// `finish_reason`, which is what makes the root hive's ingress edge skip the
/// reply and hand it to the egress path.
const ECHO_SCRIPT: &str = r#"
import sys, json
body = json.load(sys.stdin)["body"]
turns = body.get("messages", [])
said = ""
for t in turns:
    if t.get("origin") == "user":
        said = t.get("text", "")
print(json.dumps({
    "header": {"finish_reason": "assistant"},
    "messages": [{"origin": "assistant", "type": "text", "text": "echo:" + said}]
}))
"#;

/// Writes the child-colony fixture: root hive `/` + a `code` cell at `/echo`.
fn write_fixture(root: &std::path::Path) {
    let echo_dir = root.join("main/echo");
    std::fs::create_dir_all(&echo_dir).expect("create fixture dirs");
    std::fs::write(
        root.join("main/config.json"),
        serde_json::json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": ".", "to": "./echo", "condition": "!has(hop.finish_reason)"},
                {"from": "./echo", "to": "."}
            ]}}
        })
        .to_string(),
    )
    .expect("write hive config");
    std::fs::write(
        echo_dir.join("config.json"),
        serde_json::json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": ECHO_SCRIPT,
                "external_timeout_ms": 5000
            },
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        })
        .to_string(),
    )
    .expect("write code cell config");
}

/// Drive one `meclaw` process: write `input` lines, read `expect_lines` stdout
/// lines, then close stdin and wait for exit.
///
/// Reads BEFORE closing stdin, for the reason documented in
/// `stdio_bridge_demo.rs`: the EOF-shutdown path does not wait for in-flight
/// cell work, so closing early would race the answer.
fn drive(args: &[&str], root: &std::path::Path, input: &str, expect_lines: usize) -> Vec<String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
    cmd.arg("--root").arg(root);
    for a in args {
        cmd.arg(a);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meclaw");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    stdin.write_all(input.as_bytes()).expect("write stdin");
    stdin.flush().expect("flush stdin");

    let reader = std::thread::spawn(move || {
        std::io::BufReader::new(stdout)
            .lines()
            .take(expect_lines)
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });

    // Failure-marker timeout, generous per convention (30s).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !reader.is_finished() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("meclaw produced no {expect_lines} stdout line(s) within 30s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let lines = reader.join().expect("reader thread");
    drop(stdin);
    let _ = child.wait();
    lines
}

/// Drive the JSON wire with one request and return `(ready_frame, reply_frame)`.
///
/// In the JSON format the handshake is part of the protocol, so the `ready`
/// frame is ALWAYS the first line and every caller has to account for it.
fn drive_json(
    root: &std::path::Path,
    request: &serde_json::Value,
) -> (serde_json::Value, serde_json::Value) {
    let out = drive(
        &["--stdio-format", "json"],
        root,
        &format!("{request}\n"),
        2,
    );
    assert_eq!(out.len(), 2, "expected ready + reply, got {out:?}");
    let parse = |i: usize| -> serde_json::Value {
        serde_json::from_str(&out[i])
            .unwrap_or_else(|e| panic!("line {i} is not JSON: {e}; got {out:?}"))
    };
    (parse(0), parse(1))
}

/// T-CHILD-1: a JSON request frame goes in, a JSON reply frame comes out, and
/// the whole body crosses.
#[test]
fn a_json_request_frame_is_answered_with_a_json_reply_frame() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_fixture(td.path());

    let req = serde_json::json!({
        "v": 1, "type": "message",
        "body": {"messages": [{"origin": "user", "type": "text", "text": "ping"}]}
    });
    let (_ready, frame) = drive_json(td.path(), &req);
    assert_eq!(frame["v"], 1, "every frame carries the protocol version");
    assert_eq!(frame["type"], "message");
    assert_eq!(
        frame["body"]["messages"][0]["text"], "echo:ping",
        "the child topology actually ran; got {frame}"
    );
}

/// T-TRACE-1 (child half): the parent's trace_id survives the whole way through
/// the child topology and comes back on the reply.
#[test]
fn the_trace_id_crosses_the_process_boundary_and_returns() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_fixture(td.path());

    let trace = uuid::Uuid::now_v7();
    let req = serde_json::json!({
        "v": 1, "type": "message", "trace_id": trace.to_string(),
        "body": {"messages": [{"origin": "user", "type": "text", "text": "hi"}]}
    });
    let (_ready, frame) = drive_json(td.path(), &req);
    assert_eq!(
        frame["trace_id"],
        serde_json::json!(trace.to_string()),
        "one conversation stays one trace across colonies; got {frame}"
    );
}

/// The correlation key comes back — without this a parent cannot match an answer
/// to its request, and the whole facade is impossible.
#[test]
fn the_turn_id_comes_back_so_an_answer_can_be_matched() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_fixture(td.path());

    let req = serde_json::json!({
        "v": 1, "type": "message", "context": {"turn_id": "correlation-key-1"},
        "body": {"messages": [{"origin": "user", "type": "text", "text": "hi"}]}
    });
    let (_ready, frame) = drive_json(td.path(), &req);
    assert_eq!(
        frame["context"]["turn_id"], "correlation-key-1",
        "the correlation key must survive the round trip; got {frame}"
    );
}

/// The boot handshake: `ready` is the FIRST line, before any answer, and it is
/// what tells a parent the child booted successfully.
#[test]
fn the_ready_frame_arrives_before_any_answer() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_fixture(td.path());

    let req = serde_json::json!({
        "v": 1, "type": "message",
        "body": {"messages": [{"origin": "user", "type": "text", "text": "hi"}]}
    });
    let (ready, reply) = drive_json(td.path(), &req);
    assert_eq!(ready["type"], "ready", "the handshake comes first");
    assert_eq!(ready["v"], 1);
    assert!(
        ready["version"].as_str().is_some_and(|v| !v.is_empty()),
        "the build reports itself: {ready}"
    );
    assert_eq!(reply["type"], "message", "and the answer follows it");
}

/// A malformed request is answered with a typed error frame instead of being
/// silently swallowed — a parent waiting on a correlation key must be released.
#[test]
fn a_malformed_request_frame_produces_an_error_frame() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_fixture(td.path());

    // A frame with no body: well-formed JSON, invalid as a request.
    let out = drive(
        &["--stdio-format", "json"],
        td.path(),
        "{\"v\":1,\"type\":\"message\"}\n",
        2,
    );
    let frame: serde_json::Value = serde_json::from_str(&out[1])
        .unwrap_or_else(|e| panic!("line 1 is JSON: {e}; got {out:?}"));
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["error_code"], "invalid_frame");
    assert!(
        frame["detail"].as_str().is_some_and(|d| d.contains("body")),
        "the error names the offending field: {frame}"
    );
}

/// T-CHILD-2: without the flag nothing changes. The regression lock for the
/// D0(a) obligation — every existing pipe and interactive session is untouched.
#[test]
fn without_the_flag_the_bridge_still_speaks_plain_text() {
    let td = tempfile::TempDir::new().expect("tempdir");
    write_fixture(td.path());

    let out = drive(&[], td.path(), "ping\n", 1);
    assert_eq!(
        out[0], "echo:ping",
        "the text format must stay byte-identical; got {out:?}"
    );
}
