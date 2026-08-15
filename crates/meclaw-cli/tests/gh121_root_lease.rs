//! GH #121: two daemons on one `{root}` must be impossible.
//!
//! Every proof here uses REAL processes. The lease exists to stop a second
//! `meclaw` from booting beside a live one, and the interesting cases are all
//! about what the kernel says about a pid — so nothing about the holder is
//! faked: the living holder is a running `meclaw`, the dead holder is a child
//! this test reaped, and the recycled pid is a real foreign process that
//! genuinely occupies the pid the lease names.
//!
//! Two disciplines this file keeps, both learned from the RED run: every child
//! is owned by a [`Proc`] guard so a failing assertion cannot leak a daemon,
//! and no expectation is ever awaited without a deadline — a regression here
//! means a second daemon RUNS, and a run that hangs forever reports nothing.

use meclaw_cli::lease::{HOLDER_FILE, LEASE_DIR, ProcStatus, process_status};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Generous failure marker (30 s convention): a boot is expected in well under
/// a second; the deadline only fences a hang.
const DEADLINE: Duration = Duration::from_secs(30);

/// A child process this test owns. Dropping it ends the process, so a panicking
/// assertion cannot leave a daemon running on a temp root.
struct Proc(Option<std::process::Child>);

impl Proc {
    fn pid(&self) -> u32 {
        self.0.as_ref().expect("child is still owned").id()
    }

    /// Ask for an orderly stop. `Child::kill` sends SIGKILL, which is the
    /// *other* case this file tests, so SIGTERM goes through `kill(1)`.
    fn sigterm(&self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.pid().to_string())
            .status();
    }

    /// SIGTERM, then wait for the exit status.
    ///
    /// Only call this once [`wait_for_boot`] has seen the daemon finish booting:
    /// `meclaw` installs its SIGTERM handler after the filesystem bootstrap, and
    /// a signal that arrives before that runs the DEFAULT disposition — the
    /// process dies by signal, no destructor runs, and the stop is not orderly
    /// at all. That is a property of the boot, not of the lease.
    fn stop(mut self) -> std::process::ExitStatus {
        self.sigterm();
        let mut child = self.0.take().expect("child is still owned");
        child.wait().expect("wait for child")
    }

    /// Hard kill and reap — a crashed daemon, seen from outside.
    fn kill_and_reap(mut self) -> u32 {
        let pid = self.pid();
        let mut child = self.0.take().expect("child is still owned");
        let _ = child.kill();
        let _ = child.wait();
        wait_until("the killed process to leave /proc", || {
            process_status(pid) == ProcStatus::Gone
        });
        pid
    }

    /// Has it exited on its own yet?
    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.0.as_mut()?.try_wait().ok().flatten()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A minimal bootable root: one empty hive, no cells.
