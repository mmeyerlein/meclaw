//! GH #121: the root lease — one daemon per `{root}`, and no more.
//!
//! Two daemons on the same root both boot, share the same WAL and spawn a
//! duplicate of every cell with a duplicate of every child process. SQLite's
//! `busy_timeout` serialises the writes; it is not a guard against a second
//! colony. This module is that guard.

use std::path::{Path, PathBuf};

/// Name of the lease directory inside `{root}`.
///
/// Deliberately a `colony.db-*` sibling: the lease is an operational artefact of
/// the same database, transient in exactly the way `colony.db-wal` and
/// `colony.db-shm` are, and every ignore rule that already covers those covers
/// it too. See the No-Delete note on [`acquire`].
pub const LEASE_DIR: &str = "colony.db-lease";

/// Name of the holder record inside the lease directory.
///
/// Its presence is what makes the lease directory non-empty, which is what
/// makes `rename` onto it fail — the atomic "occupied" answer this module is
/// built on.
pub const HOLDER_FILE: &str = "holder.json";

/// How many times an acquire will reclaim-and-retry before giving up. A boot
/// races at most one stale reclaim; more attempts than this means something is
/// re-creating the lease faster than we can take it, and that is a report, not
/// a spin.
const MAX_ATTEMPTS: u32 = 8;

/// Who a lease says is holding the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holder {
    /// Process id of the holder.
    pub pid: u32,
    /// Start identity of the holder at the time it took the lease.
    pub start_id: u64,
}

/// Why a root could not be leased.
#[derive(Debug)]
pub enum LeaseError {
    /// Another colony is running on this root right now.
    Held {
        /// The living holder named in the lease record.
        holder: Holder,
        /// Where the lease record sits.
        path: PathBuf,
    },
    /// A lease exists but this host cannot decide whether its holder lives.
    /// Refusing is the only safe answer: guessing "dead" would run a second
    /// colony next to a live one.
    Unverifiable {
        /// What made the record undecidable.
        reason: String,
        /// Where the lease record sits.
        path: PathBuf,
    },
    /// The lease could not be written or reclaimed.
    Io(std::io::Error),
    /// The lease changed hands faster than [`MAX_ATTEMPTS`] reclaims.
    Contended {
        /// Where the lease record sits.
        path: PathBuf,
    },
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { holder, path } => write!(
                f,
                "another meclaw colony is already running on this root: pid {} holds {} \
                 (start_id {}). Two daemons on one root duplicate every cell and every \
                 child process — stop pid {} first, or boot a different --root.",
                holder.pid,
                path.display(),
                holder.start_id,
                holder.pid,
            ),
            Self::Unverifiable { reason, path } => write!(
                f,
                "the root lease {} cannot be verified ({reason}) — refusing to boot rather \
                 than risk a second colony beside a live one. Inspect it and remove the \
                 directory by hand if no meclaw is running on this root.",
                path.display(),
            ),
            Self::Io(e) => write!(f, "root lease I/O failed: {e}"),
            Self::Contended { path } => write!(
                f,
                "the root lease {} changed hands on every one of {MAX_ATTEMPTS} attempts — \
                 something is racing this boot for the root.",
                path.display(),
            ),
        }
    }
}

impl std::error::Error for LeaseError {}

impl From<std::io::Error> for LeaseError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A held root lease. Dropping it releases the root.
///
/// The guard is what makes an orderly shutdown clean: every return path out of
/// the boot — success, `?`, panic unwind — runs [`Drop`], and the release only
/// touches a lease that still names US (see [`RootLease::release`]).
#[derive(Debug)]
pub struct RootLease {
    dir: PathBuf,
    token: Holder,
}

impl RootLease {
    /// The record this process published.
    pub fn holder(&self) -> Holder {
        self.token
    }

