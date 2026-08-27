//! GH #47 + issue #6: a watchdog trip is not a graceful shutdown in disguise.
//!
//! A fatal trip means the colony loop is not coming back. Draining through it
//! would mean waiting on the thing that is already broken, and the process would
//! sit out its whole budget before dying — turning a fast, loud failure into a
//! slow one. The trip therefore takes the `ShutdownNow` door.
//!
//! **Why both tests run the real binary instead of `run_with_hooks_tuned`.**
//! The plan sketched this file as two in-process runs over an EMPTY hive. That
//! version was measured against HEAD before the CLI change and passed in 0.23 s:
//! with no cell and nothing in flight, the drain reaches quiescence on its first
//! look, so "the trip ended fast" holds whether the trip drained or not. The
//! assertion was hollow — it discriminated nothing. A drain only costs time when
//! there is work to wait for, so both tests here put a real, slow `code` cell in
//! flight and then shut the process down: the trip must walk past that work, the
//! signal must wait for it. Same fixture, same 60 s budget, opposite verdicts.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long the cell stays inside `handle()` in the trip case.
///
/// Four times the timing discriminator below, so a trip that drained would be
/// caught even if the drain were cut short by some other budget on the way.
const TRIP_SLEEP_MS: u64 = 20_000;

/// The drain budget both tests hand the colony: far larger than either sleep, so
/// nothing here ends because the deadline arrived.
const DRAIN_BUDGET_MS: u64 = 60_000;

/// Root hive "/" with a conditional ingress edge and a return edge, plus a
/// `code` cell at "/echo" whose script sleeps before answering.
///
/// The same fixture shape as `gh47_a_drain_is_not_a_wedge.rs`, and it reads its
/// turns from `payload["body"]` for the same reason: a `code` cell is handed
/// three objects on stdin since 0.9.0 — `envelope`, `body`, `params`
/// (`docs/cell-types.md` § code, "Die drei Objekte auf stdin") — so
/// `payload["messages"]` is always absent and a script reading the top level
/// echoes an empty string.
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
                "external_timeout_ms": 120000
            },
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
}

/// Redirect a child's stdout/stderr into files rather than pipes.
///
/// A `code` cell forks a `python3` that inherits whatever the process was given;
/// a pipe would therefore not reach EOF until that grandchild is gone, and the
/// trip case deliberately leaves one behind. Files have no such coupling, and
/// they are readable after `wait()` either way.
fn capture_files(dir: &std::path::Path) -> (std::fs::File, std::fs::File) {
    (
        std::fs::File::create(dir.join("stdout.txt")).unwrap(),
        std::fs::File::create(dir.join("stderr.txt")).unwrap(),
    )
}

