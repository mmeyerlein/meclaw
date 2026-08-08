//! Phase-10-C: the `cell.db.update_cursor` single row + persist helpers.
//! W3: one instance = one bot = one cursor. Sync rusqlite, called via
//! `DbConn::call` (except in the factory, where it runs before `DbConn::wrap`).

use rusqlite::Connection;

/// Idempotent DDL for the `update_cursor` single-row table.
/// Calling it repeatedly is safe (`CREATE TABLE IF NOT EXISTS`). Invoked by the
/// factory once per spawn — before `DbConn::wrap`, outside the corridor.
pub fn setup_proxy_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS update_cursor (
            id     INTEGER PRIMARY KEY CHECK (id = 1),
            offset INTEGER NOT NULL
        );",
    )
}

/// Reads the persisted cursor offset. On a fresh schema (no row in
/// `update_cursor`), returns `0` (Created-path default, W9).
pub fn load_offset(conn: &Connection) -> rusqlite::Result<i64> {
    let mut stmt = conn.prepare("SELECT offset FROM update_cursor WHERE id = 1")?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        None => Ok(0),
        Some(r) => r.get(0),
    }
}

/// UPSERTs the cursor offset (single row, `id = 1`). The handler calls this
/// per update inside `handle_event` before the `OriginSink` emit (Phase-5
/// canon: state-before-emit).
pub fn save_offset(conn: &Connection, offset: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO update_cursor (id, offset) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET offset = excluded.offset",
        [offset],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_proxy_schema_is_idempotent_and_creates_update_cursor() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_proxy_schema(&conn).expect("first call");
        setup_proxy_schema(&conn).expect("second call — must be idempotent");
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='update_cursor'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
