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
//! a missing reply is visible to the agent tree as `send_failed`, while a
//! doubled one reaches a human and cannot be taken back.
//!
//! GH #1043 (review): "visible" holds on a replay too. The platform's answer
//! is booked after the call ([`settle`]), and a replay answers what the
//! original answered: silence after a confirmed send, the original's
//! `send_failed` after a failed one, and `send_failed` with the reason
//! `unconfirmed` when the booking has no answer — the crash fell between the
//! booking and the platform's answer, and whether a human got the message is
//! not known. Before, every replay was silent, and a message whose
//! `send_failed` never reached the log was gone without anyone hearing of it.

use rusqlite::{Connection, OptionalExtension};

/// The dedupe record. `sent`: `NULL` booked and not (yet) answered, `1` the
/// platform took it, `0` it refused (`reason` is the original's detail).
/// Idempotent, so it is both part of each proxy's schema setup and ensured
/// again on first use (a `cell.db` opened by an older binary, or a test
/// connection, has no table).
pub const CONSUMED_DDL: &str = "CREATE TABLE IF NOT EXISTS consumed (
    message_id TEXT PRIMARY KEY, sent INTEGER, reason TEXT) WITHOUT ROWID";

/// The answer columns a record from 0.61.5/0.61.6 (ids only) lacks.
const ANSWER_COLUMNS: &[(&str, &str)] = &[("sent", "INTEGER"), ("reason", "TEXT")];

/// What a replay finds booked for its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prior {
    /// Booked, no answer recorded: the crash fell between the booking and the
    /// platform's answer (or the row is older than the answer columns).
    Unconfirmed,
    /// The platform took the message.
    Sent,
    /// The platform refused it; the original reported this detail.
    Failed(String),
}

/// What [`book_once`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Booking {
    /// First sighting: send it, then [`settle`] the answer.
    First,
    /// A replay: send nothing, answer as the original did.
    Replay(Prior),
}

/// The detail of the `send_failed` a replay of an unconfirmed send reports;
/// it starts with the reason `unconfirmed`.
pub const UNCONFIRMED_DETAIL: &str = "unconfirmed: booked before a restart, the platform's answer \
     was never recorded; not sent again, whether it reached the recipient is unknown";

fn ensure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(CONSUMED_DDL)?;
    crate::store::consumed::add_missing_columns(conn, "consumed", ANSWER_COLUMNS)
}

/// Books `message_id`. [`Booking::First`]: first sighting, send it.
/// [`Booking::Replay`]: send nothing, answer as the booked original did.
///
/// The decision is the INSERT itself (`OR IGNORE` + changed-row count), not a
/// SELECT followed by an INSERT, so there is no read-then-write gap.
pub fn book_once(conn: &Connection, message_id: &str) -> rusqlite::Result<Booking> {
    ensure(conn)?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO consumed (message_id) VALUES (?1)",
        [message_id],
    )?;
    if n == 1 {
        return Ok(Booking::First);
    }
    let row: Option<(Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT sent, reason FROM consumed WHERE message_id = ?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(Booking::Replay(match row {
        Some((Some(1), _)) => Prior::Sent,
        Some((Some(_), reason)) => Prior::Failed(reason.unwrap_or_default()),
        _ => Prior::Unconfirmed,
    }))
}

/// Books the platform's answer to `message_id`: `Ok(())` sent, `Err(detail)`
/// refused with the detail the original reports.
pub fn settle(
    conn: &Connection,
    message_id: &str,
    answer: Result<(), &str>,
) -> rusqlite::Result<()> {
    ensure(conn)?;
    let (sent, reason) = match answer {
        Ok(()) => (1, None),
        Err(detail) => (0, Some(detail)),
    };
    conn.execute(
        "UPDATE consumed SET sent = ?2, reason = ?3 WHERE message_id = ?1",
        rusqlite::params![message_id, sent, reason],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh1015_book_once_says_yes_exactly_once() {
        let c = Connection::open_in_memory().unwrap();
        assert_eq!(book_once(&c, "m-1").unwrap(), Booking::First);
        assert_eq!(
            book_once(&c, "m-1").unwrap(),
            Booking::Replay(Prior::Unconfirmed)
        );
        assert_eq!(book_once(&c, "m-2").unwrap(), Booking::First);
    }

    /// GH #1043 review, finding 2: the answer is booked and a replay reads it.
    #[test]
    fn gh1043_a_replay_finds_the_answer_of_the_original() {
        let c = Connection::open_in_memory().unwrap();
        book_once(&c, "ok").unwrap();
        settle(&c, "ok", Ok(())).unwrap();
        book_once(&c, "bad").unwrap();
        settle(&c, "bad", Err("status: 500")).unwrap();
        assert_eq!(book_once(&c, "ok").unwrap(), Booking::Replay(Prior::Sent));
        assert_eq!(
            book_once(&c, "bad").unwrap(),
            Booking::Replay(Prior::Failed("status: 500".into()))
        );
    }

    /// A record from 0.61.5/0.61.6 is upgraded in place; its rows have no
    /// answer and replay as unconfirmed.
    #[test]
    fn gh1043_an_old_record_is_upgraded_in_place() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE consumed (message_id TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO consumed VALUES ('old');",
        )
        .unwrap();
        assert_eq!(
            book_once(&c, "old").unwrap(),
            Booking::Replay(Prior::Unconfirmed)
        );
        assert_eq!(book_once(&c, "new").unwrap(), Booking::First);
        settle(&c, "new", Ok(())).unwrap();
        assert_eq!(book_once(&c, "new").unwrap(), Booking::Replay(Prior::Sent));
    }
}
