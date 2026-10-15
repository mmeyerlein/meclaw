//! P8 block 5 — the harness I/O sub-task: idle, run one task, idle again.
//!
//! This is where the cell type differs structurally from every other
//! long-running one. `mcp` spawns its child once and dies with it; a harness
//! spawns one child per task and must come back for the next. The loop is
//! therefore a cycle, and the one thing it may never do is return while the
//! cell is alive (A1′).

use meclaw_cells::harness::io::{HarnessEvent, HarnessIo, HarnessReconfig, StartTask, run_io};
use meclaw_cells::stdio_child::{ChildEvent, ChildSpec, ServeConfig};
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
