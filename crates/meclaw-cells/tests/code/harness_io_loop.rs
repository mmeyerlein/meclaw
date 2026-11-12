//! P8 block 5 — the harness I/O sub-task: idle, run one task, idle again.
//!
//! This is where the cell type differs structurally from every other
//! long-running one. `mcp` spawns its child once and dies with it; a harness
//! spawns one child per task and must come back for the next. The loop is
//! therefore a cycle, and the one thing it may never do is return while the
//! cell is alive (A1′).

use meclaw_cells::harness::claude_code::build_user_turn;
use meclaw_cells::harness::io::{HarnessEvent, HarnessIo, HarnessReconfig, StartTask, run_io};
use meclaw_cells::stdio_child::{ChildCommand, ChildEvent, ChildSpec, ServeConfig};
use std::time::Duration;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_stream_json_harness_fixture");

fn start(task_id: &str, args: &[&str], startup_ms: u64) -> HarnessReconfig {
    HarnessReconfig::Start(StartTask {
        task_id: task_id.to_string(),
        spec: ChildSpec {
            program: FIXTURE.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            kill_grace_ms: 500,
            process_group: true,
            ..ChildSpec::default()
        },
        first_turn: None,
        startup_timeout: Duration::from_millis(startup_ms),
        serve: ServeConfig {
            write_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(500),
        },
    })
}

/// Spawn the I/O task and hand back its two channels.
fn spawn_io() -> (
    mpsc::Sender<HarnessReconfig>,
    mpsc::Receiver<HarnessEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<HarnessReconfig>(8);
    let (ev_tx, ev_rx) = mpsc::channel::<HarnessEvent>(64);
    let join = tokio::spawn(run_io(HarnessIo, ev_tx, cmd_rx));
    (cmd_tx, ev_rx, join)
}

async fn next_event(rx: &mut mpsc::Receiver<HarnessEvent>) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no event within 30s")
        .expect("event channel closed")
}

/// Collect events until the task's exit, returning them in order.
async fn drain_until_exit(rx: &mut mpsc::Receiver<HarnessEvent>) -> Vec<HarnessEvent> {
    let mut seen = Vec::new();
    loop {
        let ev = next_event(rx).await;
        let done = matches!(ev, HarnessEvent::Child(ChildEvent::Exited(_)));
        seen.push(ev);
        if done {
            return seen;
        }
    }
}

/// The very first thing the I/O task does is announce itself — that event is
/// what triggers the handler's restart recovery, so it must arrive before any
/// task can be accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_io_task_announces_itself_and_starts_no_child_on_its_own() {
    let (_cmds, mut events, _join) = spawn_io();

    match next_event(&mut events).await {
        HarnessEvent::Booted => {}
        other => panic!("the first event must be Booted, got {other:?}"),
    }

    // Nothing else happens without a task. Tight on purpose: this asserts an
    // absence, so it only has to outlast a scheduling delay.
    let quiet = tokio::time::timeout(Duration::from_millis(300), events.recv()).await;
    assert!(quiet.is_err(), "the idle task produced an event on its own");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_streams_its_frames_and_ends_with_an_exit() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    cmds.send(start("t1", &["ok"], 5_000)).await.expect("send");
    let seen = drain_until_exit(&mut events).await;

    let frames: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::Child(ChildEvent::Frame(v)) => Some(v["type"].as_str().unwrap_or("")),
            _ => None,
        })
        .collect();
    assert_eq!(
        frames,
        vec!["system", "assistant", "result"],
        "every frame must reach the handler, in order: {seen:?}"
    );
    assert!(
        matches!(
            seen.last(),
            Some(HarnessEvent::Child(ChildEvent::Exited(_)))
        ),
        "the exit must be the last word: {seen:?}"
    );
}

/// The cycle: after a task ends the I/O task must be ready for the next one.
/// A parked loop — the `mcp` behaviour — would fail this.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tasks_run_one_after_the_other_in_the_same_cell() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    for task_id in ["t1", "t2"] {
        cmds.send(start(task_id, &["ok"], 5_000))
            .await
            .expect("send");
        let seen = drain_until_exit(&mut events).await;
        assert!(
            seen.iter().any(|e| matches!(
                e,
                HarnessEvent::Child(ChildEvent::Frame(v)) if v["type"] == "result"
            )),
            "task {task_id} produced no result frame: {seen:?}"
        );
    }
}

