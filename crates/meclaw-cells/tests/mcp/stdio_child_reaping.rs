//! P7 step 2.7 — orphan reaping: no child process survives the death of the
//! task that owns it, and none is left behind as a zombie.
//!
//! The proof is the disappearance of `/proc/<pid>`. That single check covers
//! both failure modes at once: a still-running child keeps its entry, and a
//! zombie keeps its entry too (in state `Z`) until its parent reaps it. Only a
//! killed AND reaped process loses the entry.
//!
//! Linux-only by construction — `/proc` is the evidence. The core itself is
//! portable; the proof is not.
#![cfg(target_os = "linux")]

use meclaw_cells::stdio_child::{
    ChildCommand, ChildEvent, ChildSpec, CorrelationKey, ServeConfig, StdioChild,
    serve::serve_child,
};
use serde_json::Value as JsonValue;
use std::time::Duration;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");

fn spec_with_pid_file(mode: &str, pid_file: &std::path::Path) -> ChildSpec {
    ChildSpec {
        program: FIXTURE.to_string(),
        args: vec![
            mode.to_string(),
            "--pid-file".to_string(),
            pid_file.display().to_string(),
        ],
        env: Vec::new(),
        cwd: None,
        kill_grace_ms: 500,
        ..ChildSpec::default()
    }
}

fn no_correlation(_v: &JsonValue) -> Option<CorrelationKey> {
    None
}

/// Read the pid the fixture wrote, waiting for the file to appear.
/// Polling is fine here: this is a test, not production code.
async fn read_pid(path: &std::path::Path) -> u32 {
    for _ in 0..300 {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(pid) = s.trim().parse::<u32>()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture never wrote its pid to {}", path.display());
}

/// Assert the process is gone within 30s (generous failure-marker timeout).
async fn assert_process_gone(pid: u32) {
    let proc_entry = format!("/proc/{pid}");
    for _ in 0..3000 {
        if !std::path::Path::new(&proc_entry).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    panic!("pid {pid} survived: /proc entry still present (stat: {state})");
}

/// P8: a child that spawns a child of its own.
///
/// `sh` starts a long `sleep` in the background, prints its pid, then blocks on
/// `cat` so the parent stays alive until stdin closes. The `sleep` is a real
/// grandchild: it survives its parent unless something reaps the whole group.
///
/// The grandchild gets its own file descriptors on purpose. Children inherit
/// this process's stderr (see `StdioChild::spawn`), and a surviving orphan
/// holding that pipe open wedges the whole test run — the control case below
/// deliberately produces such an orphan.
fn grandparent_spec(process_group: bool) -> ChildSpec {
    ChildSpec {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "sleep 300 </dev/null >/dev/null 2>/dev/null & echo $!; exec cat".to_string(),
        ],
        kill_grace_ms: 200,
        process_group,
        ..ChildSpec::default()
    }
}

/// The grandchild's pid, printed by the shell as its first (non-JSON) line.
async fn read_grandchild_pid(child: &mut StdioChild) -> u32 {
    let frame = tokio::time::timeout(Duration::from_secs(30), child.read_frame())
        .await
        .expect("no line from the shell within 30s")
        .expect("read failed")
        .expect("the shell closed stdout without printing a pid");
    // A bare pid is valid JSON (a number), so it arrives as `Frame::Json` — not
    // as `Malformed`, which is only for lines that do not parse at all.
    match frame {
        meclaw_cells::stdio_child::Frame::Json(v) => {
            v.as_u64().expect("the shell printed a non-numeric pid") as u32
        }
        meclaw_cells::stdio_child::Frame::Malformed(raw) => {
            raw.trim().parse().expect("the shell printed a non-pid")
        }
    }
}

/// P8, the registered P7 follow-up (`docs/defer-register.md`): killing the direct
/// child is not enough for a harness — it spawns process trees.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminating_a_process_group_reaps_the_grandchild_too() {
    let mut child = StdioChild::spawn(&grandparent_spec(true)).expect("spawn shell");
    let child_pid = child.pid().expect("child pid");
    let grandchild_pid = read_grandchild_pid(&mut child).await;
    assert!(
        std::path::Path::new(&format!("/proc/{grandchild_pid}")).exists(),
        "grandchild {grandchild_pid} was not running to begin with"
    );

    child.terminate(Duration::from_millis(200)).await;

    assert_process_gone(child_pid).await;
    assert_process_gone(grandchild_pid).await;
}

