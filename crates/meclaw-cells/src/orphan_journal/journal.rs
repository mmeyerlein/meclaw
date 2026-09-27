//! Writing the journal: the append-only, fsync'd file and the RAII note that
//! every spawn site holds for as long as its child lives.
//!
//! Everything here is synchronous `std::fs`. That is a decision, not an
//! oversight (track ruling B-R2):
//!
//! * the record must hit stable storage BEFORE the spawn path goes on, and the
//!   whole point of the file is to survive a `SIGKILL` between two
//!   instructions — the write is awaited, never fired and forgotten;
//! * [`SpawnNote::drop`] runs on teardown paths where nothing is awaited any
//!   more (aborted task, panicking peer, colony exit), and `Drop` cannot await;
//! * the same reasoning already makes
//!   [`StdioChild::spawn`](crate::stdio_child::StdioChild::spawn) synchronous.
//!
//! Cost is one small `open`/`write`/`fsync` per child, paid twice per tool call.
//!
//! **Synchronous, but never on a runtime worker (GH #866).** Under write-back
//! pressure an ext4 `fdatasync` waits for the journal commit — measured on a
//! live colony as hundreds of milliseconds, 87 of 124 fatal `colony_loop`
//! trips in the second in which it spawns its children. A worker blocked there
//! polls nothing else, and the colony task can sit in that worker's LIFO slot,
//! where no other worker may steal it. So the `code` and `bash` spawn sites call
//! [`note_spawn`] inside the same `spawn_blocking` section as the `fork` (the
//! record has to be read from the thread that forked, see
//! [`read_settled_identity`]) and await it, and they retire the note with
//! [`SpawnNote::retire`], which writes the `exited` record in `spawn_blocking`
//! as well. `Drop` stays the fallback for the paths that cannot await.
//! `StdioChild::spawn` is a synchronous `pub fn` and still journals on its
//! caller's thread (`reg:stdio-child-spawn-off-worker`).

use crate::orphan_journal::identity::{read_identity, read_settled_identity};
use crate::orphan_journal::record::{JournalRecord, RecordState};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The journal file name inside `{root}` — beside `colony.db`, as GH #116 asks.
pub const JOURNAL_FILE: &str = "orphan-journal.jsonl";

/// Where the journal lives for a colony rooted at `root`.
pub fn default_path(root: &Path) -> PathBuf {
    root.join(JOURNAL_FILE)
}

/// A handle on one colony's journal. Cheap to clone; holds no open file and no
/// lock, so nothing here can wedge a spawn.
#[derive(Debug, Clone)]
pub struct OrphanJournal {
    path: PathBuf,
    daemon_pid: u32,
    daemon_start_id: Option<u64>,
}

impl OrphanJournal {
    /// Bind a journal to `path`, capturing this daemon's own identity once.
    pub fn at(path: PathBuf) -> Self {
        let pid = std::process::id();
        Self {
            path,
            daemon_pid: pid,
            daemon_start_id: read_identity(pid).map(|i| i.start_id),
        }
    }

    /// The file this journal appends to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Identity of the daemon that owns the records this journal writes.
    pub fn owner(&self) -> (u32, Option<u64>) {
        (self.daemon_pid, self.daemon_start_id)
    }

    /// Append one record and fsync it. Errors are logged, never returned to the
    /// spawn path: a journal that cannot be written degrades the crash
    /// behaviour, it does not break a working tool call (GH #116).
    pub fn append(&self, rec: &JournalRecord) {
        if let Err(e) = self.try_append(rec) {
            tracing::warn!(
                journal = %self.path.display(),
                pid = rec.pid,
                error = %e,
                "orphan journal: append failed — a crash from here on may leak this child"
            );
        }
    }

    fn try_append(&self, rec: &JournalRecord) -> std::io::Result<()> {
        let mut line = serde_json::to_string(rec)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.sync_data()
    }

