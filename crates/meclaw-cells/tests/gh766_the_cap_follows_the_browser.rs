//! GH #766, R-G7: the ceiling follows the process -- and the service manager
//! is the one who writes it.
//!
//! The cell declares `params.sandbox.limits` and puts them on a cgroup it
//! creates itself. Measured against the packaged browser on this host,
//! 2026-09-20, five runs out of five: 46-62 ms after the spawn the child is no
//! longer in that cgroup. Its launcher hands it to the service manager, which
//! puts it in a scope of its own, and there the declared ceiling is not
//! enforced (finding B-G9, wave G).
//!
//! Writing that scope's cgroup files works, and it works only until somebody
//! reloads the service manager: the manager knows its scope with no cap
//! declared on it and writes that view back (measured 2026-09-20, strand
//! g11). A cgroup whose owner is systemd is systemd's to write, so the cell
//! stops writing it and SAYS it instead -- `set-property`, in the runtime
//! form, the same decision the container world made years ago between the
//! `cgroupfs` and the `systemd` cgroup driver.
//!
//! What this suite pins is that seam: a real transient unit of the real
//! service manager, the ceiling asked for through the manager, and the three
//! cgroup files read back. The live proof -- a real browser, a deliberately
//! small ceiling, an `oom_kill` in the report, and the two `daemon-reload`s
//! the ceiling survives -- is in the strand's own record; it needs a colony
//! and a browser, and a reload in a test run would strip the ceilings of
//! every other browser on the host.

use meclaw_cells::browser::cdp::{CapSite, death_detail};
use meclaw_cells::sandbox::{
    ResourceLimits, cgroup_delegation_supported, cgroup_of, delegated_root, follow_process,
};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The ceiling the shipped template declares (OR-G4, R-G10).
fn declared() -> ResourceLimits {
    ResourceLimits {
        memory_max_bytes: Some(2_000_000_000),
        pids_max: Some(512),
        cpu_max_percent: Some(200),
    }
}

/// The A-timeout the browser cell hands down (`params.external_timeout_ms`).
fn budget() -> Duration {
    Duration::from_millis(10_000)
}

/// A cgroup that is certainly not where the child is, so the "it stayed put"
/// shortcut cannot fire.
fn elsewhere() -> &'static Path {
    Path::new("/sys/fs/cgroup/meclaw-g12-somewhere-else")
}

/// A transient unit of the user's service manager, standing in for the scope a
/// launcher hands its child to -- and it IS one: same manager, same kind of
/// unit, same `app.slice` above it.
struct Transient(String);

impl Transient {
    /// `None` when there is no user service manager to ask, which is the only
    /// honest answer on a host that has none.
    fn start(tag: &str) -> Option<Self> {
        let name = format!("meclaw-g12-{tag}-{}", std::process::id());
        let ok = std::process::Command::new("systemd-run")
            .args(["--user", "--quiet", "--unit", &name, "/bin/sleep", "600"])
            .status()
            .ok()?
            .success();
        if !ok {
            return None;
        }
        Some(Self(format!("{name}.service")))
    }