    /// Where the lease lives.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Give the root back — but only if the lease still names us.
    ///
    /// Token match is the whole point: if a reclaimer already took this lease
    /// over, tearing it down here would delete somebody else's live lease.
    fn release(&mut self) {
        match read_holder(&self.dir) {
            Ok(h) if h == self.token => {
                if let Err(e) = discard(&self.dir) {
                    tracing::warn!(
                        lease = %self.dir.display(), error = %e,
                        "root lease could not be released; the next boot will reclaim it"
                    );
                }
            }
            Ok(h) => tracing::warn!(
                lease = %self.dir.display(), holder_pid = h.pid,
                "root lease is no longer ours — leaving it to its current holder"
            ),
            Err(_) => { /* gone or unreadable: nothing of ours left to release */ }
        }
    }
}

impl Drop for RootLease {
    fn drop(&mut self) {
        self.release();
    }
}

/// Take the lease on `root`, or explain who has it.
///
/// This is the FIRST thing a boot does — before `colony.db` is opened, before
/// the template scan, before any registry read or write — because everything
/// after it assumes it is the only colony on this root.
///
/// **Mechanics** (Track-Ruling G7-R1): a candidate directory carrying the
/// holder record is `rename`d onto `{root}/colony.db-lease`. POSIX `rename`
/// refuses to replace a NON-EMPTY directory, so the record inside the published
/// lease is what makes "occupied" an atomic, race-free answer. No advisory lock,
/// no new dependency, and — unlike a bare pid file — a holder is verified
/// against `/proc` so a recycled pid never blocks a boot.
///
/// **Stale reclaim**: a lease whose holder is gone, a zombie, or a different
/// process wearing a recycled pid is renamed away and then deleted, loudly. The
/// rename-then-delete order is load-bearing: only the reclaimer whose rename
/// succeeded deletes anything, and it deletes the path it renamed TO — never a
/// fresh lease a competitor published in the meantime.
///
/// **No-Delete**: the lease directory and its `.new.*` / `.stale.*` siblings are
/// declared transient operational files of `colony.db`, in the same class as
/// `colony.db-wal`. The No-Delete policy governs the colony's own filesystem
/// state — cells, `cell.db`, `config.json` — and none of that is touched here.
pub fn acquire(root: &Path) -> Result<RootLease, LeaseError> {
    let dir = root.join(LEASE_DIR);
    let token = Holder {
        pid: std::process::id(),
        start_id: my_start_id().ok_or_else(|| LeaseError::Unverifiable {
            reason: "this host cannot report its own process start identity (no /proc)".into(),
            path: dir.clone(),
        })?,
    };
    std::fs::create_dir_all(root)?;

    for attempt in 0..MAX_ATTEMPTS {
        if publish(root, &dir, &token, attempt)? {
            tracing::info!(
                lease = %dir.display(), pid = token.pid, start_id = token.start_id,
                "root lease acquired"
            );
            return Ok(RootLease { dir, token });
        }
        let holder = read_holder(&dir).map_err(|reason| LeaseError::Unverifiable {
            reason,
            path: dir.clone(),
        })?;
        match process_status(holder.pid) {
            ProcStatus::Running { start_id, zombie } if start_id == holder.start_id && !zombie => {
                return Err(LeaseError::Held {
                    holder,
                    path: dir.clone(),
                });
            }
            ProcStatus::Running {
                start_id,
                zombie: true,
            } if start_id == holder.start_id => {
                tracing::warn!(
                    lease = %dir.display(), holder_pid = holder.pid,
                    "root lease held by a ZOMBIE process — it executes no code; reclaiming"
                );
            }
            ProcStatus::Running { start_id, .. } => {
                tracing::warn!(
                    lease = %dir.display(), holder_pid = holder.pid,
                    lease_start_id = holder.start_id, found_start_id = start_id,
                    "root lease names a pid that has since been RECYCLED — the process \
                     running under it is not the holder; reclaiming"
                );
            }
            ProcStatus::Gone => {
                tracing::warn!(
                    lease = %dir.display(), holder_pid = holder.pid,
                    "root lease left behind by a DEAD holder (hard kill or crash); reclaiming"
                );
            }
            ProcStatus::Unknowable => {
                return Err(LeaseError::Unverifiable {
                    reason: format!("pid {} cannot be checked on this host", holder.pid),
                    path: dir.clone(),
                });
            }
        }
        reclaim(&dir, &token, attempt)?;
    }
    Err(LeaseError::Contended { path: dir })
}

