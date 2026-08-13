//! Per-cell process sandbox for the cell types that run foreign code
//! (S4 / GH #35, completed in GH #85).
//!
//! A `bash`, `code` or `harness` cell spawns a child with the daemon's rights.
//! This module is the boundary that takes those rights away again, declared
//! per cell in `params.sandbox` and enforced at spawn time.
//!
//! Four properties, each a separate mechanism and each enforced only when the
//! profile asks for it:
//!
//! - **filesystem view** via Landlock, the kernel's unprivileged filesystem
//!   access LSM: the child may reach only the declared paths ([`linux`]).
//! - **network deny** via `unshare(CLONE_NEWUSER | CLONE_NEWNET)`: the child
//!   lands in a fresh network namespace whose only interface is a down `lo`.
//! - **resource caps** via a delegated cgroup v2 sub-cgroup ([`cgroup`]) --
//!   the one property that is state OUTSIDE the process, which is why
//!   [`apply`] hands back a [`SandboxScope`] the caller must keep alive.
//! - **syscall filter** via a seccomp-bpf program assembled in this tree
//!   ([`seccomp`]), closing what a filesystem LSM cannot: `ptrace`, signals to
//!   processes outside the sandbox, and raw sockets.
//!
//! Everything here is fail-closed. A profile that cannot be applied makes the
//! spawn fail; there is no path on which a `restricted` profile quietly does
//! nothing. That is also why every mechanism has a probe that answers by
//! TRYING rather than by reading a knob ([`landlock_abi`],
//! [`network_isolation_supported`], [`cgroup_delegation_supported`],
//! [`seccomp_supported`]): the tests skip visibly on a host that cannot
//! enforce a property instead of going red, and a red test on such a host
//! would only invite somebody to weaken the assertion.
//!
//! Design, the measured environment survey and the phase-2 record:
//! `plans/s4-sandbox/design.md`.

mod profile;
pub use profile::{
    FilesystemProfile, NetworkPolicy, ResourceLimits, SandboxProfile, SyscallPolicy,
};

#[cfg(target_os = "linux")]
mod cgroup;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod seccomp;
#[cfg(target_os = "linux")]
pub use cgroup::{SandboxScope, cgroup_delegation_supported, delegated_root};
#[cfg(target_os = "linux")]
pub use linux::{apply, landlock_abi, network_isolation_supported};
#[cfg(target_os = "linux")]
pub use seccomp::seccomp_supported;

#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
pub use other::{
    SandboxScope, apply, cgroup_delegation_supported, delegated_root, landlock_abi,
    network_isolation_supported, seccomp_supported,
};
