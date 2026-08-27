//! GH #47: a drain must never look like a wedged loop.
//!
//! The default watchdog window is 5 × 100 ms of silence. A drain lasts seconds.
//! If the drain were a wait INSIDE one work item, `starved()` would report
//! `slow_work_item` and, past the 10× budget, the fatal `stuck_work_item` — the
//! process would die non-zero in the middle of saving the work it was trying to
//! save. It is a loop PHASE instead, so the loop keeps parking and beating
//! throughout, and this file is the lock on that.
//!
//! **Two receipts, because the first one alone would be hollow.** The exit-code
//! path (`Err("watchdog trip: …")`) is armed only while the CLI's shutdown
//! future is alive: that future OWNS the trip channel and the fatal receiver,
//! and it resolves — and is dropped — the instant the shutdown signal arrives,
//! which is BEFORE the drain phase begins. A trip fired during the drain window
//! therefore cannot colour the exit code, so `Ok(())` is a necessary but not a
//! sufficient receipt for the drain. What a trip DOES do for as long as the
//! process lives is get written down: the reporter task in `run_with_hooks_tuned`
//! prints `meclaw: watchdog trip …` to stderr under every policy, fatal or not.
//! So the second test runs the real binary across the same three-second drain
//! and reads that stream. Both tests use the DEFAULT deadline (no `watchdog`
//! keys in `colony.json`) — the drain must survive the window nobody tuned for
//! it. `watchdog.rs` is not touched here; this file only measures.

use meclaw_cli::{Cli, run_with_hooks};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long the cell stays inside `handle()` — six times the default watchdog
/// window (5 × 100 ms), so a drain that waited inside a work item would have
/// crossed the deadline several times over before the colony came back.
const SLEEP_MS: u64 = 3_000;

/// `colony.json` for both tests: the drain budget, and NOTHING else. No
/// `watchdog_threshold`, no `watchdog_period_ms` — the pre-#84 default deadline
/// applies, which is the whole point.
const COLONY_JSON: &[u8] = br#"{"shutdown_drain_timeout_ms": 10000}"#;

/// Root hive "/" with a conditional ingress edge and a return edge, plus a
/// `code` cell at "/echo" whose script sleeps before answering.
///
/// A MIRROR of `write_slow_echo_fixture` in
/// `gh47_a_batch_pipe_keeps_its_answers.rs`, deliberately copied rather than
/// included: each integration test file is its own crate, so pulling that file
/// in with `#[path = …] mod …` would compile its `#[test]` functions into this
/// binary as well and run the 20-line batch-pipe case a second time here. The
/// fixture is data, not logic — a copy costs nothing and keeps the two files
/// independently readable.
///
/// ONE deliberate divergence from that copy: the script reads its turns from
/// `payload["body"]`, not from `payload` itself. A `code` cell has been handed
/// three objects on stdin since 0.9.0 — `envelope`, `body`, `params`
/// (`docs/cell-types.md` § code, "Die drei Objekte auf stdin") — so
/// `payload["messages"]` is always absent and the sibling's script echoes an
/// empty string for every line. The receipt below needs the echo to carry the
/// text, so this copy addresses the documented level.
fn write_slow_echo_fixture(root: &std::path::Path, sleep_ms: u64) {
    let echo_dir = root.join("main/echo");
    std::fs::create_dir_all(&echo_dir).unwrap();
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

    let script = format!(
        r#"
import sys, json, time
payload = json.loads(sys.stdin.read())
turns = payload["body"].get("messages", [])
text = turns[-1]["text"] if turns else ""
time.sleep({})
print(json.dumps({{"header": {{"finish_reason": "assistant"}},
                   "messages": [{{"origin": "assistant", "type": "text", "text": text}}]}}))
"#,
        sleep_ms as f64 / 1000.0
    );

    std::fs::write(
        echo_dir.join("config.json"),
        serde_json::json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": script,
                "external_timeout_ms": 30000
            },
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
}

