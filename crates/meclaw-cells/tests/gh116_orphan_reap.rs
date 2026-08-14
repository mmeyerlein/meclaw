//! GH #116 — the orphan-process journal, proved end to end.
//!
//! The load-bearing claim is a negative one about a code path that never runs:
//! after a `SIGKILL`ed daemon, no `Drop`, no `kill_on_drop` and no `terminate`
//! executes, so the ONLY thing that can still reach the child is a record that
//! was on disk before the crash. This file proves that with a real child
//! process, a real hard kill and a real re-boot — plus the two ways the reaper
//! must refuse to kill: a recycled pid and an entry that still has a living
//! owner.

use meclaw_cells::orphan_journal::{
    self, JournalRecord, OrphanJournal, RecordState, read_identity,
};
use tokio::io::{AsyncBufReadExt, BufReader};

const FIXTURE: &str = env!("CARGO_BIN_EXE_orphan_journal_fixture");

/// Failure-marker budget (30 s convention): generous, because it only bounds
/// "did the thing ever happen", never a semantic timing claim.
const MARKER: std::time::Duration = std::time::Duration::from_secs(30);

/// Is this pid a live, non-zombie process?
fn is_running(pid: u32) -> bool {
    read_identity(pid).is_some_and(|id| !id.is_zombie())
}

/// A journal handle on a fresh temp root.
fn journal_at(root: &std::path::Path) -> (std::path::PathBuf, OrphanJournal) {
    let path = orphan_journal::default_path(root);
    let journal = OrphanJournal::at(path.clone());
    (path, journal)
}

