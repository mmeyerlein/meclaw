//! Resource caps through a delegated cgroup v2 sub-cgroup (GH #85).
//!
//! # Why this is a lifecycle and not a flag
//!
//! Landlock and the network namespace are properties of a process: one syscall
//! in `pre_exec` and they are true forever. A resource cap is not. It is a
//! directory under `/sys/fs/cgroup` that somebody has to create, fill, move the
//! child into and take away again, on every path including the ones nobody
//! walks deliberately. That is why the cap lives behind an RAII scope
//! ([`SandboxScope`]) rather than behind a flag.
//!
//! # Where the sub-cgroup goes
//!
//! An unprivileged process may only create cgroups inside a directory that was
//! delegated to it. Two candidates are tried, in this order:
//!
//! 1. the topmost writable ancestor of the daemon's own cgroup, which is what
//!    delegation looks like for a systemd user service (`Delegate=yes`) and
//!    inside a container;
//! 2. `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service`, the
//!    delegated root of a systemd user session.
//!
//! # The permission that is easy to miss
//!
//! Creating the directory and writing the caps is not enough. Moving a process
//! into a cgroup additionally requires write access to `cgroup.procs` of the
//! COMMON ANCESTOR of the source and the destination cgroup. Measured on the
//! target platform: a daemon started from an ssh login lives in
//! `user.slice/user-<uid>.slice/session-<n>.scope`, so the common ancestor with
//! `user@<uid>.service` is the root-owned `user-<uid>.slice` and the move fails
//! with `EACCES` even though the directory was created successfully. The same
//! daemon started as `systemctl --user` lives under `user@<uid>.service` and
//! the move succeeds. This is why [`cgroup_delegation_supported`] answers by
//! performing the whole sequence on a real child instead of by reading modes
//! off a directory.

use super::probe::CgroupDelegation;
use super::profile::ResourceLimits;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

/// The cgroup v2 mount point. Fixed by convention on every systemd host, and a
/// host that mounted it elsewhere simply reports no delegation.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Prefix of every directory this module creates. Carries the daemon's pid so
/// that a later run can tell its own leftovers from a live sibling's.
const DIR_PREFIX: &str = "meclaw-sbx-";

/// The `cpu.max` period, in microseconds. `cpu_max_percent` is expressed
/// against it, so 100 percent is one whole core.
const CPU_PERIOD_US: u64 = 100_000;

/// A created sub-cgroup, removed again when this value is dropped.
///
/// `Drop` is the only teardown that runs on every path: a finished child, an
/// error between creation and spawn, a cancelled task, a panicking peer,
/// colony shutdown. Anything still alive inside is killed first, because a
/// cgroup that a descendant kept alive can never be removed and a cap that a
/// descendant escapes is not a cap.
#[derive(Debug)]
pub struct SandboxScope {
    dir: Option<PathBuf>,
}

impl SandboxScope {
    /// A scope that owns nothing: what an uncapped profile gets.
    pub(crate) fn empty() -> Self {
        Self { dir: None }
    }
}

impl Drop for SandboxScope {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            destroy(&dir);
        }
    }
}

/// Kill whatever is left inside and remove the directory.
///
/// `cgroup.kill` (Linux 5.14+) is a best-effort first step: `rmdir` on a
/// cgroup that still holds a task fails with `EBUSY` forever, and the only
/// thing that can still be in there is a descendant that outlived the child we
/// spawned. Killing is also the honest reading of the cap: the cgroup exists
/// for exactly one sandboxed child, so nothing may outlive it.
///
/// The retry is bounded and short. Kill delivery is synchronous, process exit
/// is not, so the first `rmdir` after a kill can still see `EBUSY`.
fn destroy(dir: &Path) {
    // Only ever WRITE the control file, never create it: on a kernel below
    // 5.14 -- or on any directory that is not a cgroup at all -- creating it
    // would leave behind exactly the entry that makes the `rmdir` below
    // impossible.
    let kill = dir.join("cgroup.kill");
    if kill.exists() {
        let _ = std::fs::write(&kill, b"1");
    }
    for _ in 0..20 {
        match std::fs::remove_dir(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(1)),
        }
    }
}

/// The delegated cgroup root this process may create sub-cgroups in, or `None`
/// when there is none.
///
/// Cheap and side-effect free: it creates nothing. It also proves nothing --
/// a writable directory is necessary but not sufficient, see the module docs.
pub fn delegated_root() -> Option<PathBuf> {
    // A cgroup v1 or cgroup-less host has no `cgroup.controllers` here.
    if !Path::new(CGROUP_ROOT).join("cgroup.controllers").is_file() {
        return None;
    }
    root_candidates()
        .into_iter()
        .find(|c| is_writable_dir(c) && has_enabled_controllers(c))
}

