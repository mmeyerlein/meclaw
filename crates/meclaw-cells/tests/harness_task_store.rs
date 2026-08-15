//! P8 block 3 — the task table in `cell.db`.
//!
//! This table is the reason the `harness` cell type can exist at all. Every
//! other cell type is idempotent: replay a message, get the same answer. A
//! harness task mutates a repository, so replaying it is not "the same answer",
//! it is a second run against a workspace somebody may already be reviewing.
//!
//! The table is therefore a TOMBSTONE REGISTER, not a work queue. Nothing in
//! it can be used to resume anything; its only jobs are (a) refusing a task id
//! that already ran, and (b) turning an interrupted task into a loud "unknown
//! outcome" after a restart.

use meclaw_cells::harness::db::{
    finish_task, insert_running_task, load_task, mark_running_as_unknown, set_session_id,
    setup_harness_schema,
};

fn conn() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().expect("open");
    setup_harness_schema(&c).expect("schema");
    c
}

#[test]
fn the_schema_is_idempotent_across_restarts() {
    let c = conn();
    setup_harness_schema(&c).expect("second setup must be a no-op");
    insert_running_task(&c, "t1", "/ws/one", "2026-08-08T00:00:00Z").expect("insert");
    setup_harness_schema(&c).expect("third setup must not drop anything");
    assert!(load_task(&c, "t1").expect("load").is_some());
}

#[test]
fn a_started_task_round_trips_with_its_session_and_outcome() {
    let c = conn();
    insert_running_task(&c, "t1", "/ws/one", "2026-08-08T00:00:00Z").expect("insert");

    let row = load_task(&c, "t1").expect("load").expect("row");
    assert_eq!(row.task_id, "t1");
    assert_eq!(row.workspace, "/ws/one");
    assert_eq!(row.status, "running");
    assert_eq!(row.session_id, None, "the session id is only known later");
    assert_eq!(row.finished_at, None);

    set_session_id(&c, "t1", "sess-42").expect("session");
    finish_task(&c, "t1", "ok", "all done", "2026-08-08T00:05:00Z").expect("finish");

    let row = load_task(&c, "t1").expect("load").expect("row");
    assert_eq!(row.session_id.as_deref(), Some("sess-42"));
    assert_eq!(row.status, "ok");
    assert_eq!(row.detail.as_deref(), Some("all done"));
    assert_eq!(row.finished_at.as_deref(), Some("2026-08-08T00:05:00Z"));
}

#[test]
fn an_unknown_task_id_loads_as_none() {
    let c = conn();
    assert!(load_task(&c, "nope").expect("load").is_none());
}

/// Dedup: the same task id may never run twice. The primary key does the work,
/// and the error surfaces rather than being swallowed.
#[test]
fn the_same_task_id_cannot_be_started_twice() {
    let c = conn();
    insert_running_task(&c, "t1", "/ws/one", "2026-08-08T00:00:00Z").expect("first insert");
    let err = insert_running_task(&c, "t1", "/ws/two", "2026-08-08T00:01:00Z")
        .expect_err("the second insert must fail");
    assert!(
        format!("{err}").to_lowercase().contains("unique")
            || format!("{err}").to_lowercase().contains("constraint"),
        "expected a uniqueness violation, got: {err}"
    );

    // And the first row is untouched — a rejected duplicate must not corrupt
    // the record of the run that is actually happening.
    let row = load_task(&c, "t1").expect("load").expect("row");
    assert_eq!(row.workspace, "/ws/one");
}

/// The restart path, and the single most important behaviour of this package:
/// an interrupted task becomes `unknown`, never a new run.
#[test]
fn a_restart_turns_running_tasks_into_unknown_outcomes() {
    let c = conn();
    insert_running_task(&c, "t1", "/ws/one", "2026-08-08T00:00:00Z").expect("insert");
    insert_running_task(&c, "t2", "/ws/two", "2026-08-08T00:00:01Z").expect("insert");
    finish_task(&c, "t2", "ok", "done", "2026-08-08T00:02:00Z").expect("finish");

    let orphans = mark_running_as_unknown(&c, "2026-08-08T01:00:00Z").expect("recovery");
    assert_eq!(orphans.len(), 1, "only the unfinished task is an orphan");
    assert_eq!(orphans[0].task_id, "t1");
    assert_eq!(
        orphans[0].workspace, "/ws/one",
        "the workspace must come along — it is the only place the outcome can be inspected"
    );

    let row = load_task(&c, "t1").expect("load").expect("row");
    assert_eq!(row.status, "unknown");
    assert_eq!(row.finished_at.as_deref(), Some("2026-08-08T01:00:00Z"));

    // The finished task is not touched by recovery.
    assert_eq!(
        load_task(&c, "t2").expect("load").expect("row").status,
        "ok"
    );

    // Idempotent: a second restart finds nothing left to report.
    let again = mark_running_as_unknown(&c, "2026-08-08T02:00:00Z").expect("recovery");
    assert!(again.is_empty(), "recovery must not re-report old orphans");
}
