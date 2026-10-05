//! GH #1012: the two books a `meclaw` proxy keeps in its own `cell.db`.
//!
//! Before #1012 a crossing was at-most-once (OR-HV-3): the sender posted once,
//! the far mount answered `200` with the event only in memory, and nothing was
//! replayed after a crash. The books make it at-least-once with an idempotent
//! receive:
//!
//! - `peer_inbox` (receiving side): the mount commits a frame here BEFORE it
//!   answers `200`; a known `id` is a duplicate and raises nothing; rows not
//!   yet `done` are replayed in arrival order at the next start.
//! - `peer_outbox` (sending side): every message that may cross is booked
//!   here before its first attempt and sent per target, serially and in
//!   order, with retries until it crossed, was refused, or its deadline ran
//!   out.
//!
//! Both are state, not a log replacement: a row is never deleted, only its
//! `state` moves (R-94). Plain sync rusqlite, called through `DbConn::call*`.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Inbox row waiting for the handler half.
pub const PENDING: &str = "pending";
/// Inbox row whose arrival the handler half has emitted.
pub const DONE: &str = "done";
/// Outbox row that crossed.
pub const SENT: &str = "sent";
/// Outbox row the far side, or a final carrier answer, refused.
pub const REFUSED: &str = "refused";
/// Outbox row past `peer_retry_deadline_s`.
pub const EXPIRED: &str = "expired";

/// Idempotent DDL for both books (`CREATE … IF NOT EXISTS`).
pub fn setup_peer_book(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS peer_inbox (
            id     TEXT PRIMARY KEY,
            frame  TEXT NOT NULL,
            state  TEXT NOT NULL,
            at_ms  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS peer_outbox (
            id         TEXT PRIMARY KEY,
            target     TEXT NOT NULL,
            lane       TEXT NOT NULL,
            frame      TEXT NOT NULL,
            envelope   TEXT NOT NULL,
            tries      INTEGER NOT NULL DEFAULT 0,
            next_at    INTEGER NOT NULL,
            state      TEXT NOT NULL,
            created_ms INTEGER NOT NULL,
            deferred   INTEGER NOT NULL DEFAULT 0,
            last_error TEXT
        );
        CREATE INDEX IF NOT EXISTS peer_outbox_by_target ON peer_outbox(target, state);",
    )
}

// ------------------------------------------------------------------- inbox

/// Books an arrival. `Ok(true)` for a new id, `Ok(false)` for one already in
/// the book (a duplicate: nothing changes).
pub fn inbox_insert(
    conn: &Connection,
    id: &str,
    frame: &str,
    at_ms: i64,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "INSERT INTO peer_inbox (id, frame, state, at_ms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO NOTHING",
        params![id, frame, PENDING, at_ms],
    )?;
    Ok(n == 1)
}

