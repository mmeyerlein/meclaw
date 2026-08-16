//! Writing the journal: the append-only, fsync'd file and the RAII note that
//! every spawn site holds for as long as its child lives.
//!
//! Everything here is synchronous `std::fs`. That is a decision, not an
//! oversight (track ruling B-R2):
//!
//! * the record must hit stable storage BEFORE the daemon can plausibly die,
//!   and the whole point of the file is to survive a `SIGKILL` between two
//!   instructions — an `.await` there would widen exactly the window we close;
//! * [`SpawnNote::drop`] runs on teardown paths where nothing is awaited any
//!   more (aborted task, panicking peer, colony exit), and `Drop` cannot await;
//! * the same reasoning already makes
//!   [`StdioChild::spawn`](crate::stdio_child::StdioChild::spawn) synchronous.
//!
//! Cost is one small `open`/`write`/`fsync` per child, paid twice per tool call.

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
/// run `Drop` are precisely the ones the journal exists for.
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