    /// Journal a freshly spawned child and hand back the note that retires it.
    ///
    /// `pid` is `None` when the child was already reaped by the time the caller
    /// asked (`Child::id()` goes `None` then) — nothing to journal, and the
    /// returned note is inert.
    pub fn note_spawn(&self, pid: Option<u32>, pgid: Option<u32>, cell_path: &str) -> SpawnNote {
        let Some(pid) = pid else {
            return SpawnNote::inert();
        };
        // Settled, not raw: right after `Command::spawn` the child can still be
        // its own pre-exec image, whose `comm` is OUR thread's name. Journalling
        // that name would make the boot reaper veto a genuine orphan on a
        // manufactured "identity mismatch" (GH #116).
        let ident = read_settled_identity(pid);
        let rec = JournalRecord {
            pid,
            start_id: ident.as_ref().map(|i| i.start_id),
            comm: ident.map(|i| i.comm),
            pgid,
            cell_path: cell_path.to_string(),
            spawned_at: now_secs(),
            state: RecordState::Spawned,
            daemon_pid: self.daemon_pid,
            daemon_start_id: self.daemon_start_id,
            note: None,
        };
        self.append(&rec);
        SpawnNote {
            journal: Some(self.clone()),
            rec: Some(rec),
        }
    }
}

/// Unix seconds, saturating to 0 on a clock before the epoch.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Retires a journal entry when the child it names is done with.
///
/// Held next to the child by every spawn site, so that every ordinary teardown
/// path — return, timeout kill, task abort, panic unwind — writes the `exited`
/// record without the call site having to remember it. The paths that DO NOT
/// run `Drop` are precisely the ones the journal exists for. A site that can
/// await retires the note with [`SpawnNote::retire`] instead, which writes the
/// same record off the runtime workers (GH #866); `Drop` is then a no-op.
#[derive(Debug)]
pub struct SpawnNote {
    journal: Option<OrphanJournal>,
    rec: Option<JournalRecord>,
}

impl SpawnNote {
    /// A note that journals nothing — no journal installed, or no pid to name.
    pub fn inert() -> Self {
        Self {
            journal: None,
            rec: None,
        }
    }

    /// The record this note would retire, for tests and diagnostics.
    pub fn record(&self) -> Option<&JournalRecord> {
        self.rec.as_ref()
    }

    /// Retire the child with its `exited` record, written off the runtime
    /// workers and awaited (GH #866).
    ///
    /// The same record [`Drop`] would write, from a blocking-pool thread: the
    /// `fsync` behind it can wait hundreds of milliseconds on a busy disk, and
    /// on a worker that wait holds whatever the worker would poll next. The
    /// note is empty afterwards, so the `Drop` that follows writes nothing. Call
    /// it once the child is reaped; `Drop` stays the fallback for every path
    /// that never gets here (abort, panic, colony exit).
    pub async fn retire(mut self) {
        let (Some(journal), Some(mut rec)) = (self.journal.take(), self.rec.take()) else {
            return;
        };
        rec.state = RecordState::Exited;
        let fallback = (journal.clone(), rec.clone());
        if let Err(e) = tokio::task::spawn_blocking(move || journal.append(&rec)).await {
            // Only a runtime that is shutting down refuses the blocking task;
            // the record is still owed, so it is written here, as `Drop` would.
            tracing::debug!(error = %e, "orphan journal: retire ran on the caller's thread");
            fallback.0.append(&fallback.1);
        }
    }
}

impl Drop for SpawnNote {
    fn drop(&mut self) {
        let (Some(journal), Some(mut rec)) = (self.journal.take(), self.rec.take()) else {
            return;
        };
        rec.state = RecordState::Exited;
        journal.append(&rec);
    }
}

/// The process-wide journal, installed once by the boot path.
///
/// A `OnceLock` holding an immutable handle — the same shape the token broker
/// already uses in this crate. It is process configuration, not actor state, so
/// it does not touch the "no `Mutex` in cell/colony state" invariant: nothing
/// here is mutable after installation and nothing is shared but a `PathBuf`.
static JOURNAL: OnceLock<OrphanJournal> = OnceLock::new();

