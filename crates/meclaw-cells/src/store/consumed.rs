//! GH #1015 (OR-HV-61) — the store applies each message's effect at most once.
//!
//! After a crash the colony replays every delivery that was still open, so a
//! store can see the same `Message.id` a second time. Without a record of what
//! it already applied, a replayed `insert` writes a second row and a replayed
//! write that is not idempotent in itself (a trigger, a counter, an append) is
//! applied twice. The `meclaw_consumed` table is that record (named so a user
//! table called `consumed` cannot collide): the id of every message
//! whose write succeeded is booked in the SAME savepoint as the write itself,
//! so the effect and its booking commit or vanish together — there is no
//! window in which one exists without the other.
//!
//! GH #1043: the booking keeps the write's answer too (`rows_affected`,
//! `payload`), and a replay answers exactly that. The answer a store gives
//! reaches the log only after the store's own commit, in a later writer batch
//! of the colony; a crash between the two replays the write, and the caller
//! sees only the replayed answer. Before #1043 that answer was
//! `rows_affected: 0`, and a caller that branches on it — a compare-and-set
//! claim (`update ... where state = 'open'`) reads "taken elsewhere" and ends
//! the work — dropped it while the row stayed claimed by nobody: the frame
//! lost in the receiving gate of the HV-14 SIGKILL run (`lost 1` of 300).
//!
//! Reads are never booked, also not the read legs of a bundle that writes. A
//! replayed read runs again and answers the state the store holds at the
//! replay, which can be later than the state the first answer saw (the window
//! is documented in `docs/stability.en.md`). Booking the read legs of a
//! writing bundle was tried in the review of #1043 and taken back: the
//! shipped file space reads its parked values back in the same writing bundle
//! (values far larger than any bound worth keeping), and a kept answer would
//! stay in the book for good, growing it by a multiple of each file per op.

use crate::store::ops::{CanonicalMap, OpOutcome, dispatch_with};
use meclaw_core::serde_json::{self, Value, json};
use rusqlite::OptionalExtension;

/// The dedupe record. `WITHOUT ROWID` because the id IS the key. GH #1043:
/// `rows_affected` and `payload` (JSON text) are the answer the write gave;
/// `NULL` in a row booked before 0.61.7, whose answer was not kept.
/// `replays` counts how often a replay was answered from the row — the one
/// trace a crash leaves in the store, and what the SIGKILL lock counts.
/// Idempotent, so every write may ensure it.
pub const CONSUMED_DDL: &str = "CREATE TABLE IF NOT EXISTS meclaw_consumed (
    message_id TEXT PRIMARY KEY, rows_affected INTEGER, payload TEXT,
    replays INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID";

/// The columns a book from 0.61.5/0.61.6 (ids only) lacks.
const ANSWER_COLUMNS: &[(&str, &str)] = &[
    ("rows_affected", "INTEGER"),
    ("payload", "TEXT"),
    ("replays", "INTEGER NOT NULL DEFAULT 0"),
];

/// Adds every column of `columns` that `table` lacks (GH #1043 review,
/// finding 3). Each column is its own step, so an upgrade that stopped
/// halfway is finished by the next call; a column that another connection
/// added between the look and the `ALTER TABLE` ("duplicate column name") is
/// what this wanted, not a failure.
pub(crate) fn add_missing_columns(
    conn: &rusqlite::Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let have: Vec<String> = stmt
        .query_map([table], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for (name, decl) in columns {
        if have.iter().any(|h| h == name) {
            continue;
        }
        match conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {decl}")) {
            Err(e) if e.to_string().contains("duplicate column name") => {}
            other => other?,
        }
    }
    Ok(())
}

/// GH #1043: the table as 0.61.5 and 0.61.6 created it holds the id only; the
/// answer columns are added in place (`NULL` for the rows already there).
fn ensure_book(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(CONSUMED_DDL)?;
    add_missing_columns(conn, "meclaw_consumed", ANSWER_COLUMNS)
}