/// The two places a delegated root can be, in preference order.
fn root_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(rel) = own_cgroup_path() {
        // The topmost writable ancestor: everything below it is ours too, but
        // the top is where systemd enabled the controllers.
        let mut cur = PathBuf::from(CGROUP_ROOT);
        for comp in rel.split('/').filter(|c| !c.is_empty()) {
            cur = cur.join(comp);
            if is_writable_dir(&cur) {
                out.push(cur.clone());
                break;
            }
        }
    }
    // SAFETY: `getuid` reads a field of the calling process's credentials and
    // cannot fail.
    let uid = unsafe { libc::getuid() };
    out.push(
        PathBuf::from(CGROUP_ROOT)
            .join("user.slice")
            .join(format!("user-{uid}.slice"))
            .join(format!("user@{uid}.service")),
    );
    out
}

/// This process's cgroup v2 path, relative to the mount point.
///
/// `/proc/self/cgroup` carries one `0::<path>` line for the unified hierarchy.
fn own_cgroup_path() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    raw.lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(|p| p.trim().to_string())
}

/// Whether this process may create a directory inside `path`.
fn is_writable_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a live NUL-terminated string for the duration of the call
    // and `access` writes nothing.
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

/// Whether a candidate root hands any controller down to its children. Without
/// an enabled controller a child cgroup has no `memory.max` to write.
fn has_enabled_controllers(path: &Path) -> bool {
    std::fs::read_to_string(path.join("cgroup.subtree_control"))
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Whether this host lets an unprivileged process cap a child through cgroup v2.
///
/// Answered by doing the whole thing: create a sub-cgroup, write a cap, spawn
/// `/bin/sh -c :` that moves itself in before `exec`, and clean up. Directory
/// modes, controller lists and the common-ancestor permission each have their
/// own way of saying no, and checking them all is less reliable than one
/// attempt. Blocking, so this belongs in a test or a validation pass, never in
/// an async hot path.
pub fn cgroup_delegation_supported() -> bool {
    matches!(delegation_probe(), CgroupDelegation::Delegated { .. })
}

/// The same probe, but keeping what it learned (GH #97).
///
/// `cgroup_delegation_supported` throws the reason away, and the reason is the
/// interesting half: an absent mechanism and a wrong launch both answer
/// `false`, and only one of them is fixed by starting the daemon differently.
/// Every failing step of the sequence therefore gets its own variant. See
/// [`CgroupDelegation`] and the module docs for the measurement behind it.
///
/// Blocking, and it forks a child: a validation path or a test, never an async
/// hot path.
pub fn delegation_probe() -> CgroupDelegation {
    let Some(root) = delegated_root() else {
        return CgroupDelegation::NoDelegatedRoot;
    };
    let limits = ResourceLimits {
        pids_max: Some(64),
        ..ResourceLimits::default()
    };
    let (_scope, fd) = match create(&limits) {
        Ok(v) => v,
        Err(e) => {
            return CgroupDelegation::SetupFailed {
                reason: e.to_string(),
            };
        }
    };
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(":")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let raw = fd.as_raw_fd();
    // SAFETY: `join_via` performs one `write` on an already-open descriptor and
    // allocates nothing, so it is safe in the forked child of a multithreaded
    // parent. `fd` outlives the call because it is dropped after `status()`.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(move || join_via(raw));
    }
    match cmd.status() {
        Ok(st) if st.success() => CgroupDelegation::Delegated {
            root: root.display().to_string(),
        },
        // A `pre_exec` that returns `Err` travels back to the parent as the
        // spawn error, so THIS is where the common-ancestor `EACCES` surfaces.
        Err(e) => CgroupDelegation::MoveRefused {
            permission_denied: matches!(e.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)),
            reason: e.to_string(),
        },
        // The child started but did not exit cleanly. Nothing in `/bin/sh -c :`
        // can do that, so it is reported rather than folded into a success.
        Ok(st) => CgroupDelegation::MoveRefused {
            permission_denied: false,
            reason: format!("the probe child exited {st}"),
        },
    }
}

