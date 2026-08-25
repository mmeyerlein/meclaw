//! W8 (GH #380): the `web` cell's schema.
//!
//! Four tables, and the split between them is the design. The **object tree**
//! is the single source of truth for what is displayed; **components** are the
//! vocabulary it is written in and are *data*, not code, so a message can
//! define a new one at runtime; **pages** map routes onto roots and are the
//! only route source this cell has; **assets** are the files it serves under
//! its own origin.
//!
//! The schema is **fixed**, unlike the store cell's, which is declared per
//! instance in `params.schema`. A store is a typed box whose type its owner
//! chooses; a display's tables are its contract with the renderer, and a cell
//! that let an instance redefine them could not render a page it had not been
//! configured for.

use rusqlite::Connection;

/// The tables a `web` cell has. Also the closed set of names a seed file may
/// be called after — see [`crate::web::seed`].
pub const TABLES: &[&str] = &["objects", "components", "pages", "assets"];

/// Create the schema. Idempotent (`IF NOT EXISTS` throughout), so it runs on
/// every spawn and every respawn without a status check.
pub fn setup_web_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS objects (
             id        TEXT PRIMARY KEY,
             parent    TEXT,
             component TEXT NOT NULL,
             ord       INTEGER NOT NULL DEFAULT 0,
             props     TEXT NOT NULL DEFAULT '{}'
         );
         CREATE TABLE IF NOT EXISTS components (
             name        TEXT PRIMARY KEY,
             template    TEXT NOT NULL,
             prop_schema TEXT NOT NULL DEFAULT '{}',
             editable    TEXT NOT NULL DEFAULT '[]',
             layer       TEXT NOT NULL DEFAULT 'content'
         );
         CREATE TABLE IF NOT EXISTS pages (
             route TEXT PRIMARY KEY,
             root  TEXT NOT NULL,
             title TEXT NOT NULL DEFAULT ''
         );
         CREATE TABLE IF NOT EXISTS assets (
             path         TEXT PRIMARY KEY,
             content_type TEXT NOT NULL,
             body         BLOB NOT NULL
         );
         -- Every render walks children of a parent in order; without this the
         -- walk is a table scan per node.
         CREATE INDEX IF NOT EXISTS idx_objects_parent ON objects(parent, ord);",
    )
}

/// The columns of one table, in declaration order.
///
/// Used by the seed loader to build its INSERT and to check a seed header
/// covers every column. Kept next to the DDL above so the two cannot drift:
/// a column added there and forgotten here would be silently unseedable.
pub fn columns_of(table: &str) -> Option<&'static [&'static str]> {
    Some(match table {
        "objects" => &["id", "parent", "component", "ord", "props"],
        "components" => &["name", "template", "prop_schema", "editable", "layer"],
        "pages" => &["route", "root", "title"],
        "assets" => &["path", "content_type", "body"],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_web_schema(&conn).unwrap();
        // Runs on every respawn, so applying it twice must be a no-op rather
        // than an error.
        setup_web_schema(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, TABLES.len() as i64);
    }

    #[test]
    fn every_table_declares_its_columns() {
        // The drift guard: `columns_of` must answer for every table the DDL
        // creates, or that table could never be seeded.
        for t in TABLES {
            assert!(columns_of(t).is_some(), "no columns declared for {t}");
        }
        assert!(columns_of("widgets").is_none());
    }

    #[test]
    fn the_declared_columns_match_the_created_tables() {
        // Compares against sqlite's own view rather than against a second
        // hand-written list: a column renamed in the DDL fails here.
        let conn = Connection::open_in_memory().unwrap();
        setup_web_schema(&conn).unwrap();
        for t in TABLES {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({t})")).unwrap();
            let actual: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let declared: Vec<String> = columns_of(t)
                .unwrap()
                .iter()
                .map(|s| s.to_string())
                .collect();
            assert_eq!(actual, declared, "column drift in {t}");
        }
    }
}
