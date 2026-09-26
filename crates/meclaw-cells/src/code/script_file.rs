//! GH #349 — getting an oversized `script_inline` to the runner.
//!
//! Linux caps a **single** `argv` string at `MAX_ARG_STRLEN` = `32 * PAGE_SIZE`
//! = 131 072 bytes. The cap is independent of `ARG_MAX` and cannot be raised,
//! so `<runner> -c <script>` has a hard ceiling on how big an inline script may
//! be: above it `spawn()` fails with `Argument list too long (os error 7)` and
//! the cell never starts. `templates/memory-hive/recall` crossed that line and
//! its whole read path stopped working.
//!
//! # Why not stdin
//!
//! The obvious remedy is the one the probe tests already use: hand the program
//! to the interpreter on **stdin** (`python3 -`), where no cap exists. That
//! route is closed HERE, because in production stdin is not free: it carries
//! the serialized Message — the document the script reads with
//! `json.load(sys.stdin)`. A script and a document cannot share one pipe, and
//! moving the document elsewhere would change the contract of every shipped
//! `code` cell.
//!
//! # What happens instead
//!
//! An inline script above the cap is written to a per-spawn temporary file and
//! the runner is pointed at that path — the very `<runner> <path>` form
//! [`Script::Path`] already uses, so no new invocation shape enters the
//! substrate. The file is created with mode `0600` and `O_EXCL`, and is
//! unlinked again when the [`MaterialisedScript`] guard drops, i.e. once the
//! child has been reaped (or the handler left by any other path).
//!
//! # Isolated, and named after its process (GH #844)
//!
//! The file lives in the SHARED temporary directory, and `python3 <path>` puts
//! the script's own directory first on `sys.path` -- so any module lying next
//! to it (a `json.py` somebody else left in `/tmp`) would win the import over
//! the standard library. The runner is therefore started as
//! `<runner> -I <path>` on this path and only here (`cell.rs`
//! `build_command`): isolated mode keeps the script's directory off
//! `sys.path` and ignores `PYTHON*` and the user site. `-I` exists since
//! Python 3.4; `-P`, the narrower flag, only since 3.11, and no Python minimum
//! is written down anywhere in this tree. No shipped `code` cell imports
//! anything but the standard library or reads `PYTHON*`, so the flag takes
//! nothing away from any of them.
//!
//! The guard's `Drop` does not run when the process is killed, so a SIGKILL
//! leaves the file behind for good. The name carries the pid that wrote it
//! (`meclaw-code-<pid>-<uuid7>.py`), and [`sweep_dead_leftovers`] removes the
//! files of exactly that shape whose pid is no longer a live process -- never
//! a file of a live process, never a file of any other name.
//!
//! # Why only above the cap
//!
//! Below the cap nothing changes. A file and a `-c` string are not perfectly
//! interchangeable to the runner — under `python3 <path>` the script's
//! directory, not the working directory, is `sys.path[0]`, `__file__` exists,
//! and a traceback quotes source lines instead of `<string>` — and 73 of the 74
//! shipped `code` cells run fine on the argv path today. Switching all of them
//! for the sake of one code path would trade a repair for a risk. The cells
//! this touches are exactly the ones that could not run at all.

use meclaw_core::Uuid;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `MAX_ARG_STRLEN` on the target platform: `32 * PAGE_SIZE` with the usual
/// 4 KiB page. A platform with larger pages has a HIGHER real cap, so this
/// value only ever materialises a script earlier than strictly necessary —
/// never too late.
const MAX_ARG_STRLEN: usize = 32 * 4096;

/// The largest inline script that still fits into one `argv` string. The kernel
/// counts the terminating NUL against `MAX_ARG_STRLEN`, so the last size that
/// still spawns is one byte below it (measured: 131 071 spawns, 131 072 does
/// not).
pub const MAX_INLINE_ARGV_BYTES: usize = MAX_ARG_STRLEN - 1;

