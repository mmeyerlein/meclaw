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
//! A candidate counts when it hands controllers down to its children, and it
//! also counts when it merely OFFERS them (GH #686): a system unit with
//! `Delegate=yes` gets its directory handed over with `cgroup.controllers`
//! filled and `cgroup.subtree_control` empty, because enabling is the
//! delegatee's own step under cgroup v2. The daemon takes that step itself
//! ([`adopt_root`]). The kernel enables controllers only in a directory
//! without processes, so a daemon that is the only process there -- a unit
//! without `DelegateSubgroup=` -- first moves itself one level down into
//! [`DAEMON_DIR`], which stays inside the delegated boundary. That directory
//! carries the daemon itself and is never swept. A transient user
//! (`DynamicUser=yes`) has no `user@<uid>.service`, so for it the first
//! candidate is the only one, and before this step it fell through.
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

/// Where the daemon moves itself when it has to leave a delegated directory
/// empty before enabling controllers in it. Not a `DIR_PREFIX` directory: it
/// carries the daemon, and the sweep never touches it.
const DAEMON_DIR: &str = "meclaw-daemon";

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

    /// The cgroup this scope created, when it created one.
    ///
    /// Read by a caller that has to ask whether its child is still IN it
    /// ([`follow_process`]). Nobody may write through it: the caps are this
    /// module's to write, and the directory is this value's to remove.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
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
/// Cheap once a root is adopted: it creates no sub-cgroup. The one side effect
/// it may have is the adoption itself (GH #686) -- enabling the offered
/// controllers in a delegated directory, after moving this process into
/// [`DAEMON_DIR`] when it was the only one there -- and that happens once. It
/// proves nothing either way: a writable directory is necessary but not
/// sufficient, see the module docs.
pub fn delegated_root() -> Option<PathBuf> {
    // A cgroup v1 or cgroup-less host has no `cgroup.controllers` here.
    if !Path::new(CGROUP_ROOT).join("cgroup.controllers").is_file() {
        return None;
    }
    root_candidates().into_iter().find(|c| adopt_root(c))
}

/// A writable delegated directory whose controllers nobody enabled yet becomes
/// a root by enabling them -- the delegatee's own step under cgroup v2
/// (GH #686). A directory that already hands controllers down is taken as it
/// is; one that is not writable or offers nothing is not a root.
fn adopt_root(path: &Path) -> bool {
    if !is_writable_dir(path) {
        return false;
    }
    if has_enabled_controllers(path) {
        return true;
    }
    if offered_controllers(path).is_empty() {
        return false;
    }
    if enable_controllers(path).is_ok() {
        return true;
    }
    evacuate_self(path).is_ok() && enable_controllers(path).is_ok()
}

/// Controllers a delegated directory offers, enabled or not.
fn offered_controllers(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path.join("cgroup.controllers"))
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Enable every offered controller this module writes caps for.
fn enable_controllers(path: &Path) -> io::Result<()> {
    let wanted = ["cpu", "memory", "pids"];
    let line: Vec<String> = offered_controllers(path)
        .into_iter()
        .filter(|c| wanted.contains(&c.as_str()))
        .map(|c| format!("+{c}"))
        .collect();
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no usable controller offered",
        ));
    }
    std::fs::write(path.join("cgroup.subtree_control"), line.join(" "))
}

