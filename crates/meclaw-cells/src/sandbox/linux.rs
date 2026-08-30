//! Linux enforcement of a [`SandboxProfile`]: Landlock for the filesystem
//! view, `unshare(CLONE_NEWUSER | CLONE_NEWNET)` for the network deny, and the
//! ordering that ties all four mechanisms together.
//!
//! The caps live in [`super::cgroup`] and the syscall filter in
//! [`super::seccomp`]; what belongs HERE is the sequence, because the order is
//! load-bearing and every argument for it is a different one:
//!
//! 1. **join the cgroup**, while the credentials are still plainly the
//!    daemon's -- a `cgroup.procs` write is a permission question about the
//!    writer, and a user namespace would only make it harder to reason about;
//! 2. **`PR_SET_NO_NEW_PRIVS`**, because both `landlock_restrict_self` and
//!    `seccomp(SECCOMP_SET_MODE_FILTER, ...)` require it;
//! 3. **`unshare`**, because creating the namespaces needs no filesystem
//!    access and doing it before Landlock keeps the ordering obvious;
//! 4. **Landlock**, because from here on nothing outside the allow-list can be
//!    opened -- including the binary about to be `exec`ed, which is why it has
//!    to be in the runtime set or in `read`;
//! 5. **seccomp**, last, because every step above is a syscall the filter
//!    would otherwise have to allow, and a filter installed earlier would just
//!    be a longer allow-list.
//!
//! # Why Landlock and not a mount namespace
//!
//! The obvious route to a filesystem view is `CLONE_NEWNS` plus bind mounts.
//! Measured on the target platform (Ubuntu, kernel 6.8, AppArmor with
//! `kernel.apparmor_restrict_unprivileged_userns = 1`) that route is closed for
//! an unprivileged daemon: the process lands in a restricted AppArmor profile
//! without `CAP_SYS_ADMIN`, so every `mount(2)` fails and `/proc/self/uid_map`
//! cannot be written. Requiring the operator to weaken the host's hardening in
//! exchange for cell hardening is the wrong trade. Landlock needs no
//! capability, no mount and no uid mapping, only `PR_SET_NO_NEW_PRIVS`, and it
//! answers exactly the question the profile asks. The full
//! host survey behind that choice was ratified in GH #35.
//!
//! # Why the network namespace is still a namespace
//!
//! `unshare(CLONE_NEWUSER | CLONE_NEWNET)` does work unprivileged, and a fresh
//! network namespace has nothing in it but a `lo` in state DOWN. Without a
//! written `uid_map` the child *displays* as `nobody`, but that is display
//! only: the kernel compares `kuid_t` against `kuid_t`, which a user namespace
//! does not change, so file permissions behave exactly as before. The child
//! loses the network and nothing else.
//!
//! # Still not handled
//!
//! `LANDLOCK_ACCESS_FS_IOCTL_DEV` (ABI 5) is deliberately left unhandled, so
//! `ioctl` on device nodes stays unrestricted. The seccomp filter of GH #85
//! did not pick it up either: its three axes are the ones the issue named, and
//! a blanket `ioctl` rule would break far more than it closes.

use super::cgroup::SandboxScope;
use super::profile::{FilesystemProfile, NetworkPolicy, SandboxProfile};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

// ---- Landlock uapi --------------------------------------------------------

/// `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` reports
/// the ABI version instead of creating a ruleset.
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
/// `enum landlock_rule_type::LANDLOCK_RULE_PATH_BENEATH`.
const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;

const ACCESS_FS_EXECUTE: u64 = 1 << 0;
const ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const ACCESS_FS_READ_FILE: u64 = 1 << 2;
const ACCESS_FS_READ_DIR: u64 = 1 << 3;
const ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
/// ABI 2. Link/rename across directories. Must be handled, otherwise the kernel
/// denies every rename outright once any filesystem access is handled.
const ACCESS_FS_REFER: u64 = 1 << 13;
/// ABI 3.
const ACCESS_FS_TRUNCATE: u64 = 1 << 14;