/// The first part of every materialised script's file name.
const TEMP_PREFIX: &str = "meclaw-code-";

/// The file name of a script this process materialises now:
/// `meclaw-code-<pid>-<uuid7>.py`.
fn temp_name() -> String {
    format!("{TEMP_PREFIX}{}-{}.py", std::process::id(), Uuid::now_v7())
}

/// The pid a file of [`temp_name`]'s exact shape was written by, or `None` for
/// every other name -- including the pre-#844 shape `meclaw-code-<uuid7>.py`,
/// whose first uuid group can be all digits and must not be read as a pid.
fn leftover_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(TEMP_PREFIX)?.strip_suffix(".py")?;
    let (pid, uuid) = rest.split_once('-')?;
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if uuid.len() != 36 || Uuid::parse_str(uuid).is_err() {
        return None;
    }
    pid.parse().ok()
}

/// Is `pid` a running process? A zombie is not: it has exited and only waits
/// to be reaped, and the script it ran is over.
fn pid_is_alive(pid: u32) -> bool {
    crate::orphan_journal::read_identity(pid).is_some_and(|id| !id.is_zombie())
}

/// Would `code` be written to a temporary file on the cold path?
#[must_use]
pub fn is_oversized(code: &str) -> bool {
    code.len() > MAX_INLINE_ARGV_BYTES
}

/// Remove the materialised scripts in `dir` that a process which no longer
/// runs left behind (GH #844), and return how many went.
///
/// Only files of [`temp_name`]'s exact shape are touched, only regular files
/// (a symlink of that name is left alone), only files this process's user owns,
/// and only when their pid is not a live process. The owner comes first: in a
/// shared temporary directory (sticky `/tmp`, a container sharing it with
/// another user) a dead writer's file of another user cannot be unlinked, and
/// without the filter every spawn logged an `EPERM` warning for it (review of
/// strand C, wave substrate). When liveness cannot be established at all -- no `/proc`, as
/// on a non-Linux host -- nothing is removed: a probe that cannot see this very
/// process cannot tell a dead writer from a live one.
///
/// Synchronous: it runs once per spawn of a cell whose script is materialised
/// (`CodeCellFactory::spawn_cell`, which is not async), and it is one
/// `readdir` of the temporary directory plus one `unlink` per leftover.
pub fn sweep_dead_leftovers(dir: &Path) -> usize {
    sweep_with(dir, pid_is_alive, own_uid())
}

/// The user this process runs as, read from its own `/proc` entry (whose owner
/// is the effective uid). `None` where there is no `/proc` -- and then the
/// sweep removes nothing, like the liveness probe it sits beside.
#[cfg(unix)]
fn own_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn own_uid() -> Option<u32> {
    None
}

/// Does this directory entry belong to `uid`? Unknowable off unix: no.
#[cfg(unix)]
fn owned_by(meta: &std::fs::Metadata, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.uid() == uid
}

#[cfg(not(unix))]
fn owned_by(_meta: &std::fs::Metadata, _uid: u32) -> bool {
    false
}

fn sweep_with(dir: &Path, alive: impl Fn(u32) -> bool, own_uid: Option<u32>) -> usize {
    let Some(uid) = own_uid else {
        return 0;
    };
    if !alive(std::process::id()) {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(leftover_pid) else {
            continue;
        };
        // `DirEntry::metadata` does not follow a symlink, so a link of that
        // name is not a regular file here and stays.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() || !owned_by(&meta, uid) || alive(pid) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            // Another process may sweep the same directory at the same time.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %entry.path().display(),
                error = %e,
                "could not remove a code script left behind by a dead process"
            ),
        }
    }
    if removed > 0 {
        tracing::info!(
            dir = %dir.display(),
            removed,
            "removed code scripts left behind by processes that no longer run"
        );
    }
    removed
}

