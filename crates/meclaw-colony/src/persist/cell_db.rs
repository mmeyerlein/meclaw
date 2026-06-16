//! cell.db Helper: snapshot_tx schreibt system.* + last_input in EINER Transaktion.
//!
//! Phase-6.5: `open_or_create_cell_db` ist die einzige Authority über cell.db-
//! Identität (M1 Resume-mit-State). Existierende DB wird geöffnet (resume, alle
//! Rows bleiben), nicht-existierende neu erstellt (fresh + `setup_cell_db`).
//! `setup_cell_db` ist idempotent (CREATE TABLE IF NOT EXISTS), darum auch im
//! resume-Pfad ohne Schaden aufrufbar.
//!
//! Wird ausschließlich von der Factory-RespawnFn-Closure aufgerufen — vor
//! `tokio::spawn(cell_task_stateful(..., conn))`. Open-Failure ist `panic!` in
//! der Closure → Supervisor-Restart-Loop bis `restart_limit` (Phase 5
//! § Restart-Strategie, "deterministisch panickende Cell").

/// Open an existing cell.db or create a new one (M1 Resume-mit-State).
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
    Ok(conn)
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

/// Snapshot am handle()-Ende: alle system.*-Slots Upsert + last_input Replace,
/// in EINER Transaktion. Plan E3.
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
