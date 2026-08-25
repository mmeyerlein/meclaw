//! GH #393: the files a `web` cell serves under its own origin.
//!
//! The `assets` table shipped with the type (GH #380) and nothing delivered it.
//! This module is the missing half: it reads the table once into an immutable
//! snapshot, which the I/O half then answers out of the same way it answers out
//! of the materialised page map — no database on the request path, no cell call
//! (R-W8-4a).
//!
//! # Assets are seed data, today
//!
//! There is **no op that writes `assets`** — [`crate::web::ops`] has
//! `object.*`, `component.define`, `page.set` and `query`, and none of them
//! touches this table. So the snapshot is built once, at start, and does not
//! change for the life of the cell; the shape it is published in ([`AssetMap`]
//! behind a `watch` channel) is nonetheless the shape a write would need, so a
//! future op that adds a file re-publishes here and changes nothing else.
//!
//! # Why the reader takes TEXT as well as BLOB
//!
//! `crate::web::seed` maps every JSON string onto SQLite `TEXT`, because a seed
//! file is JSON and JSON has no byte string. A seeded asset body therefore sits
//! in the `BLOB NOT NULL` column **as text** — SQLite stores what it is given,
//! the column type is a hint — and `FromSql for Vec<u8>` refuses that with
//! `InvalidType`. So the read goes through [`rusqlite::types::ValueRef`] and
//! takes either storage class.
//!
//! The alternative was to change the DDL or to make the seed loader guess which
//! column wants bytes. Both are more expensive than a tolerant reader: the
//! table is already shipped, a migration on a freshly released schema costs
//! every existing `cell.db`, and a loader that guessed per column would be a
//! second place where the schema is written down.

use rusqlite::Connection;
use rusqlite::types::ValueRef;
use std::collections::BTreeMap;

/// One row of `assets`, ready to be written to a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The `Content-Type` this file is served with — from the row, never from
    /// the path. What a file *is* is stated by whoever put it there; guessing
    /// from an extension would make a display's answer depend on a table of
    /// suffixes nobody declared.
    pub content_type: String,
    /// The file itself.
    pub body: Vec<u8>,
}

/// Every file this cell serves, by the path it answers on.
///
/// A `BTreeMap` for the same reason [`crate::web::render::PageMap`] is one: an
/// operator comparing two dumps should not have to sort them first.
pub type AssetMap = BTreeMap<String, Asset>;

/// Read the whole `assets` table into a snapshot.
///
/// A row whose body is neither text nor a blob — which a seed can produce by
/// writing a number where a file belongs — is **skipped with its path named**
/// rather than failing the load: one malformed row must not cost every other
/// file in the same cell, and the path in the log is what makes it fixable.
pub fn load_assets(conn: &Connection) -> rusqlite::Result<AssetMap> {
    let mut stmt = conn.prepare("SELECT path, content_type, body FROM assets ORDER BY path")?;
    let mut rows = stmt.query([])?;
    let mut out = AssetMap::new();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let content_type: String = row.get(1)?;
        let body = match row.get_ref(2)? {
            // The hand-written case, and what an op would write.
            ValueRef::Blob(b) => b.to_vec(),
            // The seeded case: `json_to_sql` had a JSON string and stored TEXT.
            ValueRef::Text(t) => t.to_vec(),
            other => {
                let kind = other.data_type();
                tracing::warn!(
                    asset = %path,
                    storage = %kind,
                    "web: asset body is neither text nor a blob — this file is not served"
                );
                continue;
            }
        };
        out.insert(path, Asset { content_type, body });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::db::setup_web_schema;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in memory");
        setup_web_schema(&conn).expect("schema");
        conn
    }

    #[test]
    fn a_body_stored_as_text_reads_back_as_its_bytes() {
        // The seeded case. `FromSql for Vec<u8>` is an `InvalidType` here, which
        // is the whole reason this function exists.
        let conn = db();
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES (?1, ?2, ?3)",
            rusqlite::params!["/a.css", "text/css", "body{}"],
        )
        .expect("insert text");
        let map = load_assets(&conn).expect("load");
        assert_eq!(map["/a.css"].body, b"body{}".to_vec());
        assert_eq!(map["/a.css"].content_type, "text/css");
    }

    #[test]
    fn a_body_stored_as_a_blob_reads_back_as_its_bytes() {
        let conn = db();
        let bytes: Vec<u8> = vec![0x00, 0xff, 0x10, 0x0a];
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES (?1, ?2, ?3)",
            rusqlite::params!["/f.bin", "application/octet-stream", bytes.clone()],
        )
        .expect("insert blob");
        let map = load_assets(&conn).expect("load");
        // Bytes no UTF-8 decode would survive: the reader hands them through.
        assert_eq!(map["/f.bin"].body, bytes);
    }

    #[test]
    fn a_body_that_is_a_number_is_skipped_and_the_others_survive() {
        let conn = db();
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES ('/n', 'text/plain', 7)",
            [],
        )
        .expect("insert number");
        conn.execute(
            "INSERT INTO assets (path, content_type, body) VALUES ('/ok', 'text/plain', 'x')",
            [],
        )
        .expect("insert text");
        let map = load_assets(&conn).expect("load");
        assert!(!map.contains_key("/n"), "a number is not a file");
        assert_eq!(map["/ok"].body, b"x".to_vec(), "and it costs nobody else");
    }

    #[test]
    fn an_empty_table_is_an_empty_snapshot() {
        // A cell with no assets is the normal case, not an error.
        assert!(load_assets(&db()).expect("load").is_empty());
    }
}