    /// The process the manager started, once it has one.
    fn pid(&self) -> Option<u32> {
        for _ in 0..100 {
            let out = std::process::Command::new("systemctl")
                .args(["--user", "show", &self.0, "-p", "MainPID", "--value"])
                .output()
                .ok()?;
            let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
            if pid != 0 {
                return Some(pid);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// One property as the MANAGER sees it -- the view a `daemon-reload`
    /// writes back, and therefore the view that has to carry the ceiling.
    fn manager_says(&self, property: &str) -> String {
        let out = std::process::Command::new("systemctl")
            .args(["--user", "show", &self.0, "-p", property, "--value"])
            .output()
            .expect("systemctl show");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

impl Drop for Transient {
    fn drop(&mut self) {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", &self.0])
            .status();
    }
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file))
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_moved_is_capped_through_the_service_manager() {
    const T: &str = "a_child_that_moved_is_capped_through_the_service_manager";
    let Some(unit) = Transient::start("moved") else {
        eprintln!("[{T}] SKIPPED: no user service manager to ask (systemd-run --user failed)");
        return;
    };
    let Some(pid) = unit.pid() else {
        eprintln!("[{T}] SKIPPED: the transient unit never reported a main process");
        return;
    };
    let dir = cgroup_of(pid).expect("the unit's process sits in a cgroup v2 directory");

    assert_eq!(read(&dir, "memory.max"), "max", "nobody capped it yet");
    assert_eq!(unit.manager_says("MemoryMax"), "infinity");

    let witness = follow_process(pid, Some(elsewhere()), &declared(), budget())
        .await
        .expect("the manager places the ceiling on a unit of its own");

    // Page-rounded by the kernel, which is why the cell confirms a rounded
    // read rather than the byte it asked for.
    let seen: u64 = read(&dir, "memory.max")
        .parse()
        .expect("a number, not `max`");
    assert!(
        seen <= 2_000_000_000 && 2_000_000_000 - seen < 4096,
        "memory.max is the declared ceiling, page-rounded: {seen}"
    );
    assert_eq!(read(&dir, "pids.max"), "512");
    assert_eq!(read(&dir, "cpu.max"), "200000 100000");
    assert_eq!(read(&dir, "memory.swap.max"), "0", "no escape into swap");

    // The reason this way was taken at all: the ceiling is in the manager's
    // own view of the unit, which is what a `daemon-reload` re-applies. The
    // reload itself is measured in the strand's record -- doing it here would
    // strip the ceiling of every other browser on the host.
    assert_eq!(unit.manager_says("MemoryMax"), "2000000000");
    assert_eq!(unit.manager_says("TasksMax"), "512");
    assert_eq!(unit.manager_says("CPUQuotaPerSecUSec"), "2s");

    let (ancestor, _) = witness.expect("a child that moved leaves a witness behind");
    assert_eq!(
        ancestor,
        dir.parent().unwrap(),
        "the witness is the nearest cgroup that outlives the child's own"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_stayed_needs_no_second_write() {
    // The cgroup the cell created itself is not a unit and never was: nothing
    // is asked of the manager, because `create` already wrote those files and
    // no manager will overwrite a directory it does not own.
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("read x")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the child");
    let here = cgroup_of(child.id()).expect("a cgroup v2 host");
    let witness = follow_process(child.id(), Some(&here), &declared(), budget())
        .await
        .expect("a child in its own cgroup is no failure");
    assert!(
        witness.is_none(),
        "nothing moved, so nothing needs a witness: the cgroup the cell created \
         outlives the child and carries the counter"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cgroup_that_is_no_unit_of_the_manager_is_a_refusal() {
    const T: &str = "a_cgroup_that_is_no_unit_of_the_manager_is_a_refusal";
    if !cgroup_delegation_supported() {
        eprintln!(
            "[{T}] SKIPPED: no usable cgroup v2 delegation here (root {:?}), so there is no \
             way to make a cgroup that is not a unit -- retry under `systemd-run --user --scope`",
            delegated_root()
        );
        return;
    }
    let gone = Foreign::new("no-unit");
    let mut child = child_in(gone.path());

    let err = follow_process(child.id(), Some(elsewhere()), &declared(), budget())
        .await
        .expect_err("a ceiling nobody can be asked for is refused, never assumed");
    let said = err.to_string();
    assert!(
        said.contains("service manager") && said.contains("trusted"),
        "the refusal names who could not be asked and the one deliberate way out \
         (OR-G56): {said}"
    );
    assert_eq!(
        gone.read("memory.max"),
        "max",
        "and nothing was written anyway"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn the_kill_is_read_off_the_witness_once_the_childs_own_cgroup_is_gone() {
    // The measured shape: a scope created by the service manager is REMOVED
    // with its last process, so `memory.events` of the child's own cgroup is
    // gone exactly when the cell wants to read it (measured 2026-09-20: the
    // directory was already absent when the pipe closed). The ancestor's
    // counter is hierarchical and survives, so the cell remembers it and the
    // count it carried before the ceiling was written.
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("memory.events"),
        "low 0\nhigh 0\nmax 40\noom 3\noom_kill 16\n",
    )
    .expect("events");
    let site = CapSite {
        dir: td.path().join("a-scope-that-is-gone"),
        witness: Some((td.path().to_path_buf(), 15)),
    };
    let said = death_detail(Some(&site), "the browser's pipe closed");
    assert!(
        said.contains("oom_kill=1"),
        "one kill since the ceiling: {said}"
    );
    assert!(
        said.contains("memory_max_bytes"),
        "and it names the knob that did it: {said}"
    );
}

#[test]
fn a_witness_without_a_kill_says_nothing_extra() {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(td.path().join("memory.events"), "oom_kill 15\n").expect("events");
    let site = CapSite {
        dir: td.path().join("a-scope-that-is-gone"),
        witness: Some((td.path().to_path_buf(), 15)),
    };
    assert_eq!(
        death_detail(Some(&site), "the browser's pipe closed"),
        "the browser's pipe closed",
        "a browser that died of something else is not blamed on the ceiling"
    );
}

// ---- helpers that need a delegated cgroup -------------------------------

/// A cgroup nobody capped and no manager owns: a bare directory under the
/// delegated root.
struct Foreign(PathBuf);

impl Foreign {
    fn new(tag: &str) -> Self {
        let root = delegated_root().expect("a delegated root");
        // The pid in the name is what the module's own sweep reads, so a run
        // that dies before the drop leaves something the next one recognises.
        let dir = root.join(format!("meclaw-sbx-{}-{tag}", std::process::id()));
        std::fs::create_dir(&dir).expect("create the foreign cgroup");
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn read(&self, file: &str) -> String {
        read(&self.0, file)
    }
}

impl Drop for Foreign {
    fn drop(&mut self) {
        let kill = self.0.join("cgroup.kill");
        if kill.exists() {
            let _ = std::fs::write(&kill, b"1");
        }
        for _ in 0..50 {
            if std::fs::remove_dir(&self.0).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// A child that joins `dir` and then stays put until it is killed.
fn child_in(dir: &Path) -> std::process::Child {
    let procs = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("cgroup.procs"))
        .expect("open cgroup.procs of the foreign cgroup");
    let raw = procs.as_raw_fd();
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg("read x")
        .stdin(std::process::Stdio::piped());
    // SAFETY: one `write` of two constant bytes on an already-open descriptor,
    // no allocation -- safe in the forked child of a multithreaded parent.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(move || {
            let buf: [u8; 2] = *b"0\n";
            let n = libc::write(raw, buf.as_ptr() as *const libc::c_void, buf.len());
            if n < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().expect("spawn the child");
    drop(procs);
    child
}