/// A script written to a temporary file for the lifetime of one spawn.
///
/// The unlink happens in `Drop` for the same reason
/// [`crate::orphan_journal::SpawnNote`] retires its record there: every
/// ordinary way out of the handler — return, spawn error, timeout kill, task
/// abort, panic unwind — has to clean up, and no call site should have to
/// remember it.
#[derive(Debug)]
pub struct MaterialisedScript {
    path: PathBuf,
}

impl MaterialisedScript {
    /// The path to hand the runner.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MaterialisedScript {
    fn drop(&mut self) {
        // One `unlink(2)` on a file this process created. Sync on purpose: it
        // is a single sub-millisecond syscall with no data to flush, and `Drop`
        // cannot await. A failure here leaves a temp file behind and is not
        // worth failing an already-finished message over — it is logged, not
        // returned.
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "could not remove the temporary code script"
            );
        }
    }
}

/// Write `code` to a fresh temporary file and return the guard that owns it.
///
/// Operation-timeout per `CLAUDE.md` rule 12: the write is filesystem I/O in
/// cell code, so it carries its own deadline rather than relying on the
/// message-timeout backstop.
async fn write_temp_script(code: &str, timeout: Duration) -> io::Result<MaterialisedScript> {
    let path = std::env::temp_dir().join(temp_name());
    let write = async {
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        // The script is the cell's own source and may carry values that
        // `${VAR}` substitution resolved into it. A world-readable copy in a
        // shared temp directory would be a leak the argv form never had.
        #[cfg(unix)]
        opts.mode(0o600);
        let mut f = opts.open(&path).await?;
        use tokio::io::AsyncWriteExt;
        f.write_all(code.as_bytes()).await?;
        f.flush().await?;
        Ok::<(), io::Error>(())
    };
    match tokio::time::timeout(timeout, write).await {
        // The guard is only built once the bytes are on disk; a failed write
        // leaves nothing to clean up but the file itself, removed here.
        Ok(Ok(())) => Ok(MaterialisedScript { path }),
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "writing the inline script to a temporary file timed out",
            ))
        }
    }
}