/// Try to publish `token` as the lease. `Ok(false)` means the root is occupied.
fn publish(root: &Path, dir: &Path, token: &Holder, attempt: u32) -> Result<bool, LeaseError> {
    let candidate = scratch_path(root, "new", token.pid, attempt);
    std::fs::create_dir(&candidate)?;
    let record = serde_json::json!({
        "pid": token.pid,
        "start_id": token.start_id,
        "acquired_at_unix": unix_now(),
    });
    let written = std::fs::write(candidate.join(HOLDER_FILE), record.to_string());
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&candidate);
        return Err(e.into());
    }
    // The atomic move. A non-empty target — i.e. a published lease — makes this
    // fail, which is the "occupied" answer no check-then-act could give.
    match std::fs::rename(&candidate, dir) {
        Ok(()) => Ok(true),
        Err(_) => {
            let _ = std::fs::remove_dir_all(&candidate);
            Ok(false)
        }
    }
}

/// Move a stale lease out of the way and delete it. Losing the rename is not an
/// error: somebody else got there first and the caller simply retries.
fn reclaim(dir: &Path, token: &Holder, attempt: u32) -> Result<(), LeaseError> {
    let Some(root) = dir.parent() else {
        return Ok(());
    };
    let stale = scratch_path(root, "stale", token.pid, attempt);
    match std::fs::rename(dir, &stale) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&stale);
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

/// Rename the lease away and delete it — the release half of the same move.
fn discard(dir: &Path) -> std::io::Result<()> {
    let Some(root) = dir.parent() else {
        return Ok(());
    };
    let stale = scratch_path(root, "stale", std::process::id(), u32::MAX);
    std::fs::rename(dir, &stale)?;
    std::fs::remove_dir_all(&stale)
}

/// A unique, `colony.db-*`-shaped scratch path beside the lease.
fn scratch_path(root: &Path, kind: &str, pid: u32, attempt: u32) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    root.join(format!("{LEASE_DIR}.{kind}.{pid}.{attempt}.{nanos}"))
}

