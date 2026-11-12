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
//! - `lost` — with `--input-format stream-json`: reads the first turn, answers
//!   with exactly one `result/error_during_execution` frame carrying `errors`
//!   and no `init`, then exits 1 — the shape the real CLI gives for a
//!   `--resume` of an unknown session (measured on 2.1.322).
//!
//! The mode is the first known mode word anywhere on the command line, and its
//! number directly follows it: through the cell a mode only arrives via
//! `extra_args`, at the end of a long argv.
//!
//! Input dialect: with `--input-format stream-json` the fixture is silent until
//! the first input, like the real CLI, and every `{"type":"user"}` line on
//! stdin is one turn: `init` (same session every turn), the mode's stream, a
//! `result` whose `total_cost_usd` is cumulative for the process. A
//! `control_response` answers `ask`; EOF ends the process with exit 0.
//! Without the flag the fixture runs one turn on its own and exits, as before.
//!
//! Flags (any mode):
//! - `--session-id <id>` — use this session id instead of a generated one.
//! - `--pid-file <path>` / `--grandchild-pid-file <path>` — write pids, so a
//!   test can prove a process really is gone afterwards.
//! - `--grandchild` — spawn a long-running child of its own, the way a real
//!   harness spawns build and search tools.
//! - `--banner` — write a non-JSON line first, exercising the parent's
//!   tolerance for noise on stdout.
//! - `--resume <id>` — the session id to report when `--session-id` is absent.
//! - `--context-tokens <n>` — `message.usage.input_tokens` of every assistant
//!   frame (default 1).
//! - `--turn-cost <usd>` — what each turn adds to `total_cost_usd` (default 0).
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

    let (mode, number) = mode_of(&args);
    let session = flag_value(&args, "--session-id")
        .or_else(|| flag_value(&args, "--resume"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("fixture-session-{}", std::process::id()));
    let fx = Fixture {
        session,
        context_tokens: flag_value(&args, "--context-tokens")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
        turn_cost: flag_value(&args, "--turn-cost")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
    };

    let stream_input = args
        .windows(2)
        .any(|w| w[0] == "--input-format" && w[1] == "stream-json");
    if stream_input {
        run_turns(&fx, mode, number);
        return;
    }

    let mut cost = 0.0;
    match mode {
        "silent" => {}
        "slow" => {
            std::thread::sleep(std::time::Duration::from_millis(number.unwrap_or(5_000)));
            fx.turn(mode, number, &mut cost, None);
        }
        "ask" => run_ask(&fx),
        _ => fx.turn(mode, number, &mut cost, None),
    }
}

/// Every mode word the fixture knows.
const MODES: [&str; 8] = [
    "ok", "tooluse", "ask", "stall", "slow", "crash", "silent", "lost",
];

/// The first known mode word anywhere in argv, and the number right after it.
fn mode_of(args: &[String]) -> (&str, Option<u64>) {
    match args.iter().position(|a| MODES.contains(&a.as_str())) {
        Some(i) => (
            args[i].as_str(),
            args.get(i + 1).and_then(|v| v.parse().ok()),
        ),
        None => ("ok", None),
    }
}

/// Per-process knobs of the stream.
struct Fixture {
    session: String,
    context_tokens: u64,
    turn_cost: f64,
}

