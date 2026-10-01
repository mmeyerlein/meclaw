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
use std::process::{Child, Command, Stdio};
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
/// Before it sleeps, the script creates the file [`IN_FLIGHT_MARKER`] in
/// `root`: the one sign a test can see from outside that an answer is inside
/// `handle()` right now (see [`wait_until_in_flight`]).
///
/// The same fixture shape as `gh47_a_drain_is_not_a_wedge.rs`, and it reads its
/// turns from `payload["body"]` for the same reason: a `code` cell is handed
/// three objects on stdin since 0.9.0 — `envelope`, `body`, `params`
/// (`docs/cell-types.md` § code, "Die drei Objekte auf stdin") — so
/// `payload["messages"]` is always absent and a script reading the top level
/// echoes an empty string.
fn write_slow_echo_fixture(root: &std::path::Path, sleep_ms: u64) {
    let marker = serde_json::to_string(&root.join(IN_FLIGHT_MARKER).to_string_lossy())
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
print(json.dumps({{"header": {{"finish_reason": "assistant"}},
                   "messages": [{{"origin": "assistant", "type": "text", "text": text}}]}}))
"#,
        marker,
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

/// The file the fixture's script creates in the root before it sleeps.
const IN_FLIGHT_MARKER: &str = "in_flight";

/// Block until the fixture's cell is inside `handle()`, i.e. until the script
/// has created [`IN_FLIGHT_MARKER`]; panic after 30 s (the failure-marker
/// convention of these files).
///
/// **Why a marker and not a fixed sleep.** `meclaw` installs its SIGTERM
/// handler only after the boot: the first poll of the shutdown select, reached
/// without a yield point right after the stdin reader is spawned
/// (`gh121_root_lease.rs` states the same property). A SIGTERM that lands
/// before that runs the DEFAULT disposition and the process dies by signal 15.
/// The test used to wait a fixed 700 ms for "boot plus the routing hop"; the
/// integration pass of 2026-09-29 (6604 tests beside it) got
/// `ExitStatus(unix_wait_status(15))` with nothing on stderr after the boot's
/// second line, and ten runs pinned to two cores beside twelve nice-19 busy
/// loops failed 10 of 10 the same way (idle: 10 of 10 green). The marker is
/// written only after the stdin reader has read the line, routed it through
/// the colony and started the cell's `python3` -- all of which happens after
/// the handler is in place -- and it is also the thing the receipt below is
/// about: an answer in flight when the signal lands.
fn wait_until_in_flight(root: &std::path::Path) {
    let marker = root.join(IN_FLIGHT_MARKER);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the cell never started its answer within 30 s -- stderr: {}",
            std::fs::read_to_string(root.join("stderr.txt")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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

/// The line the trip reporter writes for a FATAL trip, verbatim up to the
/// reason (`crates/meclaw-cli/src/lib.rs`, the task that writes every trip
/// down). The two non-fatal variants continue `watchdog trip (` instead of
/// `watchdog trip — `, so this prefix names the trip that ends the process and
/// no other: under load the independent witness can miss a window, and such a
/// trip is written down as `uncorroborated` while the colony keeps running.
const FATAL_TRIP_LINE: &str = "meclaw: watchdog trip — ";

/// Stop our own child before a failure marker panics: `Child` does not kill on
/// drop, and the unwind would otherwise close its stdin (the EOF door, up to one
/// cell sleep long) while the temp root is deleted under the running process.
fn fail_with_child(child: &mut Child, message: String) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    panic!("{message}");
}

/// Block until `stderr.txt` in `root` holds a line starting with `prefix`;
/// return when it was seen. Panics after 30 s (the failure-marker convention of
/// these files) with the stderr it had.
fn wait_for_stderr_line(
    root: &std::path::Path,
    prefix: &str,
    child: &mut Child,
) -> std::time::Instant {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let stderr = std::fs::read_to_string(root.join("stderr.txt")).unwrap_or_default();
        if stderr.lines().any(|l| l.starts_with(prefix)) {
            return std::time::Instant::now();
        }
        if std::time::Instant::now() >= deadline {
            fail_with_child(
                child,
                format!("no stderr line starting with {prefix:?} within 30 s -- stderr: {stderr}"),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Block until the root lease is gone, i.e. until `run_with_hooks_tuned` has
/// returned; panic after 30 s with the stderr it had.
///
/// The lease guard lives to the end of that function (`lease.rs`, and the
/// comment at `lease::acquire` in `lib.rs`), which is past the shutdown door,
/// the colony's teardown and the abort of the stdin reader. A vanished lease
/// while the test still holds stdin open therefore says: the process chose its
/// door and walked through it without the EOF door ever being on offer. Only
/// the runtime is still waiting, for the blocking stdin read it cannot cancel.
fn wait_until_lease_released(root: &std::path::Path, child: &mut Child) {
    let lease = root.join(meclaw_cli::lease::LEASE_DIR);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while lease.exists() {
        if std::time::Instant::now() >= deadline {
            fail_with_child(
                child,
                format!(
                    "the run never ended while stdin was open (lease still held after 30 s) \
                     -- stderr: {}",
                    std::fs::read_to_string(root.join("stderr.txt")).unwrap_or_default()
                ),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
/// (pre-#47: 10 s of ack wait plus 5 s of join wait) -- at least 15 s after the
/// trip. A trip that skips the drain ends in teardown time. Five seconds sits
/// between those; the measured values were 0.9 s with the change and 16 s
/// without it, both counted from the spawn. The clock now starts at the fatal
/// trip line instead of the spawn: boot time is the host's property, not the
/// door's, and a bound that also counted a slow boot on a loaded runner would
/// be a second race hidden inside the discriminator. Counted from the trip,
/// the bound carries one premise: today a draining trip would end at the
/// cell's quiescence, 20 s after the cell started, so it only overruns 5 s if
/// the fatal trip lands within 15 s of that start. It lands ~60-100 ms after the
/// watchdog is armed, the window the cell starts in; only 15 s of unbroken
/// `uncorroborated` trips together with a draining regression could hollow the
/// bound out. The stdout check at the end does not rest on that premise: a
/// trip that skips the drain never delivers the answer in flight.
///
/// **Why stdin stays open until the run has ended, and why closing it is not
/// the shutdown.** The direct-mode stdin reader sits in a blocking read
/// (`tokio::io::stdin`), which `abort()` cannot cancel and which the runtime
/// waits for on its way out; a process whose stdin stays open therefore never
/// leaves, whatever ended its colony. That is a property of the bridge and
/// predates this lane -- it was measured with `shutdown_drain_timeout_ms: 0`,
/// byte-exact pre-#47 behaviour, and the process outlived its SIGTERM until the
/// writing end was closed. The close is that release and nothing else, and the
/// test makes it so by construction: it waits for the fatal trip line
/// ([`FATAL_TRIP_LINE`]) and then for the root lease to be released
/// ([`wait_until_lease_released`]) before it lets go of stdin. The EOF is a
/// door of its own (a graceful shutdown, exit 0), so a close that lands before
/// the trip has been acted on races the trip for the shutdown select.
///
/// GH #914: this test used to close stdin after a fixed 800 ms, on the
/// assumption that the trip fires ~60 ms after boot. On a loaded CI runner
/// (run 36601260904, shard 2 of 3) boot plus trip took longer than that, the
/// close arrived first, and the process took the EOF door with status 0.
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
    // The trip has been reported as fatal, and the run has ended through the
    // door it chose, before stdin is let go; see the doc comment.
    let tripped_at = wait_for_stderr_line(td.path(), FATAL_TRIP_LINE, &mut child);
    wait_until_lease_released(td.path(), &mut child);
    drop(stdin);

    let status = child
        .wait()
        .expect("the trip must end the process, not hang");
    let took = tripped_at.elapsed();

    let stdout = std::fs::read_to_string(td.path().join("stdout.txt")).unwrap();
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
        "a trip must not sit out the {DRAIN_BUDGET_MS} ms drain budget — took {took:?} \
         from the fatal trip line to the exit"
    );
    // The clock-free half of the discriminator: the answer in flight is the
    // drain's receipt (the counterpart below requires exactly this line), and a
    // trip that walked past the drain never delivers it.
    assert!(
        !stdout.lines().any(|l| l == "line-0"),
        "a trip must not deliver the answer that was in flight -- that is a drain; \
         stdout was: {stdout:?}"
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
/// The signal waits for the cell to be inside `handle()` ([`wait_until_in_flight`]),
/// not for a fixed time: a signal before the boot has installed the handler
/// kills the process by signal 15, which is the boot's property and not the
/// drain's.
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

    // The cell has started its 1.5 s sleep, so the handler stands and the
    // answer is in flight when the signal lands.
    wait_until_in_flight(td.path());
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