/// Create a sub-cgroup carrying `limits` and return it together with a write
/// descriptor on its `cgroup.procs`.
///
/// The descriptor is opened HERE, in the parent, with the parent's
/// credentials. That is not an optimisation: the kernel checks a `cgroup.procs`
/// write against the credentials the file was opened with, and it keeps the
/// post-fork half down to a single `write` on integer arguments.
pub(crate) fn create(limits: &ResourceLimits) -> io::Result<(SandboxScope, OwnedFd)> {
    let root = delegated_root().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "params.sandbox.limits declares resource caps but this host delegates no writable \
             cgroup v2 directory (a systemd user session delegates \
             /sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service); refusing to run \
             the cell uncapped",
        )
    })?;
    sweep_stale_once(&root);

    let dir = root.join(format!(
        "{DIR_PREFIX}{}-{}",
        std::process::id(),
        meclaw_core::Uuid::now_v7().simple()
    ));
    std::fs::create_dir(&dir)?;
    // From here on the directory is owned: every `?` below unwinds through the
    // scope's `Drop` and takes it away again.
    let scope = SandboxScope {
        dir: Some(dir.clone()),
    };

    if let Some(bytes) = limits.memory_max_bytes {
        write_cap(&dir, "memory.max", &bytes.to_string())?;
        // A memory cap a process can escape into swap is not a cap. Best
        // effort: the file is absent when the kernel was built without swap
        // accounting, and then there is no swap to escape into either.
        let _ = std::fs::write(dir.join("memory.swap.max"), b"0");
    }
    if let Some(n) = limits.pids_max {
        write_cap(&dir, "pids.max", &n.to_string())?;
    }
    if let Some(pct) = limits.cpu_max_percent {
        let quota = pct.saturating_mul(CPU_PERIOD_US) / 100;
        write_cap(&dir, "cpu.max", &format!("{quota} {CPU_PERIOD_US}"))?;
    }

    let procs = std::ffi::CString::new(dir.join("cgroup.procs").as_os_str().as_encoded_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `procs` is a live NUL-terminated string for the duration of the
    // call; the return value is checked before it becomes an `OwnedFd`.
    let raw = unsafe { libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a fresh, valid, unowned descriptor; `OwnedFd` takes sole
    // ownership of it here.
    Ok((scope, unsafe { OwnedFd::from_raw_fd(raw) }))
}

/// Write one cap file, translating the two failures an operator can act on.
fn write_cap(dir: &Path, file: &str, value: &str) -> io::Result<()> {
    let path = dir.join(file);
    if !path.exists() {
        let controller = file.split('.').next().unwrap_or(file);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "the delegated cgroup root {} does not hand the {controller} controller down to \
                 its children (add it to that directory's cgroup.subtree_control); refusing to \
                 run the cell with a cap nobody enforces",
                dir.parent().unwrap_or(dir).display()
            ),
        ));
    }
    std::fs::write(&path, value.as_bytes()).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("writing {value:?} to {}: {e}", path.display()),
        )
    })
}

/// Move the calling process into the cgroup behind `fd`.
///
/// Runs post-fork: one `write` of two constant bytes, no allocation, no
/// formatting. `"0"` is the kernel's spelling of "the process doing the write".
pub(crate) fn join_via(fd: std::os::fd::RawFd) -> io::Result<()> {
    let buf: [u8; 2] = *b"0\n";
    // SAFETY: `fd` is an open write descriptor on a `cgroup.procs` file and
    // `buf` is a live stack buffer of the length passed.
    let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Remove the sub-cgroups of daemons that are gone, once per process.
///
/// A crash, a `SIGKILL` or a power loss leaves a directory behind that nothing
/// will ever remove on its own. The pid in the name is what makes the sweep
/// safe: a directory is only touched when `/proc/<pid>` no longer exists, so a
/// live sibling daemon's freshly created and not yet populated cgroup is never
/// mistaken for rubbish.
fn sweep_stale_once(root: &Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| sweep_stale(root));
}