/// Install the process-wide journal. Returns `false` if one was already
/// installed (the first one keeps winning — a second colony in the same process
/// is not a supported shape).
pub fn install(journal: OrphanJournal) -> bool {
    JOURNAL.set(journal).is_ok()
}

/// The installed journal, or `None` in a process that never booted a colony
/// (library use, unit tests). Then journaling is a silent no-op.
pub fn installed() -> Option<&'static OrphanJournal> {
    JOURNAL.get()
}

/// Journal a spawned child against the process-wide journal, or hand back an
/// inert note when there is none. The one call every spawn site makes.
pub fn note_spawn(pid: Option<u32>, pgid: Option<u32>, cell_path: &str) -> SpawnNote {
    match installed() {
        Some(j) => j.note_spawn(pid, pgid, cell_path),
        None => SpawnNote::inert(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orphan_journal::read_records;

    /// GH #866: `retire` writes exactly the record `Drop` writes, just off the
    /// runtime workers. Two notes on the same child, one retired and one
    /// dropped: their `exited` records are field for field the same.
    #[tokio::test]
    async fn retire_writes_the_record_drop_would_write() {
        let td = tempfile::tempdir().expect("tempdir");
        let path = td.path().join(JOURNAL_FILE);
        let journal = OrphanJournal::at(path.clone());
        let retired = journal.note_spawn(Some(std::process::id()), Some(7), "/tools/x");
        let mut dropped = journal.note_spawn(Some(std::process::id()), Some(7), "/tools/x");
        // Same spawn time on both, so the comparison is about the fields the
        // retire path writes, not about a second boundary between two calls.
        if let (Some(a), Some(b)) = (retired.rec.as_ref(), dropped.rec.as_mut()) {
            b.spawned_at = a.spawned_at;
        }
        retired.retire().await;
        drop(dropped);

        let recs = read_records(&path);
        assert_eq!(recs.len(), 4, "two spawned, two exited: {recs:?}");
        assert_eq!(recs[2].state, RecordState::Exited, "{recs:?}");
        assert_eq!(recs[2], recs[3], "retire and Drop write the same record");
    }

    /// A retired note is empty: the `Drop` that ends it writes nothing more.
    #[tokio::test]
    async fn a_retired_note_drops_without_a_second_record() {
        let td = tempfile::tempdir().expect("tempdir");
        let path = td.path().join(JOURNAL_FILE);
        let note = OrphanJournal::at(path.clone()).note_spawn(Some(std::process::id()), None, "/c");
        note.retire().await;
        let recs = read_records(&path);
        assert_eq!(
            recs.len(),
            2,
            "one spawned, one exited, nothing else: {recs:?}"
        );
        assert_eq!(recs[1].state, RecordState::Exited, "{recs:?}");
    }

    /// The `spawned` record taken inside a blocking section names the CHILD,
    /// not the blocking thread that forked it (the GH #116 trap, `identity.rs`):
    /// the spawn and the read share one thread, so the settle loop compares the
    /// child against the right spawner. And a `tokio::process::Command` spawns
    /// from a blocking-pool thread at all -- the form the spawn sites use.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_spawn_journalled_off_the_workers_names_the_child() {
        let td = tempfile::tempdir().expect("tempdir");
        let path = td.path().join(JOURNAL_FILE);
        let journal = OrphanJournal::at(path.clone());
        let (mut child, note) = tokio::task::spawn_blocking(move || {
            let child = tokio::process::Command::new("sleep")
                .arg("30")
                .kill_on_drop(true)
                .spawn()
                .expect("sleep spawns from a blocking thread");
            let note = journal.note_spawn(child.id(), None, "/c");
            (child, note)
        })
        .await
        .expect("the blocking section returns");
        assert_eq!(
            note.record().and_then(|r| r.comm.as_deref()),
            Some("sleep"),
            "the record names the child: {:?}",
            note.record()
        );
        child.kill().await.expect("kill the child");
        note.retire().await;
        let recs = read_records(&path);
        assert_eq!(recs.len(), 2, "{recs:?}");
    }
}
