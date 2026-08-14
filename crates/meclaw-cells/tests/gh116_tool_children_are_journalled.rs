//! GH #116 — the spawn sites actually write to the journal.
//!
//! The reap in `gh116_orphan_reap.rs` can only find what somebody recorded, so
//! this battery pins the other half: a real `bash` cell, driven through its
//! production factory, leaves a `spawned` record and retires it with `exited`
//! when the child is done.
//!
//! Own test binary on purpose — installing the process-wide journal is a
//! one-shot per process, so it must not race the other file's tests.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::BashCellFactory;
use meclaw_cells::orphan_journal::{self, OrphanJournal, RecordState};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use support::ToolRig;

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

    let recs = orphan_journal::read_records(&path);
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