/// The store ops that change state and therefore must not run twice for one
/// message — the same set the write surface bounds (`WRITE_OPS` in `cell.rs`,
/// minus the β `params_update` slot, which never reaches `dispatch`).
fn effect_op(op: &str) -> Option<&'static str> {
    Some(match op {
        "insert" => "insert",
        "update" => "update",
        "delete" => "delete",
        "create_table" => "create_table",
        "set_alias" => "set_alias",
        "reject_pair" => "reject_pair",
        "canonicalize" => "canonicalize",
        _ => return None,
    })
}

/// The answer to a key already booked: the one its write gave (GH #1043).
/// A row booked before the answer was kept answers as 0.61.5/0.61.6 did — a
/// success that did nothing, payload `{"duplicate": true}` — because the
/// first answer is not known.
fn replayed(op: &'static str, rows_affected: Option<i64>, payload: Option<String>) -> OpOutcome {
    let (rows_affected, payload) = match rows_affected {
        Some(n) => (
            n,
            payload
                .and_then(|p| serde_json::from_str(&p).ok())
                .unwrap_or(Value::Null),
        ),
        None => (0, json!({"duplicate": true})),
    };
    OpOutcome {
        operation: op,
        rows_affected,
        payload,
        error_code: None,
        error_text: None,
        error_index: None,
    }
}

