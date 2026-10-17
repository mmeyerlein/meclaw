//! cell.db helpers: snapshot_tx writes system.* + last_input in ONE transaction.
//!
//! Phase-6.5: `open_or_create_cell_db` is the only authority over cell.db
//! identity (M1 resume-with-state). An existing DB is opened (resume, all rows
//! stay), a non-existing one is created fresh (`setup_cell_db`).
//! `setup_cell_db` is idempotent (CREATE TABLE IF NOT EXISTS), so it can be
//! called on the resume path without harm.
//!
//! Called from the factory's RespawnFn closure — before
//! `tokio::spawn(cell_task_stateful(..., conn))`. An open failure there is a
//! `panic!` inside the closure → supervisor restart loop up to `restart_limit`
//! (phase 5 § Restart strategy, "deterministically panicking cell"). At
//! `spawn_cell` the factory checks first, on the error path
//! ([`check_own_cell_db`], GH #1231), so a database that is unusable at boot
//! makes the cell `failed` instead; a cell that booted asleep runs the same
//! check on wake (`crate::wake_checked_respawn`).

/// Open an existing cell.db or create a new one (M1 resume-with-state).
///
/// If the parent directory does not exist, it is created via
/// `std::fs::create_dir_all` (supports nested cell paths like `/foo/bar/cell.db`).
/// On open, `setup_cell_db` is invoked (idempotent — safe for both new and
/// existing databases).
pub fn open_or_create_cell_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
                Some(e.to_string()),
            )
        })?;
    }
    let conn = rusqlite::Connection::open(path)?;
    crate::persist::setup_cell_db(&conn)?;
    // GH #1231 (T6-03): the version check moved here from the boot planner,
    // which read every cell's database (`probe_cell_db`, removed). The DDL's
    // `INSERT OR IGNORE` keeps an existing version, so a database of another
    // schema still says so after `setup_cell_db` and is refused. A file that is
    // no database at all already fails `setup_cell_db` (NOTADB). No
    // `quick_check` here: this runs on every wake and respawn, and its cost
    // grows with the database (OR-programm-tm.H1.2).
    let version = crate::persist::read_schema_version(&conn)?;
    if version != 1 {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
            Some(format!("cell.db schema_version {version}, expected 1")),
        ));
    }
    Ok(conn)
}

/// GH #1231 (T6-03): a cell factory's check of its OWN `cell.db`, on the error
/// path — called at the top of `spawn_cell` before any build closure opens the
/// file behind an `.expect`. An unusable database makes `spawn_cell` return
/// `Err`, and the boot registers the cell `failed` instead of panicking.
///
/// An absent file is fine and stays absent: creating it here would turn the
/// build closure's first open into `Resumed` and skip the seed.
pub fn check_own_cell_db(cell_dir: &std::path::Path) -> Result<(), String> {
    let path = cell_dir.join("cell.db");
    if !path.exists() {
        return Ok(());
    }
    open_or_create_cell_db(&path)
        .map(drop)
        .map_err(|e| format!("open cell.db: {e}"))
}

/// Discriminator returned by [`open_or_create_cell_db_with_status`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum OpenStatus {
    /// DB file did not exist; fresh creation + `setup_cell_db`.
    Created,
    /// DB file existed; opened existing (M1 Resume).
    Resumed,
}

/// Like [`open_or_create_cell_db`], but reports whether the DB was created
/// fresh or resumed from an existing file. Used by Phase-9 `store` to
/// decide whether to load seed JSONL (fresh only).
pub fn open_or_create_cell_db_with_status(
    path: &std::path::Path,
) -> rusqlite::Result<(rusqlite::Connection, OpenStatus)> {
    let existed = path.exists();
    let conn = open_or_create_cell_db(path)?;
    let status = if existed {
        OpenStatus::Resumed
    } else {
        OpenStatus::Created
    };
    Ok((conn, status))
}

/// Snapshot at the end of handle(): upsert every system.* slot + replace
/// last_input, in ONE transaction. Plan E3.
pub fn snapshot_tx(
    conn: &mut rusqlite::Connection,
    system_upserts: &[(String, String)],
    last_input_json: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for (k, v) in system_upserts {
        tx.execute(
            "INSERT INTO system (slot_path, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(slot_path) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
            rusqlite::params![k, v, now],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO last_input (id, message_json, received_at) VALUES (1, ?, ?)",
        rusqlite::params![last_input_json, now],
    )?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_tx_writes_system_upsert_and_last_input() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::setup_cell_db(&conn).unwrap();
        let now = 100i64;
        snapshot_tx(
            &mut conn,
            &[("counter".to_string(), "7".to_string())],
            r#"{"msg":"x"}"#,
            now,
        )
        .unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM system WHERE slot_path='counter'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "7");
        let msg: String = conn
            .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(msg, r#"{"msg":"x"}"#);
    }

    #[test]
    fn snapshot_tx_upsert_overwrites_existing_value() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::persist::setup_cell_db(&conn).unwrap();
        snapshot_tx(&mut conn, &[("counter".into(), "1".into())], "{}", 0).unwrap();
        snapshot_tx(&mut conn, &[("counter".into(), "2".into())], "{}", 1).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM system WHERE slot_path='counter'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "2");
    }

    #[test]
    fn open_with_status_reports_created_on_first_open() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("cell.db");
        let (_conn, status) = open_or_create_cell_db_with_status(&p).unwrap();
        assert_eq!(status, OpenStatus::Created);
    }

    #[test]
    fn open_with_status_reports_resumed_on_second_open() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("cell.db");
        drop(open_or_create_cell_db_with_status(&p).unwrap());
        let (_conn, status) = open_or_create_cell_db_with_status(&p).unwrap();
        assert_eq!(status, OpenStatus::Resumed);
    }

    #[test]
    fn open_creates_new_db_with_schema() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("cell.db");
        let conn = open_or_create_cell_db(&p).unwrap();
        let v: i64 = conn
            .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 0);
    }

    #[test]
    fn open_resumes_existing_db_preserving_rows() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("cell.db");
        {
            let conn = open_or_create_cell_db(&p).unwrap();
            conn.execute(
                "INSERT INTO system (slot_path, value, updated_at) VALUES ('k', 'v', 0)",
                [],
            )
            .unwrap();
        }
        let conn = open_or_create_cell_db(&p).unwrap();
        let v: String = conn
            .query_row("SELECT value FROM system WHERE slot_path='k'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, "v");
    }
}