/// The discriminating control: WITHOUT the process group the very same shape
/// leaves the grandchild running. Without this test the one above could pass
/// because the grandchild died on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_process_group_the_grandchild_survives() {
    let mut child = StdioChild::spawn(&grandparent_spec(false)).expect("spawn shell");
    let child_pid = child.pid().expect("child pid");
    let grandchild_pid = read_grandchild_pid(&mut child).await;

    child.terminate(Duration::from_millis(200)).await;
    assert_process_gone(child_pid).await;

    // Tight on purpose: the point is that nothing killed it. A generous wait
    // would only prove that `sleep 300` had not finished yet.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        std::path::Path::new(&format!("/proc/{grandchild_pid}")).exists(),
        "the control case must leave an orphan — otherwise the test above proves nothing"
    );
    // Clean up after the deliberate orphan, so the test suite leaves no
    // `sleep 300` behind — and prove the cleanup worked.
    let _ = std::process::Command::new("/bin/kill")
        .arg("-9")
        .arg(grandchild_pid.to_string())
        .status();
    assert_process_gone(grandchild_pid).await;
}

/// The teardown path that `kill_on_drop` alone cannot cover.
///
/// `kill_on_drop` signals the direct child only, so an aborted I/O task would
/// leave the harness's own children running. The group has to be reaped from a
/// `Drop` — the only code that still runs when a task is cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborting_the_serve_task_reaps_the_whole_process_group() {
    let mut child = StdioChild::spawn(&grandparent_spec(true)).expect("spawn shell");
    let child_pid = child.pid().expect("child pid");
    let grandchild_pid = read_grandchild_pid(&mut child).await;

    let (_cmd_tx, cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, _ev_rx) = mpsc::channel::<ChildEvent>(8);
    let cfg = ServeConfig {
        write_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(200),
    };
    let join = tokio::spawn(serve_child(child, no_correlation, cfg, cmd_rx, ev_tx));

    join.abort();

    assert_process_gone(child_pid).await;
    assert_process_gone(grandchild_pid).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborting_the_serve_task_kills_and_reaps_a_hanging_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("child.pid");
    let child = StdioChild::spawn(&spec_with_pid_file("hang", &pid_file)).expect("spawn");
    let (_cmd_tx, cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, _ev_rx) = mpsc::channel::<ChildEvent>(8);
    let cfg = ServeConfig {
        write_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(200),
    };
    let join = tokio::spawn(serve_child(child, no_correlation, cfg, cmd_rx, ev_tx));

    let pid = read_pid(&pid_file).await;
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "fixture {pid} was not running to begin with"
    );

    // This is the substrate's teardown path: the mailbox-close handling in
    // `cell_task_long_running` aborts the I/O sub-task's join handle.
    join.abort();
    assert_process_gone(pid).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shutdown_command_kills_and_reaps_a_hanging_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("child.pid");
    let child = StdioChild::spawn(&spec_with_pid_file("hang", &pid_file)).expect("spawn");
    let (cmd_tx, cmd_rx) = mpsc::channel::<ChildCommand>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<ChildEvent>(8);
    let cfg = ServeConfig {
        write_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(200),
    };
    tokio::spawn(serve_child(child, no_correlation, cfg, cmd_rx, ev_tx));

    let pid = read_pid(&pid_file).await;
    cmd_tx
        .send(ChildCommand::Shutdown)
        .await
        .expect("send shutdown");

    match tokio::time::timeout(Duration::from_secs(30), ev_rx.recv())
        .await
        .expect("no exit event within 30s")
        .expect("event channel closed")
    {
        ChildEvent::Exited(x) => assert_eq!(x.detail(), "killed by signal"),
        other => panic!("expected the exit event, got {other:?}"),
    }
    assert_process_gone(pid).await;
}