/// The kernel enables controllers only on a directory without processes: when
/// this daemon is the only one in it, it moves itself one level down first.
///
/// "The only one" means exactly that -- this process is in the directory and
/// nobody else is. A directory the daemon does not stand in (the user session
/// root, or its unit directory once `DelegateSubgroup=` put it a level down)
/// is never adopted this way: moving out of its own unit cgroup would take
/// the daemon out from under `systemctl stop`.
fn evacuate_self(path: &Path) -> io::Result<()> {
    let procs = std::fs::read_to_string(path.join("cgroup.procs"))?;
    let me = std::process::id().to_string();
    let lines: Vec<&str> = procs
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !lines.iter().any(|l| *l == me) {
        return Err(io::Error::other(
            "this process does not stand in the delegated directory",
        ));
    }
    if lines.iter().any(|l| *l != me) {
        return Err(io::Error::other(
            "the delegated directory holds other processes",
        ));
    }
    let own = path.join(DAEMON_DIR);
    if !own.is_dir() {
        std::fs::create_dir(&own)?;
    }
    std::fs::write(own.join("cgroup.procs"), me)
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

    write_limits(&dir, limits)?;

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

/// Write every declared cap into `dir`.
///
/// Only for a directory this module CREATED. It sits inside a delegated root,
/// so no service manager owns it and nothing will ever reconcile it away.
/// Where the owner is systemd -- the unit a re-homed child lands in -- the
/// ceiling is asked for instead ([`tell_the_manager`]), because a written
/// value there survives only until the next `daemon-reload`.
fn write_limits(dir: &Path, limits: &ResourceLimits) -> io::Result<()> {
    if let Some(bytes) = limits.memory_max_bytes {
        write_cap(dir, "memory.max", &bytes.to_string())?;
        // A memory cap a process can escape into swap is not a cap. Best
        // effort: the file is absent when the kernel was built without swap
        // accounting, and then there is no swap to escape into either.
        let _ = std::fs::write(dir.join("memory.swap.max"), b"0");
    }
    if let Some(n) = limits.pids_max {
        write_cap(dir, "pids.max", &n.to_string())?;
    }
    if let Some(pct) = limits.cpu_max_percent {
        write_cap(dir, "cpu.max", &cpu_quota(pct))?;
    }
    Ok(())
}

/// `cpu.max` for a percentage of one core, against the fixed period.
fn cpu_quota(percent: u64) -> String {
    format!(
        "{} {CPU_PERIOD_US}",
        percent.saturating_mul(CPU_PERIOD_US) / 100
    )
}

// ---- the ceiling is the service manager's to write (GH #766, R-G7) -------

/// Which service manager owns the unit a child ended up in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Manager {
    /// The calling user's own manager (`systemctl --user`).
    User,
    /// The system manager, which an unprivileged daemon will simply be
    /// refused by -- and a refusal is the right answer then.
    System,
}

impl Manager {
    fn flag(self) -> &'static str {
        match self {
            Manager::User => "--user",
            Manager::System => "--system",
        }
    }
}

/// Which unit of which manager carries `dir`, or why none does.
///
/// Two refusals, and both are the honest answer rather than a fallback:
///
/// * `dir` is no unit. A cgroup below a delegated unit belongs to whoever was
///   delegated it, and there is nobody to ask for a ceiling on it.
/// * `dir` is an ANCESTOR of this process's own cgroup. Such a unit exists and
///   the manager would cap it without complaint -- and the cap would land on
///   the colony too. R-G7 is "the browser never displaces the colony"; capping
///   the colony to keep the browser small fails it precisely. Measured in
///   strand g11: the parent of the re-homed browser's scope is `app.slice`,
///   where the colony runs.
fn unit_for(dir: &Path) -> io::Result<(Manager, String)> {
    let refuse = |why: String| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{why}; params.sandbox.limits declares a ceiling that no service manager can \
                 be asked for, and an unasked ceiling is no ceiling -- declare params.sandbox \
                 {{\"trust\": \"trusted\"}} to run without one"
            ),
        )
    };

    if let Some(own) = cgroup_of(std::process::id())
        && own.starts_with(dir)
    {
        return Err(refuse(format!(
            "the child sits in {}, which is the colony's own cgroup or an ancestor of it; \
             a ceiling there would cap the colony as well",
            dir.display()
        )));
    }

    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    if !(name.ends_with(".scope") || name.ends_with(".service") || name.ends_with(".slice")) {
        return Err(refuse(format!(
            "the child sits in {}, which is not a unit of any service manager",
            dir.display()
        )));
    }

    // A path under this user's own `user@<uid>.service` is the user manager's;
    // anything else is the system manager's, and an unprivileged daemon will
    // be refused there -- which is a refusal, not a second mechanism.
    // SAFETY: `geteuid` reads a process property and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let mine = format!("user@{uid}.service");
    let manager = if dir.iter().any(|c| c == mine.as_str()) {
        Manager::User
    } else {
        Manager::System
    };
    Ok((manager, name))
}

