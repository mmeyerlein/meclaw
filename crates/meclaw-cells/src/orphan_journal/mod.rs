//! GH #116 — the orphan-process journal: reap tool children after a hard
//! daemon crash.
//!
//! Per-spawn child hygiene (process groups, `kill_on_drop`, the staged
//! `terminate`, the [`ProcessGroupGuard`](crate::stdio_child::spawn)) covers
//! every path on which *this* process still runs code. It covers none of the
//! paths on which it does not: `SIGKILL`, the OOM killer, a yanked power cord.
//! There the `bash`/`code`/`harness`/`mcp`/`subcolony` children keep running,
//! nobody adopts them, and the next boot has no idea they exist.
//!
//! The journal is the missing crash-durable memory:
//!
//! * one fsync'd, append-only JSONL file in `{root}`, next to `colony.db`,
//! * one record per state change (`spawned` / `exited` / `reaped` / `skipped`),
//! * every record carries the child's process IDENTITY (pid plus
//!   [`ProcIdentity`](identity::ProcIdentity)) and the identity of the daemon
//!   that owns it,
//! * boot folds the journal, verifies each survivor and `SIGKILL`s only what it
//!   can positively identify as its own orphan.
//!
//! Deliberately NOT on the colony writer path: the journal has to stay writable
//! exactly when the colony loop is wedged, which is the situation it exists for.
//! And deliberately never `pkill`-by-pattern — only exact, verified pids.

/// Process identity (`/proc/<pid>/stat`), the guard against pid reuse.
pub mod identity;

/// Writing the journal: the append-only file and the per-child RAII note.
pub mod journal;

/// The record shape and the fold that turns a journal into a live-child list.
pub mod record;

/// Boot-time reaping of verified orphans.
pub mod reap;

pub use identity::{ProcIdentity, read_identity};
pub use journal::{
    JOURNAL_FILE, OrphanJournal, SpawnNote, default_path, install, installed, note_spawn,
};
pub use reap::{ReapReport, boot, read_records, reap_orphans};
pub use record::{JournalRecord, RecordState, survivors};
