//! P12 S12: the Slack variant's `cell.db` tables.
//!
//! Two tables, both owned by the handler task (the DB authority; the I/O task
//! never touches them).
//!
//! * `seen_envelopes` — envelope-id dedup. Slack redelivers an un-acked
//!   envelope up to three times over a few minutes, and an ack can also be lost
//!   on the wire after we sent it. Without this table that redelivery would
//!   emit the user's message a second time into the agent tree.
//! * `thread_owner` — which threads this bot opened. It is what makes "keep
//!   following a thread you started" expressible without re-mentioning, and it
//!   is scoped per channel because a thread timestamp is only unique within its
//!   conversation.
//!
//! Both are keyed by TEXT because Slack timestamps are addresses, not numbers.

use rusqlite::Connection;

/// Idempotent DDL. Called once per spawn from the factory, before
/// `DbConn::wrap` — same position as the Telegram variant's schema setup.
pub fn setup_slack_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS seen_envelopes (
            envelope_id TEXT PRIMARY KEY,
            seen_at     INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS thread_owner (
            channel   TEXT NOT NULL,
            thread_ts TEXT NOT NULL,
            owned_at  INTEGER NOT NULL,
            PRIMARY KEY (channel, thread_ts)
        );",
    )
}

/// Records an envelope id. Returns `true` when this is the first sighting and
/// the event should be processed, `false` when it is a redelivery.
///
/// The decision is the INSERT itself rather than a SELECT followed by an
/// INSERT: the latter is a read-modify-write that would let two deliveries of
/// the same envelope both observe "not seen".
pub fn mark_envelope_seen(
    conn: &Connection,
    envelope_id: &str,
    now_secs: i64,
) -> rusqlite::Result<bool> {
    let inserted = conn.execute(
        "INSERT INTO seen_envelopes (envelope_id, seen_at) VALUES (?1, ?2)
         ON CONFLICT(envelope_id) DO NOTHING",
        rusqlite::params![envelope_id, now_secs],
    )?;
    Ok(inserted == 1)
}

/// Drops dedup entries older than `cutoff_secs`, returning how many went.
/// Retention is a param and deliberately far longer than Slack's retry window,
/// so pruning can never resurrect an envelope that is still in flight.
pub fn prune_envelopes(conn: &Connection, cutoff_secs: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM seen_envelopes WHERE seen_at < ?1",
        [cutoff_secs],
    )
}

/// Marks a thread as owned by this bot. Idempotent.
pub fn claim_thread(
    conn: &Connection,
    channel: &str,
    thread_ts: &str,
    now_secs: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO thread_owner (channel, thread_ts, owned_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(channel, thread_ts) DO NOTHING",
        rusqlite::params![channel, thread_ts, now_secs],
    )?;
    Ok(())
}

/// Whether this bot owns the thread. Scoped per channel: a thread timestamp is
/// unique only within its own conversation.
pub fn owns_thread(conn: &Connection, channel: &str, thread_ts: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM thread_owner WHERE channel = ?1 AND thread_ts = ?2",
        rusqlite::params![channel, thread_ts],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().expect("in-memory db");
        setup_slack_schema(&c).expect("schema");
        c
    }

    #[test]
    fn schema_is_idempotent() {
        let c = conn();
        setup_slack_schema(&c).expect("second call must be safe");
        for table in ["seen_envelopes", "thread_owner"] {
            let n: i64 = c
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .expect("query");
            assert_eq!(n, 1, "{table} must exist exactly once");
        }
    }

    #[test]
    fn first_sighting_is_new_and_the_replay_is_not() {
        let c = conn();
        assert!(mark_envelope_seen(&c, "env-1", 1000).expect("first"));
        assert!(!mark_envelope_seen(&c, "env-1", 1001).expect("replay"));
        assert!(mark_envelope_seen(&c, "env-2", 1002).expect("other"));
    }

    #[test]
    fn pruning_removes_only_entries_older_than_the_cutoff() {
        let c = conn();
        mark_envelope_seen(&c, "old", 100).expect("old");
        mark_envelope_seen(&c, "new", 900).expect("new");
        let removed = prune_envelopes(&c, 500).expect("prune");
        assert_eq!(removed, 1);
        // The pruned id is forgotten, so it would be treated as new again.
        assert!(mark_envelope_seen(&c, "old", 1000).expect("re-mark"));
        // The retained one is still remembered.
        assert!(!mark_envelope_seen(&c, "new", 1000).expect("retained"));
    }

    #[test]
    fn thread_ownership_is_claimed_once_and_scoped_per_channel() {
        let c = conn();
        assert!(!owns_thread(&c, "C1", "100.1").expect("unclaimed"));
        claim_thread(&c, "C1", "100.1", 10).expect("claim");
        assert!(owns_thread(&c, "C1", "100.1").expect("claimed"));
        // Same thread ts in a different channel is a different thread.
        assert!(!owns_thread(&c, "C2", "100.1").expect("other channel"));
        // Re-claiming is idempotent, not an error.
        claim_thread(&c, "C1", "100.1", 20).expect("reclaim");
        assert!(owns_thread(&c, "C1", "100.1").expect("still owned"));
    }
}