/// Bits the kernel accepts on a rule whose target is not a directory
/// (`landlock/fs.c` rejects directory-only bits on a file with `EINVAL`).
const ACCESS_FILE_ONLY: u64 =
    ACCESS_FS_EXECUTE | ACCESS_FS_WRITE_FILE | ACCESS_FS_READ_FILE | ACCESS_FS_TRUNCATE;

/// Read plus execute, recursively.
const GRANT_RO: u64 = ACCESS_FS_EXECUTE | ACCESS_FS_READ_FILE | ACCESS_FS_READ_DIR;
/// Read, write, create and remove, recursively.
const GRANT_RW: u64 = GRANT_RO
    | ACCESS_FS_WRITE_FILE
    | ACCESS_FS_REMOVE_DIR
    | ACCESS_FS_REMOVE_FILE
    | ACCESS_FS_MAKE_DIR
    | ACCESS_FS_MAKE_REG
    | ACCESS_FS_MAKE_SOCK
    | ACCESS_FS_MAKE_FIFO
    | ACCESS_FS_MAKE_SYM
    | ACCESS_FS_REFER
    | ACCESS_FS_TRUNCATE;

/// `struct landlock_ruleset_attr`. `scoped` (ABI 6) is not declared: the
/// ruleset size passed to the kernel is this struct's size, and a kernel that
/// predates a field must not be handed one.
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

/// `struct landlock_path_beneath_attr` -- packed in the uapi header.
#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: RawFd,
}

/// The read-and-execute half of the runtime set: without it no interpreter and
/// no dynamic loader is reachable, so nothing starts at all.
const RUNTIME_RO: [&str; 8] = [
    "/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc", "/sys",
];

/// The read-and-write half of the runtime set: the device nodes every runtime
/// expects to exist.
const RUNTIME_RW: [&str; 5] = [
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
];

/// The resolver configuration every libc consults before it sends a query.
const RESOLV_CONF: &str = "/etc/resolv.conf";

/// What `network: "allow"` has to grant beyond the runtime set so that a name
/// can actually be resolved (GH #144).
///
/// `RESOLV_CONF` lives under `/etc`, which the runtime set covers -- but on a
/// systemd-resolved host it is a SYMLINK into `/run/systemd/resolve/`, and
/// `/run` is in no set at all. The result was an "allow" that opened sockets
/// and resolved nothing: every outbound call died in `getaddrinfo`, while the
/// profile said the network was open. A boundary that promises a capability it
/// then withholds is worse than one that refuses it, so the promise is kept
/// here rather than left to each template to discover.
///
/// Only under [`NetworkPolicy::Allow`]. A child in a fresh network namespace
/// has nothing to resolve for, and widening its view for a lookup it cannot
/// perform would be a grant nobody asked for.
///
/// The path is the resolved TARGET's directory, not the symlink and not a
/// blanket `/run`: `systemd-resolved` rewrites `stub-resolv.conf` by rename, and
/// a Landlock rule pins an inode -- a rule on the file would go stale the next
/// time the host changes networks, a rule on the directory survives it. When
/// the target already sits in `/etc` (the plain-file distributions), the file
/// itself is granted instead, so a hand-declared `runtime: false` profile is
/// not silently widened to all of `/etc`.
fn resolver_grant(network: NetworkPolicy) -> Option<std::path::PathBuf> {
    if network != NetworkPolicy::Allow {
        return None;
    }
    let target = std::fs::canonicalize(RESOLV_CONF).ok()?;
    match target.parent() {
        Some(dir) if dir == Path::new("/etc") => Some(target),
        Some(dir) => Some(dir.to_path_buf()),
        None => None,
    }
}

// ---- feature detection ----------------------------------------------------

/// The Landlock ABI version this kernel reports, or `None` when Landlock is
/// absent or disabled.
///
/// Cheap and side-effect free: the probe form of `landlock_create_ruleset`
/// creates nothing. Tests use it to skip instead of failing on a kernel that
/// cannot enforce the property under test.
pub fn landlock_abi() -> Option<u32> {
    // SAFETY: the probe form takes a NULL attr pointer and a zero size by
    // contract; no memory is read or written. The return value is checked.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if rc > 0 { Some(rc as u32) } else { None }
}