/// The sweep itself, separated out so a test can call it deterministically.
pub(crate) fn sweep_stale(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(DIR_PREFIX) else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        destroy(&entry.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_becomes_a_quota_against_the_fixed_period() {
        // The arithmetic the operator's percentage turns into, spelled out so a
        // later change to the period cannot silently rescale every cap.
        assert_eq!(50 * CPU_PERIOD_US / 100, 50_000);
        assert_eq!(100 * CPU_PERIOD_US / 100, 100_000);
        assert_eq!(200 * CPU_PERIOD_US / 100, 200_000);
    }

    #[test]
    fn a_candidate_root_is_never_the_mount_point_itself() {
        // The unified root is root-owned everywhere; offering it would only
        // produce a confusing EACCES deeper in.
        for c in root_candidates() {
            assert_ne!(c, Path::new(CGROUP_ROOT));
        }
    }

    #[test]
    fn the_sweep_ignores_directories_it_did_not_make() {
        let td = tempfile::TempDir::new().unwrap();
        let foreign = td.path().join("some-other-thing");
        let live = td
            .path()
            .join(format!("{DIR_PREFIX}{}-abc", std::process::id()));
        std::fs::create_dir(&foreign).unwrap();
        std::fs::create_dir(&live).unwrap();
        sweep_stale(td.path());
        assert!(foreign.is_dir(), "a foreign directory is never touched");
        assert!(
            live.is_dir(),
            "a directory whose daemon is still alive is never touched"
        );
    }

    #[test]
    fn the_sweep_removes_what_a_dead_daemon_left_behind() {
        let td = tempfile::TempDir::new().unwrap();
        // A pid that cannot be live: above `pid_max` on every Linux.
        let stale = td.path().join(format!("{DIR_PREFIX}4294967290-abc"));
        std::fs::create_dir(&stale).unwrap();
        sweep_stale(td.path());
        assert!(!stale.exists(), "a dead daemon's cgroup is swept");
    }

    /// Skip guard for the tests that need a real delegated cgroup. Prints the
    /// reason, so a green run on a host without delegation is not mistaken for
    /// a proof.
    fn have_delegation(test: &str) -> bool {
        if cgroup_delegation_supported() {
            return true;
        }
        eprintln!(
            "[{test}] SKIPPED: no usable cgroup v2 delegation here (root {:?}); an ssh session \
             scope cannot move a process into user@<uid>.service -- retry under \
             `systemd-run --user --scope`",
            delegated_root()
        );
        false
    }

    #[test]
    fn a_scope_takes_its_directory_away_again() {
        const T: &str = "a_scope_takes_its_directory_away_again";
        if !have_delegation(T) {
            return;
        }
        let limits = ResourceLimits {
            pids_max: Some(32),
            memory_max_bytes: Some(64 * 1024 * 1024),
            cpu_max_percent: Some(50),
            // all three, so that every cap file this module writes is exercised
        };
        let (scope, _fd) = create(&limits).expect("a delegated host can create the cgroup");
        let dir = scope.dir.clone().expect("a capped scope owns a directory");
        assert!(dir.is_dir(), "the cgroup exists while the scope lives");
        assert_eq!(
            std::fs::read_to_string(dir.join("pids.max"))
                .unwrap()
                .trim(),
            "32",
            "and it carries the declared cap"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("cpu.max")).unwrap().trim(),
            "50000 100000",
            "the percentage became a quota against the fixed period"
        );
        drop(scope);
        assert!(!dir.exists(), "and it is gone once the scope is dropped");
    }

    #[test]
    fn a_real_child_joins_the_cgroup_and_the_directory_outlives_neither() {
        const T: &str = "a_real_child_joins_the_cgroup_and_the_directory_outlives_neither";
        if !have_delegation(T) {
            return;
        }
        let limits = ResourceLimits {
            pids_max: Some(32),
            ..ResourceLimits::default()
        };
        let (scope, fd) = create(&limits).expect("create");
        let dir = scope.dir.clone().expect("a capped scope owns a directory");

        // The child prints its own cgroup line, which is the positive receipt
        // that the move happened: reading `cgroup.procs` from here would race
        // the child's exit.
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("cat /proc/self/cgroup");
        let raw = fd.as_raw_fd();
        // SAFETY: `join_via` performs one `write` on an already-open descriptor
        // and allocates nothing, so it is safe in the forked child.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(move || join_via(raw));
        }
        let out = cmd.output().expect("spawn");
        let seen = String::from_utf8_lossy(&out.stdout);
        let name = dir.file_name().unwrap().to_str().unwrap();
        assert!(
            seen.contains(name),
            "the child must report itself inside {name}, got {seen:?}"
        );

        drop(fd);
        drop(scope);
        assert!(
            !dir.exists(),
            "and the cgroup of a finished child is removed, not leaked"
        );
    }

    #[test]
    fn the_delegation_probe_agrees_with_itself() {
        // Whatever this host answers, the two halves must not contradict: a
        // successful probe implies a discovered root.
        if cgroup_delegation_supported() {
            assert!(delegated_root().is_some());
        }
    }
}