/// Materialise `code` into a temporary file **iff** it is too large to travel
/// in one `argv` string; otherwise return `None` and leave the argv path alone.
pub async fn materialise_if_oversized(
    code: &str,
    timeout: Duration,
) -> io::Result<Option<MaterialisedScript>> {
    if !is_oversized(code) {
        return Ok(None);
    }
    write_temp_script(code, timeout).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_script_below_the_cap_is_not_materialised() {
        let small = "x".repeat(MAX_INLINE_ARGV_BYTES);
        assert!(
            materialise_if_oversized(&small, Duration::from_secs(5))
                .await
                .unwrap()
                .is_none(),
            "the last size that still spawns must stay on the argv path"
        );
    }

    #[tokio::test]
    async fn a_script_above_the_cap_lands_on_disk_and_is_removed_again() {
        let big = "x".repeat(MAX_INLINE_ARGV_BYTES + 1);
        let kept_path;
        {
            let m = materialise_if_oversized(&big, Duration::from_secs(5))
                .await
                .unwrap()
                .expect("one byte over the cap must be materialised");
            kept_path = m.path().to_path_buf();
            assert_eq!(std::fs::read_to_string(m.path()).unwrap(), big);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(m.path()).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "the script must not be world-readable");
            }
        }
        assert!(
            !kept_path.exists(),
            "the guard must unlink the script when it drops"
        );
    }

    /// GH #844: the name says which process wrote the file, which is what
    /// makes a leftover attributable at all.
    #[tokio::test]
    async fn a_materialised_script_is_named_after_its_process() {
        let big = "x".repeat(MAX_INLINE_ARGV_BYTES + 1);
        let m = materialise_if_oversized(&big, Duration::from_secs(5))
            .await
            .unwrap()
            .expect("materialised");
        let name = m.path().file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(
            leftover_pid(&name),
            Some(std::process::id()),
            "`meclaw-code-<pid>-<uuid7>.py`, with this process's pid: {name}"
        );
    }

    /// A pid that was running and is not any more: a child that has been
    /// waited for. (Its number could be reused by the time the sweep asks --
    /// then the file is kept, which is the safe way to be wrong.)
    fn a_dead_pid() -> u32 {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        child.wait().expect("reap true");
        pid
    }

    /// GH #844: a leftover of a dead process goes; a live process's file, the
    /// pre-#844 shape and a foreign name stay.
    #[test]
    fn the_sweep_removes_only_our_files_of_dead_processes() {
        let td = tempfile::TempDir::new().unwrap();
        let dir = td.path();
        let dead = a_dead_pid();
        let uuid = Uuid::now_v7();
        let dead_file = dir.join(format!("meclaw-code-{dead}-{uuid}.py"));
        let live_file = dir.join(format!("meclaw-code-{}-{uuid}.py", std::process::id()));
        let old_shape = dir.join(format!("meclaw-code-{uuid}.py"));
        let foreign = dir.join("json.py");
        let not_a_uuid = dir.join(format!("meclaw-code-{dead}-notes.py"));
        for f in [&dead_file, &live_file, &old_shape, &foreign, &not_a_uuid] {
            std::fs::write(f, "x").unwrap();
        }
        assert!(
            !pid_is_alive(dead),
            "the probe must see the reaped child as gone"
        );
        assert_eq!(
            sweep_dead_leftovers(dir),
            1,
            "exactly the dead process's file"
        );
        assert!(!dead_file.exists(), "the dead process's leftover is gone");
        for kept in [&live_file, &old_shape, &foreign, &not_a_uuid] {
            assert!(kept.exists(), "{} must stay", kept.display());
        }
    }

    /// No liveness probe, no sweep: a host where this very process does not
    /// look alive cannot tell a dead writer from a live one.
    #[test]
    fn a_blind_probe_removes_nothing() {
        let td = tempfile::TempDir::new().unwrap();
        let f = td
            .path()
            .join(format!("meclaw-code-1-{}.py", Uuid::now_v7()));
        std::fs::write(&f, "x").unwrap();
        assert_eq!(sweep_with(td.path(), |_| false, own_uid()), 0);
        assert!(f.exists());
    }

    /// A dead writer's file that another user owns is not ours to unlink: the
    /// sweep passes it by instead of failing on it (and warning) at every spawn.
    #[test]
    fn a_leftover_of_another_user_is_left_alone() {
        let td = tempfile::TempDir::new().unwrap();
        let dead = a_dead_pid();
        let f = td
            .path()
            .join(format!("meclaw-code-{dead}-{}.py", Uuid::now_v7()));
        std::fs::write(&f, "x").unwrap();
        let mine = own_uid().expect("a unix test host has /proc");
        let alive = |pid: u32| pid == std::process::id();
        assert_eq!(
            sweep_with(td.path(), alive, Some(mine.wrapping_add(1))),
            0,
            "a file owned by somebody else stays"
        );
        assert!(f.exists());
        assert_eq!(sweep_with(td.path(), alive, Some(mine)), 1, "ours goes");
        assert!(!f.exists());
    }

    #[test]
    fn only_the_exact_shape_names_a_pid() {
        let u = Uuid::now_v7();
        assert_eq!(leftover_pid(&format!("meclaw-code-42-{u}.py")), Some(42));
        assert_eq!(leftover_pid(&format!("meclaw-code-{u}.py")), None);
        assert_eq!(leftover_pid(&format!("meclaw-code--{u}.py")), None);
        assert_eq!(leftover_pid(&format!("meclaw-code-4x2-{u}.py")), None);
        assert_eq!(leftover_pid(&format!("meclaw-code-42-{u}.pyc")), None);
        assert_eq!(leftover_pid("meclaw-code-42-.py"), None);
        assert_eq!(leftover_pid("json.py"), None);
    }
}
