//! Non-Linux stand-in for the sandbox: the same entry points, and the same
//! fail-closed answer.
//!
//! Landlock, namespaces, cgroup v2 and seccomp-bpf are all Linux mechanisms.
//! On any other platform a `restricted` profile cannot be enforced, so it is
//! refused rather than silently ignored. A cell that asked to be sandboxed
//! must never run unsandboxed because the port happened to be somewhere else.
//!
//! Every probe here answers `false`/`None` for the same reason, and that is
//! not a stub: it is the honest answer, and it is what makes the isolation
//! tests skip with a visible line on such a build instead of failing.

use super::profile::SandboxProfile;
use std::io;

/// Always `None`: Landlock does not exist off Linux.
pub fn landlock_abi() -> Option<u32> {
    None
}

/// Always `false`: network namespaces do not exist off Linux.
pub fn network_isolation_supported() -> bool {
    false
}

/// The out-of-process state a sandbox owns. There is none off Linux, because
/// there is no sandbox off Linux; the type exists so the call sites need no
/// `cfg`.
#[derive(Debug)]
pub struct SandboxScope;

impl SandboxScope {
    /// A scope that owns nothing, which is the only kind there is here.
    pub fn empty() -> Self {
        Self
    }
}

/// Always `false`: cgroup v2 is a Linux mechanism.
pub fn cgroup_delegation_supported() -> bool {
    false
}

/// Always "no delegated root" (GH #97): off Linux the MECHANISM is missing, so
/// the answer is the absent-mechanism one and never the wrong-launch one. No
/// way of starting the daemon changes it.
pub fn delegation_probe() -> super::probe::CgroupDelegation {
    super::probe::CgroupDelegation::NoDelegatedRoot
}

/// Always `false`: seccomp-bpf is a Linux mechanism.
pub fn seccomp_supported() -> bool {
    false
}

/// Always `None`: there is no cgroup hierarchy to delegate.
pub fn delegated_root() -> Option<std::path::PathBuf> {
    None
}

/// Refuse a `restricted` profile; pass a `trusted` one through untouched.
pub fn apply(
    profile: &SandboxProfile,
    _cmd: &mut tokio::process::Command,
) -> io::Result<SandboxScope> {
    match profile {
        SandboxProfile::Trusted => Ok(SandboxScope::empty()),
        SandboxProfile::Restricted { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "params.sandbox declares trust \"restricted\", which this build cannot enforce \
             (Landlock, namespaces, cgroup v2 and seccomp-bpf are Linux mechanisms); \
             refusing to run the cell unsandboxed",
        )),
    }
}