fn bootable_root(root: &Path) {
    let main = root.join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(main.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
}

/// Start a headless daemon on `root` (`--daemon` without `--api`: the colony
/// runs, no port is opened, it waits for SIGTERM).
fn spawn_daemon(root: &Path) -> Proc {
    Proc(Some(
        Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .arg("--root")
            .arg(root)
            .arg("--daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn meclaw daemon"),
    ))
}

/// Boot a daemon that is EXPECTED to refuse, and collect its exit + stderr.
///
/// Bounded on purpose: if the lease ever stops guarding, the process under test
/// boots and runs forever. Waiting for it with a deadline turns that regression
/// into a named failure instead of a wedged test run.
fn expect_refusal(root: &Path) -> (std::process::ExitStatus, String) {
    let mut proc = spawn_daemon(root);
    let deadline = Instant::now() + DEADLINE;
    let status = loop {
        if let Some(s) = proc.exited() {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "REGRESSION: the second daemon on {} booted and is still running — \
             the root lease did not refuse it",
            root.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    let mut child = proc.0.take().expect("child is still owned");
    let mut stderr = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read as _;
        let _ = s.read_to_string(&mut stderr);
    }
    // Already reaped by `try_wait` above; this returns the cached status and
    // keeps the "every spawned process is waited on" contract explicit.
    let _ = child.wait();
    (status, stderr)
}

/// Wait until the daemon has finished booting.
///
/// The marker is the boot's own log line. `tracing-appender` writes it
/// non-blocking, so by the time the line is readable the process is already
/// past it and into the shutdown select — which is exactly the guarantee a
/// SIGTERM needs.
fn wait_for_boot(root: &Path) {
    wait_until("the daemon to finish booting", || {
        log_contains(root, "filesystem bootstrap applied")
    });
}

/// Whether the JSONL log carries `needle` yet.
fn log_contains(root: &Path, needle: &str) -> bool {
    std::fs::read_to_string(root.join("log.jsonl"))
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

/// The pid the lease currently names, if there is a readable lease.
fn lease_pid(root: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(root.join(LEASE_DIR).join(HOLDER_FILE)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    u32::try_from(v["pid"].as_u64()?).ok()
}

/// Poll until `cond` holds, or fail with `what`.
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for: {what}");
}

/// Write a holder record by hand — a lease as some other process left it.
fn plant_lease(root: &Path, pid: u32, start_id: u64) {
    let dir = root.join(LEASE_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(HOLDER_FILE),
        serde_json::json!({ "pid": pid, "start_id": start_id, "acquired_at_unix": 0 }).to_string(),
    )
    .unwrap();
}

/// A real foreign process that stays alive for the duration of a test.
fn spawn_bystander() -> Proc {
    Proc(Some(
        Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bystander"),
    ))
}

fn start_id_of(pid: u32) -> u64 {
    match process_status(pid) {
        ProcStatus::Running { start_id, .. } => start_id,
        other => panic!("pid {pid} must be running, got {other:?}"),
    }
}

// ─── Proof 1: a second daemon against a living first ─────────────────────────

/// The operator trap the issue names: a second `meclaw` on the same root used
/// to boot, share the WAL and duplicate every cell. It must now refuse, name
/// the holder pid, and end non-zero — without a panic.
#[test]
fn a_second_daemon_on_a_live_root_refuses_to_boot_and_names_the_holder() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());

    let first = spawn_daemon(td.path());
    let first_pid = first.pid();
    wait_until("the first daemon to publish its lease", || {
        lease_pid(td.path()) == Some(first_pid)
    });

    let (status, stderr) = expect_refusal(td.path());

    assert!(
        !status.success(),
        "the second daemon must not boot; it exited {status:?}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!("pid {first_pid}")),
        "the refusal must name the holder pid {first_pid} so an operator can act, got:\n{stderr}"
    );
    assert!(
        stderr.contains("already running on this root"),
        "the refusal must say what is wrong in plain words, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked at"),
        "a refusal is a diagnosis, not a crash, got:\n{stderr}"
    );

    // The first daemon was never disturbed and still owns the root.
    assert_eq!(lease_pid(td.path()), Some(first_pid));
    wait_for_boot(td.path());
    assert!(
        first.stop().success(),
        "the holder stops cleanly afterwards"
    );
}

/// The lease must be the FIRST thing the boot does: a refused second daemon
/// must not have touched `colony.db` on its way to the refusal.
#[test]
fn a_refused_daemon_creates_no_colony_db_of_its_own() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());
    // No daemon has ever run here, so plant a live holder directly: the
    // bystander is a real process that really occupies its pid.
    let bystander = spawn_bystander();
    plant_lease(td.path(), bystander.pid(), start_id_of(bystander.pid()));

    let (status, stderr) = expect_refusal(td.path());

    assert!(
        !status.success(),
        "a held root must refuse the boot: {stderr}"
    );
    assert!(
        !td.path().join("colony.db").exists(),
        "the lease is taken before colony.db is opened — a refused boot must leave none"
    );
}

// ─── Proof 2: a dead holder is taken over, loudly ────────────────────────────

/// A hard kill (SIGKILL, power loss, OOM) leaves the lease behind. The next
/// boot must take it over after verifying the holder is gone — and say so.
#[test]
fn a_lease_from_a_dead_holder_is_taken_over_with_a_loud_log() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());

    // A real dead pid: spawn a child, kill it, reap it. After the reap the
    // kernel has no `/proc/{pid}` for it any more — exactly what a hard-killed
    // daemon leaves behind.
    let corpse = spawn_bystander();
    let dead_start_id = start_id_of(corpse.pid());
    let dead_pid = corpse.kill_and_reap();
    plant_lease(td.path(), dead_pid, dead_start_id);

    let daemon = spawn_daemon(td.path());
    let daemon_pid = daemon.pid();
    wait_until("the next boot to reclaim the dead lease", || {
        lease_pid(td.path()) == Some(daemon_pid)
    });

    // The takeover must be loud: an operator has to learn that a previous
    // colony did not shut down cleanly.
    wait_until("the reclaim to reach the log", || {
        log_contains(td.path(), "DEAD holder") && log_contains(td.path(), &dead_pid.to_string())
    });

    // And a SIGTERM must give the root back — a hard kill leaves no permanent
    // blocker, an orderly stop leaves nothing at all.
    wait_for_boot(td.path());
    let status = daemon.stop();
    assert!(status.success(), "SIGTERM is an orderly stop: {status:?}");
    assert!(
        !td.path().join(LEASE_DIR).exists(),
        "an orderly shutdown releases the root"
    );
}