/// A minimal record naming `pid`, owned by a daemon that no longer exists.
fn record_owned_by_a_dead_daemon(pid: u32, start_id: Option<u64>) -> JournalRecord {
    JournalRecord {
        pid,
        start_id,
        comm: None,
        pgid: None,
        cell_path: "/tools/bash".to_string(),
        spawned_at: 1_700_000_000,
        state: RecordState::Spawned,
        // A pid far above any plausible `pid_max`, so the owner cannot be alive.
        daemon_pid: 999_999_999,
        daemon_start_id: Some(1),
        note: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hard_daemon_crash_leaves_an_orphan_that_the_next_boot_reaps() {
    let root = tempfile::tempdir().expect("tempdir");

    // A stand-in daemon: it boots the journal and spawns one real child through
    // the production spawn path, then parks.
    let mut daemon = tokio::process::Command::new(FIXTURE)
        .arg(root.path())
        .arg("600")
        .stdout(std::process::Stdio::piped())
        // Null, not piped: the child inherits stderr, so a piped one would stay
        // open after the daemon dies and could wedge a reader.
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("fixture daemon spawns");

    let stdout = daemon.stdout.take().expect("stdout piped");
    let mut lines = BufReader::new(stdout).lines();
    let line = tokio::time::timeout(MARKER, lines.next_line())
        .await
        .expect("fixture printed a pid within the marker budget")
        .expect("stdout readable")
        .expect("fixture printed a line");
    let child_pid: u32 = line.trim().parse().expect("the line is a pid");
    assert!(is_running(child_pid), "the child must be running to start");

    // THE CRASH. SIGKILL runs no destructor of ours — this is the OOM / hard
    // kill case that per-spawn hygiene cannot cover.
    daemon.start_kill().expect("SIGKILL the stand-in daemon");
    daemon.wait().await.expect("daemon reaped");

    assert!(
        is_running(child_pid),
        "pid {child_pid} must have OUTLIVED its daemon — otherwise this test \
         proves nothing about reaping"
    );

    // THE RE-BOOT.
    let (path, journal) = journal_at(root.path());
    let report = orphan_journal::reap_orphans(&path, &journal);
    assert_eq!(
        report.reaped, 1,
        "exactly the one orphan must be reaped: {report:?}"
    );
    assert_eq!(report.skipped, 0, "nothing was refused: {report:?}");

    // THE PROOF: the process is gone.
    let deadline = std::time::Instant::now() + MARKER;
    while is_running(child_pid) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !is_running(child_pid),
        "pid {child_pid} still runs after the boot reap"
    );

    // And the journal says so — by appending, never by rewriting.
    let recs = orphan_journal::read_records(&path);
    assert!(
        recs.iter()
            .any(|r| r.pid == child_pid && r.state == RecordState::Spawned),
        "the spawn record must still be in the file: {recs:?}"
    );
    assert!(
        recs.iter()
            .any(|r| r.pid == child_pid && r.state == RecordState::Reaped),
        "a reaped record must have been appended: {recs:?}"
    );
    assert!(
        orphan_journal::survivors(&recs).is_empty(),
        "the fold must show nothing outstanding: {recs:?}"
    );
}

#[test]
fn a_recycled_pid_is_skipped_loudly_instead_of_killed() {
    let root = tempfile::tempdir().expect("tempdir");
    let (path, journal) = journal_at(root.path());

    // The nastiest shape available: an entry naming THIS test process, with a
    // start_id that cannot match. A reaper that trusts the bare pid kills the
    // test runner; a correct one refuses.
    let me = std::process::id();
    let mine = read_identity(me).expect("we have an identity");
    let mut rec = record_owned_by_a_dead_daemon(me, Some(mine.start_id.wrapping_add(1)));
    rec.comm = Some(mine.comm.clone());
    journal.append(&rec);

    let report = orphan_journal::reap_orphans(&path, &journal);
    assert_eq!(report.skipped, 1, "the reuse must be refused: {report:?}");
    assert_eq!(report.reaped, 0, "nothing may be killed: {report:?}");

    let recs = orphan_journal::read_records(&path);
    let skip = recs
        .iter()
        .rev()
        .find(|r| r.state == RecordState::Skipped)
        .expect("a skipped record was appended");
    assert!(
        skip.note.as_deref().unwrap_or("").contains("pid reuse"),
        "the skip must name the reason: {:?}",
        skip.note
    );
    assert!(
        orphan_journal::survivors(&recs).is_empty(),
        "a refused entry is retired, not re-examined every boot"
    );
}

#[test]
fn an_unverifiable_entry_is_never_killed() {
    let root = tempfile::tempdir().expect("tempdir");
    let (path, journal) = journal_at(root.path());

    // No start_id at all — the identity check has nothing to work with.
    journal.append(&record_owned_by_a_dead_daemon(std::process::id(), None));

    let report = orphan_journal::reap_orphans(&path, &journal);
    assert_eq!(report.reaped, 0, "no identity, no kill: {report:?}");
    assert_eq!(report.skipped, 1, "and it is reported: {report:?}");
}

#[test]
fn a_wrong_executable_name_is_a_mismatch_even_when_the_start_id_fits() {
    let root = tempfile::tempdir().expect("tempdir");
    let (path, journal) = journal_at(root.path());

    let me = std::process::id();
    let mine = read_identity(me).expect("we have an identity");
    let mut rec = record_owned_by_a_dead_daemon(me, Some(mine.start_id));
    rec.comm = Some(format!("not-{}", mine.comm));
    journal.append(&rec);

    let report = orphan_journal::reap_orphans(&path, &journal);
    assert_eq!(report.reaped, 0, "the second signal must veto: {report:?}");
    assert_eq!(report.skipped, 1, "{report:?}");
}

#[test]
fn an_entry_whose_daemon_still_runs_is_not_touched_at_all() {
    let root = tempfile::tempdir().expect("tempdir");
    let (path, journal) = journal_at(root.path());

    // Owner = this very process, which is demonstrably alive. Journal hygiene:
    // a concurrent run's children are somebody else's business, and retiring
    // the entry would blind the NEXT boot to a child that is still out there.
    let me = std::process::id();
    let mine = read_identity(me).expect("we have an identity");
    let mut rec = record_owned_by_a_dead_daemon(me, Some(mine.start_id));
    rec.comm = Some(mine.comm.clone());
    rec.daemon_pid = me;
    rec.daemon_start_id = Some(mine.start_id);
    journal.append(&rec);

    let before = std::fs::read_to_string(&path).expect("journal readable");
    let report = orphan_journal::reap_orphans(&path, &journal);
    assert_eq!(report.owned_by_live_daemon, 1, "{report:?}");
    assert_eq!(report.reaped, 0, "{report:?}");
    assert_eq!(report.skipped, 0, "{report:?}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("journal readable"),
        before,
        "a live owner's entry must not even be annotated"
    );
    assert_eq!(
        orphan_journal::survivors(&orphan_journal::read_records(&path)).len(),
        1,
        "and it must still be outstanding for the next boot"
    );
}

#[test]
fn a_half_written_last_line_does_not_break_the_boot() {
    let root = tempfile::tempdir().expect("tempdir");
    let (path, journal) = journal_at(root.path());
    journal.append(&record_owned_by_a_dead_daemon(999_999_998, Some(1)));
    // The expected shape after a crash mid-append.
    std::fs::write(
        &path,
        format!(
            "{}{{\"pid\":123,\"start_",
            std::fs::read_to_string(&path).expect("journal readable")
        ),
    )
    .expect("journal writable");

    let report = orphan_journal::reap_orphans(&path, &journal);
    assert_eq!(report.examined, 1, "the intact record still counts");
    assert_eq!(report.gone, 1, "and its process is long gone: {report:?}");
}

#[test]
fn a_missing_journal_is_an_empty_one() {
    let root = tempfile::tempdir().expect("tempdir");
    let (path, journal) = journal_at(root.path());
    assert_eq!(orphan_journal::reap_orphans(&path, &journal).examined, 0);
    assert!(!path.exists(), "a clean boot must not create the file");
}
