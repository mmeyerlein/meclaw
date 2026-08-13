//! S4 (GH #35): per-cell process sandbox for the cell types that run foreign
//! code.
//!
//! A `code` or `bash` cell spawns a child with the daemon's rights. This module
//! is the boundary that takes those rights away again, declared per cell in
//! `params.sandbox` and enforced at spawn time.
//!
//! Two properties are enforced in phase 1:
//!
//! - **filesystem view** via Landlock, the kernel's unprivileged filesystem
//!   access LSM: the child may reach only the declared paths.
//! - **network deny** via `unshare(CLONE_NEWUSER | CLONE_NEWNET)`: the child
//!   lands in a fresh network namespace whose only interface is a down `lo`.
//!
//! Resource caps (cgroup v2) and a syscall filter (seccomp-bpf) are phase 2 and
//! tracked in GH #85. Their schema keys exist but are REJECTED at config load,
//! never silently ignored: a cap nobody enforces is worse than no cap at all.
//!
//! Everything here is fail-closed. A profile that cannot be applied makes the
//! spawn fail; there is no path on which a `restricted` profile quietly does
//! nothing. Design and the measured environment survey:
//! `plans/s4-sandbox/design.md`.

mod profile;
pub use profile::{FilesystemProfile, NetworkPolicy, SandboxProfile};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{apply, landlock_abi, network_isolation_supported};

#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
pub use other::{apply, landlock_abi, network_isolation_supported};