/// Read the published holder record. `Err` carries why it is undecidable.
fn read_holder(dir: &Path) -> Result<Holder, String> {
    let raw = std::fs::read_to_string(dir.join(HOLDER_FILE))
        .map_err(|e| format!("holder record unreadable: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("holder record is not JSON: {e}"))?;
    let pid = v["pid"]
        .as_u64()
        .ok_or_else(|| "holder record has no numeric `pid`".to_string())?;
    let start_id = v["start_id"]
        .as_u64()
        .ok_or_else(|| "holder record has no numeric `start_id`".to_string())?;
    Ok(Holder {
        pid: u32::try_from(pid).map_err(|_| format!("holder record pid {pid} is out of range"))?,
        start_id,
    })
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What the host can say about a process id right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcStatus {
    /// A process with this pid exists. `start_id` is its start identity and
    /// `zombie` says whether it has already exited and only awaits its reaper.
    Running {
        /// Start identity of the running process (`/proc/{pid}/stat` field 22).
        start_id: u64,
        /// The process has exited and is only waiting to be reaped.
        zombie: bool,
    },
    /// No process with this pid exists.
    Gone,
    /// This host cannot be asked — there is no readable `/proc`.
    Unknowable,
}

/// Ask the host about `pid` (Linux `/proc`).
///
/// The start identity is field 22 (`starttime`) of `/proc/{pid}/stat`: the
/// process' start time in clock ticks since boot. Together with the pid it is
/// unique for as long as the machine is up, which is precisely what a lease
/// needs — a recycled pid carries a different start time and therefore never
/// impersonates the holder that wrote the lease.
pub fn process_status(pid: u32) -> ProcStatus {
    // Distinguish "this host has no /proc" from "that pid is gone": both look
    // like a missing file, and confusing them would either refuse every boot or
    // steal every lease.
    if std::fs::metadata("/proc/self/stat").is_err() {
        return ProcStatus::Unknowable;
    }
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return ProcStatus::Gone,
    };
    match parse_proc_stat(&stat) {
        Some((state, start_id)) => ProcStatus::Running {
            start_id,
            zombie: state == 'Z',
        },
        // A `/proc/{pid}/stat` we cannot parse is not a dead process; refusing
        // to guess is the safe answer and the caller treats it as unverifiable.
        None => ProcStatus::Unknowable,
    }
}

/// Pull `(state, starttime)` out of a `/proc/{pid}/stat` line.
///
/// Field 2 is the executable name in parentheses and may itself contain spaces
/// and parentheses, so the split starts after the LAST `)`. From there index 0
/// is field 3 (`state`) and index 19 is field 22 (`starttime`).
fn parse_proc_stat(stat: &str) -> Option<(char, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = fields.first()?.chars().next()?;
    let start_id = fields.get(19)?.parse().ok()?;
    Some((state, start_id))
}

/// The start identity of THIS process, for the record we are about to write.
pub fn my_start_id() -> Option<u64> {
    match process_status(std::process::id()) {
        ProcStatus::Running { start_id, .. } => Some(start_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_pid_is_running_and_carries_a_start_id() {
        match process_status(std::process::id()) {
            ProcStatus::Running { start_id, zombie } => {
                assert!(!zombie, "the test process is not a zombie");
                assert!(start_id > 0, "a start identity must be a real value");
            }
            other => panic!("this process must look alive to itself, got {other:?}"),
        }
    }

    #[test]
    fn our_start_id_is_stable_across_calls() {
        // Load-bearing: the lease compares a recorded start id against a fresh
        // reading. If the reading drifted, every lease would look recycled.
        assert_eq!(my_start_id(), my_start_id());
        assert!(my_start_id().is_some(), "Linux hosts can answer this");
    }

    #[test]
    fn a_pid_that_cannot_exist_is_gone() {
        // pid 0 is never a userspace process, so `/proc/0` never exists.
        assert_eq!(process_status(0), ProcStatus::Gone);
    }

    /// A synthetic stat line: pid, `comm`, `state`, the 18 fields between
    /// `state` and `starttime` (fields 4..21), then `starttime` and a tail.
    fn stat_line(comm: &str, state: char, start_id: u64) -> String {
        let filler = vec!["0"; 18].join(" ");
        format!("4242 ({comm}) {state} {filler} {start_id} tail tail")
    }

    #[test]
    fn the_field_index_matches_a_real_proc_stat_line() {
        // The load-bearing claim of the parser: after the LAST `)`, index 19 is
        // field 22 (`starttime`). Checked against the kernel's own answer for
        // this very process, not against a hand-counted fixture.
        let raw = std::fs::read_to_string("/proc/self/stat").unwrap();
        let (_, parsed) = parse_proc_stat(&raw).expect("our own stat line parses");
        assert_eq!(Some(parsed), my_start_id());
        assert!(parsed > 0);
    }

    #[test]
    fn a_comm_field_with_spaces_and_parens_does_not_derail_the_parse() {
        // The real trap of `/proc/{pid}/stat`: field 2 is unescaped, so a
        // process named `my (weird) name` breaks every naive whitespace split.
        let stat = stat_line("my (weird) name", 'S', 999_123);
        assert_eq!(parse_proc_stat(&stat), Some(('S', 999_123)));
    }

    #[test]
    fn a_zombie_state_is_read_out_of_the_stat_line() {
        assert_eq!(
            parse_proc_stat(&stat_line("meclaw", 'Z', 555)),
            Some(('Z', 555))
        );
    }

    // ---- the lease itself ----

    /// Write a holder record by hand, as a foreign process would have left it.
    fn plant_lease(root: &std::path::Path, pid: u32, start_id: u64) {
        let dir = root.join(LEASE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(HOLDER_FILE),
            serde_json::json!({ "pid": pid, "start_id": start_id, "acquired_at_unix": 0 })
                .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn acquiring_a_free_root_publishes_a_holder_record() {
        let td = tempfile::TempDir::new().unwrap();
        let lease = acquire(td.path()).expect("a free root hands out its lease");
        let raw = std::fs::read_to_string(td.path().join(LEASE_DIR).join(HOLDER_FILE))
            .expect("the published lease carries its holder record");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["pid"].as_u64(), Some(u64::from(std::process::id())));
        assert_eq!(v["start_id"].as_u64(), my_start_id());
        drop(lease);
    }

    #[test]
    fn a_second_acquire_against_a_living_holder_is_refused_naming_the_pid() {
        let td = tempfile::TempDir::new().unwrap();
        let _held = acquire(td.path()).expect("first acquire");
        // The living holder here is this very test process — the same thing a
        // second daemon sees when the first one is up.
        let err = acquire(td.path()).expect_err("a living holder must refuse the boot");
        let msg = format!("{err}");
        assert!(
            msg.contains(&std::process::id().to_string()),
            "the refusal must name the holder pid so an operator can act, got: {msg}"
        );
        assert!(matches!(err, LeaseError::Held { .. }));
    }

    #[test]
    fn dropping_the_lease_frees_the_root_for_the_next_boot() {
        let td = tempfile::TempDir::new().unwrap();
        drop(acquire(td.path()).expect("first acquire"));
        assert!(
            !td.path().join(LEASE_DIR).exists(),
            "an orderly shutdown leaves no lease behind"
        );
        drop(acquire(td.path()).expect("the next boot walks straight in"));
    }

    #[test]
    fn a_lease_from_a_dead_holder_is_reclaimed() {
        let td = tempfile::TempDir::new().unwrap();
        // pid 0 is never a process — the hard-kill case, seen from the next boot.
        plant_lease(td.path(), 0, 12345);
        let lease = acquire(td.path()).expect("a dead holder does not block a boot");
        let raw = std::fs::read_to_string(td.path().join(LEASE_DIR).join(HOLDER_FILE)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            v["pid"].as_u64(),
            Some(u64::from(std::process::id())),
            "the reclaimed lease must now name US"
        );
        drop(lease);
    }

    #[test]
    fn a_lease_whose_pid_was_recycled_is_reclaimed() {
        let td = tempfile::TempDir::new().unwrap();
        // The pid is alive (it is us), but the recorded start identity is not
        // ours: the recorded holder is long gone and the kernel handed its pid
        // to somebody else. Without the start-id check this would look held.
        let wrong = my_start_id().unwrap() + 1;
        plant_lease(td.path(), std::process::id(), wrong);
        let lease = acquire(td.path()).expect("a recycled pid is not the holder");
        let raw = std::fs::read_to_string(td.path().join(LEASE_DIR).join(HOLDER_FILE)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["start_id"].as_u64(), my_start_id());
        drop(lease);
    }

    #[test]
    fn an_unreadable_holder_record_refuses_the_boot_instead_of_guessing() {
        let td = tempfile::TempDir::new().unwrap();
        let dir = td.path().join(LEASE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(HOLDER_FILE), b"{ this is not json").unwrap();
        let err = acquire(td.path()).expect_err("do not guess about an unreadable lease");
        assert!(matches!(err, LeaseError::Unverifiable { .. }));
        assert!(
            format!("{err}").contains(LEASE_DIR),
            "the message must name the path an operator has to look at"
        );
    }

    #[test]
    fn releasing_a_lease_that_is_no_longer_ours_leaves_it_alone() {
        let td = tempfile::TempDir::new().unwrap();
        let lease = acquire(td.path()).expect("first acquire");
        // Somebody else took the root over while we were running.
        plant_lease(td.path(), 999_999, 777);
        drop(lease);
        let raw = std::fs::read_to_string(td.path().join(LEASE_DIR).join(HOLDER_FILE))
            .expect("the foreign lease must survive our release");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["pid"].as_u64(), Some(999_999));
    }

    #[test]
    fn a_reclaim_leaves_no_scratch_directory_behind() {
        // No-Delete has a declared exception for the lease's own transient
        // siblings; what it does not tolerate is litter.
        let td = tempfile::TempDir::new().unwrap();
        plant_lease(td.path(), 0, 1);
        drop(acquire(td.path()).expect("reclaim"));
        let leftovers: Vec<_> = std::fs::read_dir(td.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(LEASE_DIR))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }
}
