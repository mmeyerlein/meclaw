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
    let path = std::env::temp_dir().join(format!("meclaw-code-{}.py", Uuid::now_v7()));
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
    if code.len() <= MAX_INLINE_ARGV_BYTES {
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
}