/// A trip with a LARGE drain budget still ends fast, and still ends non-zero.
///
/// The two-sided assertion is the discriminator: a non-zero exit alone would
/// also hold if the trip had drained first (it would simply arrive later), and
/// "fast" alone would hold if there were no trip at all. Both together only hold
/// for a trip that walked past the drain.
///
/// The timing bound is a semantic discriminator, so it is tight and argued: the
/// cell sits inside `handle()` for 20 s and the drain has 60 s to wait for it,
/// so a draining trip cannot end before the budgets the CLI puts in its way
/// (pre-#47: 10 s of ack wait plus 5 s of join wait). A trip that skips the
/// drain ends in boot time plus teardown — well under a second on an idle box,
/// plus the deliberate 800 ms below. Five seconds sits between those, and the
/// measured values were 0.9 s with the change and 16 s without it.
///
/// **Why stdin is closed 800 ms in, and why that is not the shutdown.** The
/// direct-mode stdin reader sits in a blocking read (`tokio::io::stdin`), which
/// `abort()` cannot cancel and which the runtime waits for on its way out; a
/// process whose stdin stays open therefore never leaves, whatever ended its
/// colony. That is a property of the bridge and predates this lane — it was
/// measured with `shutdown_drain_timeout_ms: 0`, byte-exact pre-#47 behaviour,
/// and the process outlived its SIGTERM until the writing end was closed. The
/// close here is that release and nothing else: the trip fires ~60 ms after
/// boot, so the shutdown door has been chosen long before, and the exit code
/// plus the stderr line below say which one it was.
#[test]
fn a_watchdog_trip_skips_the_drain_and_still_exits_non_zero() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), TRIP_SLEEP_MS);
    // 3 × 20 ms of silence against the colony's own 100 ms heartbeat: the same
    // real trip `watchdog_trip_exit_code.rs` produces, on the production path.
    // Plus a drain budget the trip must NOT spend.
    std::fs::write(
        td.path().join("colony.json"),
        format!(
            r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS},
                 "watchdog_threshold": 3, "watchdog_period_ms": 20}}"#
        ),
    )
    .unwrap();

    let (out, err) = capture_files(td.path());
    let t0 = std::time::Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .expect("meclaw must start");

    let mut stdin = child.stdin.take().expect("stdin piped");
    writeln!(stdin, "line-0").unwrap();
    stdin.flush().unwrap();
    // The trip has fired and been acted on by now; see the doc comment.
    std::thread::sleep(Duration::from_millis(800));
    drop(stdin);

    let status = child
        .wait()
        .expect("the trip must end the process, not hang");
    let took = t0.elapsed();

    let stderr = std::fs::read_to_string(td.path().join("stderr.txt")).unwrap();
    assert!(
        !status.success(),
        "a watchdog trip must exit non-zero, was: {status:?} — stderr: {stderr}"
    );
    assert!(
        stderr.contains("watchdog"),
        "the process must name the watchdog as the cause, stderr was: {stderr}"
    );
    assert!(
        took < Duration::from_secs(5),
        "a trip must not sit out the {DRAIN_BUDGET_MS} ms drain budget — took {took:?}"
    );
}

/// The counterpart: a normal signal shutdown DOES drain, with the same budget.
///
/// Same fixture, same 60 s budget, no watchdog keys — only the door differs. The
/// receipt is positive: the answer that was still inside `handle()` when SIGTERM
/// arrived reaches the capture side, and the process exits 0. Without the drain
/// that answer is simply lost, which is the whole of GH #47.
///
/// The upper time bound is the second half of the receipt: the run must end at
/// QUIESCENCE, not at the deadline. A drain that sat out its 60 s budget would
/// be a wait, not a drain. The lower bound says the answer really was in flight
/// — a run that ended before one cell sleep had passed cannot have saved one.
///
/// stdin is closed 200 ms AFTER the signal, for the reason given on the test
/// above; the gap is what makes the signal provably the door that was taken.
#[cfg(unix)]
#[test]
fn a_signal_shutdown_still_takes_the_draining_door() {
    const SIGNAL_SLEEP_MS: u64 = 1_500;

    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), SIGNAL_SLEEP_MS);
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS}}}"#),
    )
    .unwrap();

    let (out, err) = capture_files(td.path());
    let t0 = std::time::Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .expect("meclaw must start");

    let mut stdin = child.stdin.take().expect("stdin piped");
    writeln!(stdin, "line-0").unwrap();
    stdin.flush().unwrap();

    // Long enough for boot plus the routing hop, short enough that the cell is
    // still inside its 1.5 s sleep when the signal lands.
    std::thread::sleep(Duration::from_millis(700));
    let killed = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill must run");
    assert!(killed.success(), "SIGTERM must reach the process");
    std::thread::sleep(Duration::from_millis(200));
    drop(stdin);

    let status = child.wait().expect("the process must end");
    let took = t0.elapsed();

    let stdout = std::fs::read_to_string(td.path().join("stdout.txt")).unwrap();
    let stderr = std::fs::read_to_string(td.path().join("stderr.txt")).unwrap();
    assert!(
        status.success(),
        "a drained signal shutdown exits 0, was: {status:?} — stderr: {stderr}"
    );
    assert!(
        stdout.lines().any(|l| l == "line-0"),
        "the in-flight answer must survive the signal — stdout was: {stdout:?}"
    );
    assert!(
        took >= Duration::from_millis(SIGNAL_SLEEP_MS),
        "the answer can only have been in flight if the run outlived one cell \
         sleep — took {took:?}"
    );
    assert!(
        took < Duration::from_secs(20),
        "the drain must end at quiescence, not at the {DRAIN_BUDGET_MS} ms \
         deadline — took {took:?}"
    );
}