/// Ask the service manager to put `limits` on the unit that carries `dir`.
///
/// # Why this is said and not written
///
/// Writing the three cgroup files works, and it holds exactly until somebody
/// runs `systemctl daemon-reload`: the manager knows its own unit with no cap
/// declared on it and writes that view back, after which the files read `max`,
/// `38416` and `max` again and nothing notices (measured 2026-09-20, strand
/// g11). A node whose owner is systemd is systemd's to write. That is the same
/// question the container world answered years ago when it chose the `systemd`
/// cgroup driver over `cgroupfs`, and this is the same answer.
///
/// `--runtime` so the ceiling lives in `/run` and never lands on disk: it
/// belongs to this browser, not to the host's configuration.
///
/// The call costs 7-18 ms against 1-3 ms for the four file writes (measured
/// 2026-09-20, five runs each, packaged browser). The cell pays it inside the
/// window it already waits in -- the child re-homes after 41-83 ms and the
/// sandbox verdict falls at 159-194 ms -- so it buys the reload with time it
/// was spending anyway.
///
/// # What it does NOT prove
///
/// The manager applies a property to the cgroup best-effort and answers the
/// call either way. So this says nothing on its own, and the caller's
/// read-back ([`confirm_limits`]) is what turns "we asked" into "it is there".
async fn tell_the_manager(
    dir: &Path,
    limits: &ResourceLimits,
    budget: std::time::Duration,
) -> io::Result<()> {
    let (manager, unit) = unit_for(dir)?;

    let mut props: Vec<String> = Vec::new();
    if let Some(bytes) = limits.memory_max_bytes {
        props.push(format!("MemoryMax={bytes}"));
        // A memory cap a process can escape into swap is not a cap. In the
        // same call, not beside it: the manager takes the ceiling whole or
        // the ceiling is not there.
        props.push("MemorySwapMax=0".to_string());
    }
    if let Some(n) = limits.pids_max {
        props.push(format!("TasksMax={n}"));
    }
    if let Some(pct) = limits.cpu_max_percent {
        props.push(format!("CPUQuota={pct}%"));
    }
    if props.is_empty() {
        return Ok(());
    }

    let mut cmd = tokio::process::Command::new("systemctl");
    cmd.arg(manager.flag())
        .arg("--runtime")
        .arg("set-property")
        .arg(&unit)
        .args(&props)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);

    let out = match tokio::time::timeout(budget, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "the service manager could not be asked for a ceiling on {unit} \
                     (running systemctl: {e}); declare params.sandbox {{\"trust\": \
                     \"trusted\"}} to run without one"
                ),
            ));
        }
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the service manager did not answer within {} ms when asked for a ceiling \
                     on {unit}; params.sandbox.limits cannot be confirmed, and an unconfirmed \
                     ceiling is no ceiling",
                    budget.as_millis()
                ),
            ));
        }
    };
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::other(format!(
            "the service manager refused the ceiling on {unit} ({}): {}; declare \
             params.sandbox {{\"trust\": \"trusted\"}} to run without one",
            out.status,
            said.trim()
        )));
    }
    Ok(())
}

// ---- the cap follows the process (GH #766, R-G7) -------------------------

/// A cgroup that outlives a child's own, and the `oom_kill` count it carried
/// before the cap was written.
///
/// The counter in `memory.events` is hierarchical, so an ancestor answers for
/// its descendants; the baseline is what makes the answer about THIS child.
pub type OomWitness = (PathBuf, u64);

/// The cgroup `pid` sits in, under cgroup v2, or `None` on a host that
/// publishes none.
///
/// `/proc/<pid>/cgroup` is one `0::<path>` line on a v2 host, and the absolute
/// path is that under the v2 mount.
pub fn cgroup_of(pid: u32) -> Option<PathBuf> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let relative = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?
        .trim()
        .trim_start_matches('/');
    Some(Path::new(CGROUP_ROOT).join(relative))
}