/// Every arrival the handler half has not booked as emitted, in arrival order.
pub fn inbox_pending(conn: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut st =
        conn.prepare("SELECT id, frame FROM peer_inbox WHERE state != ?1 ORDER BY at_ms, rowid")?;
    st.query_map([DONE], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

/// The handler half emitted the arrival: the row moves to `done`.
pub fn inbox_done(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE peer_inbox SET state = ?1 WHERE id = ?2",
        params![DONE, id],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ outbox

/// One outbox row as the sender reads it back.
#[derive(Debug, Clone)]
pub struct OutRow {
    /// The frame id, stable across retries.
    pub id: String,
    /// The `hop.peer_url` the frame goes to; the FIFO key.
    pub target: String,
    /// The lane, for the receipts.
    pub lane: String,
    /// The frame exactly as posted, every time.
    pub frame: String,
    /// The original message, for a `peer_expired` dead letter.
    pub envelope: String,
    /// Attempts so far.
    pub tries: i64,
    /// Earliest instant (Unix ms) of the next attempt.
    pub next_at: i64,
    /// When the row was booked (Unix ms); the deadline counts from here.
    pub created_ms: i64,
    /// Whether the `deferred` receipt for this row was emitted.
    pub deferred: bool,
    /// The code of the last failed attempt, if any.
    pub last_error: Option<String>,
}

/// Books a new message for `target` and answers what holds the queue: the
/// `last_error` of the oldest pending row for the same target, or `None` when
/// the new row is first in line. One transaction, so no second writer can
/// slip a row in between the look and the insert. A row booked behind
/// another is booked with `deferred = 1`: its `deferred` receipt is the one
/// the caller emits now.
pub fn outbox_enqueue(conn: &mut Connection, row: &OutRow) -> rusqlite::Result<Option<String>> {
    // IMMEDIATE, not the default DEFERRED: the inbox writer of the I/O half
    // is a second connection on this file. A deferred read-then-write whose
    // snapshot that writer outdated cannot upgrade and fails at once with
    // `SQLITE_BUSY_SNAPSHOT`, past the busy timeout (lane run of fix round 1:
    // `the_outbox_survives_a_restart` read the one-attempt fallback). Taking
    // the write lock first waits under the busy timeout instead.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ahead: Option<Option<String>> = tx
        .query_row(
            "SELECT last_error FROM peer_outbox WHERE target = ?1 AND state = ?2
             ORDER BY rowid LIMIT 1",
            params![row.target, PENDING],
            |r| r.get(0),
        )
        .optional()?;
    let blocked = ahead.map(|e| e.unwrap_or_else(|| "peer_unreachable".to_string()));
    tx.execute(
        "INSERT INTO peer_outbox
            (id, target, lane, frame, envelope, tries, next_at, state, created_ms, deferred,
             last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?6, ?8, ?9)",
        params![
            row.id,
            row.target,
            row.lane,
            row.frame,
            row.envelope,
            row.created_ms,
            PENDING,
            blocked.is_some(),
            blocked,
        ],
    )?;
    tx.commit()?;
    Ok(blocked)
}

/// The pending rows of `target`, oldest first.
pub fn outbox_pending(conn: &Connection, target: &str) -> rusqlite::Result<Vec<OutRow>> {
    let mut st = conn.prepare(
        "SELECT id, target, lane, frame, envelope, tries, next_at, created_ms, deferred,
                last_error
         FROM peer_outbox WHERE target = ?1 AND state = ?2 ORDER BY rowid",
    )?;
    st.query_map(params![target, PENDING], |r| {
        Ok(OutRow {
            id: r.get(0)?,
            target: r.get(1)?,
            lane: r.get(2)?,
            frame: r.get(3)?,
            envelope: r.get(4)?,
            tries: r.get(5)?,
            next_at: r.get(6)?,
            created_ms: r.get(7)?,
            deferred: r.get(8)?,
            last_error: r.get(9)?,
        })
    })?
    .collect()
}

/// Every target with a pending row, and the earliest `next_at` among them.
pub fn outbox_targets(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut st = conn
        .prepare("SELECT target, MIN(next_at) FROM peer_outbox WHERE state = ?1 GROUP BY target")?;
    st.query_map([PENDING], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

/// Writes back what an attempt changed. `state` is one of the constants
/// above; the row stays.
pub fn outbox_update(conn: &Connection, row: &OutRow, state: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE peer_outbox SET state = ?1, tries = ?2, next_at = ?3, deferred = ?4,
                last_error = ?5
         WHERE id = ?6",
        params![
            state,
            row.tries,
            row.next_at,
            row.deferred,
            row.last_error,
            row.id
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, target: &str) -> OutRow {
        OutRow {
            id: id.into(),
            target: target.into(),
            lane: "proposal".into(),
            frame: "{}".into(),
            envelope: "{}".into(),
            tries: 0,
            next_at: 10,
            created_ms: 10,
            deferred: false,
            last_error: None,
        }
    }

    /// Part g: the books are state; moving a row never removes one.
    #[test]
    fn rows_only_change_state() {
        let mut c = Connection::open_in_memory().expect("db");
        setup_peer_book(&c).expect("ddl");
        setup_peer_book(&c).expect("idempotent");
        assert!(inbox_insert(&c, "a", "{}", 1).expect("insert"));
        assert!(
            !inbox_insert(&c, "a", "{}", 2).expect("dup"),
            "a known id is a duplicate"
        );
        inbox_done(&c, "a").expect("done");
        assert!(inbox_pending(&c).expect("pending").is_empty());
        assert_eq!(outbox_enqueue(&mut c, &row("1", "t")).expect("q"), None);
        let mut first = outbox_pending(&c, "t").expect("p").remove(0);
        first.tries = 1;
        first.last_error = Some("peer_timeout".into());
        outbox_update(&c, &first, PENDING).expect("u");
        assert_eq!(
            outbox_enqueue(&mut c, &row("2", "t")).expect("q"),
            Some("peer_timeout".into()),
            "a second row waits behind the first and names why"
        );
        assert_eq!(
            outbox_enqueue(&mut c, &row("3", "u")).expect("q"),
            None,
            "per target"
        );
        outbox_update(&c, &first, SENT).expect("u");
        let n: i64 = c
            .query_row("SELECT count(*) FROM peer_outbox", [], |r| r.get(0))
            .expect("n");
        let m: i64 = c
            .query_row("SELECT count(*) FROM peer_inbox", [], |r| r.get(0))
            .expect("m");
        assert_eq!((n, m), (3, 1));
    }

    /// Fix round 1 (targeted run on the lane, `the_outbox_survives_a_restart`
    /// read `refused` without `id`: the outbox was "unavailable"): the
    /// handler and the I/O half are two connections on one `cell.db`. A
    /// DEFERRED transaction that reads and then writes cannot upgrade once
    /// the other connection committed in between: SQLite answers
    /// `SQLITE_BUSY_SNAPSHOT` at once, without the busy timeout. The inbox
    /// writer commits while the handler books, so the booking must take the
    /// write lock first and wait for it.
    #[test]
    fn a_booking_waits_for_the_other_writer_instead_of_failing() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("cell.db");
        let other = meclaw_colony::persist::cell_db::open_or_create_cell_db(&path).expect("db");
        setup_peer_book(&other).expect("ddl");
        let mut mine = meclaw_colony::persist::cell_db::open_or_create_cell_db(&path).expect("db");
        // The inbox writer holds the write lock and has written a row.
        other.execute_batch("BEGIN IMMEDIATE").expect("lock");
        assert!(inbox_insert(&other, "south:x", "{}", 1).expect("insert"));
        let booking = std::thread::spawn(move || {
            outbox_enqueue(&mut mine, &row("1", "t")).map_err(|e| e.to_string())
        });
        // Semantic discriminator: the booking has started and waits on the lock.
        std::thread::sleep(std::time::Duration::from_millis(300));
        other.execute_batch("COMMIT").expect("commit");
        assert_eq!(
            booking.join().expect("join"),
            Ok(None),
            "booked after the commit"
        );
        let n: i64 = other
            .query_row("SELECT count(*) FROM peer_outbox", [], |r| r.get(0))
            .expect("n");
        assert_eq!(n, 1);
    }
}
