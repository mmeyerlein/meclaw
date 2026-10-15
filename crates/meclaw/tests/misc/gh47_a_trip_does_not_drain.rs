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
//! for it. Same fixture, same drain budget, opposite verdicts. The signal case
//! runs the real binary (a SIGTERM needs a process of its own); the trip case
//! runs in-process, see its doc comment.
//!
//! **No clock decides either verdict (GH #1216, tv-br M10k; TV-Z1).** Both used
//! to bound the run's wall-clock duration (`took < window + 5 s`, `took < 20 s`).
//! Under a CPU quota the supervisor itself got its slice 7.2 s late, the trip
//! came after 17.2 s of silence instead of 10 s, and the bound failed a trip
//! that had skipped the drain correctly. The verdicts are now events: the trip
//! case holds the answer until the test releases it, so only a run that skipped
//! the drain can end at all; the signal case gets a drain budget far above every
//! hang mark, so only a drain that ended at quiescence ends under the mark.

use meclaw_testing::{deadline, wait};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// The drain budget both tests hand the colony: the product wait of the one
/// time factor (ten scaled `HANG`, five scaled `BOOT_HANG`), far above every
/// hang mark in this file under every admissible factor. A run that sat out
/// this budget cannot end under a mark, so ending under one says the shutdown
/// did not wait for the budget. A literal stayed fixed while the marks grew
/// with `MECLAW_TEST_TIME_FACTOR` (tv-br M10o, re-check C-M1).
fn drain_budget_ms() -> u64 {
    deadline::product_wait_ms()
}

/// How long a held answer waits for its release at most: a safety cap so a
/// test that died before releasing leaves no script behind for long. It lies
/// above the boot mark the trip verdict waits under (a trip that drained must
/// still be holding the answer when that mark fires) and below the cell's
/// operation timeout, under every factor (C-M1: a literal 120 s fell under
/// the mark from F = 2 on, and a draining trip ended green under it).
fn hold_cap_s() -> u64 {
    let cap = deadline::product_wait_ms() / 2_000;
    assert!(
        Duration::from_secs(cap) > deadline::scaled(deadline::BOOT_HANG),
        "the hold cap {cap} s must outlast the boot mark"
    );
    cap
}

/// The file the trip case writes once the run has ended; the held answer is
/// given only after it exists.
const RELEASE_MARKER: &str = "release";