/// How many times the kernel OOM-killed something in `dir` or below it.
pub fn oom_kills(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join("memory.events")).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("oom_kill "))
        .and_then(|n| n.trim().parse().ok())
}

/// Put `limits` on the cgroup `pid` REALLY sits in, and hand back the witness
/// that will still answer once that cgroup is gone.
///
/// # Why a cap written before the spawn is not always a cap
///
/// [`create`] writes the caps on a directory of ours and the child joins it
/// before `exec`. That holds for a child that stays where it was put. It does
/// not hold for a child that re-homes itself, and a launcher that hands its
/// process to the service manager does exactly that. Measured on the target
/// platform, 2026-09-20, five runs out of five: 46-62 ms after the spawn the
/// browser is in a scope of the service manager's making, and there
/// `memory.max` reads `max`, `pids.max` `38416`, `cpu.max` `max` — the whole
/// declared ceiling, gone (finding B-G9, wave G). Nothing about that is one
/// package's peculiarity; it is what any process that joins another cgroup
/// after `exec` looks like from here.
///
/// So the ceiling follows the process: the caller says WHEN it is worth
/// looking — after the child chain has settled, which is the same moment it
/// already waits for — and this reads where the child went and writes the
/// ceiling there.
///
/// # Who writes it
///
/// Not this function. The cgroup a launcher hands its child to is a unit of
/// the service manager, and the manager wins every reconciliation: writing
/// the three files directly held over the child's life and was wiped by the
/// next `systemctl --user daemon-reload`, which re-applies a unit view that
/// declares no cap (measured 2026-09-20, strand g11). So the ceiling is ASKED
/// FOR, through `set-property` in the runtime form ([`tell_the_manager`]),
/// and then read back. Measured 2026-09-20 on the packaged browser: the three
/// files carry the ceiling and still carry it after two `daemon-reload`s.
///
/// # Fail-closed
///
/// Every failure is a refusal, never a shrug: not being able to see where the
/// child is, not being allowed to write the ceiling there, a read-back that
/// does not agree, and a child that moved on again while we wrote. The
/// operator's one deliberate way past all of them is an explicitly written
/// `{"trust": "trusted"}` (OR-G56), which declares no ceiling and so never
/// reaches here.
pub async fn follow_process(
    pid: u32,
    owned: Option<&Path>,
    limits: &ResourceLimits,
    budget: std::time::Duration,
) -> io::Result<Option<OomWitness>> {
    let actual = cgroup_of(pid).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "cannot read which cgroup the child sits in (/proc/{pid}/cgroup); \
                 params.sandbox.limits declares a ceiling that cannot be placed, and an \
                 unplaced ceiling is no ceiling — declare params.sandbox {{\"trust\": \
                 \"trusted\"}} to run without one"
            ),
        )
    })?;
    if Some(actual.as_path()) == owned {
        // The child stayed in the cgroup it was spawned into. That directory
        // is ours and outlives the child, so it is its own witness.
        return Ok(None);
    }

    // Before the write, because a kill can only happen once the ceiling is
    // there: a baseline taken afterwards could miss the first one.
    let witness = actual
        .parent()
        .filter(|p| p.join("memory.events").is_file())
        .map(|p| (p.to_path_buf(), oom_kills(p).unwrap_or(0)));

    tell_the_manager(&actual, limits, budget).await?;
    // The manager answers the call whether or not the property reached the
    // cgroup, so this read is not a belt on braces: it is the only place the
    // ceiling becomes a fact.
    confirm_limits(&actual, limits)?;

    // It could have moved on while we wrote. Then the ceiling is on a cgroup
    // it no longer sits in, which is the defect this function exists for.
    match cgroup_of(pid) {
        Some(now) if now == actual => Ok(witness),
        other => Err(io::Error::other(format!(
            "the child moved out of {} again while its ceiling was being written (it is in {:?} \
             now); params.sandbox.limits cannot be enforced on a moving target",
            actual.display(),
            other.as_ref().map(|p| p.display().to_string())
        ))),
    }
}

