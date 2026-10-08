//! GH #47 + issue #6: a watchdog trip is not a graceful shutdown in disguise.
//!
//! A fatal trip means the colony loop is not coming back. Draining through it
//! would mean waiting on the thing that is already broken, and the process would
//! sit out its whole budget before dying — turning a fast, loud failure into a
//! slow one. The trip therefore takes the `ShutdownNow` door.
//!
//! **Why both tests put a slow cell in flight.** The plan sketched this file
//! as two in-process runs over an EMPTY hive. That version was measured against
//! HEAD before the CLI change and passed in 0.23 s: with no cell and nothing in
//! flight, the drain reaches quiescence on its first look, so "the trip ended
//! fast" holds whether the trip drained or not. The assertion was hollow — it
//! discriminated nothing. A drain only costs time when there is work to wait
//! for, so both tests here put a real, slow `code` cell in flight and then shut
//! the process down: the trip must walk past that work, the signal must wait
//! for it. Same fixture, same 60 s budget, opposite verdicts. The signal case
//! runs the real binary (a SIGTERM needs a process of its own); the trip case
//! runs in-process, see its doc comment.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long the cell stays inside `handle()` in the trip case.
///
/// Well above the timing bound of the trip case (window + 5 s = 15 s), so a
/// trip that drained would be caught even if the drain were cut short by some
/// other budget on the way.
const TRIP_SLEEP_MS: u64 = 40_000;

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

/// In-process `--api` run of the fixture root (the trip case).
fn cli_for(root: &std::path::Path) -> meclaw_cli::Cli {
    meclaw_cli::Cli {
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

/// The supervisor window of the trip case: 200 periods of 50 ms, i.e. ten
/// seconds of colony silence. Long enough that the trip cannot land before the
/// cell is inside `handle()` (the colony beats while it routes the message),
/// short enough to land well inside the cell's [`TRIP_SLEEP_MS`].
const TRIP_THRESHOLD: u32 = 200;
const TRIP_PERIOD_MS: u64 = 50;

/// A trip with a LARGE drain budget still ends fast, and still ends non-zero.
///
/// The two-sided assertion is the discriminator: a non-zero exit alone would
/// also hold if the trip had drained first (it would simply arrive later), and
/// "fast" alone would hold if there were no trip at all. Both together only hold
/// for a trip that walked past the drain.
///
/// **Where the trip comes from (GH #1099).** The colony loop beats every half
/// `watchdog_period_ms` of its `colony.json`, so a deadline taken from the same
/// file can no longer trip a quiet colony -- the old version of this test
/// relied on exactly that false alarm (3 x 20 ms against a fixed 100 ms beat)
/// and went quiet with the fix. The silence here is the silence of a parked
/// loop: `colony.json` slows the beat to one per 30 s, and the supervisor
/// (`run_with_hooks_tuned`, the function `run()` calls, with its override)
/// counts [`TRIP_THRESHOLD`] x [`TRIP_PERIOD_MS`] of it. The run is in-process
/// over `--api` for the same reason: the override is the one way to give the
/// supervisor a window different from the beat's, and the binary has none.
///
/// The timing bound is a semantic discriminator: the cell sits inside
/// `handle()` for [`TRIP_SLEEP_MS`] and the drain has [`DRAIN_BUDGET_MS`] to
/// wait for it, so a draining trip cannot end before the cell's quiescence,
/// `TRIP_SLEEP_MS` after the cell started. The trip lands one window after the
/// colony's last beat, which is when it routed the message. Counted from the
/// moment the cell is known to be in flight, a trip that skips the drain ends
/// after at most the window plus teardown; the bound sits between the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_watchdog_trip_skips_the_drain_and_still_exits_non_zero() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), TRIP_SLEEP_MS);
    std::fs::write(
        td.path().join("colony.json"),
        format!(
            r#"{{"shutdown_drain_timeout_ms": {DRAIN_BUDGET_MS},
                 "watchdog_period_ms": 60000}}"#
        ),
    )
    .unwrap();
    let supervisor = meclaw_cli::WatchdogTuning {
        threshold: TRIP_THRESHOLD,
        period: Duration::from_millis(TRIP_PERIOD_MS),
        on_trip: meclaw_cli::WatchdogOnTrip::Exit,
    };

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let run = tokio::spawn(meclaw_cli::run_with_hooks_tuned(
        cli_for(td.path()),
        Some(addr_tx),
        None,
        Some(supervisor),
    ));
    let addr = tokio::time::timeout(Duration::from_secs(30), addr_rx)
        .await
        .expect("the colony must bind HTTP within the failure marker")
        .expect("addr hook");

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

    // The positive receipt that there is work for a drain to wait for.
    let marker = td.path().join(IN_FLIGHT_MARKER);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            !run.is_finished(),
            "the run ended before the cell was in flight -- the trip came too early \
             to discriminate anything"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the cell never started its answer within 30 s"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let in_flight_at = std::time::Instant::now();

    let res = tokio::time::timeout(Duration::from_millis(DRAIN_BUDGET_MS * 2), run)
        .await
        .expect("the trip must end the run, not hang")
        .expect("the run task must not panic");
    let took = in_flight_at.elapsed();

    let err = res.expect_err("a watchdog trip must exit non-zero (Err), not Ok(())");
    let msg = format!("{err}");
    assert!(
        msg.contains("watchdog") && msg.contains("starved=colony_loop"),
        "the run must end on the fatal colony-loop trip, was: {msg}"
    );
    let window = Duration::from_millis(TRIP_PERIOD_MS * u64::from(TRIP_THRESHOLD));
    let bound = window + Duration::from_secs(5);
    assert!(
        bound < Duration::from_millis(TRIP_SLEEP_MS),
        "the bound must sit below the drain's earliest end"
    );
    assert!(
        took < bound,
        "a trip must not wait for the {TRIP_SLEEP_MS} ms answer in flight (a drain) -- \
         took {took:?} from the in-flight marker to the end of the run: {msg}"
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