/// A harness that never says hello is broken. The startup timeout is the only
/// A-timeout on the task path — the run itself is deliberately unbounded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_never_speaks_fails_the_startup_timeout() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    // `slow` stays alive and says nothing for five seconds; the startup budget
    // here is 300ms, so the timeout is what ends it.
    cmds.send(start("t1", &["slow", "5000"], 300))
        .await
        .expect("send");

    match next_event(&mut events).await {
        HarnessEvent::TaskFailed {
            task_id,
            error_code,
            ..
        } => {
            assert_eq!(task_id, "t1");
            assert_eq!(error_code, "startup_timeout");
        }
        other => panic!("expected a startup timeout, got {other:?}"),
    }

    // And the cell is usable afterwards: a failed start must not wedge the loop.
    cmds.send(start("t2", &["ok"], 5_000)).await.expect("send");
    let seen = drain_until_exit(&mut events).await;
    assert!(seen.iter().any(|e| matches!(
        e,
        HarnessEvent::Child(ChildEvent::Frame(v)) if v["type"] == "result"
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_binary_that_cannot_be_started_is_reported_not_fatal() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    let mut cmd = start("t1", &["ok"], 5_000);
    if let HarnessReconfig::Start(s) = &mut cmd {
        s.spec.program = "/nonexistent/definitely/not/here".to_string();
    }
    cmds.send(cmd).await.expect("send");

    match next_event(&mut events).await {
        HarnessEvent::TaskFailed {
            task_id,
            error_code,
            detail,
        } => {
            assert_eq!(task_id, "t1");
            assert_eq!(error_code, "spawn_failed");
            assert!(!detail.is_empty(), "a spawn failure must say why");
        }
        other => panic!("expected a spawn failure, got {other:?}"),
    }
}

/// A1′ (`docs/meclaw-overview.md` § Long-running cells: double task): a clean
/// return from `run_io` while the cell lives would let the outer `select!`
/// abort a healthy handler. With no commands left to receive, the loop parks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_io_task_parks_instead_of_returning_when_its_channel_closes() {
    let (cmds, mut events, join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    drop(cmds);

    let parked = tokio::time::timeout(Duration::from_millis(300), join).await;
    assert!(parked.is_err(), "run_io returned instead of parking");
}

// ------------------------------------------------- the input dialect (D57) ----

/// A child in the `stream-json` input dialect, with its first turn attached.
fn start_turn(task_id: &str, extra: &[&str]) -> HarnessReconfig {
    let mut args = vec!["--input-format", "stream-json"];
    args.extend_from_slice(extra);
    let mut cmd = start(task_id, &args, 5_000);
    if let HarnessReconfig::Start(s) = &mut cmd {
        s.first_turn = Some(build_user_turn("do the thing"));
    }
    cmd
}

fn frame_types(seen: &[HarnessEvent]) -> Vec<String> {
    seen.iter()
        .filter_map(|e| match e {
            HarnessEvent::Child(ChildEvent::Frame(v)) => Some(v["type"].as_str()?.to_string()),
            _ => None,
        })
        .collect()
}

/// Events up to and including the next `result` frame.
async fn drain_until_result(rx: &mut mpsc::Receiver<HarnessEvent>) -> Vec<HarnessEvent> {
    let mut seen = Vec::new();
    loop {
        let ev = next_event(rx).await;
        let done =
            matches!(&ev, HarnessEvent::Child(ChildEvent::Frame(v)) if v["type"] == "result");
        seen.push(ev);
        if done {
            return seen;
        }
    }
}

fn session_of(seen: &[HarnessEvent]) -> Option<String> {
    seen.iter().find_map(|e| match e {
        HarnessEvent::Child(ChildEvent::Frame(v)) if v["subtype"] == "init" => {
            v["session_id"].as_str().map(str::to_string)
        }
        _ => None,
    })
}

/// End a process of our own by pid (the fixture writes it), from outside the
/// I/O sub-task — the child then ends without anyone asking it to.
fn kill_pid_in(path: &std::path::Path) {
    let pid: i32 = std::fs::read_to_string(path)
        .expect("pid file")
        .trim()
        .parse()
        .expect("pid");
    // SAFETY: plain kill(2) on a pid this test's own fixture wrote.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// D57 V2 / S0 1: the CLI says nothing until its first input. The first turn
/// is therefore written before the startup read; without it this child (silent
/// until input) would run into the startup timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_turn_is_written_before_the_startup_read() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    cmds.send(start_turn("t1", &[])).await.expect("send");
    let seen = drain_until_result(&mut events).await;
    assert_eq!(frame_types(&seen), vec!["system", "assistant", "result"]);

    // The child stays up for the next turn; only a Shutdown ends it.
    cmds.send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("send");
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Child(ChildEvent::Exited(_))
    ));
}

