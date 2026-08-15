//! Test fixture, not a product surface.
//!
//! A tiny child process that speaks newline-delimited JSON ("line-JSON") over
//! stdin/stdout. It exists so the stdio-child core can be tested against a
//! REAL process instead of an in-process fake: spawning, framing, correlation,
//! EOF, non-zero exit, kill-after-timeout and orphan reaping are all
//! properties of an actual operating-system process.
//!
//! Usage: `line_json_test_server <mode> [args] [--pid-file <path>]`
//!
//! Modes:
//! - `echo` — read one JSON object per line, write it back verbatim.
//! - `hang` — never read, never answer, never exit (drives A-timeout paths).
//! - `exit <code>` — exit immediately with that status code.
//! - `die-on-request <code>` — read one line, then exit with that code without
//!   answering (drives the "child dies while a request is in flight" path).
//! - `mcp` — a minimal MCP server over JSON-RPC: `initialize`, `tools/list`
//!   (one tool, `echo`) and `tools/call`. With `--die-after <n>` it exits with
//!   code 3 instead of answering the n-th `tools/call` (0-based).
//!
//! Flags (any mode):
//! - `--pid-file <path>` — write this process's pid before doing anything
//!   else, so a test can prove the process is really gone afterwards.
//! - `--banner` — emit a non-JSON line and a blank line first, exercising the
//!   parent's tolerance for noise on stdout.
//!
//! Deliberately dependency-free and synchronous: the fixture must be trivially
//! obvious, because a subtle bug here would look like a bug in the code under
//! test.

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    write_pid_file(&args);
    if args.iter().any(|a| a == "--banner") {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "line_json_test_server ready");
        let _ = writeln!(out);
        let _ = out.flush();
    }
    let mode = args.first().map(String::as_str).unwrap_or("echo");
    match mode {
        "echo" => run_echo(),
        "hang" => run_hang(),
        "mcp" => run_mcp(&args),
        "exit" => std::process::exit(positional_code(&args)),
        "die-on-request" => {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            std::process::exit(positional_code(&args));
        }
        other => {
            eprintln!("line_json_test_server: unknown mode {other:?}");
            std::process::exit(2);
        }
    }
}

/// Second positional argument as an exit code; `1` if absent or unparsable.
fn positional_code(args: &[String]) -> i32 {
    args.get(1).and_then(|c| c.parse::<i32>().ok()).unwrap_or(1)
}

/// Write the pid to the path following `--pid-file`, if that flag is present.
fn write_pid_file(args: &[String]) {
    let Some(path) = flag_value(args, "--pid-file") else {
        return;
    };
    let _ = std::fs::write(path, std::process::id().to_string());
}

/// Echo every non-empty input line back on stdout, flushing per line so the
/// parent never waits on a buffer. Ends on stdin EOF.
fn run_echo() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if writeln!(out, "{line}").is_err() || out.flush().is_err() {
            break;
        }
    }
}

/// A minimal MCP provider: enough of the protocol for the cell's handshake and
/// one tool call, and nothing more.
fn run_mcp(args: &[String]) {
    let die_after = flag_value(args, "--die-after").and_then(|v| v.parse::<usize>().ok());
    let mut calls_seen = 0usize;
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let reply = match method {
            "initialize" => ok(&id, serde_json::json!({"protocolVersion": "2024-11-05"})),
            "tools/list" => ok(
                &id,
                serde_json::json!({"tools": [
                    {"name": "echo", "inputSchema": {"type": "object"}}
                ]}),
            ),
            "tools/call" => {
                let seen = calls_seen;
                calls_seen += 1;
                if die_after == Some(seen) {
                    std::process::exit(3);
                }
                let params = req.get("params").cloned().unwrap_or_default();
                match params.get("name").and_then(|n| n.as_str()) {
                    Some("echo") => {
                        let text = params
                            .get("arguments")
                            .and_then(|a| a.get("text"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        ok(
                            &id,
                            serde_json::json!({"content": [{"type": "text", "text": text}]}),
                        )
                    }
                    _ => rpc_error(&id, -32601, "unknown tool"),
                }
            }
            _ => rpc_error(&id, -32601, "method not found"),
        };
        if writeln!(out, "{reply}").is_err() || out.flush().is_err() {
            break;
        }
    }
}

/// A JSON-RPC success envelope.
fn ok(id: &serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// A JSON-RPC error envelope.
fn rpc_error(id: &serde_json::Value, code: i64, message: &str) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        .to_string()
}

/// The value following `flag` in the argument list, if present.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(String::as_str)
}

/// Stay alive without ever answering. Used to drive the parent's A-timeout and
/// its kill-after-grace path; only a signal ends this process.
fn run_hang() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
