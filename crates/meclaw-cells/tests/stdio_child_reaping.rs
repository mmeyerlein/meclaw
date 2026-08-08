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
