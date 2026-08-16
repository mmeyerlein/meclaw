//! Process identity: the answer to "is pid 4711 still the process I spawned?".
//!
//! A pid alone is not an identity — the kernel hands it out again once the
//! process is reaped, and a boot-time reaper that trusts a bare pid eventually
//! SIGKILLs an innocent stranger. The identity used here is the pair
//! (`start_id`, `comm`):
//!
//! * `start_id` — field 22 of `/proc/<pid>/stat` (`starttime`, in clock ticks
//!   since boot). Two processes with the same pid cannot share it unless they
//!   started in the same tick AND the pid was recycled in between, which the
//!   kernel's monotonic pid allocation makes impossible in practice.
//! * `comm` — the executable name, a cheap second signal that turns an
//!   improbable collision into an implausible one.
//!
//! Unreadable identity is never treated as "it must be mine": [`read_identity`]
//! returns `None` and every caller treats `None` as "do not touch" (GH #116).

/// The identity of a running process, as read from `/proc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcIdentity {
    /// `starttime` from `/proc/<pid>/stat` — clock ticks since boot.
    pub start_id: u64,
    /// The executable name, without the surrounding parentheses.
    pub comm: String,
    /// Scheduling state, field 3 of the stat line. `Z` is a process that has
    /// already died and is only waiting to be waited for.
    pub state: char,
}

impl ProcIdentity {
    /// A zombie is a dead process with a live `/proc` entry. Signalling it does
    /// nothing; only its parent (or `init`, after reparenting) can clear it.
    pub fn is_zombie(&self) -> bool {
        self.state == 'Z'
    }
}

/// Parse the contents of a `/proc/<pid>/stat` line.
///
/// The `comm` field is the reason this cannot be a plain `split_whitespace`:
/// it is wrapped in parentheses and may itself contain spaces and parentheses
/// (`(my prog (1))`). Everything after the LAST `)` is fixed-width, so the
/// parse anchors there — `state` is the first field of that remainder (field 3
/// overall), which puts `starttime` (field 22) at index 19.
///
/// Returns `None` for anything that does not parse — a malformed stat line is
/// an unknown identity, not a permission to kill.
pub fn parse_proc_stat(stat: &str) -> Option<ProcIdentity> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = stat.get(open + 1..close)?.to_string();
    let rest = stat.get(close + 1..)?;
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let start_id = fields.nth(18)?.parse().ok()?;
    Some(ProcIdentity {
        start_id,
        comm,
        state,
    })
}

/// Read the identity of `pid`, or `None` when it cannot be established
/// (process gone, `/proc` not mounted, unreadable, malformed).
///
/// Synchronous on purpose: it runs on the spawn path right after `spawn()`
/// (where an `.await` would widen the window in which an un-journalled child
/// exists) and from `Drop`, where awaiting is not available at all.
#[cfg(unix)]
pub fn read_identity(pid: u32) -> Option<ProcIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat(&stat)
}

/// Non-unix hosts have no `/proc`: every identity is unknown, so nothing is
/// ever reaped.
#[cfg(not(unix))]
pub fn read_identity(pid: u32) -> Option<ProcIdentity> {
    let _ = pid;
    None
}

/// The name of the CALLING THREAD — and therefore the `comm` that any child it
/// forks carries until that child's `execve` has landed.
///
/// `/proc/thread-self` is the calling thread's directory; `/proc/self` would be
/// the thread group leader's, which is the wrong question: a child forked from a
/// Tokio worker is called `tokio-runtime-w` until the kernel renames it, not
/// after the daemon.
#[cfg(unix)]
pub fn spawning_thread_comm() -> Option<String> {
    std::fs::read_to_string("/proc/thread-self/comm")
        .ok()
        .map(|s| s.trim_end_matches('\n').to_string())
}

/// Without `/proc` there is no thread name to read.
#[cfg(not(unix))]
pub fn spawning_thread_comm() -> Option<String> {
    None
}

/// How many scheduler yields the spawn path will spend waiting for a child's
/// `execve` to land. Bounded and sleep-free on purpose: the record has to hit
/// the disk before the daemon can plausibly die, so this may cost scheduler
/// slots, never milliseconds of wall clock.
const SETTLE_YIELDS: usize = 64;

/// Does this identity still look like the fork of `spawner` from BEFORE its
/// `execve` — i.e. is its `comm` merely inherited rather than its own?
fn is_pre_exec_image(ident: Option<&ProcIdentity>, spawner: Option<&str>) -> bool {
    match (ident, spawner) {
        (Some(id), Some(name)) => id.comm == name,
        _ => false,
    }
}