/// Whether this host lets an unprivileged process drop into a fresh network
/// namespace.
///
/// Answered by doing it: spawns `/bin/sh -c :` through the very `pre_exec` the
/// real path uses. Kernel knobs, container runtimes and LSM policies each have
/// their own way of saying no, and reading them all is less reliable than one
/// attempt. Blocking, so this belongs in a test or a validation pass, never in
/// an async hot path.
pub fn network_isolation_supported() -> bool {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(":")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: `unshare_network` performs one syscall on integer arguments and
    // allocates nothing, so it is safe in the forked child of a multithreaded
    // parent. See `apply` for the full argument.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(unshare_network);
    }
    matches!(cmd.status(), Ok(st) if st.success())
}

// ---- application ----------------------------------------------------------

/// Apply `profile` to `cmd` so that the spawned child runs inside it.
///
/// Fail-closed in both halves. Everything that can fail with a readable reason
/// (a declared path that does not exist, a kernel without Landlock) fails here,
/// in the parent, before a child exists. Everything that can only fail after
/// the fork returns `Err` out of `pre_exec`, which makes `spawn()` itself fail.
/// There is no path on which a `restricted` profile silently does nothing.
///
/// A [`SandboxProfile::Trusted`] profile is the declared escape hatch and does
/// nothing at all.
pub fn apply(
    profile: &SandboxProfile,
    cmd: &mut tokio::process::Command,
) -> io::Result<SandboxScope> {
    let (network, filesystem, limits, syscalls) = match profile {
        SandboxProfile::Trusted => return Ok(SandboxScope::empty()),
        SandboxProfile::Restricted {
            network,
            filesystem,
            limits,
            syscalls,
        } => (*network, filesystem, *limits, *syscalls),
    };

    let abi = landlock_abi().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "params.sandbox declares trust \"restricted\" but this kernel has no Landlock \
             (needs Linux 5.13+ with the LSM enabled); refusing to run the cell unsandboxed",
        )
    })?;

    let handled = handled_access_fs(abi);
    let fds = open_allowed_paths(filesystem, network, handled)?;

    // The cap is state outside the process, so it is created here, in the
    // parent, and owned by the returned scope. A failure to create it fails the
    // spawn: `limits` that nobody enforces would be the comforting lie the
    // whole key exists to avoid.
    let (scope, procs_fd) = match limits {
        Some(l) => {
            let (scope, fd) = super::cgroup::create(&l)?;
            (scope, Some(fd))
        }
        None => (SandboxScope::empty(), None),
    };
    let procs_raw = procs_fd.as_ref().map(|fd| fd.as_raw_fd());

    // The seccomp program is assembled here too, so an architecture this
    // substrate cannot key a filter to fails before a child exists rather than
    // as an opaque errno out of `pre_exec`.
    let mut filter = match syscalls {
        Some(policy) => Some(super::seccomp::Filter::build(&policy)?),
        None => None,
    };

    // SAFETY (the whole point of this module): the closure below runs in the
    // child after `fork()` in a multithreaded parent, so it must be
    // async-signal-safe. It allocates nothing, formats nothing and logs
    // nothing: every value it touches (`handled`, the `rules` vector and its
    // raw fds, the cgroup descriptor) is built above, before the fork, and is
    // only read afterwards. `io::Error::last_os_error` wraps an `errno` and
    // does not allocate. The raw fds stay valid because their `OwnedFd`s are
    // moved into the closure and therefore outlive every use.
    unsafe {
        cmd.pre_exec(move || {
            // First, while the credentials are still plainly this daemon's:
            // joining a cgroup is a permission question about the writer, and
            // a user namespace would only make it harder to reason about.
            if let Some(fd) = procs_raw {
                super::cgroup::join_via(fd)?;
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            // Before Landlock: creating the namespaces needs no filesystem
            // access, and doing it first keeps the ordering obvious.
            if network == NetworkPolicy::Deny {
                unshare_network()?;
            }
            restrict_self(handled, &fds)?;
            // Last, and deliberately so: everything above is a syscall the
            // filter would have to allow, and a filter installed before them
            // would only be a longer allow-list. From here on nothing else in
            // this closure runs.
            if let Some(f) = filter.as_mut() {
                f.install()?;
            }
            // `procs_fd` is only moved in so that the descriptor outlives the
            // call above; nothing reads it after this point.
            let _ = &procs_fd;
            Ok(())
        });
    }
    Ok(scope)
}

