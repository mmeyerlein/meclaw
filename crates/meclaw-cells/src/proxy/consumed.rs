//! GH #1015 (OR-HV-61) — an outbound proxy sends each message at most once.
//!
//! After a crash the colony replays every delivery that was still open, so a
//! proxy can be handed the same `Message.id` a second time. For an outbound
//! chat proxy that would be the one failure a person notices directly: the
//! same answer arriving twice. `consumed` books the id in the proxy's own
//! `cell.db` BEFORE the platform call, so a replay finds it and sends nothing.
//!
//! Booking first makes the delivery at-most-once. The remaining window is a
//! crash (or a failed send) after the booking and before the platform has the
//! message: that message is lost rather than doubled — deliberately, because
//! a missing reply is visible to the agent tree as `send_failed` or a gap,
//! while a doubled one reaches a human and cannot be taken back.

use rusqlite::Connection;

/// The dedupe record — the same shape as the store's. Idempotent, so it is
/// both part of each proxy's schema setup and ensured again on first use (a
/// `cell.db` opened by an older binary, or a test connection, has no table).
pub const CONSUMED_DDL: &str =
    "CREATE TABLE IF NOT EXISTS consumed (message_id TEXT PRIMARY KEY) WITHOUT ROWID";

/// Books `message_id`. `Ok(true)`: first sighting, send it. `Ok(false)`: a
/// replay, send nothing.
///
/// The decision is the INSERT itself (`OR IGNORE` + changed-row count), not a
/// SELECT followed by an INSERT, so there is no read-then-write gap.
pub fn book_once(conn: &Connection, message_id: &str) -> rusqlite::Result<bool> {
    conn.execute_batch(CONSUMED_DDL)?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO consumed (message_id) VALUES (?1)",
        [message_id],
    )?;
    Ok(n == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh1015_book_once_says_yes_exactly_once() {
        let c = Connection::open_in_memory().unwrap();
        assert!(book_once(&c, "m-1").unwrap());
        assert!(!book_once(&c, "m-1").unwrap());
        assert!(book_once(&c, "m-2").unwrap());
    }
}
