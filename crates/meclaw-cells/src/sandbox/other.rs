//! Non-Linux stand-in for the sandbox: the same three entry points, and the
//! same fail-closed answer.
//!
//! Landlock and namespaces are Linux mechanisms. On any other platform a
//! `restricted` profile cannot be enforced, so it is refused rather than
//! silently ignored. A cell that asked to be sandboxed must never run
//! unsandboxed because the port happened to be somewhere else.

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

/// Refuse a `restricted` profile; pass a `trusted` one through untouched.
pub fn apply(profile: &SandboxProfile, _cmd: &mut tokio::process::Command) -> io::Result<()> {
    match profile {
        SandboxProfile::Trusted => Ok(()),
        SandboxProfile::Restricted { .. } => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "params.sandbox declares trust \"restricted\", which this build cannot enforce \
             (Landlock and network namespaces are Linux mechanisms); refusing to run the \
             cell unsandboxed",
        )),
    }
}
