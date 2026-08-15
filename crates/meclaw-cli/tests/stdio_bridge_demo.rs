//! Task 6 (new): stdio-bridge phase demo + EOF lifecycle tests.
//!
//! Demo (step 6.1): `echo "ping" | meclaw --root <fixture>` with a synchronous,
//! deterministic code cell, a real return edge and a CEL condition on the
//! ingress edge. No HTTP, no mock server.
//!
//! Fixture topology:
//!   <root>/main/config.json       — root hive `/`
//!                                   ingress edge: `from="." to="./echo" condition="!has(hop.finish_reason)"`
//!                                   return edge:  `from="./echo" to="."`
//!   <root>/main/echo/config.json  — `type: "code"` (inline Python, emits finish_reason="assistant")
//!
//! Flow (edge-driven, task-2 egress via enqueue_hive_transit):
//!   The bridge ingress sends the stdin line (no hop.finish_reason) → root hive
//!   "/" → the ingress edge matches (condition true) → routes to "/echo".
//!   code cell: the Python script sets header.finish_reason="assistant" →
//!   emits an assistant turn to msg.target="/".
//!   Colony outputs_rx: sender="/echo", apply_edges → the return edge matches →
//!   HiveTransit { hive_path="/", msg }.
//!   enqueue_hive_transit("/"): apply_edges from the hive "/" → the ingress edge
//!   checks condition !has(hop.finish_reason) → false
//!   (hop.finish_reason="assistant") → skip the edge → decisions empty →
//!   egress_tx → stdout (task-2 egress).
//!
//! EOF tests (step 6.2):
//! - Direct mode: stdin EOF → the process exits 0 without a signal.
//! - --daemon + immediate stdin EOF → the process keeps running (only a signal
//!   ends it) — proves that EOF is ignored in daemon mode.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Python script for the echo cell: ignores the input, always emits "pong".
///
/// Sets `header.finish_reason = "assistant"` in the content header. That value
/// lands in the `hop` compartment of the message headers. The ingress edge in
/// the root hive checks `!has(hop.finish_reason)` — so it only forwards user
/// messages (no finish_reason set) and lets reply messages through
/// (finish_reason == "assistant" → Edge-Bedingung false → decisions empty
/// → enqueue_hive_transit → egress).
const ECHO_SCRIPT: &str = r#"
import sys, json
# Sets finish_reason so the ingress edge condition `!has(hop.finish_reason)` skips
# the reply message, leaving enqueue_hive_transit with empty decisions -> egress.
print(json.dumps({"header": {"finish_reason": "assistant"}, "messages": [{"origin": "assistant", "type": "text", "text": "pong"}]}))
"#;

/// Writes the fixture topology for the demo test.
///
/// Root hive "/" with two edges:
/// - Ingress edge: from="." to="./echo" (no filter — all messages to /echo)
/// - Return-Edge: from="./echo" to="." (triggert HiveTransit auf "/")
///
/// code cell at "/echo": inline Python, always returns "pong".
fn write_echo_fixture(root: &std::path::Path) {
    let echo_dir = root.join("main/echo");
    std::fs::create_dir_all(&echo_dir).unwrap();

    // Root hive with a conditional ingress edge and a return edge.
    //
    // Ingress-Edge (bedingt): `from="." to="./echo" condition="!has(hop.finish_reason)"`
    //   → forwards only user messages to /echo (no hop.finish_reason set).
    //   → reply messages (hop.finish_reason=="assistant") are NOT forwarded.
    //
    // Return-Edge (unbedingt): `from="./echo" to="."`
    //   → In outputs_rx: sender="/echo", apply_edges findet Return-Edge → HiveTransit("/").
    //   → enqueue_hive_transit("/") checks apply_edges from the hive "/":
    //     the ingress edge has condition=!has(hop.finish_reason) → false for a
    //     reply. decisions empty → egress channel (task-2 egress,
    //     enqueue_hive_transit).
    std::fs::write(
        root.join("main/config.json"),
        serde_json::json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": ".", "to": "./echo", "condition": "!has(hop.finish_reason)"},
                {"from": "./echo", "to": "."}
            ]}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();

    // code cell: inline Python, always "pong", no HTTP.
    // Params format: `script_inline` (a flat key, not nested) — CodeParams::parse.
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
        .to_string()
        .as_bytes(),
    )
    .unwrap();
}

// ─── Step 6.1: Demo-Test — stdin → assistant-stdout ──────────────────────────