/// How the fixture's cell gets to its answer.
#[derive(Clone, Copy)]
enum Answer {
    /// After a fixed sleep (the signal case: the answer is on its way).
    After(u64),
    /// Only once [`RELEASE_MARKER`] exists in the root (the trip case).
    Held,
}

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
fn write_slow_echo_fixture(root: &std::path::Path, answer: Answer) {
    let marker = serde_json::to_string(&root.join(IN_FLIGHT_MARKER).to_string_lossy())
        .expect("a path string serialises");
    let release = serde_json::to_string(&root.join(RELEASE_MARKER).to_string_lossy())
        .expect("a path string serialises");
    let hold_cap = hold_cap_s();
    let wait = match answer {
        Answer::After(ms) => format!("time.sleep({})", ms as f64 / 1000.0),
        Answer::Held => format!(
            "cap = time.monotonic() + {hold_cap}\nwhile not os.path.exists({release}) and time.monotonic() < cap:\n    time.sleep(0.05)"
        ),
    };
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
import sys, json, time, os
payload = json.loads(sys.stdin.read())
turns = payload["body"].get("messages", [])
text = turns[-1]["text"] if turns else ""
open({}, "w").close()
{}
print(json.dumps({{"header": {{"finish_reason": "assistant"}},
                   "messages": [{{"origin": "assistant", "type": "text", "text": text}}]}}))
"#,
        marker, wait
    );

    std::fs::write(
        echo_dir.join("config.json"),
        serde_json::json!({
            "cell": {"type": "code", "message_timeout": meclaw_testing::deadline::product_backstop_ms()},
            "params": {
                "runner": "python3",
                "script_inline": script,
                "external_timeout_ms": meclaw_testing::deadline::product_wait_ms()
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

/// Wait until the fixture's cell is inside `handle()`, i.e. until the script
/// has created [`IN_FLIGHT_MARKER`] -- or `ended` says the run is already over.
/// Under the boot mark (TV-Z3): the wait spans the routing hop and the start of
/// the cell's `python3`, a process boot.
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
async fn wait_until_in_flight(root: &std::path::Path, mut ended: impl FnMut() -> bool) {
    let marker = root.join(IN_FLIGHT_MARKER);
    wait::boot_until_with(
        "the cell starts its answer (in-flight marker)",
        || std::future::ready(marker.exists() || ended()),
        || std::fs::read_to_string(root.join("stderr.txt")).unwrap_or_default(),
    )
    .await;
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
/// cell is inside `handle()` (the colony beats while it routes the message).
const TRIP_THRESHOLD: u32 = 200;
const TRIP_PERIOD_MS: u64 = 50;

/// A trip with a LARGE drain budget still ends the run, and still ends it
/// non-zero, while the answer in flight is held.
///
/// The cell holds its answer until the test writes [`RELEASE_MARKER`], which it
/// does only after the run has ended. A trip that drained would wait for that
/// answer -- for the whole [`drain_budget_ms`] -- so it could not end the run at
/// all, and the hang mark below would fire. Ending under the mark, non-zero, on
/// the fatal colony-loop trip, is therefore the receipt that the trip walked
/// past the drain; no clock measures how fast (GH #1216, tv-br M10k: a bound on
/// the duration failed a correct trip that came 7.2 s late on a host under a
/// CPU quota).
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
/// The run's end is awaited under the boot mark (60 s, TV-Z3): from the
/// in-flight marker it spans the 10 s window plus the supervisor's own lag on a
/// starved host -- 17.2 s of silence at most, seen on build04 under an 80 %
/// quota with 44 tests beside it (3.5 x below the mark).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_watchdog_trip_skips_the_drain_and_still_exits_non_zero() {
    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), Answer::Held);
    std::fs::write(
        td.path().join("colony.json"),
        format!(
            r#"{{"shutdown_drain_timeout_ms": {},
                 "watchdog_period_ms": 60000}}"#,
            drain_budget_ms()
        ),
    )
    .unwrap();
    let supervisor = meclaw_cli::WatchdogTuning {
        threshold: TRIP_THRESHOLD,
        period: Duration::from_millis(TRIP_PERIOD_MS),
        on_trip: meclaw_cli::WatchdogOnTrip::Exit,
    };

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let mut run = tokio::spawn(meclaw_cli::run_with_hooks_tuned(
        cli_for(td.path()),
        Some(addr_tx),
        None,
        Some(supervisor),
    ));
    let addr = deadline::boot_hang("the colony binds HTTP", addr_rx)
        .await
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
    wait_until_in_flight(td.path(), || run.is_finished()).await;
    assert!(
        td.path().join(IN_FLIGHT_MARKER).exists(),
        "the run ended before the cell was in flight -- the trip came too early \
         to discriminate anything"
    );

    let res = deadline::boot_hang_with(
        "the trip ends the run while its answer is held (a draining trip waits for it)",
        &mut run,
        || "the answer is still held; the run has not ended",
    )
    .await
    .expect("the run task must not panic");
    // Only now may the answer go: the run is over without it.
    std::fs::write(td.path().join(RELEASE_MARKER), b"").unwrap();

    let err = res.expect_err("a watchdog trip must exit non-zero (Err), not Ok(())");
    let msg = format!("{err}");
    assert!(
        msg.contains("watchdog") && msg.contains("starved=colony_loop"),
        "the run must end on the fatal colony-loop trip, was: {msg}"
    );
}

/// The counterpart: a normal signal shutdown DOES drain, with the same budget.
///
/// Same fixture, same drain budget, no watchdog keys — only the door differs.
/// The receipt is positive: the answer that was still inside `handle()` when
/// SIGTERM arrived reaches the capture side, and the process exits 0. Without
/// the drain that answer is simply lost, which is the whole of GH #47.
///
/// The run must end at QUIESCENCE, not at the deadline: the drain budget is
/// twenty minutes, so a drain that sat it out would hit the hang mark on the
/// exit instead of ending under it (GH #1216, tv-br M10k: this replaced a
/// wall-clock bound of 20 s; the lower bound "outlived one cell sleep" went
/// too -- the in-flight marker already proves the answer was in flight).
///
/// The signal waits for the cell to be inside `handle()` ([`wait_until_in_flight`]),
/// not for a fixed time: a signal before the boot has installed the handler
/// kills the process by signal 15, which is the boot's property and not the
/// drain's.
///
/// stdin stays open until the process has exited, so the end of input can
/// never be the door that was taken -- only the signal can. This is also the
/// binary pin of OR-T12-14 (GH #1216, tv-br M10m): while stdin was read on
/// tokio's blocking pool, the drain completed and the process then sat in
/// `pipe_read` until the writer closed stdin; the reader is now a thread the
/// runtime's shutdown does not wait for. Never make this test green by
/// closing stdin before the exit.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signal_shutdown_still_takes_the_draining_door() {
    const SIGNAL_SLEEP_MS: u64 = 1_500;

    let td = tempfile::TempDir::new().unwrap();
    write_slow_echo_fixture(td.path(), Answer::After(SIGNAL_SLEEP_MS));
    std::fs::write(
        td.path().join("colony.json"),
        format!(r#"{{"shutdown_drain_timeout_ms": {}}}"#, drain_budget_ms()),
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

    // The cell has started its 1.5 s sleep, so the handler stands and the
    // answer is in flight when the signal lands.
    wait_until_in_flight(td.path(), || false).await;
    let killed = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill must run");
    assert!(killed.success(), "SIGTERM must reach the process");

    let status = wait::until_some("the signalled process exits after its drain", || {
        std::future::ready(child.try_wait().ok().flatten())
    })
    .await;
    drop(stdin);

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
}
