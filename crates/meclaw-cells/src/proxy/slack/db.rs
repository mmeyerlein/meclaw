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
//! * `typing_reactions` — GH #1530: every reaction the connector set as the
//!   sign of a running turn whose removal Slack has not yet confirmed. A row
//!   is written before the `reactions.add` and goes only once a
//!   `reactions.remove` of it came back as done (`ok`, or `no_reaction`)
//!   (OR-d54g.acktyp.2, [`forget_reaction`]). The keeper reports that on the
//!   cell's report lane; a removal that failed keeps its row. So whatever
//!   still stands after the keeper gave up -- a restart between the two ends
//!   of a turn, a removal that failed at the turn's end after its retries,
//!   one that failed at the start -- is still a row, and the next start tries
//!   it again ([`remembered_reactions`]). The one reaction no row covers: an
//!   add that timed out and that Slack carries out only after the confirmed
//!   removal -- nothing the connector can observe.
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
        );
        CREATE TABLE IF NOT EXISTS typing_reactions (
            channel TEXT NOT NULL,
            ts      TEXT NOT NULL,
            name    TEXT NOT NULL,
            chat_id TEXT NOT NULL,
            set_at  INTEGER NOT NULL,
            PRIMARY KEY (channel, ts, name)
        );",
    )?;
    // GH #1015: the outbound dedupe record (see `proxy::consumed`).
    conn.execute_batch(crate::proxy::consumed::CONSUMED_DDL)
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

/// GH #1530: one reaction the connector set on a person's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberedReaction {
    /// The channel of the person's message.
    pub channel: String,
    /// The person's message (`ts`).
    pub ts: String,
    /// The emoji name.
    pub name: String,
}

/// GH #1530: remembers a reaction BEFORE its `reactions.add` goes out (state
/// before effect): an add that fails at its timeout may still go through, so
/// it has to be remembered either way. `chat_id` is the keeper's key (the
/// turn's chat), kept for diagnosis. Idempotent.
pub fn remember_reaction(
    conn: &Connection,
    chat_id: &str,
    reaction: &RememberedReaction,
    now_secs: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO typing_reactions (channel, ts, name, chat_id, set_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(channel, ts, name) DO UPDATE SET
            chat_id = excluded.chat_id, set_at = excluded.set_at",
        rusqlite::params![
            reaction.channel,
            reaction.ts,
            reaction.name,
            chat_id,
            now_secs
        ],
    )?;
    Ok(())
}

/// OR-d54g.acktyp.2: forgets one reaction once Slack confirmed its removal
/// (the keeper's report, or the start's). Never before: a row whose removal
/// failed is what the next start takes away. Returns how many rows went.
pub fn forget_reaction(
    conn: &Connection,
    reaction: &RememberedReaction,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM typing_reactions WHERE channel = ?1 AND ts = ?2 AND name = ?3",
        rusqlite::params![reaction.channel, reaction.ts, reaction.name],
    )
}

/// GH #1530: every remembered reaction, oldest first -- the start takes each
/// of them away. Reading forgets nothing (OR-d54g.acktyp.2): a row goes with
/// its confirmed removal ([`forget_reaction`]).
pub fn remembered_reactions(conn: &Connection) -> rusqlite::Result<Vec<RememberedReaction>> {
    let mut stmt =
        conn.prepare("SELECT channel, ts, name FROM typing_reactions ORDER BY set_at")?;
    stmt.query_map([], |r| {
        Ok(RememberedReaction {
            channel: r.get(0)?,
            ts: r.get(1)?,
            name: r.get(2)?,
        })
    })?
    .collect()
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
        for table in ["seen_envelopes", "thread_owner", "typing_reactions"] {
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

    fn reaction(channel: &str, ts: &str) -> RememberedReaction {
        RememberedReaction {
            channel: channel.into(),
            ts: ts.into(),
            name: "eyes".into(),
        }
    }

    #[test]
    fn a_remembered_reaction_is_read_until_its_removal_is_confirmed() {
        let c = conn();
        remember_reaction(&c, "C1:1.0", &reaction("C1", "1.1"), 10).expect("one");
        remember_reaction(&c, "C1:1.0", &reaction("C1", "1.2"), 11).expect("two");
        remember_reaction(&c, "C2", &reaction("C2", "2.1"), 12).expect("three");
        // Idempotent: the same reaction twice is one row.
        remember_reaction(&c, "C2", &reaction("C2", "2.1"), 13).expect("again");
        let all = vec![
            reaction("C1", "1.1"),
            reaction("C1", "1.2"),
            reaction("C2", "2.1"),
        ];
        assert_eq!(remembered_reactions(&c).expect("read"), all);
        // Reading forgets nothing (OR-d54g.acktyp.2).
        assert_eq!(remembered_reactions(&c).expect("read again"), all);
        // A confirmed removal forgets exactly its own row.
        assert_eq!(
            forget_reaction(&c, &reaction("C1", "1.2")).expect("forget"),
            1
        );
        assert_eq!(
            forget_reaction(&c, &reaction("C1", "1.2")).expect("twice"),
            0
        );
        assert_eq!(
            remembered_reactions(&c).expect("left"),
            vec![reaction("C1", "1.1"), reaction("C2", "2.1")]
        );
    }
}
