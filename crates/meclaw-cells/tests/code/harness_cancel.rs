//! P8 — the cancel path, proven against a task that would never end on its own.
//!
//! The handler side (tombstone before kill, `cancelled` status) is covered in
//! `harness_cell_lifecycle`. What is proven HERE is the part no unit test can
//! fake: that the kill actually reaches the operating system, and that it
//! reaches the harness's OWN children too. A coding agent spawns build and
//! search tools; a cancel that leaves those running has not cancelled anything.
//!
//! Linux-only by construction — `/proc` is the evidence.
#![cfg(target_os = "linux")]

use meclaw_cells::harness::io::{HarnessEvent, HarnessIo, HarnessReconfig, StartTask, run_io};
use meclaw_cells::stdio_child::{ChildCommand, ChildEvent, ChildSpec, ServeConfig};
use std::time::Duration;
use tokio::sync::mpsc;

const FIXTURE: &str = env!("CARGO_BIN_EXE_stream_json_harness_fixture");

/// Wait for a pid file to appear and parse it. Polling is fine in a test.
async fn read_pid(path: &std::path::Path) -> u32 {
    for _ in 0..300 {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(pid) = s.trim().parse::<u32>()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no pid was ever written to {}", path.display());
}

/// The `/proc` entry disappears only for a process that is both dead AND
/// reaped — a zombie would keep it.
async fn assert_process_gone(pid: u32, what: &str) {
    let entry = format!("/proc/{pid}");
    for _ in 0..3000 {
        if !std::path::Path::new(&entry).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    panic!("{what} (pid {pid}) survived the cancel: {stat}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_stalled_task_reaps_the_harness_and_its_children() {
    let dir = tempfile::tempdir().expect("tempdir");
    let child_pid_file = dir.path().join("child.pid");
    let grandchild_pid_file = dir.path().join("grandchild.pid");

    let (cmd_tx, cmd_rx) = mpsc::channel::<HarnessReconfig>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<HarnessEvent>(64);
    tokio::spawn(run_io(HarnessIo, ev_tx, cmd_rx));
    assert!(matches!(
        ev_rx.recv().await.expect("booted"),
        HarnessEvent::Booted
    ));

    cmd_tx
        .send(HarnessReconfig::Start(StartTask {
            task_id: "t-1".to_string(),
            spec: ChildSpec {
                program: FIXTURE.to_string(),
                args: vec![
                    // Never finishes on its own, and brings a child of its own.
                    "stall".to_string(),
                    "--grandchild".to_string(),
                    "--pid-file".to_string(),
                    child_pid_file.display().to_string(),
                    "--grandchild-pid-file".to_string(),
                    grandchild_pid_file.display().to_string(),
                ],
                kill_grace_ms: 300,
                process_group: true,
                ..ChildSpec::default()
            },
            startup_timeout: Duration::from_secs(5),
            serve: ServeConfig {
                write_timeout: Duration::from_secs(5),
                kill_grace: Duration::from_millis(300),
            },
        }))
        .await
        .expect("send start");

    let child_pid = read_pid(&child_pid_file).await;
    let grandchild_pid = read_pid(&grandchild_pid_file).await;
    assert!(
        std::path::Path::new(&format!("/proc/{grandchild_pid}")).exists(),
        "the grandchild was not running to begin with"
    );

    // The task is genuinely under way: it announced itself and said it was
    // working. Without this the test could pass against a child that had
    // already died for unrelated reasons.
    for _ in 0..2 {
        match tokio::time::timeout(Duration::from_secs(30), ev_rx.recv())
            .await
            .expect("no frame within 30s")
            .expect("channel closed")
        {
            HarnessEvent::Child(ChildEvent::Frame(_)) => {}
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    // What `cancel` sends. Tight window on purpose: the kill must not wait out
    // some multi-second timeout, or a cancel would be indistinguishable from
    // giving up.
    let started = std::time::Instant::now();
    cmd_tx
        .send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("send cancel");

    match tokio::time::timeout(Duration::from_secs(30), ev_rx.recv())
        .await
        .expect("no exit within 30s")
        .expect("channel closed")
    {
        HarnessEvent::Child(ChildEvent::Exited(_)) => {}
        other => panic!("expected the exit event, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the cancel took {:?} — a stop lever has to be prompt",
        started.elapsed()
    );

    assert_process_gone(child_pid, "the harness").await;
    assert_process_gone(grandchild_pid, "the harness's own child").await;
}

/// After a cancel the cell is not spent: the I/O task returns to its idle wait
/// and the next task runs. A cancel that wedged the cell would turn one
/// abandoned task into a dead harness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cell_accepts_another_task_after_a_cancel() {
    let (cmd_tx, cmd_rx) = mpsc::channel::<HarnessReconfig>(8);
    let (ev_tx, mut ev_rx) = mpsc::channel::<HarnessEvent>(64);
    tokio::spawn(run_io(HarnessIo, ev_tx, cmd_rx));
    assert!(matches!(
        ev_rx.recv().await.expect("booted"),
        HarnessEvent::Booted
    ));

    let start = |task_id: &str, mode: &str| {
        HarnessReconfig::Start(StartTask {
            task_id: task_id.to_string(),
            spec: ChildSpec {
                program: FIXTURE.to_string(),
                args: vec![mode.to_string()],
                kill_grace_ms: 300,
                process_group: true,
                ..ChildSpec::default()
            },
            startup_timeout: Duration::from_secs(5),
            serve: ServeConfig {
                write_timeout: Duration::from_secs(5),
                kill_grace: Duration::from_millis(300),
            },
        })
    };

    cmd_tx.send(start("t-1", "stall")).await.expect("send");
    // Let it get going, then stop it.
    for _ in 0..2 {
        let _ = tokio::time::timeout(Duration::from_secs(30), ev_rx.recv()).await;
    }
    cmd_tx
        .send(HarnessReconfig::Child(ChildCommand::Shutdown))
        .await
        .expect("cancel");
    loop {
        match ev_rx.recv().await.expect("channel closed") {
            HarnessEvent::Child(ChildEvent::Exited(_)) => break,
            _ => continue,
        }
    }

    cmd_tx.send(start("t-2", "ok")).await.expect("send second");
    let mut saw_result = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(30), ev_rx.recv())
            .await
            .expect("no events within 30s")
            .expect("channel closed")
        {
            HarnessEvent::Child(ChildEvent::Frame(v)) if v["type"] == "result" => saw_result = true,
            HarnessEvent::Child(ChildEvent::Exited(_)) => break,
            _ => continue,
        }
    }
    assert!(saw_result, "the task after a cancel produced no result");
}
