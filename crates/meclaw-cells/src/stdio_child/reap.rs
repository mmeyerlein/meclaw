//! Signalling a whole process group (unix only).
//!
//! `std` and `tokio` can start a child in its own process group but offer no
//! way to signal that group afterwards — `Child::start_kill` reaches the direct
//! child only. An agent harness spawns process trees (shells, ripgrep,
//! sub-agents), so killing the leader alone leaves orphans burning quota. That
//! gap is what `libc::killpg` fills here. One of two places in the workspace
//! that touch `libc` — the other is `sandbox::linux` (S4, GH #35), which needs
//! `unshare` and the Landlock syscalls for the same reason: `std` does not
//! expose them.
//!
//! Signals, not the tokio reaper, are the mechanism: sending to `-pgid`
//! reaches every descendant that has not left the group.

use std::io;

/// Send `signal` to every process in the group led by `pgid`.
///
/// `signal` 0 sends nothing and only probes whether the group exists — the
/// standard existence check.
///
/// Errors are returned, never panicked: a group that is already gone (`ESRCH`)
/// is the normal outcome of a race with a child that exited on its own, and a
/// teardown path must not blow up on it.
pub(crate) fn killpg(pgid: u32, signal: i32) -> io::Result<()> {
    // SAFETY: `killpg` is a plain libc syscall wrapper with no memory
    // arguments. The only inputs are two integers, and the return value is
    // fully checked below.
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `SIGTERM` — ask the group to wind down.
pub(crate) const SIGTERM: i32 = libc::SIGTERM;

/// `SIGKILL` — the escalation that cannot be ignored.
pub(crate) const SIGKILL: i32 = libc::SIGKILL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_zero_finds_our_own_process_group() {
        // SAFETY: `getpgrp` takes no arguments and cannot fail.
        let own = unsafe { libc::getpgrp() } as u32;
        killpg(own, 0).expect("our own process group must exist");
    }

    #[test]
    fn a_missing_group_is_an_error_not_a_panic() {
        // Far above any plausible pid_max, so the group cannot exist.
        let err = killpg(999_999_999, 0).expect_err("that group cannot exist");
        assert!(
            err.raw_os_error().is_some(),
            "expected an errno-carrying error, got {err:?}"
        );
    }

    #[test]
    fn the_two_signals_are_the_real_ones() {
        assert_eq!(SIGTERM, 15);
        assert_eq!(SIGKILL, 9);
    }
}