impl Fixture {
    /// One turn of the given mode. `cost` is the process-wide total;
    /// `stdin` is the input reader in the input dialect (`ask` reads its
    /// answer from it), `None` in the old one-shot form.
    fn turn(
        &self,
        mode: &str,
        number: Option<u64>,
        cost: &mut f64,
        stdin: Option<&mut dyn Iterator<Item = String>>,
    ) {
        match mode {
            "crash" => {
                emit_init(&self.session);
                std::process::exit(number.unwrap_or(1) as i32);
            }
            "stall" => {
                emit_init(&self.session);
                self.assistant_text("working on it");
                // No result, no exit: only an outside kill ends this.
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            }
            "tooluse" => {
                emit_init(&self.session);
                self.assistant_tool_call("toolu_fixture_1", "Bash", "ls -la");
                emit_tool_result(&self.session, "toolu_fixture_1", "total 0");
                self.result(cost, "listed the directory", 2);
            }
            "ask" => {
                emit_init(&self.session);
                emit_control_request();
                let mut behavior = None;
                if let Some(lines) = stdin {
                    for line in &mut *lines {
                        behavior = decision_of(&line);
                        if behavior.is_some() {
                            break;
                        }
                    }
                }
                match behavior {
                    Some(b) => self.result(cost, &format!("permission decision: {b}"), 1),
                    // stdin closed without an answer: end without a result.
                    None => std::process::exit(0),
                }
            }
            "slow" => {
                emit_init(&self.session);
                self.result(cost, "slow but finished", 1);
            }
            _ => {
                emit_init(&self.session);
                self.assistant_text("done");
                self.result(cost, "done", 1);
            }
        }
    }

    fn assistant_text(&self, text: &str) {
        emit(&serde_json::json!({
            "type": "assistant",
            "session_id": self.session,
            "parent_tool_use_id": serde_json::Value::Null,
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}],
                        "usage": self.usage()}
        }));
    }

    fn assistant_tool_call(&self, id: &str, name: &str, command: &str) {
        emit(&serde_json::json!({
            "type": "assistant",
            "session_id": self.session,
            "parent_tool_use_id": serde_json::Value::Null,
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": id, "name": name, "input": {"command": command}}
            ], "usage": self.usage()}
        }));
    }

    fn usage(&self) -> serde_json::Value {
        serde_json::json!({"input_tokens": self.context_tokens,
                           "cache_creation_input_tokens": 0,
                           "cache_read_input_tokens": 0, "output_tokens": 1})
    }

    fn result(&self, cost: &mut f64, text: &str, num_turns: u64) {
        *cost += self.turn_cost;
        emit(&serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "duration_ms": 42,
            "num_turns": num_turns,
            "result": text,
            "session_id": self.session,
            "total_cost_usd": *cost,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }));
    }
}

/// The input dialect: silent until the first input, one turn per `user` line,
/// exit 0 at EOF.
fn run_turns(fx: &Fixture, mode: &str, number: Option<u64>) {
    if mode == "silent" {
        return;
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    let mut cost = 0.0;
    let mut first = true;
    while let Some(line) = lines.next() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["type"] != "user" {
            continue;
        }
        if mode == "lost" {
            emit(&serde_json::json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "num_turns": 0,
                "session_id": fx.session,
                "total_cost_usd": 0.0,
                "errors": [format!("No conversation found with session ID: {}", fx.session)]
            }));
            std::process::exit(1);
        }
        if mode == "slow" && first {
            std::thread::sleep(std::time::Duration::from_millis(number.unwrap_or(5_000)));
        }
        first = false;
        fx.turn(mode, number, &mut cost, Some(&mut lines));
    }
}

/// The decision a `control_response` line carries, if it is one.
fn decision_of(line: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if v["type"] != "control_response" {
        return None;
    }
    Some(
        v["response"]["response"]["behavior"]
            .as_str()
            .or_else(|| v["response"]["behavior"].as_str())
            .unwrap_or("unknown")
            .to_string(),
    )
}

fn emit_control_request() {
    emit(&serde_json::json!({
        "type": "control_request",
        "request_id": "req-fixture-1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "rm -rf /tmp/nothing"}
        }
    }));
}

/// The permission round trip of the one-shot form: ask, then report what was
/// decided.
///
/// The control protocol is the real CLI's own shape — a `control_request` with
/// subtype `can_use_tool` on stdout, a `control_response` carrying
/// `{behavior: allow|deny}` back on stdin.
fn run_ask(fx: &Fixture) {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    fx.turn("ask", None, &mut 0.0, Some(&mut lines));
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

/// The value following `flag`, if the flag is present.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(String::as_str)
}
