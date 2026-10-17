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
use meclaw_testing::{deadline, wait};
use std::io::Write;
use std::process::{Command, Stdio};

/// How long the cell stays inside `handle()` — six times the default watchdog
/// window (5 × 100 ms), so a drain that waited inside a work item would have
/// crossed the deadline several times over before the colony came back.
const SLEEP_MS: u64 = 3_000;

/// `colony.json` for both tests: the drain budget, and NOTHING else. No
/// `watchdog_threshold`, no `watchdog_period_ms` — the pre-#84 default deadline
/// applies, which is the whole point.
/// The drain budget is a product deadline on the path this test awaits, so it
/// comes from the one time factor (`deadline::product_wait_ms`, OR-T12-13,
/// tv-br M10m) instead of a literal 10 s a loaded host could outrun.
fn colony_json() -> String {
    format!(
        r#"{{"shutdown_drain_timeout_ms": {}}}"#,
        meclaw_testing::deadline::product_wait_ms()
    )
}

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
///
/// The script also marks the two edges of its answer in the root: it creates
/// [`IN_FLIGHT_MARKER`] before it sleeps and [`ANSWERED_MARKER`] after it, so
/// a test can tell "the answer is in flight" and "the answer is done" from
/// events instead of from a clock (GH #1216, tv-br M10k).
fn write_slow_echo_fixture(root: &std::path::Path, sleep_ms: u64) {
    let in_flight = serde_json::to_string(&root.join(IN_FLIGHT_MARKER).to_string_lossy())
        .expect("a path string serialises");
    let answered = serde_json::to_string(&root.join(ANSWERED_MARKER).to_string_lossy())
        .expect("a path string serialises");
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
open({}, "w").close()
time.sleep({})
open({}, "w").close()
print(json.dumps({{"header": {{"finish_reason": "assistant"}},
                   "messages": [{{"origin": "assistant", "type": "text", "text": text}}]}}))
"#,
        in_flight,
        sleep_ms as f64 / 1000.0,
        answered
    );

    std::fs::write(
        echo_dir.join("config.json"),
        serde_json::json!({
            "cell": {"type": "code", "message_timeout": meclaw_testing::deadline::product_backstop_ms()},
            "params": {
                "runner": "python3",
                "script_inline": script,
                "external_timeout_ms": meclaw_testing::deadline::product_wait_ms(),
                // GH #1235 (T6-16): without `sandbox` the cell boots under the
                // default-deny profile, and the script leaves its marker in the
                // tree's root -- so it declares that one writable directory.
                "sandbox": {"trust": "restricted", "network": "deny",
                            "filesystem": {"runtime": true,
                                           "write": [root.to_string_lossy()]}}
            },
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
}

/// The file the fixture's script creates before it sleeps.
const IN_FLIGHT_MARKER: &str = "in_flight";

/// The file the fixture's script creates after its sleep, before it answers.
const ANSWERED_MARKER: &str = "answered";

/// The in-process CLI, HTTP door open on an ephemeral port — the only way to
/// hand a message to a colony this test drives inside its own process (the
/// stdin bridge would read the test harness's stdin).
fn cli_for(root: &std::path::Path) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        log_stderr: meclaw_cli::LogSink::Auto,
        log_file: meclaw_cli::LogSink::Auto,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("bind addr")),
        daemon: false,
        validate: false,
        validate_strict: false,
        env_report: false,
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
        command: None,
    }
}

/// A three-second drain under the DEFAULT watchdog deadline exits 0 — no trip,
/// no `watchdog` in the error, because there is no error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_three_second_drain_under_the_default_watchdog_exits_clean() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), SLEEP_MS);
    std::fs::write(td.path().join("colony.json"), colony_json()).unwrap();

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run = tokio::spawn(run_with_hooks(
        cli_for(td.path()),
        Some(addr_tx),
        Some(shutdown_rx),
    ));

    let addr = deadline::boot_hang("the colony binds HTTP", addr_rx)
        .await
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

    // The cell is inside its sleep (an event, not a fixed wait); the shutdown
    // therefore opens a drain of up to SLEEP_MS, six times the untuned window.
    let in_flight = td.path().join(IN_FLIGHT_MARKER);
    wait::boot_until_with(
        "the cell starts its answer (in-flight marker)",
        || std::future::ready(in_flight.exists()),
        || run.is_finished(),
    )
    .await;
    shutdown_tx.send(()).expect("the run must still be alive");

    let res = deadline::hang("the shutdown ends the run after its drain", run)
        .await
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
    // The drain really happened: the run ended only after the cell had
    // finished its answer. A shutdown that had cut the cell off would have
    // returned while the script still slept and proved nothing about a drain.
    assert!(
        td.path().join(ANSWERED_MARKER).exists(),
        "the shutdown must have WAITED for the in-flight work; the run ended \
         before the cell had finished its answer"
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
///
/// GH #1216 (tv-br M10z): the stream also covers the TEARDOWN that follows the
/// drain — writer close, cell joins, the process ending. Under CPU quota that
/// outlasted the 5 × 100 ms window, and a supervisor still armed read the
/// colony task's closed channel as `colony_task_gone`. The loop now stands
/// down by event before the teardown; the exact window arithmetic is pinned in
/// virtual time in `watchdog::tests` and the ordering in
/// `colony::tests::the_loop_stands_down_after_its_drain_and_never_inside_it`.
/// Rule 11, topology class: a child process, so events and invariants only —
/// the shutdown is driven by the in-flight event (stdin closes once the cell is
/// inside its answer, never on a race with the bridge), the wait is a hang
/// mark, every verdict is read from the streams. `SLEEP_MS` is the workload
/// (a drain longer than the window), never read back as a verdict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_three_second_drain_writes_no_watchdog_trip_to_stderr() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), SLEEP_MS);
    std::fs::write(td.path().join("colony.json"), colony_json()).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("meclaw must start");

    let mut stdin = child.stdin.take().expect("stdin piped");
    writeln!(stdin, "in-flight").unwrap();
    // EOF only once the cell is inside its answer (an event): the same
    // graceful shutdown a SIGTERM drives, with work demonstrably in flight.
    let in_flight = td.path().join(IN_FLIGHT_MARKER);
    wait::boot_until_with(
        "the cell starts its answer (in-flight marker)",
        || std::future::ready(in_flight.exists()),
        || "the process runs",
    )
    .await;
    drop(stdin);

    // Under the boot mark (TV-Z3): the run is a whole boot, the drain and the
    // teardown of a fresh binary.
    let out = deadline::boot_hang(
        "the drained process ends",
        tokio::task::spawn_blocking(move || child.wait_with_output()),
    )
    .await
    .expect("the waiting task must not panic")
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
