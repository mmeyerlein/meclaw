//! Reading the journal at boot and reaping what survived the crash.
//!
//! The rules this pass obeys, all of them from GH #116:
//!
//! * **Never a pattern.** No `pkill`, no "anything called `sleep` under our
//!   cgroup" — only the exact pids the journal names.
//! * **Never a bare pid.** A pid is killed only when its live identity matches
//!   the identity recorded at spawn (start time, then executable name). A
//!   mismatch is pid reuse and is skipped with a loud log.
//! * **Never a living owner's child.** An entry whose daemon is still running
//!   belongs to that daemon; the reaper does not touch the entry at all — not
//!   even to retire it, because retiring it would blind the NEXT boot to a
//!   child that is still out there.
//! * **Append, never rewrite.** Outcomes are appended as further records
//!   (`reaped`, `skipped`, `exited`); the fold in
//!   [`survivors`](super::record::survivors) makes them retire the entry. GH
//!   #116 says "compact"; `{root}` is under the No-Delete policy, so the
//!   compaction is logical rather than physical (track ruling B-R1).

use crate::orphan_journal::identity::read_identity;
use crate::orphan_journal::journal::OrphanJournal;
use crate::orphan_journal::record::{JournalRecord, RecordState, survivors};
use std::path::Path;

/// What one boot-time reap did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReapReport {
    /// Entries the fold left as "spawned".
    pub examined: usize,
    /// Verified orphans that were killed.
    pub reaped: usize,
    /// Entries whose process was already gone.
    pub gone: usize,
    /// Entries deliberately not killed (pid reuse, unverifiable identity).
    pub skipped: usize,
    /// Entries owned by a daemon that is still running — left untouched.
    pub owned_by_live_daemon: usize,
}

/// Read every parsable record from `path`. A missing file is an empty journal;
/// a corrupt line is skipped with a warning rather than failing the boot — a
/// half-written last line is the expected shape after a crash mid-append.
pub fn read_records(path: &Path) -> Vec<JournalRecord> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(journal = %path.display(), error = %e, "orphan journal: unreadable");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<JournalRecord>(line) {
            Ok(rec) => out.push(rec),
            Err(e) => tracing::warn!(
                journal = %path.display(),
                line = n + 1,
                error = %e,
                "orphan journal: unparsable line skipped"
            ),
        }
    }
    out
}

/// Is the daemon named by this record still running?
fn owner_is_alive(rec: &JournalRecord) -> bool {
    let Some(recorded) = rec.daemon_start_id else {
        // An owner we cannot identify is not evidence of a live owner; the
        // child's own identity check downstream still protects the innocent.
        return false;
    };
    read_identity(rec.daemon_pid).is_some_and(|id| id.start_id == recorded)
}

/// Verdict for one survivor.
enum Verdict {
    Kill,
    Gone,
    Skip(String),
    LiveOwner,
}

fn judge(rec: &JournalRecord) -> Verdict {
    if owner_is_alive(rec) {
        return Verdict::LiveOwner;
    }
    let Some(recorded) = rec.start_id else {
        return Verdict::Skip("no start_id recorded — identity unverifiable".into());
    };
    let Some(live) = read_identity(rec.pid) else {
        return Verdict::Gone;
    };
    // A zombie has already stopped running; signalling it changes nothing and
    // its `/proc` entry disappears as soon as somebody waits for it.
    if live.is_zombie() {
        return Verdict::Gone;
    }
    if live.start_id != recorded {
        return Verdict::Skip(format!(
            "pid reuse: recorded start_id {recorded}, live process started at {}",
            live.start_id
        ));
    }
    if let Some(recorded_comm) = &rec.comm
        && *recorded_comm != live.comm
    {
        return Verdict::Skip(format!(
            "identity mismatch: recorded comm {recorded_comm:?}, live comm {:?}",
            live.comm
        ));
    }
    Verdict::Kill
}

/// SIGKILL a verified orphan: its process group first when it leads one, then
/// the process itself.
///
/// The group sweep only runs when the recorded pgid IS the recorded pid — that
/// is the only case in which the identity check above has actually verified the
/// group leader. A pgid that names some other process is not covered by any
/// evidence we hold, so it is left alone.
#[cfg(unix)]
fn kill_verified(rec: &JournalRecord) {
    if rec.pgid == Some(rec.pid) {
        let _ = crate::stdio_child::reap::killpg(rec.pid, crate::stdio_child::reap::SIGKILL);
    }
    let _ = crate::stdio_child::reap::kill(rec.pid, crate::stdio_child::reap::SIGKILL);
}

#[cfg(not(unix))]
fn kill_verified(rec: &JournalRecord) {
    let _ = rec;
}

/// Walk the journal at `path` and reap what a previous daemon left behind.
///
/// Runs at boot, BEFORE any cell is spawned, so that the journal it folds
/// contains only the previous run's entries. `journal` is the handle the
/// outcome records are appended through — the same file, but written with THIS
/// daemon's identity as owner.
pub fn reap_orphans(path: &Path, journal: &OrphanJournal) -> ReapReport {
    let records = read_records(path);
    let alive = survivors(&records);
    let mut report = ReapReport {
        examined: alive.len(),
        ..ReapReport::default()
    };
    for rec in alive {
        match judge(&rec) {
            Verdict::LiveOwner => {
                report.owned_by_live_daemon += 1;
                tracing::info!(
                    pid = rec.pid,
                    daemon_pid = rec.daemon_pid,
                    cell = %rec.cell_path,
                    "orphan journal: entry belongs to a running daemon — untouched"
                );
            }
            Verdict::Gone => {
                report.gone += 1;
                journal.append(&outcome(&rec, RecordState::Exited, "not running at boot"));
            }
            Verdict::Skip(why) => {
                report.skipped += 1;
                tracing::warn!(
                    pid = rec.pid,
                    cell = %rec.cell_path,
                    reason = %why,
                    "orphan journal: NOT killing this pid — identity could not be confirmed"
                );
                journal.append(&outcome(&rec, RecordState::Skipped, &why));
            }
            Verdict::Kill => {
                kill_verified(&rec);
                report.reaped += 1;
                tracing::warn!(
                    pid = rec.pid,
                    pgid = ?rec.pgid,
                    cell = %rec.cell_path,
                    spawned_at = rec.spawned_at,
                    "orphan journal: reaped a tool child orphaned by a hard daemon crash"
                );
                journal.append(&outcome(
                    &rec,
                    RecordState::Reaped,
                    "verified orphan, SIGKILL",
                ));
            }
        }
    }
    report
}

/// Build the outcome record that retires `rec`.
fn outcome(rec: &JournalRecord, state: RecordState, note: &str) -> JournalRecord {
    JournalRecord {
        state,
        note: Some(note.to_string()),
        ..rec.clone()
    }
}

/// The boot entry point: reap the previous run's orphans, then install the
/// journal so this run's children are recorded too.
///
/// Order is load-bearing in both directions — the reap must see a journal that
/// contains no entry of ours, and the install must happen before the first cell
/// spawns. Both hold because the boot path calls this before `colony_task`.
pub fn boot(root: &Path) -> ReapReport {
    let path = crate::orphan_journal::journal::default_path(root);
    let journal = OrphanJournal::at(path.clone());
    let report = reap_orphans(&path, &journal);
    if !crate::orphan_journal::journal::install(journal) {
        tracing::warn!("orphan journal: already installed in this process — keeping the first");
    }
    report
}