/// `unshare(CLONE_NEWUSER | CLONE_NEWNET)`: the child keeps its credentials but
/// loses every interface except a `lo` in state DOWN.
fn unshare_network() -> io::Result<()> {
    // SAFETY: `unshare` takes a single integer flag word and touches no memory
    // owned by this process. The return value is checked.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Build the ruleset from the pre-opened fds and enforce it on the calling
/// thread and all its children. Runs post-fork: syscalls only.
fn restrict_self(handled: u64, rules: &[(OwnedFd, u64)]) -> io::Result<()> {
    let attr = RulesetAttr {
        handled_access_fs: handled,
        handled_access_net: 0,
    };
    // SAFETY: `attr` is a live, correctly sized `landlock_ruleset_attr` and the
    // size argument matches it exactly.
    let ruleset = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            0u32,
        )
    };
    if ruleset < 0 {
        return Err(io::Error::last_os_error());
    }
    let ruleset = ruleset as libc::c_int;

    for (fd, access) in rules {
        let rule = PathBeneathAttr {
            allowed_access: *access,
            parent_fd: fd.as_raw_fd(),
        };
        // SAFETY: `rule` is a live, correctly typed `landlock_path_beneath_attr`
        // and `fd` is open for the duration of this call.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset,
                LANDLOCK_RULE_PATH_BENEATH,
                &rule as *const PathBeneathAttr,
                0u32,
            )
        };
        if rc != 0 {
            let e = io::Error::last_os_error();
            // SAFETY: `ruleset` is a valid fd this function created.
            unsafe { libc::close(ruleset) };
            return Err(e);
        }
    }

    // SAFETY: `ruleset` is a valid fd this function created; the second
    // argument is the required zero flags word.
    let rc = unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) };
    let err = if rc != 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    // SAFETY: as above; closing is correct whether or not the restriction took.
    unsafe { libc::close(ruleset) };
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// The access bits this kernel's ABI understands.
///
/// A ruleset that declares a bit the running kernel does not know is rejected
/// with `EINVAL`, so the mask is negotiated down: this is the compatibility
/// procedure the Landlock uapi prescribes.
fn handled_access_fs(abi: u32) -> u64 {
    let mut handled = GRANT_RW | GRANT_RO;
    if abi < 3 {
        handled &= !ACCESS_FS_TRUNCATE;
    }
    if abi < 2 {
        handled &= !ACCESS_FS_REFER;
    }
    handled
}

/// Open every allowed path with `O_PATH | O_CLOEXEC` and pair it with the
/// access mask it is granted.
///
/// `O_CLOEXEC` matters: the fds exist only to describe rules to the kernel
/// before `execve`, and must not leak into the sandboxed program.
fn open_allowed_paths(
    fs: &FilesystemProfile,
    network: NetworkPolicy,
    handled: u64,
) -> io::Result<Vec<(OwnedFd, u64)>> {
    let mut out = Vec::new();
    // What "allow" owes the child before any of its own paths (GH #144): a
    // socket without a resolver is not a network. Optional like the runtime
    // set, for the same reason -- a host without the file is a host that
    // resolves differently, not a misconfigured cell.
    if let Some(p) = resolver_grant(network)
        && let Some(entry) = open_optional(&p, GRANT_RO, handled)?
    {
        out.push(entry);
    }
    if fs.runtime {
        // The runtime set is a convenience, not a declaration: an entry that
        // does not exist on this distribution (`/lib64`, say) is skipped.
        for p in RUNTIME_RO {
            if let Some(entry) = open_optional(Path::new(p), GRANT_RO, handled)? {
                out.push(entry);
            }
        }
        for p in RUNTIME_RW {
            if let Some(entry) = open_optional(Path::new(p), GRANT_RW, handled)? {
                out.push(entry);
            }
        }
    }
    // A path the operator declared is a different matter: a typo there would
    // turn into a silent denial that only shows up in production.
    for p in &fs.read {
        out.push(open_required(p, GRANT_RO, handled)?);
    }
    for p in &fs.write {
        out.push(open_required(p, GRANT_RW, handled)?);
    }
    Ok(out)
}