/// Read every written cap back and refuse anything that does not agree.
///
/// A write to a cgroup file can succeed and mean nothing: a controller the
/// parent does not hand down, a value the kernel clamped, a directory that is
/// not the one we think. The read-back is what turns "we wrote it" into "it is
/// there".
fn confirm_limits(dir: &Path, limits: &ResourceLimits) -> io::Result<()> {
    if let Some(bytes) = limits.memory_max_bytes {
        let seen = read_cap(dir, "memory.max")?;
        // The kernel rounds a memory cap DOWN to a whole page, so the byte we
        // asked for is not the byte we read: 2 000 000 000 comes back as
        // 1 999 998 976 on a 4 KiB page. Anything further off — `max` above
        // all — is a cap that is not there.
        let got: u64 = seen.parse().map_err(|_| {
            io::Error::other(format!(
                "memory.max in {} reads {seen:?} after the ceiling was written; \
                 params.sandbox.limits.memory_max_bytes is not enforced there",
                dir.display()
            ))
        })?;
        if got > bytes || bytes - got >= page_size() {
            return Err(io::Error::other(format!(
                "memory.max in {} reads {got} after {bytes} was written; \
                 params.sandbox.limits.memory_max_bytes is not enforced there",
                dir.display()
            )));
        }
    }
    if let Some(n) = limits.pids_max {
        confirm_exact(dir, "pids.max", &n.to_string(), "pids_max")?;
    }
    if let Some(pct) = limits.cpu_max_percent {
        confirm_exact(dir, "cpu.max", &cpu_quota(pct), "cpu_max_percent")?;
    }
    Ok(())
}

/// One cap file, trimmed.
fn read_cap(dir: &Path, file: &str) -> io::Result<String> {
    Ok(std::fs::read_to_string(dir.join(file))?.trim().to_string())
}

/// A cap the kernel stores verbatim: it reads back as it was written or it is
/// not enforced.
fn confirm_exact(dir: &Path, file: &str, want: &str, knob: &str) -> io::Result<()> {
    let seen = read_cap(dir, file)?;
    if seen != want {
        return Err(io::Error::other(format!(
            "{file} in {} reads {seen:?} after {want:?} was written; \
             params.sandbox.limits.{knob} is not enforced there",
            dir.display()
        )));
    }
    Ok(())
}

