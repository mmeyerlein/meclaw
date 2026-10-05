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
//! Reads are never booked: answering a replayed `select` again is harmless
//! and the second answer is the right one.

use crate::store::ops::{CanonicalMap, OpOutcome, dispatch_with};
use meclaw_core::serde_json::{Value, json};

/// The dedupe record. `WITHOUT ROWID` because the id IS the key and nothing
/// else lives in a row. Idempotent, so every write may ensure it.
pub const CONSUMED_DDL: &str =
    "CREATE TABLE IF NOT EXISTS meclaw_consumed (message_id TEXT PRIMARY KEY) WITHOUT ROWID";

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

/// [`dispatch_with`], applied at most once per `key` (GH #1015).
///
/// `key` is the message id (or `<message id>#<leg>` for one leg of a bundle,
/// since a bundle carries several writes under one id). A read, or a call
/// without a key, is plain [`dispatch_with`].
///
/// A key already booked answers like a success that did nothing:
/// `rows_affected: 0` and the payload `{"duplicate": true}`, so the replayed
/// caller (if its reply is not dropped by the colony as already logged) sees a
/// normal `tool_result`, not an error. Only a successful write is booked: a
/// write that came back with an `error_code` changed nothing (every store
/// write is a single statement), so running it again on replay is the honest
/// answer and cannot double anything.
///
/// A SAVEPOINT rather than a transaction because it nests: it works whether
/// or not the connection is already inside a transaction.
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
    let sql = |e: rusqlite::Error| format!("cell.db consumed-record failed: {e}");
    conn.execute_batch(CONSUMED_DDL).map_err(sql)?;
    conn.execute_batch("SAVEPOINT consumed_once").map_err(sql)?;
    let result = (|| -> Result<OpOutcome, String> {
        let seen: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM meclaw_consumed WHERE message_id = ?1)",
                [key],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if seen {
            return Ok(OpOutcome {
                operation: op,
                rows_affected: 0,
                payload: json!({"duplicate": true}),
                error_code: None,
                error_text: None,
                error_index: None,
            });
        }
        let outcome = dispatch_with(conn, args, canonical)?;
        if outcome.error_code.is_none() {
            conn.execute(
                "INSERT INTO meclaw_consumed (message_id) VALUES (?1)",
                [key],
            )
            .map_err(sql)?;
        }
        Ok(outcome)
    })();
    match &result {
        Ok(_) => conn.execute_batch("RELEASE consumed_once").map_err(sql)?,
        // A failure after the write but before the booking must take the write
        // with it, or a replay would apply it a second time.
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK TO consumed_once; RELEASE consumed_once");
        }
    }
    result
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

    #[test]
    fn gh1015_store_insert_twice_writes_one_row() {
        let c = conn();
        let args = json!({"operation":"insert","table":"items","row":{"id":1,"name":"a"}});
        let first = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1")).unwrap();
        let second = dispatch_once(&c, &args, &CanonicalMap::new(), Some("m-1")).unwrap();
        assert_eq!(first.rows_affected, 1);
        assert_eq!(second.error_code, None, "a replay answers as a success");
        assert_eq!(second.payload, json!({"duplicate": true}));
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
}