/// Starts meclaw with a synchronous code-cell fixture (no HTTP, no mock), pipes
/// "ping" into stdin and expects "pong" on stdout plus exit 0.
///
/// Positive receipt: stdout contains "pong" ←→ bridge + colony + code cell +
/// return edge + task-2 egress all run through correctly.
///
/// Egress path (edge-driven):
///   code-Cell emittiert an target="/" → Colony outputs_rx: sender="/echo",
///   apply_edges findet Return-Edge from="/echo" to="/" → HiveTransit("/")
///   → enqueue_hive_transit(egress_tx) → egress-Kanal → stdout.
///
/// Test strategy: stdout is read BEFORE the stdin EOF (blocking, max 10s). Only
/// once the first stdout line ("pong") has arrived is stdin closed → EOF →
/// shutdown. That avoids the race between the cell worker and the colony
/// shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_mode_stdin_line_produces_assistant_stdout() {
    // Fixture schreiben.
    let td = tempfile::TempDir::new().unwrap();
    write_echo_fixture(td.path());

    // Start the meclaw process with piped stdin + stdout.
    let root_path = td.path().to_path_buf();
    let (status, stdout_content) = tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .arg("--root")
            .arg(&root_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn meclaw");

        let mut child_stdin = child.stdin.take().expect("stdin");
        let child_stdout = child.stdout.take().expect("stdout");

        // Stdout reader: reads one line blocking (max 10s until "pong" arrives).
        // Strategy: first send "ping", then block until "pong" appears, THEN
        // close stdin → EOF → shutdown.
        // That way the cell output is guaranteed to precede the shutdown.
        child_stdin.write_all(b"ping\n").expect("write");
        // No stdout flush via flush() needed (unbuffered write_all).

        let stdout_line = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stdout_line_clone = stdout_line.clone();
        let reader_thread = std::thread::spawn(move || {
            use std::io::BufRead as _;
            let reader = std::io::BufReader::new(child_stdout);
            // Read ONE line (blocks until "pong\n" arrives).
            if let Some(Ok(l)) = reader.lines().next() {
                *stdout_line_clone.lock().unwrap() = l;
            }
        });

        // Wait at most 10s for the first stdout line.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if reader_thread.is_finished() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("meclaw did not produce stdout within 10s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = reader_thread.join();

        // Only now close stdin → EOF → shutdown.
        drop(child_stdin);

        // Wait for the process to end (max 5s: shutdown after EOF is fast).
        let exit_deadline = std::time::Instant::now() + Duration::from_secs(5);
        let status = loop {
            match child.try_wait().expect("try_wait") {
                Some(s) => break s,
                None => {
                    if std::time::Instant::now() >= exit_deadline {
                        let _ = child.kill();
                        panic!("meclaw did not exit within 5s after stdin-EOF");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };

        let content = stdout_line.lock().unwrap().clone();
        (status, content)
    })
    .await
    .expect("spawn_blocking");

    // Assertions.
    assert_eq!(
        status.code(),
        Some(0),
        "Direct-Mode must exit 0 after stdin-EOF; stdout: {stdout_content:?}"
    );
    assert!(
        stdout_content.contains("pong"),
        "stdout must contain 'pong'; got: {stdout_content:?}"
    );
}

// ─── Step 6.2: EOF-Lifecycle-Tests ───────────────────────────────────────────

/// Direct mode: stdin EOF → the process exits 0 without a signal.
/// Uses a simple root-hive topology (no cell needed).
#[test]
fn direct_mode_eof_exits_zero() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();

    // `output()` closes stdin immediately (no input) → EOF → graceful shutdown.
    // No quiesce wait: EOF triggers the shutdown directly.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .output()
        .expect("spawn meclaw");

    assert_eq!(
        output.status.code(),
        Some(0),
        "Direct-Mode stdin-EOF must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Counter-check: `--daemon` + immediate stdin EOF → the process keeps running
/// (EOF is ignored). Only a signal ends it.
///
/// Semantic timing discriminator window: still alive 5s after start proves that
/// daemon mode did NOT wire the EOF arm.
/// Cleanup via SIGKILL (child.kill()).
#[test]
fn daemon_mode_eof_does_not_exit() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        br#"{"cell":{"type":"hive"}}"#,
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--daemon")
        .stdin(Stdio::null()) // stdin sofort EOF
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn meclaw --daemon");

    // Semantic timing: 5s. The daemon must NOT have been ended by EOF.
    std::thread::sleep(Duration::from_secs(5));

    let still_running = child.try_wait().expect("try_wait").is_none();
    assert!(
        still_running,
        "--daemon must ignore stdin-EOF and keep running (process exited unexpectedly)"
    );

    child.kill().expect("kill daemon process");
    let _ = child.wait();
}
