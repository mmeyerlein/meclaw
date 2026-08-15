//! GH #116 — the spawn sites actually write to the journal.
//!
//! The reap in `gh116_orphan_reap.rs` can only find what somebody recorded, so
//! this battery pins the other half: a real `bash` cell, driven through its
//! production factory, leaves a `spawned` record and retires it with `exited`
//! when the child is done.
//!
//! Own test binary on purpose — installing the process-wide journal is a
//! one-shot per process, so it must not race the other file's tests.
//!
//! The reply is NOT a receipt for the retire record (GH #134): the spawn site
//! pushes the `tool_result` to its sink and only then leaves the scope that
//! holds the `SpawnNote`, whose `Drop` writes and fsyncs the `exited` line. So
//! "the emission is here" always precedes "the exit record is on disk", and
//! under load the gap is wide enough to lose a race that was never promised.
//! Every read of the journal therefore goes through [`records_when_settled`].

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::BashCellFactory;
use meclaw_cells::orphan_journal::{self, JournalRecord, OrphanJournal, RecordState};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::ToolRig;

/// Failure-marker budget (30 s convention): generous, because it only bounds
/// "did the write ever happen", never a semantic timing claim.
const MARKER: Duration = Duration::from_secs(30);

/// The journal once it holds at least `want` records, or a panic at the marker.
///
/// Bounded poll instead of an immediate read — the `exited` record is appended
/// by a `Drop` that runs after the reply was already handed to the test, so the
/// caller must wait for the record, not for the reply.
async fn records_when_settled(path: &Path, want: usize) -> Vec<JournalRecord> {
    let deadline = Instant::now() + MARKER;
    loop {
        let recs = orphan_journal::read_records(path);
        if recs.len() >= want {
            return recs;
        }
        assert!(
            Instant::now() < deadline,
            "only {} of {want} journal records after {MARKER:?}: {recs:?}",
            recs.len()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bash_child_is_journalled_on_spawn_and_retired_on_exit() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = orphan_journal::default_path(root.path());
    assert!(
        orphan_journal::install(OrphanJournal::at(path.clone())),
        "this binary installs the journal exactly once"
    );

    let mut rig = ToolRig::spawn(
        Arc::new(BashCellFactory) as Arc<dyn CellFactory>,
        "/tools/bash",
        json!({"max_concurrency": 1, "external_timeout_ms": 20000}),
    );
    let em = rig.call(json!({"command": "echo journalled"}), "c1").await;
    assert_eq!(
        em.content["messages"][0]["text"].as_str(),
        Some("journalled\n"),
        "the command must have really run"
    );

    let recs = records_when_settled(&path, 2).await;
    assert_eq!(
        recs.len(),
        2,
        "one spawn plus one exit record, nothing else: {recs:?}"
    );
    assert_eq!(recs[0].state, RecordState::Spawned, "{recs:?}");
    assert_eq!(recs[1].state, RecordState::Exited, "{recs:?}");
    assert_eq!(recs[0].pid, recs[1].pid, "the same child: {recs:?}");
    assert_eq!(
        recs[0].cell_path, "/tools/bash",
        "the record names the spawning cell: {recs:?}"
    );
    assert!(
        recs[0].start_id.is_some(),
        "without a start_id the entry would be unreapable: {recs:?}"
    );
    assert_eq!(
        recs[0].daemon_pid,
        std::process::id(),
        "the owner is this process: {recs:?}"
    );
    assert!(
        orphan_journal::survivors(&recs).is_empty(),
        "a finished child leaves nothing outstanding: {recs:?}"
    );
}

#[test]
fn without_an_installed_journal_a_spawn_note_is_inert() {
    // The library-use / unit-test shape: nothing installed for THIS pid's
    // records beyond what the test above did, and an inert note writes nothing.
    let note = orphan_journal::SpawnNote::inert();
    assert!(note.record().is_none());
}