/// [`dispatch_with`], applied at most once per `key` (GH #1015).
///
/// `key` is the message id (or `<message id>#<leg>` for one leg of a bundle,
/// since a bundle carries several writes under one id). A read, or a call
/// without a key, is plain [`dispatch_with`] — in a bundle that writes, too.
///
/// A key already booked runs nothing and answers what its first write
/// answered (GH #1043): the same `rows_affected` and `payload`, so a caller
/// that branches on the answer takes the same branch on a replay. Only a
/// successful write is booked: a write that came back with an `error_code`
/// changed nothing -- a single statement fails whole, and a write of several
/// statements (`insert` with `rows` or `replace`, GH #1199) runs under a
/// savepoint that a failure rolls back with everything it wrote -- so running
/// it again on replay is the honest answer and cannot double anything.
///
/// On a connection in autocommit (every store leg is its own `db.call`) the
/// call owns the transaction and opens it `BEGIN IMMEDIATE`; inside a
/// transaction the caller opened it nests as a SAVEPOINT, and the outer
/// transaction answers for the lock mode.
pub fn dispatch_once(
    conn: &rusqlite::Connection,
    args: &Value,
    canonical: &CanonicalMap,
    key: Option<&str>,
) -> Result<OpOutcome, String> {
    let op = args
        .get("operation")
        .and_then(|v| v.as_str())
        .and_then(effect_op);
    let (Some(op), Some(key)) = (op, key) else {
        return dispatch_with(conn, args, canonical);
    };
    // The book is created lazily by the first once-keyed write of a store
    // `cell.db`; its DDL needs the write lock in autocommit, so a lock it
    // cannot get is the same transient conflict as the `BEGIN IMMEDIATE`
    // below (re-check legacy-a2 fix4 M1 rest, pin
    // `gh98_a_once_keyed_write_that_cannot_create_the_book_answers_sql_error`).
    if let Err(e) = ensure_book(conn) {
        return Ok(crate::store::ops::sql_error_outcome(op, &e));
    }
    // IMMEDIATE, not the DEFERRED a bare SAVEPOINT opens in autocommit: the
    // book is read (`SELECT … meclaw_consumed`) before the first write, and
    // SQLite never calls the busy handler for a read-to-write upgrade. A
    // store `cell.db` has a second writer (`mutation::seed_rows::apply_entries`
    // opens its own connection on a live cell), and a once-keyed write racing
    // it failed at once with "database is locked" instead of waiting out the
    // GH #98 busy budget (pin `gh98_a_once_keyed_write_racing_another_writer_waits`,
    // red on the SAVEPOINT form). Same class as `persist::migrations::migrate`.
    let (open, commit, rollback) = if conn.is_autocommit() {
        ("BEGIN IMMEDIATE", "COMMIT", "ROLLBACK")
    } else {
        (
            "SAVEPOINT consumed_once",
            "RELEASE consumed_once",
            "ROLLBACK TO consumed_once; RELEASE consumed_once",
        )
    };
    // A transaction that cannot be opened (the busy budget ran out against
    // another writer) or committed is a transient lock conflict: it answers as
    // the store outcome `sql_error`, as the write did before the IMMEDIATE fix.
    // `Err` would reach both callers in `store/cell.rs` as `invalid_input` and
    // blame the caller's input for a lock (re-check legacy-a2 fix3 M1, pins
    // `gh98_a_once_keyed_write_that_cannot_open_answers_sql_error` and
    // `gh98_a_once_keyed_write_whose_commit_fails_answers_sql_error`).
    if let Err(e) = conn.execute_batch(open) {
        return Ok(crate::store::ops::sql_error_outcome(op, &e));
    }
    // Inside the transaction an error of the book's own SQL is a store
    // failure, not the caller's input: it answers `sql_error` like the open
    // and the commit (re-check legacy-a2 fix4 H-b). Only `dispatch_with`'s
    // `Err` (a malformed request) stays `Err`.
    let result = (|| -> Result<OpOutcome, BookFail> {
        let seen: Option<(Option<i64>, Option<String>)> = conn
            .query_row(
                "SELECT rows_affected, payload FROM meclaw_consumed WHERE message_id = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(BookFail::Sql)?;
        if let Some((rows_affected, payload)) = seen {
            conn.execute(
                "UPDATE meclaw_consumed SET replays = replays + 1 WHERE message_id = ?1",
                [key],
            )
            .map_err(BookFail::Sql)?;
            return Ok(replayed(op, rows_affected, payload));
        }
        let outcome = dispatch_with(conn, args, canonical).map_err(BookFail::Input)?;
        if outcome.error_code.is_none() {
            let payload = serde_json::to_string(&outcome.payload)
                .map_err(|e| BookFail::Input(format!("cell.db consumed-record failed: {e}")))?;
            conn.execute(
                "INSERT INTO meclaw_consumed (message_id, rows_affected, payload)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![key, outcome.rows_affected, payload],
            )
            .map_err(BookFail::Sql)?;
        }
        Ok(outcome)
    })();
    match &result {
        Ok(outcome) => {
            // A commit that fails leaves the transaction open; roll it back so
            // the connection is never left inside a `BEGIN` the next message
            // would inherit. A write that failed and that the database rolled
            // back whole (`RAISE(ROLLBACK)`) has ended the transaction too, so
            // the commit fails with "no transaction is active": the answer is
            // then the write's own failure, as in `ops` (GH #1199, pin
            // `gh1199_a_once_keyed_write_the_database_rolls_back_answers_its_own_cause`).
            if let Err(e) = conn.execute_batch(commit) {
                let _ = conn.execute_batch(rollback);
                if outcome.error_code.is_none() {
                    return Ok(crate::store::ops::sql_error_outcome(op, &e));
                }
            }
        }
        // A failure after the write but before the booking must take the write
        // with it, or a replay would apply it a second time.
        Err(_) => {
            let _ = conn.execute_batch(rollback);
        }
    }
    match result {
        Ok(outcome) => Ok(outcome),
        Err(BookFail::Sql(e)) => Ok(crate::store::ops::sql_error_outcome(op, &e)),
        Err(BookFail::Input(e)) => Err(e),
    }
}

/// Why the transaction of [`dispatch_once`] failed: the book's own SQL (a
/// store failure, answered as `sql_error`) or the request (`Err`).
enum BookFail {
    Sql(rusqlite::Error),
    Input(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE items (id INTEGER, name TEXT);
             CREATE TABLE applied (n INTEGER);
             INSERT INTO applied VALUES (0);
             CREATE TRIGGER count_updates AFTER UPDATE ON items
               BEGIN UPDATE applied SET n = n + 1; END;",
        )
        .unwrap();
        c
    }

    fn count(c: &rusqlite::Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    static OTHER_WRITER: std::sync::Mutex<Option<rusqlite::Connection>> =
        std::sync::Mutex::new(None);

    fn let_the_other_writer_commit(_attempt: i32) -> bool {
        if let Some(other) = OTHER_WRITER.lock().unwrap().take() {
            other.execute_batch("COMMIT").unwrap();
        }
        true
    }

    /// GH #98, the store half: `dispatch_once` reads the book before its first
    /// write. Under the DEFERRED transaction a bare SAVEPOINT opens that is a
    /// read-to-write upgrade, for which SQLite never calls the busy handler --
    /// a once-keyed write next to a second writer on the same `cell.db`
    /// (`seed_rows` on a live cell) failed at once with "database is locked".
    /// Deterministic: the busy handler itself releases the other writer, so
    /// the write lands only if the handler is called at all.
    #[test]
    fn gh98_a_once_keyed_write_racing_another_writer_waits() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("cell.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.execute_batch("CREATE TABLE items (id INTEGER, name TEXT);")
            .unwrap();
        // The book exists before the race, so nothing before the read writes
        // (a first `CREATE TABLE` would itself start from no transaction and
        // call the handler, hiding the upgrade).
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-0")).unwrap();

        let other = rusqlite::Connection::open(&path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        *OTHER_WRITER.lock().unwrap() = Some(other);
        c.busy_handler(Some(let_the_other_writer_commit)).unwrap();

        let args = json!({"operation":"insert","table":"items","row":{"id":2,"name":"b"}});
        let out = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1"))
            .expect("the once-keyed write waits for the other writer");
        // A store write that fails comes back as an outcome, not as `Err`.
        assert_eq!(
            (out.error_code, out.error_text.as_deref()),
            (None, None),
            "the once-keyed write waits for the other writer"
        );
        assert_eq!(out.rows_affected, 1);
        assert!(c.is_autocommit(), "no transaction is left open");
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 2);
    }

    /// Inside a transaction the caller opened, the book nests as a SAVEPOINT
    /// and a rollback of the outer transaction takes the write and its booking.
    #[test]
    fn gh1015_inside_an_outer_transaction_the_book_nests() {
        let c = conn();
        ensure_book(&c).unwrap();
        c.execute_batch("BEGIN").unwrap();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-n")).unwrap();
        assert!(!c.is_autocommit(), "the outer transaction stays open");
        c.execute_batch("ROLLBACK").unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 0);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM meclaw_consumed"), 0);
    }

    #[test]
    fn gh1015_store_insert_twice_writes_one_row() {
        let c = conn();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        let first = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1")).unwrap();
        let second = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1")).unwrap();
        assert_eq!(first.rows_affected, 1);
        assert_eq!(second.error_code, None, "a replay answers as a success");
        // GH #1043: and with the first write's answer.
        assert_eq!(
            (second.rows_affected, &second.payload),
            (first.rows_affected, &first.payload)
        );
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
        // A different message is a different write.
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-2")).unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 2);
    }

    #[test]
    fn gh1015_store_update_twice_applies_once() {
        // `update` has no relative operator; the AFTER UPDATE trigger stands in
        // for any effect that is not idempotent by itself (it counts runs).
        let c = conn();
        c.execute("INSERT INTO items VALUES (1, 'a')", []).unwrap();
        let args = json!({"operation":"update","table":"items","set":{"name":"b"},
                          "where":{"id":1}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-u")).unwrap();
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-u")).unwrap();
        assert_eq!(count(&c, "SELECT n FROM applied"), 1);
    }

    #[test]
    fn gh1015_store_delete_twice_is_one_effect_no_error() {
        let c = conn();
        c.execute_batch("INSERT INTO items VALUES (1,'a'); INSERT INTO items VALUES (2,'b');")
            .unwrap();
        let args = json!({"operation":"delete","table":"items","where":{"id":1}});
        let first = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-d")).unwrap();
        let second = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-d")).unwrap();
        assert_eq!(first.rows_affected, 1);
        assert_eq!(second.error_code, None);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
    }

    /// GH #1043: a replayed write answers what the first answered. The lab
    /// lost a peer frame on a SIGKILL because a replayed compare-and-set
    /// (`update ... where state = 'open'`) whose first answer never reached
    /// the log was answered `rows_affected: 0`; the caller read "taken
    /// elsewhere" and dropped the frame, while the row stayed claimed by
    /// nobody.
    #[test]
    fn gh1043_a_replayed_claim_answers_what_the_first_answered() {
        let c = conn();
        c.execute("INSERT INTO items VALUES (1, 'open')", [])
            .unwrap();
        let claim = json!({"operation":"update","table":"items","set":{"name":"claimed"},
                           "where":{"id":1,"name":"open"}});
        let first = dispatch_once(&c, &claim, &CanonicalMap::new(), Some("m-c")).unwrap();
        let replay = dispatch_once(&c, &claim, &CanonicalMap::new(), Some("m-c")).unwrap();
        assert_eq!(first.rows_affected, 1, "the first claim moved the row");
        assert_eq!(
            (replay.rows_affected, &replay.payload, replay.error_code),
            (first.rows_affected, &first.payload, None),
            "the replay answers the first claim's answer"
        );
        assert_eq!(count(&c, "SELECT n FROM applied"), 1, "and applies nothing");
        // Another message's claim on the same row is honestly too late.
        let other = dispatch_once(&c, &claim, &CanonicalMap::new(), Some("m-o")).unwrap();
        assert_eq!(other.rows_affected, 0, "a second claimant reads taken");
    }

    /// GH #1043: the same for an insert, and per bundle leg.
    #[test]
    fn gh1043_a_replayed_insert_answers_what_the_first_answered() {
        let c = conn();
        let args = json!({"operation":"insert","table":"items","row":{"id":7,"name":"x"}});
        let first = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-i#0")).unwrap();
        let replay = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-i#0")).unwrap();
        assert_eq!(first.rows_affected, 1);
        assert_eq!(
            (replay.rows_affected, &replay.payload),
            (first.rows_affected, &first.payload)
        );
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
    }

    /// GH #1043: a book written by 0.61.5/0.61.6 (ids only) is upgraded in
    /// place; an id booked before the upgrade has no recorded answer and keeps
    /// the old one, every write after it records its own.
    #[test]
    fn gh1043_a_book_without_answers_is_upgraded_in_place() {
        let c = conn();
        c.execute_batch(
            "CREATE TABLE meclaw_consumed (message_id TEXT PRIMARY KEY) WITHOUT ROWID;
             INSERT INTO meclaw_consumed VALUES ('old');",
        )
        .unwrap();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        let old = dispatch_once(&c, &args, &CanonicalMap::new(), Some("old")).unwrap();
        assert_eq!(
            (old.rows_affected, &old.payload),
            (0, &json!({"duplicate": true})),
            "an id without a recorded answer answers as before"
        );
        let first = dispatch_once(&c, &args, &CanonicalMap::new(), Some("new")).unwrap();
        let replay = dispatch_once(&c, &args, &CanonicalMap::new(), Some("new")).unwrap();
        assert_eq!(first.rows_affected, 1);
        assert_eq!(replay.rows_affected, 1, "recorded after the upgrade");
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
    }

    /// GH #1043 (review Z-M3, finding 3): an upgrade that stopped halfway —
    /// one answer column there, the other not (a crash between the two
    /// `ALTER TABLE`s, or a second connection that added one first) — is
    /// finished on the next write instead of failing every write after it.
    #[test]
    fn gh1043_a_half_upgraded_book_is_finished() {
        let c = conn();
        c.execute_batch(
            "CREATE TABLE meclaw_consumed (message_id TEXT PRIMARY KEY, rows_affected INTEGER)
             WITHOUT ROWID;",
        )
        .unwrap();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        let first = dispatch_once(&c, &args, &CanonicalMap::new(), Some("h")).unwrap();
        let replay = dispatch_once(&c, &args, &CanonicalMap::new(), Some("h")).unwrap();
        assert_eq!(first.error_code, None, "{:?}", first.error_text);
        assert_eq!(replay.rows_affected, 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
    }

    /// GH #1043 (review Z-M3, finding 3): a column another connection added
    /// between the look and the `ALTER TABLE` is no failure.
    #[test]
    fn gh1043_a_column_added_meanwhile_is_no_failure() {
        let c = conn();
        c.execute_batch("CREATE TABLE t (a TEXT)").unwrap();
        add_missing_columns(&c, "t", &[("b", "TEXT")]).unwrap();
        // The race, replayed: the look said "missing", the column is there.
        let e = c
            .execute_batch("ALTER TABLE t ADD COLUMN b TEXT")
            .unwrap_err();
        assert!(e.to_string().contains("duplicate column name"), "{e}");
        add_missing_columns(&c, "t", &[("b", "TEXT"), ("c", "INTEGER")]).unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM pragma_table_info('t')"), 3);
    }

    /// The SIGKILL lock counts replays in the book (GH #1043 review, finding
    /// 5): every replay answered from a row bumps its `replays`.
    #[test]
    fn gh1043_a_replay_is_counted_in_the_book() {
        let c = conn();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        for _ in 0..3 {
            dispatch_once(&c, &args, &CanonicalMap::new(), Some("r")).unwrap();
        }
        assert_eq!(count(&c, "SELECT replays FROM meclaw_consumed"), 2);
    }

    #[test]
    fn gh1015_store_reads_and_failed_writes_are_not_booked() {
        let c = conn();
        let sel = json!({"operation":"select","table":"items","columns":["id"]});
        dispatch_once(&c, &sel, &CanonicalMap::new(), Some("m-r")).unwrap();
        let bad = json!({"operation":"insert","table":"missing","row":{"x":1}});
        let o = dispatch_once(&c, &bad, &CanonicalMap::new(), Some("m-x")).unwrap();
        assert!(o.error_code.is_some());
        assert_eq!(count(&c, "SELECT COUNT(*) FROM meclaw_consumed"), 0);
    }

    /// A busy handler that gives up at once: the budget is spent.
    fn give_up_at_once(_attempt: i32) -> bool {
        false
    }

    /// GH #98, the error path of the fix above: a once-keyed write that cannot
    /// open its `BEGIN IMMEDIATE` within the busy budget is a transient lock
    /// conflict, so it answers as the store outcome `sql_error`, never as `Err`
    /// -- both callers (`store/cell.rs`, bundle leg and single message) turn an
    /// `Err` into `invalid_input` and blame the caller's input for a lock.
    #[test]
    fn gh98_a_once_keyed_write_that_cannot_open_answers_sql_error() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("cell.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.execute_batch("CREATE TABLE items (id INTEGER, name TEXT);")
            .unwrap();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-0")).unwrap();

        let other = rusqlite::Connection::open(&path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        c.busy_handler(Some(give_up_at_once)).unwrap();

        let args = json!({"operation":"insert","table":"items","row":{"id":2,"name":"b"}});
        let out = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1"))
            .expect("a lock conflict is an outcome, not Err");
        assert_eq!(out.error_code, Some("sql_error"), "{:?}", out.error_text);
        assert!(c.is_autocommit(), "no transaction is left open");
        other.execute_batch("COMMIT").unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM meclaw_consumed"), 1);
    }

    /// The same one step earlier: the book is created lazily by the first
    /// once-keyed write of a store `cell.db`, and its `CREATE TABLE` needs the
    /// write lock in autocommit. Another writer holding it past the busy
    /// budget is the same transient lock conflict -- `sql_error`, not `Err`
    /// (re-check legacy-a2 fix4, M1 rest).
    #[test]
    fn gh98_a_once_keyed_write_that_cannot_create_the_book_answers_sql_error() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("cell.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.execute_batch("CREATE TABLE items (id INTEGER, name TEXT);")
            .unwrap();

        let other = rusqlite::Connection::open(&path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        c.busy_handler(Some(give_up_at_once)).unwrap();

        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        let out = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-0"))
            .expect("a lock conflict is an outcome, not Err");
        assert_eq!(out.error_code, Some("sql_error"), "{:?}", out.error_text);
        assert!(c.is_autocommit(), "no transaction is left open");
        other.execute_batch("COMMIT").unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 0);
    }

    /// A booking whose own SQL fails inside the transaction is a store
    /// failure, not the caller's input: the write is rolled back with it and
    /// the answer is a store error outcome, not `Err` (re-check legacy-a2 fix4 H-b).
    #[test]
    fn gh98_a_once_keyed_write_whose_booking_fails_answers_an_outcome() {
        let c = conn();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-0")).unwrap();
        c.execute_batch(
            "CREATE TRIGGER refuse_booking BEFORE INSERT ON meclaw_consumed
               BEGIN SELECT RAISE(ABORT, 'booking refused'); END;",
        )
        .unwrap();

        let args = json!({"operation":"insert","table":"items","row":{"id":2,"name":"b"}});
        let out = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1"))
            .expect("a failed booking is an outcome, not Err");
        assert!(out.error_code.is_some(), "{out:?}");
        assert!(c.is_autocommit(), "no transaction is left open");
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM meclaw_consumed"), 1);
    }

    /// The same for a COMMIT that fails: in rollback-journal mode the COMMIT
    /// needs the EXCLUSIVE lock, which a reader's SHARED lock withholds. The
    /// write is rolled back, the answer is `sql_error`, not `Err`.
    #[test]
    fn gh98_a_once_keyed_write_whose_commit_fails_answers_sql_error() {
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("cell.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "DELETE").unwrap();
        c.execute_batch("CREATE TABLE items (id INTEGER, name TEXT);")
            .unwrap();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-0")).unwrap();

        let reader = rusqlite::Connection::open(&path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        c.busy_handler(Some(give_up_at_once)).unwrap();

        let args = json!({"operation":"insert","table":"items","row":{"id":2,"name":"b"}});
        let out = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1"))
            .expect("a failed commit is an outcome, not Err");
        assert_eq!(out.error_code, Some("sql_error"), "{:?}", out.error_text);
        assert!(c.is_autocommit(), "no transaction is left open");
        reader.execute_batch("COMMIT").unwrap();
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM meclaw_consumed"), 1);
    }

    /// GH #1199 (re-check RD-G5 m-3): a write the database rolls back whole
    /// (`RAISE(ROLLBACK)`) also ends the `BEGIN IMMEDIATE` this call opened, so
    /// the closing COMMIT fails with "no transaction is active". The answer is
    /// the write's own failure, which `ops` hands up, not that COMMIT.
    #[test]
    fn gh1199_a_once_keyed_write_the_database_rolls_back_answers_its_own_cause() {
        let c = conn();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"old"}});
        dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-0")).unwrap();
        c.execute_batch(
            "CREATE TRIGGER t_gone BEFORE INSERT ON items WHEN NEW.name = 'gone'
               BEGIN SELECT RAISE(ROLLBACK, 'injected rollback'); END;",
        )
        .unwrap();

        let args = json!({"operation":"insert","table":"items",
                          "rows":[{"id":1,"name":"new"},{"id":1,"name":"gone"}],
                          "replace":{"id":1}});
        let out = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1"))
            .expect("a rolled-back write is an outcome, not Err");
        let text = out.error_text.clone().unwrap_or_default();
        assert!(
            out.error_code.is_some() && text.contains("injected rollback"),
            "the write's own failure is the answer, not the closing COMMIT: {:?} {text}",
            out.error_code
        );
        assert!(c.is_autocommit(), "no transaction is left open");
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM items WHERE name = 'old'"),
            1
        );
        assert_eq!(count(&c, "SELECT COUNT(*) FROM items"), 1);
        assert_eq!(count(&c, "SELECT COUNT(*) FROM meclaw_consumed"), 1);
    }
}