/// The page a memory cap is rounded down to.
fn page_size() -> u64 {
    // SAFETY: `sysconf` reads a static system parameter and writes nothing.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as u64 } else { 4096 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three shapes of "which unit carries this cgroup", decided without
    /// touching a manager. Pure, so it runs on every host.
    #[test]
    fn a_cgroup_below_a_unit_belongs_to_nobody_we_can_ask() {
        // SAFETY: `geteuid` reads a process property and cannot fail.
        let uid = unsafe { libc::geteuid() };
        let app = PathBuf::from(format!(
            "{CGROUP_ROOT}/user.slice/user-{uid}.slice/user@{uid}.service/app.slice"
        ));

        let scope = app.join("snap.chromium.chromium-abc.scope");
        assert_eq!(
            unit_for(&scope).expect("a scope of the user manager"),
            (
                Manager::User,
                "snap.chromium.chromium-abc.scope".to_string()
            )
        );

        let system = PathBuf::from(format!("{CGROUP_ROOT}/system.slice/something.service"));
        assert_eq!(
            unit_for(&system).expect("a unit of the system manager"),
            (Manager::System, "something.service".to_string())
        );

        // A directory INSIDE a delegated unit: the delegatee owns it, and no
        // manager has a name for it.
        let leaf = app.join("some.scope").join("meclaw-sbx-1-abc");
        let err = unit_for(&leaf).expect_err("a cgroup that is not a unit is a refusal");
        let said = err.to_string();
        assert!(
            said.contains("not a unit") && said.contains("trusted"),
            "the refusal names what is missing and the one deliberate way out (OR-G56): {said}"
        );
    }

    /// R-G7 read backwards: the ceiling may never land on a cgroup the colony
    /// itself sits in or below. Such a unit exists and the manager would cap
    /// it without a word.
    #[test]
    fn an_ancestor_of_our_own_cgroup_is_refused_by_name() {
        let Some(own) = cgroup_of(std::process::id()) else {
            eprintln!("[an_ancestor_of_our_own_cgroup_is_refused_by_name] SKIPPED: no cgroup v2");
            return;
        };
        for dir in std::iter::successors(Some(own.as_path()), |p| p.parent())
            .take_while(|p| p.starts_with(CGROUP_ROOT))
        {
            let err = unit_for(dir).expect_err("our own cgroup and its ancestors are refused");
            assert!(
                err.to_string().contains("the colony"),
                "the refusal says whose ceiling it would have been: {err}"
            );
        }
    }

    /// A unit nobody knows: the manager says so, and the cell passes that on
    /// instead of shrugging. No delegation needed, and the unit name cannot
    /// collide with a real one.
    #[tokio::test]
    async fn a_unit_the_manager_does_not_know_is_a_refusal() {
        const T: &str = "a_unit_the_manager_does_not_know_is_a_refusal";
        // SAFETY: `geteuid` reads a process property and cannot fail.
        let uid = unsafe { libc::geteuid() };
        let dir = PathBuf::from(format!(
            "{CGROUP_ROOT}/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/\
             meclaw-g12-no-such-unit-{}.scope",
            std::process::id()
        ));
        let limits = ResourceLimits {
            memory_max_bytes: Some(2_000_000_000),
            pids_max: Some(512),
            cpu_max_percent: Some(200),
        };
        let err = tell_the_manager(&dir, &limits, std::time::Duration::from_millis(10_000)).await;
        let Err(err) = err else {
            panic!("[{T}] a unit that does not exist was capped anyway");
        };
        let said = err.to_string();
        assert!(
            said.contains("trusted"),
            "every refusal names the one deliberate way out (OR-G56): {said}"
        );
    }

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

    /// GH #686: a delegated directory arrives with `cgroup.controllers` filled
    /// and `cgroup.subtree_control` empty -- enabling is the delegatee's step,
    /// and this module takes it. A fresh sub-cgroup of the delegated root is
    /// exactly that shape, so it stands in for the unit directory a transient
    /// system user gets.
    #[test]
    fn an_empty_subtree_control_is_enabled_by_the_delegatee() {
        const T: &str = "an_empty_subtree_control_is_enabled_by_the_delegatee";
        let Some(root) = delegated_root() else {
            eprintln!("[{T}] SKIPPED: no delegated cgroup root for this uid");
            return;
        };
        // `<prefix><pid>-test`: the sweep reads the pid off the first segment,
        // so a run that dies before `destroy` leaves something the next one
        // recognises as its own leftover.
        let dir = root.join(format!("{DIR_PREFIX}{}-test", std::process::id()));
        if let Err(e) = std::fs::create_dir(&dir) {
            eprintln!(
                "[{T}] SKIPPED: cannot create a sub-cgroup under {}: {e}",
                root.display()
            );
            return;
        }
        let result = std::panic::catch_unwind(|| {
            assert!(
                !has_enabled_controllers(&dir),
                "a fresh sub-cgroup enables nothing on its own"
            );
            assert!(
                !offered_controllers(&dir).is_empty(),
                "but the parent hands it controllers to enable"
            );
            assert!(adopt_root(&dir), "the delegatee enables them itself");
            assert!(
                has_enabled_controllers(&dir),
                "and the directory now hands controllers down to its children"
            );
            assert!(
                adopt_root(&dir),
                "a directory that already enables controllers is adopted as it is"
            );
        });
        destroy(&dir);
        assert!(!dir.exists(), "the test's sub-cgroup is removed again");
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
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
