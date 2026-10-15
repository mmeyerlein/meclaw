//! GH #1082: the colony's open-file budget.
//!
//! A colony holds three file descriptors per open `cell.db` (database, WAL,
//! shared-memory index) plus the pipes of its child processes, and that count
//! grows with members × cells. The lab colony that filed #1082 had 10 members
//! and 226 `cell.db` files: 226 × 3 fds plus ~216 pipes, past the default soft
//! limit of 1024. From then on every wake that had to open a database failed
//! (`unable to open database file`), and each such store started degraded.
//!
//! The hard limit on the same host was 1048576. The soft limit is only the
//! starting value a process inherits, and a process may raise it up to the hard
//! limit without any privilege, so the colony does that once at start instead
//! of leaving it to every unit file (`LimitNOFILE=`) or to a `prlimit` by hand.

/// The open-file limit before and after [`raise_nofile_soft_limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NofileLimit {
    /// The soft limit the process started with.
    pub old_soft: u64,
    /// The soft limit after the raise (the hard limit, or `old_soft` when the
    /// two were already equal).
    pub new_soft: u64,
    /// The hard limit, which this function never changes.
    pub hard: u64,
}

/// Raise this process's soft `RLIMIT_NOFILE` to its hard limit.
///
/// Called once at colony start (`meclaw_cli::entrypoint`). Returns the limits
/// before and after, or the OS error of `getrlimit`/`setrlimit`; the caller
/// logs a warning and starts anyway, because a colony with few cells runs fine
/// on the default limit. Never lowers the soft limit and never touches the hard
/// one. On a non-unix target it returns `ErrorKind::Unsupported`.
pub fn raise_nofile_soft_limit() -> std::io::Result<NofileLimit> {
    #[cfg(unix)]
    {
        let (old_soft, hard) = nofile()?;
        if old_soft >= hard {
            return Ok(NofileLimit {
                old_soft,
                new_soft: old_soft,
                hard,
            });
        }
        set_nofile(hard, hard)?;
        let (new_soft, hard) = nofile()?;
        Ok(NofileLimit {
            old_soft,
            new_soft,
            hard,
        })
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

/// The current `(soft, hard)` `RLIMIT_NOFILE` of this process.
#[cfg(unix)]
fn nofile() -> std::io::Result<(u64, u64)> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` writes one `rlimit` into the struct we own and pass
    // by valid pointer; it reads nothing else.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // `rlim_t` is `u64` on the Linux targets clippy runs on (the gate's clippy
    // station flagged the cast as unnecessary), but the alias is not `u64` on
    // every unix target, so the cast stays and only the lint is silenced here.
    #[allow(clippy::unnecessary_cast)]
    let limits = (lim.rlim_cur as u64, lim.rlim_max as u64);
    Ok(limits)
}

/// Set `RLIMIT_NOFILE` of this process to `(soft, hard)`.
#[cfg(unix)]
fn set_nofile(soft: u64, hard: u64) -> std::io::Result<()> {
    let lim = libc::rlimit {
        rlim_cur: soft as libc::rlim_t,
        rlim_max: hard as libc::rlim_t,
    };
    // SAFETY: `setrlimit` reads one `rlimit` from the struct we own and pass by
    // valid pointer.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The raise lifts a lowered soft limit back to the hard limit.
    ///
    /// The test lowers the soft limit of its OWN process first, so it proves the
    /// raise even on a host whose soft limit already equals the hard one. Under
    /// nextest every test is its own process, so the lowered limit is never seen
    /// by another test; under a plain `cargo test` the lowering is one below the
    /// current value and the raise follows at once, which no sibling test can
    /// trip over.
    #[test]
    fn the_soft_limit_is_raised_to_the_hard_limit() {
        let (soft, hard) = nofile().expect("getrlimit");
        let lowered = soft.min(hard) - 1;
        set_nofile(lowered, hard).expect("lowering the soft limit needs no privilege");
        assert_eq!(nofile().unwrap(), (lowered, hard), "the fixture took");

        let got =
            raise_nofile_soft_limit().expect("raising up to the hard limit needs no privilege");

        assert_eq!(got.old_soft, lowered, "{got:?}");
        assert_eq!(got.new_soft, hard, "{got:?}");
        assert_eq!(got.hard, hard, "the hard limit is never touched: {got:?}");
        assert_eq!(nofile().unwrap(), (hard, hard), "the process limit moved");
    }

    /// A soft limit already at the hard limit is left as it is.
    #[test]
    fn a_soft_limit_at_the_hard_limit_is_left_alone() {
        let (_, hard) = nofile().expect("getrlimit");
        set_nofile(hard, hard).expect("raising to the hard limit needs no privilege");
        let got = raise_nofile_soft_limit().unwrap();
        assert_eq!(
            got,
            NofileLimit {
                old_soft: hard,
                new_soft: hard,
                hard
            }
        );
    }
}
