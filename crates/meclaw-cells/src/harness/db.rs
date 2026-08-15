//! `cell.db.harness_tasks` — the task tombstone register.
//!
//! Sync rusqlite, like every other cell-type schema module: called in the
//! factory before `DbConn::wrap` (outside the restart corridor) and in the
//! handler through `DbConn::call_with_timeout(|c| ...)`.
//!
//! **What this table is NOT:** a work queue. There is no "pending" status and
//! no way to ask it for something to run. A row is written before a child is
//! spawned and updated when the outcome is known; a restart converts every
//! unfinished row into `unknown`. Nothing here ever causes a task to start.

use rusqlite::{Connection, params};

/// One row of `harness_tasks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    /// Caller-supplied or cell-generated id. Primary key: the dedup guarantee.
    pub task_id: String,
    /// Vendor session id, learned from the child's init event.
    pub session_id: Option<String>,
    /// Absolute workspace path the task was started against.
    pub workspace: String,
    /// `running` | `ok` | `error` | `crashed` | `cancelled` | `unknown`.
    pub status: String,
    /// RFC-3339 timestamp of the spawn decision.
    pub started_at: String,
    /// RFC-3339 timestamp of the outcome, once there is one.
    pub finished_at: Option<String>,
    /// Short human-readable outcome note.
    pub detail: Option<String>,
}

/// Idempotent DDL. Safe across restarts.
pub fn setup_harness_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS harness_tasks (
            task_id     TEXT PRIMARY KEY,
            session_id  TEXT,
            workspace   TEXT NOT NULL,
            status      TEXT NOT NULL,
            started_at  TEXT NOT NULL,
            finished_at TEXT,
            detail      TEXT
        );",
    )
}

/// Record a task as `running` BEFORE its child process is spawned.
///
/// The order is the invariant (phase-5 canon, state before action): a crash
/// between this write and the spawn leaves a row that recovery reports as
/// `unknown` — noisy but harmless. The reverse order would leave a running
/// agent process with no record at all.
///
/// A duplicate `task_id` fails on the primary key, and the caller turns that
/// into an `invalid_input` reject. That is the dedup guarantee: the same task
/// id cannot be run twice, whatever the topology retries.
pub fn insert_running_task(
    conn: &Connection,
    task_id: &str,
    workspace: &str,
    started_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO harness_tasks (task_id, workspace, status, started_at)
         VALUES (?1, ?2, 'running', ?3)",
        params![task_id, workspace, started_at],
    )?;
    Ok(())
}

/// Attach the vendor session id, learned from the child's init event. It is
/// the audit anchor: transcript and resume both key off it.
pub fn set_session_id(conn: &Connection, task_id: &str, session_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE harness_tasks SET session_id = ?2 WHERE task_id = ?1",
        params![task_id, session_id],
    )?;
    Ok(())
}

/// Close a task with its outcome.
pub fn finish_task(
    conn: &Connection,
    task_id: &str,
    status: &str,
    detail: &str,
    finished_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE harness_tasks SET status = ?2, detail = ?3, finished_at = ?4 WHERE task_id = ?1",
        params![task_id, status, detail, finished_at],
    )?;
    Ok(())
}

/// One task's last known state, for the status lookup.
pub fn load_task(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<TaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT task_id, session_id, workspace, status, started_at, finished_at, detail
         FROM harness_tasks WHERE task_id = ?1",
    )?;
    let mut rows = stmt.query_map(params![task_id], row_of)?;
    rows.next().transpose()
}

/// Restart recovery: every task still marked `running` becomes `unknown`, and
/// the affected rows are returned so the cell can report them.
///
/// This is the whole non-idempotency answer. The cell cannot know whether an
/// interrupted task changed the workspace, so it says exactly that and hands
/// over the path. It never re-runs anything — a resumed task would be a second
/// mutation of a tree somebody may already be reviewing.
///
/// Idempotent: once converted, a row is no longer `running`, so a second
/// restart reports nothing.
pub fn mark_running_as_unknown(conn: &Connection, at: &str) -> rusqlite::Result<Vec<TaskRow>> {
    let orphans = {
        let mut stmt = conn.prepare(
            "SELECT task_id, session_id, workspace, status, started_at, finished_at, detail
             FROM harness_tasks WHERE status = 'running' ORDER BY started_at",
        )?;
        let rows = stmt.query_map([], row_of)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    conn.execute(
        "UPDATE harness_tasks SET status = 'unknown', finished_at = ?1,
             detail = 'interrupted by a cell restart; inspect the workspace'
         WHERE status = 'running'",
        params![at],
    )?;
    Ok(orphans)
}

/// Shared row mapper.
fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        task_id: r.get(0)?,
        session_id: r.get(1)?,
        workspace: r.get(2)?,
        status: r.get(3)?,
        started_at: r.get(4)?,
        finished_at: r.get(5)?,
        detail: r.get(6)?,
    })
}