/// The in-process CLI, HTTP door open on an ephemeral port — the only way to
/// hand a message to a colony this test drives inside its own process (the
/// stdin bridge would read the test harness's stdin).
fn cli_for(root: &std::path::Path) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("bind addr")),
        daemon: false,
        validate: false,
        validate_strict: false,
        apply: None,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        vault: None,
        vault_add: None,
        vault_status: false,
        vault_revoke: None,
        vault_key_source: "auto".to_string(),
        vault_key_file: None,
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

/// A three-second drain under the DEFAULT watchdog deadline exits 0 — no trip,
/// no `watchdog` in the error, because there is no error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_three_second_drain_under_the_default_watchdog_exits_clean() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), SLEEP_MS);
    std::fs::write(td.path().join("colony.json"), COLONY_JSON).unwrap();

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run = tokio::spawn(run_with_hooks(
        cli_for(td.path()),
        Some(addr_tx),
        Some(shutdown_rx),
    ));

    // Generous failure marker (30 s convention) around every wait.
    let addr = tokio::time::timeout(Duration::from_secs(30), addr_rx)
        .await
        .expect("the colony must bind HTTP within the failure marker")
        .expect("addr hook");

    // One message into the sleeping cell. The 202 is the positive receipt that
    // the work really is in flight — without it the drain below would have
    // nothing to wait for and this test would measure an empty shutdown.
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/messages"))
        .json(&serde_json::json!({
            "target": "/echo",
            "body": {"messages": [{"origin": "user", "type": "text", "text": "in-flight"}]}
        }))
        .send()
        .await
        .expect("POST /messages");
    assert_eq!(
        resp.status().as_u16(),
        202,
        "the message must be accepted, body: {:?}",
        resp.text().await
    );

    // The cell is inside its sleep by now; the shutdown therefore opens a drain
    // of roughly SLEEP_MS, six times the untuned watchdog window.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    shutdown_tx.send(()).expect("the run must still be alive");

    let res = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("the shutdown must end the run, not hang")
        .expect("the run task must not panic");

    match res {
        Ok(()) => {}
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                !msg.contains("watchdog"),
                "a drain is not a wedge — the drain tripped the untuned default \
                 deadline: {msg}"
            );
            panic!("the run must exit cleanly after a drain, was: {msg}");
        }
    }
    // The drain really happened: the shutdown took about as long as the work
    // still in flight. A shutdown that had cut the cell off would have returned
    // in milliseconds and proved nothing about a long drain.
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(SLEEP_MS / 2),
        "the shutdown must have WAITED for the in-flight work; it took {elapsed:?}"
    );
}

/// The half an exit code cannot see: across the same three-second drain the
/// real binary must not write a single watchdog trip to stderr.
///
/// A trip is reported unconditionally — fatal, log-only or uncorroborated, the
/// reporter task prints `meclaw: watchdog trip …` and logs `watchdog trip` for
/// as long as the process lives. So this stream stays a live witness during the
/// drain, long after the exit-code path has been dropped. Positive receipts on
/// both sides: the answer that was in flight comes back out of stdout, and the
/// process exits 0.
#[test]
fn a_three_second_drain_writes_no_watchdog_trip_to_stderr() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), SLEEP_MS);
    std::fs::write(td.path().join("colony.json"), COLONY_JSON).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("meclaw must start");

    {
        let mut stdin = child.stdin.take().expect("stdin piped");
        writeln!(stdin, "in-flight").unwrap();
        // EOF while the answer is still three seconds away: the same graceful
        // shutdown a SIGTERM drives, with work in flight.
    }

    // Generous failure marker (30 s convention); the run should take ~4 s.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("the process must end, not hang")
        .expect("the process must be waitable");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("watchdog trip"),
        "a drain is not a wedge — the untuned default deadline tripped during \
         the drain; stderr was:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l == "in-flight"),
        "the answer that was in flight must survive the drain; stdout was:\n{stdout}\n\
         stderr:\n{stderr}"
    );
    assert!(
        out.status.success(),
        "a drained shutdown exits 0, was: {:?}\nstderr:\n{stderr}",
        out.status
    );
}