/// A hard kill must not leave a permanent blocker: the boot after a SIGKILL
/// walks in.
#[test]
fn a_hard_killed_daemon_leaves_no_permanent_blocker() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());

    let first = spawn_daemon(td.path());
    let first_pid = first.pid();
    wait_until("the first daemon to publish its lease", || {
        lease_pid(td.path()) == Some(first_pid)
    });
    let killed_pid = first.kill_and_reap();
    assert_eq!(
        lease_pid(td.path()),
        Some(killed_pid),
        "SIGKILL runs no destructor: the stale lease is still there"
    );

    let second = spawn_daemon(td.path());
    let second_pid = second.pid();
    wait_until("the next boot to take the root", || {
        lease_pid(td.path()) == Some(second_pid)
    });
    wait_for_boot(td.path());
    assert!(
        second.stop().success(),
        "and it stops cleanly, leaving the root free again"
    );
    assert!(!td.path().join(LEASE_DIR).exists());
}

// ─── Proof 3: pid reuse decides on the start identity, not the pid ───────────

/// The case a bare pid file gets wrong. The lease names a pid that IS alive —
/// but the process wearing it now is a different one, started later. The
/// recorded start identity says so, and the boot proceeds.
///
/// The control below is the same test with the SAME live pid and the CORRECT
/// start identity: opposite decision. Together they prove the verdict rests on
/// the start identity and not on the pid's mere existence.
#[test]
fn a_recycled_pid_does_not_block_a_boot() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());

    let bystander = spawn_bystander();
    let reused_pid = bystander.pid();
    // The holder that wrote this lease started BEFORE the process now wearing
    // its pid — the signature of a recycled pid.
    let stale_start_id = start_id_of(reused_pid) - 1;
    plant_lease(td.path(), reused_pid, stale_start_id);

    let daemon = spawn_daemon(td.path());
    let daemon_pid = daemon.pid();
    wait_until("the boot to see through the recycled pid", || {
        lease_pid(td.path()) == Some(daemon_pid)
    });

    wait_until("the recycled-pid verdict to reach the log", || {
        log_contains(td.path(), "RECYCLED") && log_contains(td.path(), &reused_pid.to_string())
    });

    wait_for_boot(td.path());
    let _ = daemon.stop();
    assert!(
        process_status(reused_pid) != ProcStatus::Gone,
        "the bystander must have been left completely alone"
    );
}

/// The control: the very same live pid, this time with the start identity it
/// really has. Now the holder IS that process and the boot must refuse.
#[test]
fn the_same_live_pid_with_a_matching_start_id_does_block_the_boot() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());

    let bystander = spawn_bystander();
    let pid = bystander.pid();
    plant_lease(td.path(), pid, start_id_of(pid));

    let (status, stderr) = expect_refusal(td.path());

    assert!(
        !status.success(),
        "a matching start identity means the holder lives: {status:?}\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("pid {pid}")),
        "the refusal must name the holder pid {pid}, got:\n{stderr}"
    );
    assert_eq!(
        lease_pid(td.path()),
        Some(pid),
        "a refused boot must leave the holder's lease untouched"
    );
}

// ─── The lease is not a daemon-only affair ───────────────────────────────────

/// Track-Ruling G7-R2: `--validate` takes the lease too. It spawns no cell, but
/// `boot_load_or_scan` writes a full template index into `colony.db` whenever
/// the stored one is empty — a write into a live colony's database.
#[test]
fn validate_also_refuses_a_root_that_is_already_held() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());
    let bystander = spawn_bystander();
    plant_lease(td.path(), bystander.pid(), start_id_of(bystander.pid()));

    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--validate")
        .stdin(Stdio::null())
        .output()
        .expect("run meclaw --validate");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a held root refuses --validate too");
    assert!(
        stderr.contains("already running on this root"),
        "got:\n{stderr}"
    );
    assert!(
        !td.path().join("colony.db").exists(),
        "--validate must not have opened the database of a held root"
    );
}

/// `--sandbox-probe` stays exempt: it asks about the HOST, is routinely run in
/// a directory that is not a colony at all, and must keep working while a
/// daemon holds the root.
#[test]
fn the_sandbox_probe_is_not_gated_by_the_lease() {
    let td = tempfile::TempDir::new().unwrap();
    bootable_root(td.path());
    let bystander = spawn_bystander();
    plant_lease(td.path(), bystander.pid(), start_id_of(bystander.pid()));

    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--sandbox-probe")
        .stdin(Stdio::null())
        .output()
        .expect("run meclaw --sandbox-probe");

    assert!(
        out.status.success(),
        "a host question must not be blocked by a colony lease: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.stdout.is_empty(), "the probe report IS the answer");
    assert_eq!(
        lease_pid(td.path()),
        Some(bystander.pid()),
        "the probe must not have touched the lease"
    );
}