/// D57 V2: a follow-up turn is a `Send` to the live child — one process, one
/// session, two results.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_turn_reaches_the_same_child() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    cmds.send(start_turn("t1", &[])).await.expect("send");
    let first = drain_until_result(&mut events).await;

    cmds.send(HarnessReconfig::Child(ChildCommand::Send {
        line: build_user_turn("and the next thing"),
        correlate: None,
        reply: None,
    }))
    .await
    .expect("send");
    let second = drain_until_result(&mut events).await;

    // The fixture's default session id carries its pid: same id, same process.
    assert!(session_of(&first).is_some());
    assert_eq!(
        session_of(&first),
        session_of(&second),
        "the second turn must reach the same child"
    );

    cmds.send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("send");
    let rest = drain_until_exit(&mut events).await;
    assert_eq!(
        frame_types(&rest),
        Vec::<String>::new(),
        "one child, two results: {rest:?}"
    );
}

/// D57 V6: a `Start` that arrives while a child still runs is held and runs
/// after that child's exit — it is never dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_behind_a_closing_child_runs_after_its_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("a.pid");
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    let pid_arg = pid_file.display().to_string();
    cmds.send(start("a", &["stall", "--pid-file", &pid_arg], 5_000))
        .await
        .expect("send");
    // init + assistant: A is up and stays up.
    next_event(&mut events).await;
    next_event(&mut events).await;

    cmds.send(start_turn("b", &["--session-id", "s-b"]))
        .await
        .expect("send");
    kill_pid_in(&pid_file);

    let a = drain_until_exit(&mut events).await;
    assert!(frame_types(&a).is_empty(), "A said nothing more: {a:?}");
    let b = drain_until_result(&mut events).await;
    assert_eq!(session_of(&b).as_deref(), Some("s-b"), "B ran: {b:?}");

    cmds.send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("send");
    drain_until_exit(&mut events).await;
}

/// D57 V6 / OR-57-24: the commands behind a held `Start` belong to the new
/// child and reach it in order — a `Shutdown` there ends the NEW child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_behind_a_held_start_ends_the_new_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("a.pid");
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    let pid_arg = pid_file.display().to_string();
    cmds.send(start("a", &["stall", "--pid-file", &pid_arg], 5_000))
        .await
        .expect("send");
    next_event(&mut events).await;
    next_event(&mut events).await;

    // B never finishes on its own; only the held Shutdown can end it.
    cmds.send(start_turn("b", &["stall", "--session-id", "s-b"]))
        .await
        .expect("send");
    cmds.send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("send");
    kill_pid_in(&pid_file);

    drain_until_exit(&mut events).await;
    let b = drain_until_exit(&mut events).await;
    assert_eq!(
        session_of(&b).as_deref(),
        Some("s-b"),
        "B started and was ended by the Shutdown held behind its Start: {b:?}"
    );
}

/// D57 V10 / OR-57-28: an idle deadline armed after a turn reaches the
/// handler as `IdleExpired` while the real child idles — an event, not a
/// statement about time (the exact timing is pinned in virtual time in
/// `harness::io`). The child stays up: closing it is the handler's call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_armed_idle_deadline_reaches_the_handler() {
    let (cmds, mut events, _join) = spawn_io();
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Booted
    ));

    cmds.send(start_turn("t1", &[])).await.expect("send");
    drain_until_result(&mut events).await;

    cmds.send(HarnessReconfig::ArmIdle(Duration::from_millis(50)))
        .await
        .expect("send");
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::IdleExpired
    ));

    cmds.send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("send");
    assert!(matches!(
        next_event(&mut events).await,
        HarnessEvent::Child(ChildEvent::Exited(_))
    ));
}
