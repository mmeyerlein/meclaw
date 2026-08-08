//! Test fixture, not a product surface.
//!
//! A stand-in for an agent harness in print mode: it speaks the same
//! newline-delimited JSON event stream (`stream-json`) that Claude Code emits,
//! so the `harness` cell type can be tested end to end without spending a
//! single token. Every deterministic test in this package runs against this
//! binary; exactly one paid smoke run checks the real thing against it.
//!
//! Usage: `stream_json_harness_fixture <mode> [flags]`
//!
//! Modes:
//! - `ok` — init, one assistant turn, a successful result. The happy path.
//! - `tooluse` — init, an assistant turn containing a tool call, the matching
//!   tool result, then a successful result.
//! - `ask` — init, then a `can_use_tool` control request; waits for the
//!   control response on stdin and reports the decision in its result.
//! - `stall` — init and one assistant turn, then nothing: never a result,
//!   never an exit. The subject of the cancel path.
//! - `slow <ms>` — waits before the init event, to drive the startup timeout.
//! - `crash <code>` — init, then exits with that code WITHOUT a result event.
//! - `silent` — exits immediately without writing anything.
//!
//! Flags (any mode):
//! - `--session-id <id>` — use this session id instead of a generated one.
//! - `--pid-file <path>` / `--grandchild-pid-file <path>` — write pids, so a
//!   test can prove a process really is gone afterwards.
//! - `--grandchild` — spawn a long-running child of its own, the way a real
//!   harness spawns build and search tools.
//! - `--banner` — write a non-JSON line first, exercising the parent's
//!   tolerance for noise on stdout.
//!
//! Deliberately synchronous and dependency-free apart from `serde_json`: a
//! subtle bug in here would look like a bug in the code under test.

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(path) = flag_value(&args, "--pid-file") {
        let _ = std::fs::write(path, std::process::id().to_string());
    }
    if args.iter().any(|a| a == "--grandchild") {
        spawn_grandchild(&args);
    }
    if args.iter().any(|a| a == "--banner") {
        emit_raw("stream_json_harness_fixture ready");
    }

    let mode = args.first().map(String::as_str).unwrap_or("ok");
    let session = flag_value(&args, "--session-id")
        .map(str::to_string)
        .unwrap_or_else(|| format!("fixture-session-{}", std::process::id()));

    match mode {
        "silent" => {}
        "slow" => {
            let ms = positional_number(&args).unwrap_or(5_000);
            std::thread::sleep(std::time::Duration::from_millis(ms));
            emit_init(&session);
            emit_result(&session, "slow but finished", 1);
        }
        "crash" => {
            emit_init(&session);
            std::process::exit(positional_number(&args).unwrap_or(1) as i32);
        }
        "stall" => {
            emit_init(&session);
            emit_assistant_text(&session, "working on it");
            // No result, no exit: only an outside kill ends this.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        "tooluse" => {
            emit_init(&session);
            emit_assistant_tool_call(&session, "toolu_fixture_1", "Bash", "ls -la");
            emit_tool_result(&session, "toolu_fixture_1", "total 0");
            emit_result(&session, "listed the directory", 2);
        }
        "ask" => run_ask(&session),
        _ => {
            emit_init(&session);
            emit_assistant_text(&session, "done");
            emit_result(&session, "done", 1);
        }
    }
}

/// The permission round trip: ask, then report what was decided.
///
/// The control protocol is the real CLI's own shape — a `control_request` with
/// subtype `can_use_tool` on stdout, a `control_response` carrying
/// `{behavior: allow|deny}` back on stdin.
fn run_ask(session: &str) {
    emit_init(session);
    let request_id = "req-fixture-1";
    emit(&serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "rm -rf /tmp/nothing"}
        }
    }));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["type"] != "control_response" {
            continue;
        }
        let behavior = v["response"]["response"]["behavior"]
            .as_str()
            .or_else(|| v["response"]["behavior"].as_str())
            .unwrap_or("unknown");
        emit_result(session, &format!("permission decision: {behavior}"), 1);
        return;
    }
    // stdin closed without an answer: end without a result, like a harness that
    // was shut down while waiting.
}

/// Start a long-lived process of our own, with its own descriptors so a
/// surviving orphan cannot wedge the parent's pipes.
fn spawn_grandchild(args: &[String]) {
    let child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("sleep 300 </dev/null >/dev/null 2>/dev/null")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let (Ok(child), Some(path)) = (child, flag_value(args, "--grandchild-pid-file")) {
        let _ = std::fs::write(path, child.id().to_string());
    }
}

fn emit_init(session: &str) {
    emit(&serde_json::json!({
        "type": "system",
        "subtype": "init",
        "session_id": session,
        "model": "fixture-model",
        "cwd": std::env::current_dir().unwrap_or_default().display().to_string(),
        "tools": ["Bash", "Read", "Write"],
        "mcp_servers": []
    }));
}

fn emit_assistant_text(session: &str, text: &str) {
    emit(&serde_json::json!({
        "type": "assistant",
        "session_id": session,
        "parent_tool_use_id": serde_json::Value::Null,
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}
    }));
}

fn emit_assistant_tool_call(session: &str, id: &str, name: &str, command: &str) {
    emit(&serde_json::json!({
        "type": "assistant",
        "session_id": session,
        "parent_tool_use_id": serde_json::Value::Null,
        "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": name, "input": {"command": command}}
        ]}
    }));
}

fn emit_tool_result(session: &str, id: &str, content: &str) {
    emit(&serde_json::json!({
        "type": "user",
        "session_id": session,
        "parent_tool_use_id": serde_json::Value::Null,
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": content}
        ]}
    }));
}

fn emit_result(session: &str, text: &str, num_turns: u64) {
    emit(&serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "duration_ms": 42,
        "num_turns": num_turns,
        "result": text,
        "session_id": session,
        "total_cost_usd": 0.0,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }));
}

/// One frame: compact JSON, newline, flush. Flushing per line is what keeps
/// the parent from waiting on a buffer.
fn emit(value: &serde_json::Value) {
    emit_raw(&value.to_string());
}

fn emit_raw(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The second positional argument as a number, if it parses.
fn positional_number(args: &[String]) -> Option<u64> {
    args.get(1).and_then(|v| v.parse().ok())
}

/// The value following `flag`, if the flag is present.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(String::as_str)
}