/// Open a path that must exist. A missing one is a configuration error.
fn open_required(path: &Path, grant: u64, handled: u64) -> io::Result<(OwnedFd, u64)> {
    open_optional(path, grant, handled)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "params.sandbox.filesystem lists {} but it does not exist; \
                 refusing to spawn with a boundary nobody can check",
                path.display()
            ),
        )
    })
}

/// Open a path, returning `Ok(None)` when it is simply not there.
fn open_optional(path: &Path, grant: u64, handled: u64) -> io::Result<Option<(OwnedFd, u64)>> {
    let is_dir = match std::fs::metadata(path) {
        Ok(md) => md.is_dir(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `c` is a live NUL-terminated string for the duration of the call;
    // the return value is checked before it is turned into an `OwnedFd`.
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a fresh, valid, unowned fd; `OwnedFd` takes sole
    // ownership of it here.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mask = if is_dir {
        grant
    } else {
        grant & ACCESS_FILE_ONLY
    };
    Ok(Some((fd, mask & handled)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handled_mask_drops_bits_the_abi_does_not_know() {
        assert_eq!(handled_access_fs(1) & ACCESS_FS_REFER, 0);
        assert_eq!(handled_access_fs(1) & ACCESS_FS_TRUNCATE, 0);
        assert_eq!(handled_access_fs(2) & ACCESS_FS_REFER, ACCESS_FS_REFER);
        assert_eq!(handled_access_fs(2) & ACCESS_FS_TRUNCATE, 0);
        assert_eq!(
            handled_access_fs(4) & ACCESS_FS_TRUNCATE,
            ACCESS_FS_TRUNCATE
        );
    }

    #[test]
    fn a_file_rule_never_carries_directory_only_bits() {
        // The kernel rejects those with EINVAL, so the mask has to notice that
        // the target is not a directory.
        let td = tempfile::TempDir::new().unwrap();
        let f = td.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        let handled = handled_access_fs(4);
        let (_fd, mask) = open_optional(&f, GRANT_RW, handled).unwrap().unwrap();
        assert_eq!(
            mask & !ACCESS_FILE_ONLY,
            0,
            "file rule must be a subset of the file-only access bits"
        );
        assert_ne!(mask & ACCESS_FS_WRITE_FILE, 0, "but still writable");
    }

    #[test]
    fn a_missing_declared_path_is_an_error_a_missing_runtime_path_is_not() {
        let handled = handled_access_fs(4);
        let missing = Path::new("/nonexistent-s4-sandbox-probe");
        assert!(open_required(missing, GRANT_RO, handled).is_err());
        assert!(open_optional(missing, GRANT_RO, handled).unwrap().is_none());
    }

    #[test]
    fn the_resolver_grant_exists_only_under_network_allow() {
        // GH #144. The kernel-side proof is the regression lock in
        // `tests/gh144_network_allow_resolves_names.rs`; what belongs here is
        // the half that holds on every host, with or without Landlock: the
        // grant is a property of "allow" and of nothing else.
        assert!(
            resolver_grant(NetworkPolicy::Deny).is_none(),
            "a child in a fresh network namespace has nothing to resolve for"
        );
        match (
            resolver_grant(NetworkPolicy::Allow),
            std::fs::canonicalize(RESOLV_CONF),
        ) {
            (Some(granted), Ok(target)) => {
                assert!(
                    target.starts_with(&granted),
                    "the grant must cover the resolved target: {} does not reach {}",
                    granted.display(),
                    target.display()
                );
            }
            (None, Ok(t)) => panic!(
                "{RESOLV_CONF} resolves to {} but nothing is granted",
                t.display()
            ),
            // No resolver configuration on this host: nothing to grant, and a
            // profile that names a path which does not exist would fail the
            // spawn instead of opening the network.
            (granted, Err(_)) => assert!(granted.is_none()),
        }
    }

    #[test]
    fn landlock_abi_probe_does_not_lie_about_absence() {
        // Whatever this kernel answers, it must be a plausible ABI or None.
        match landlock_abi() {
            None => {}
            Some(v) => assert!(v >= 1, "an ABI version below 1 is not a version"),
        }
    }
}