/// Read `pid`'s identity for the record written at spawn time.
///
/// [`read_identity`] alone is not enough here. `Command::spawn` returns as soon
/// as the kernel releases the `CLONE_VFORK` parent, and that release happens
/// INSIDE `execve` (`exec_mmap`) and therefore BEFORE `set_task_comm` — so a
/// `/proc` read taken right after the spawn can still see the pre-exec image,
/// whose `comm` is the spawning thread's name. Recording that name is worse than
/// recording nothing: the boot reaper compares it against the live `comm`, sees
/// a mismatch and refuses to kill a genuine orphan (GH #116).
///
/// So: re-read while the child still carries the spawning thread's name, for at
/// most [`SETTLE_YIELDS`] yields. If it never changes, the observed value is
/// recorded anyway — a child that really IS named like its spawner is the one
/// case where the pre-exec image and the truth agree, and being wrong there
/// costs a skipped orphan, never a wrongly killed stranger.
pub fn read_settled_identity(pid: u32) -> Option<ProcIdentity> {
    let spawner = spawning_thread_comm();
    let mut ident = read_identity(pid);
    for _ in 0..SETTLE_YIELDS {
        if !is_pre_exec_image(ident.as_ref(), spawner.as_deref()) {
            break;
        }
        std::thread::yield_now();
        ident = read_identity(pid);
    }
    ident
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "3131194 (cat) R 3131171 3131194 3131171 0 -1 4194304 90 0 0 0 0 0 0 0 \
                          20 0 1 0 156081463 5967872 448 18446744073709551615";

    #[test]
    fn starttime_is_field_22_counted_past_the_comm() {
        let id = parse_proc_stat(SAMPLE).expect("sample stat parses");
        assert_eq!(id.start_id, 156_081_463);
        assert_eq!(id.comm, "cat");
        assert_eq!(id.state, 'R');
        assert!(!id.is_zombie());
    }

    #[test]
    fn a_zombie_is_recognised_as_one() {
        let line = SAMPLE.replacen(") R ", ") Z ", 1);
        let id = parse_proc_stat(&line).expect("a zombie still has a stat line");
        assert!(id.is_zombie(), "state {:?} must read as zombie", id.state);
        assert_eq!(id.start_id, 156_081_463, "the fields must not shift");
    }

    #[test]
    fn a_comm_with_spaces_and_parens_does_not_shift_the_fields() {
        let line = SAMPLE.replace("(cat)", "(my prog (1))");
        let id = parse_proc_stat(&line).expect("weird comm still parses");
        assert_eq!(id.start_id, 156_081_463);
        assert_eq!(id.comm, "my prog (1)");
    }

    #[test]
    fn a_truncated_stat_line_is_an_unknown_identity_not_a_zero() {
        assert!(parse_proc_stat("42 (sh) R 1 2 3").is_none());
        assert!(parse_proc_stat("no parens here at all").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn our_own_process_has_a_readable_identity() {
        let pid = std::process::id();
        let id = read_identity(pid).expect("we exist");
        assert!(id.start_id > 0, "start_id must be a real tick count");
    }

    #[cfg(unix)]
    #[test]
    fn a_pid_that_cannot_exist_has_no_identity() {
        assert!(read_identity(999_999_999).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_spawning_thread_comm_is_this_thread_not_the_process() {
        // A named thread proves it reads `/proc/thread-self` and not
        // `/proc/self`: the latter would answer with the test binary's name.
        let seen = std::thread::Builder::new()
            .name("reap-probe".to_string())
            .spawn(spawning_thread_comm)
            .expect("thread spawns")
            .join()
            .expect("thread joins");
        assert_eq!(
            seen.as_deref(),
            Some("reap-probe"),
            "the settle check must compare against the spawning THREAD's name"
        );
    }

    #[test]
    fn only_a_comm_equal_to_the_spawners_reads_as_a_pre_exec_image() {
        let id = |comm: &str| ProcIdentity {
            start_id: 1,
            comm: comm.to_string(),
            state: 'S',
        };
        assert!(is_pre_exec_image(
            Some(&id("tokio-runtime-w")),
            Some("tokio-runtime-w")
        ));
        assert!(!is_pre_exec_image(
            Some(&id("sleep")),
            Some("tokio-runtime-w")
        ));
        // No evidence either way is never a reason to keep polling.
        assert!(!is_pre_exec_image(Some(&id("sleep")), None));
        assert!(!is_pre_exec_image(None, Some("tokio-runtime-w")));
    }

    #[cfg(unix)]
    #[test]
    fn the_settle_budget_terminates_on_a_process_that_never_renames_itself() {
        // Our own pid, read from a thread named exactly like it: the settle
        // condition is true on every pass, so only the budget can end the loop.
        let me = std::process::id();
        let comm = read_identity(me).expect("we have an identity").comm;
        let settled = std::thread::Builder::new()
            .name(comm.clone())
            .spawn(move || read_settled_identity(me))
            .expect("thread spawns")
            .join()
            .expect("thread joins")
            .expect("we still exist");
        assert_eq!(
            settled.comm, comm,
            "an exhausted budget records what it saw, it does not give up on the identity"
        );
    }
}
