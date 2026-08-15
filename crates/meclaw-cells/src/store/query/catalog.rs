//! Identifier resolution against the live SQLite catalog.
//!
//! This is the injection barrier of the store cell. Every identifier that ends
//! up inside a SQL string is a `String` *returned by SQLite here* — caller text
//! reaches a statement only as a bound parameter. Escaping is not part of the
//! design: an identifier that the catalog does not know is rejected, not quoted.
//!
//! The lookup runs per op — no cache across messages, so a table created by one
//! message is visible to the next one (`create_table` is a runtime capability
//! per `docs/cell-types.md` § store).

use rusqlite::OptionalExtension;
use std::collections::BTreeSet;

/// Failure of identifier resolution. Maps 1:1 onto the store's existing error
/// codes (`docs/cell-types.md` § store, Failure-Klassifikation) — no new code.
#[derive(Debug)]
pub enum CatalogError {
    /// No such table (or view) in this `cell.db`.
    UnknownTable(String),
    /// The table exists, the column does not.
    UnknownColumn(String),
    /// The catalog query itself failed.
    Sql(rusqlite::Error),
}

/// A table's identifiers as SQLite itself spells them.
pub struct Catalog {
    table: String,
    columns: BTreeSet<String>,
}

impl Catalog {
    /// Load the catalog entry for `table`. Both queries are fully parameterized,
    /// so a hostile table name can only ever fail to match.
    pub fn load(conn: &rusqlite::Connection, table: &str) -> Result<Self, CatalogError> {
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
                [table],
                |r| r.get(0),
            )
            .optional()
            .map_err(CatalogError::Sql)?;
        let name = name.ok_or_else(|| CatalogError::UnknownTable(table.to_string()))?;
        let mut st = conn
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(CatalogError::Sql)?;
        let columns = st
            .query_map([&name], |r| r.get::<_, String>(0))
            .map_err(CatalogError::Sql)?
            .collect::<rusqlite::Result<BTreeSet<String>>>()
            .map_err(CatalogError::Sql)?;
        Ok(Self {
            table: name,
            columns,
        })
    }

    /// The catalog-owned table name — safe to quote into a statement.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Resolve one caller-supplied column name to its catalog-owned spelling.
    pub fn column(&self, want: &str) -> Result<&str, CatalogError> {
        self.columns
            .get(want)
            .map(String::as_str)
            .ok_or_else(|| CatalogError::UnknownColumn(want.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_resolves_existing_table_and_columns() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        let cat = Catalog::load(&c, "items").unwrap();
        assert_eq!(cat.table(), "items");
        assert_eq!(cat.column("name").unwrap(), "name");
        assert!(matches!(
            cat.column("nope"),
            Err(CatalogError::UnknownColumn(_))
        ));
        assert!(matches!(
            Catalog::load(&c, "nope"),
            Err(CatalogError::UnknownTable(_))
        ));
    }

    #[test]
    fn catalog_rejects_injection_shaped_identifiers_without_touching_sql() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE items (id INTEGER)", []).unwrap();
        let evil = "items\"; DROP TABLE items; --";
        assert!(matches!(
            Catalog::load(&c, evil),
            Err(CatalogError::UnknownTable(_))
        ));
        let cat = Catalog::load(&c, "items").unwrap();
        assert!(matches!(
            cat.column("id\") UNION SELECT 1 --"),
            Err(CatalogError::UnknownColumn(_))
        ));
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='items'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "table must still exist");
    }
}
