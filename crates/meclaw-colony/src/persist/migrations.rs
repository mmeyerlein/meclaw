//! Schema version migration for `colony.db`.
//!
//! v1 (phase-5 baseline): edges table without CEL state.
//! v2 (phase-13.5 durable edges): edges gains `condition` + `modifier`
//! as TEXT NULL columns. ALTER TABLE in-place, no CREATE-new+COPY (F4).
//! v3 (phase-16 W3 / A6): `mutation_log` gains `error_code` + `trace_id`
//! as TEXT NULL columns — the two reject-row fields without a home in v2
//! (status/failure_reason/created_at already exist). ALTER TABLE in-place.
//! v4 (phase-16 W6d / A6): new table `dead_letters` — the DLQ is no longer a
//! volatile in-memory `VecDeque` but persisted in `colony.db`
//! (crash/shutdown survival of the diagnostic truth). Columns: the 6 localization
//! fields (`DeadLetterDto`) PLUS `message_json` — the full message envelope,
//! serialized with the same primitives as `message_log` (ruling W6d:
//! persist the envelope too → drain reconstructs the full `DeadLetter` from DB).
//! `CREATE TABLE IF NOT EXISTS`, additive (no column change to existing tables).
//! v5 (GH #62 instantiation provenance): `registry` gains `template`,
//! `template_version` and `instantiated_at` as NULL-able columns — the query
//! index over the provenance that every instantiated node now carries in its own
//! `config.json` (`cell.provenance`). ALTER TABLE in-place; rows written before
//! the stamp existed read NULL, which is the honest answer for a node whose
//! origin was never recorded.
//! v6 (GH #277 composition substrate): `registry` gains `template_chain` as a
//! NULL-able TEXT column — the JSON-serialized list of every template a node
//! came through, outermost first, the node's own template last. The v5 triple
//! answers "what is this node an instance of"; the chain answers "which
//! instances does a bump of an INNER template touch", which a composite
//! instance cannot answer from the leaf stamp alone. ALTER TABLE in-place;
//! rows written before the chain existed read NULL, and so does a row whose
//! value cannot be parsed — the instance's own `config.json` stays the source,
//! the table is the index.
//! v7 (GH #283 default edges): `edges` gains `is_default` as an
//! `INTEGER NOT NULL DEFAULT 0` column — the routing phase an edge belongs to.
//! A `0` edge is consulted in phase one, a `1` edge only after every phase-one
//! edge of the same sender declined. The column is `is_default` and not
//! `default` because `DEFAULT` is a SQL reserved word. ALTER TABLE in-place;
//! rows written before the phase existed read `0`, which is what they were
//! routed as — an old lane must never be rehydrated as a default.
//!
//! IMPORTANT: affects `colony.db` exclusively. `cell.db` stays at v1.

use rusqlite::Connection;

/// Target schema version for `colony.db` after this slice.
pub(crate) const TARGET_SCHEMA_VERSION: u32 = 7;

/// Error during the `colony.db` schema migration.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MigrationError {
    /// rusqlite error while reading the version or during the ALTER TABLE.
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    /// `schema_version` is neither the current nor a known predecessor version.
    #[error("unknown schema_version {0}; target is {}", TARGET_SCHEMA_VERSION)]
    UnknownVersion(u32),
}

/// Migrates `colony.db` idempotently to [`TARGET_SCHEMA_VERSION`].
///
/// Atomic: every `ALTER TABLE` + the version bump run in ONE transaction.
/// If a step fails, everything rolls back → `schema_version` stays
/// unchanged. `ADD COLUMN` only runs when the column is missing → an aborted
/// earlier run can be caught up cleanly on re-run (no duplicate-column dead end).
///
/// The migration is staged (v1→v2: edges CEL columns, v2→v3: mutation_log
/// reject columns) and chains in one run: a v1 DB receives both stages.
/// `mutation_log` always exists at call time — `setup_colony_db` executes the
/// `CREATE TABLE IF NOT EXISTS` DDL BEFORE `migrate`.
pub(crate) fn migrate(conn: &Connection) -> Result<(), MigrationError> {
    let current = super::schema::read_schema_version(conn)?;
    match current {
        v if v == TARGET_SCHEMA_VERSION => Ok(()),
        1..=6 => {
            let tx = conn.unchecked_transaction()?;
            // v1→v2: durable-edges CEL columns. `table_exists`-guarded like
            // v4→v5 below: since GH #90 this runs BEFORE the DDL batch, so a
            // sparse old DB may lack the table entirely — the batch then
            // creates it at the target shape, and there is nothing to ALTER.
            if current <= 1 && table_exists(&tx, "edges")? {
                if !column_exists(&tx, "edges", "condition")? {
                    tx.execute("ALTER TABLE edges ADD COLUMN condition TEXT", [])?;
                }
                if !column_exists(&tx, "edges", "modifier")? {
                    tx.execute("ALTER TABLE edges ADD COLUMN modifier TEXT", [])?;
                }
            }
            // v2→v3 (A6): mutation_log reject columns. Same guard rationale
            // as v1→v2 above.
            if current <= 2 && table_exists(&tx, "mutation_log")? {
                if !column_exists(&tx, "mutation_log", "error_code")? {
                    tx.execute("ALTER TABLE mutation_log ADD COLUMN error_code TEXT", [])?;
                }
                if !column_exists(&tx, "mutation_log", "trace_id")? {
                    tx.execute("ALTER TABLE mutation_log ADD COLUMN trace_id TEXT", [])?;
                }
            }
            // v3→v4 (W6d/A6): persistent DLQ table. `CREATE TABLE IF NOT EXISTS`
            // is idempotent — a table already created via `setup_colony_db` (DDL)
            // stays untouched; an older migrated DB (or the pure `migrate` path
            // without DDL) receives it here. `id` = rowid (monotonic insert
            // order); the 6 fields mirror the `DeadLetterDto`.
            if current <= 3 {
                tx.execute(
                    "CREATE TABLE IF NOT EXISTS dead_letters (
                       id              INTEGER PRIMARY KEY,
                       sender_path     TEXT NOT NULL,
                       original_target TEXT NOT NULL,
                       resolved_target TEXT NOT NULL,
                       error_code      TEXT NOT NULL,
                       trace_id        TEXT NOT NULL,
                       created_at      INTEGER NOT NULL,
                       message_json    TEXT NOT NULL
                     )",
                    [],
                )?;
                tx.execute(
                    "CREATE INDEX IF NOT EXISTS idx_dlq_created ON dead_letters(created_at)",
                    [],
                )?;
                tx.execute(
                    "CREATE INDEX IF NOT EXISTS idx_dlq_error_code ON dead_letters(error_code)",
                    [],
                )?;
            }
            // v4→v5 (GH #62): registry provenance index. NULL-able, additive —
            // an existing row keeps every value it had and reads NULL for the
            // three new columns. Guarded by `column_exists` so an aborted
            // earlier run re-runs cleanly, and by `table_exists` because the
            // pure-`migrate` path (without the `setup_colony_db` DDL) may be
            // handed a DB that has no `registry` table at all.
            if current <= 4 && table_exists(&tx, "registry")? {
                for (col, ty) in [
                    ("template", "TEXT"),
                    ("template_version", "TEXT"),
                    ("instantiated_at", "INTEGER"),
                ] {
                    if !column_exists(&tx, "registry", col)? {
                        tx.execute(&format!("ALTER TABLE registry ADD COLUMN {col} {ty}"), [])?;
                    }
                }
            }
            // v5→v6 (GH #277): the fourth provenance column, the template
            // chain. NULL-able, additive — an existing row keeps every value it
            // had and reads NULL for the new column. Same two guards and the
            // same rationale as v4→v5 above: `column_exists` so an aborted
            // earlier run re-runs cleanly, `table_exists` because the
            // pure-`migrate` path (without the `setup_colony_db` DDL) may be
            // handed a DB that has no `registry` table at all.
            if current <= 5
                && table_exists(&tx, "registry")?
                && !column_exists(&tx, "registry", "template_chain")?
            {
                tx.execute("ALTER TABLE registry ADD COLUMN template_chain TEXT", [])?;
            }
            // v6→v7 (GH #283): the routing phase of an edge. `NOT NULL
            // DEFAULT 0`, so every pre-existing row is a REGULAR edge — the
            // phase it was routed in before the column existed. Same two guards
            // and the same rationale as the stages above: `column_exists` so an
            // aborted earlier run re-runs cleanly, `table_exists` because the
            // pure-`migrate` path (without the `setup_colony_db` DDL) may be
            // handed a DB that has no `edges` table at all.
            if current <= 6
                && table_exists(&tx, "edges")?
                && !column_exists(&tx, "edges", "is_default")?
            {
                tx.execute(
                    "ALTER TABLE edges ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            tx.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                rusqlite::params![TARGET_SCHEMA_VERSION.to_string()],
            )?;
            tx.commit()?;
            Ok(())
        }
        v => Err(MigrationError::UnknownVersion(v)),
    }
}

/// True when `table` exists in this database (`sqlite_master` lookup).
fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// True when `table` already has the column `col` (PRAGMA table_info).
fn column_exists(conn: &Connection, table: &str, col: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == col);
    Ok(exists)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_v1(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', '1');
             CREATE TABLE edges (
               id TEXT PRIMARY KEY, from_path TEXT NOT NULL,
               to_path TEXT NOT NULL, created_at INTEGER NOT NULL);
             CREATE TABLE registry (
               path TEXT PRIMARY KEY, cell_id TEXT NOT NULL, cell_type TEXT NOT NULL,
               status TEXT NOT NULL, created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL);
             CREATE TABLE mutation_log (
               id TEXT PRIMARY KEY, scope TEXT NOT NULL, payload_json TEXT NOT NULL,
               status TEXT NOT NULL, failure_reason TEXT NULL,
               created_at INTEGER NOT NULL, committed_at INTEGER NULL);",
        )
        .unwrap();
    }

    fn registry_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(registry)").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// GH #62 (v4→v5): the `registry` table gains the three provenance columns.
    /// Additive `ADD COLUMN`s, so an existing colony keeps every row and reads
    /// NULL for nodes born before the stamp existed.
    #[test]
    fn migrate_v1_to_v5_adds_the_registry_provenance_columns() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute(
            "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at) \
             VALUES ('/a', 'id-a', 'echo', 'active', 1, 1)",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent second pass
        let cols = registry_columns(&conn);
        for col in ["template", "template_version", "instantiated_at"] {
            assert!(
                cols.contains(&col.to_string()),
                "{col} missing, got {cols:?}"
            );
        }
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
        let (tpl, at): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT template, instantiated_at FROM registry WHERE path = '/a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(tpl, None, "a pre-existing row keeps NULL provenance");
        assert_eq!(at, None);
    }

    /// A colony.db that is already at v4 skips the v1–v4 stages and receives
    /// the v5 step (plus every stage after it) in one pass.
    #[test]
    fn migrate_v4_to_v5_adds_only_the_registry_provenance_columns() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute_batch(
            "ALTER TABLE edges ADD COLUMN condition TEXT;
             ALTER TABLE edges ADD COLUMN modifier TEXT;
             ALTER TABLE mutation_log ADD COLUMN error_code TEXT;
             ALTER TABLE mutation_log ADD COLUMN trace_id TEXT;
             UPDATE meta SET value='4' WHERE key='schema_version';",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let cols = registry_columns(&conn);
        assert!(cols.contains(&"template".to_string()), "got {cols:?}");
        assert!(cols.contains(&"template_version".to_string()));
        assert!(cols.contains(&"instantiated_at".to_string()));
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    /// GH #277 (v5→v6): the `registry` table gains `template_chain`. Additive
    /// `ADD COLUMN` like the v5 triple, so an existing colony keeps every row
    /// and reads NULL for nodes born before the chain was recorded.
    #[test]
    fn migrate_v1_to_v6_adds_the_registry_template_chain_column() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute(
            "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at) \
             VALUES ('/a', 'id-a', 'echo', 'active', 1, 1)",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent second pass
        let cols = registry_columns(&conn);
        assert!(
            cols.contains(&"template_chain".to_string()),
            "template_chain missing, got {cols:?}"
        );
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
        let row: (String, String, String, String, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT path, cell_id, cell_type, status, created_at, updated_at, \
                 template_chain FROM registry WHERE path = '/a'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (
                row.0.as_str(),
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4,
                row.5
            ),
            ("/a", "id-a", "echo", "active", 1, 1),
            "a pre-existing row keeps every value it had"
        );
        assert_eq!(row.6, None, "a pre-existing row keeps NULL for the chain");
    }

    fn columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(edges)").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn mutation_log_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(mutation_log)").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// GH #283 (v6→v7): the `edges` table gains `is_default`. `NOT NULL
    /// DEFAULT 0`, so a row written before the default phase existed reads `0`
    /// — a REGULAR edge, never a default. Rehydrating an old lane as a default
    /// would hand it every message the sender's other lanes decline, which is
    /// the opposite of what that topology was booted with.
    #[test]
    fn migrate_v1_to_v7_adds_the_edges_is_default_column() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute(
            "INSERT INTO edges (id, from_path, to_path, created_at) \
             VALUES ('edge-1', '/a', '/b', 7)",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent second pass
        let cols = columns(&conn);
        assert!(
            cols.contains(&"is_default".to_string()),
            "is_default missing, got {cols:?}"
        );
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
        let (from, to, is_default): (String, String, i64) = conn
            .query_row(
                "SELECT from_path, to_path, is_default FROM edges WHERE id = 'edge-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((from.as_str(), to.as_str()), ("/a", "/b"));
        assert_eq!(is_default, 0, "a pre-existing edge row is a REGULAR edge");
    }

    /// A colony.db already at v6 skips every earlier stage and receives only
    /// the v7 step in one pass.
    #[test]
    fn migrate_v6_to_v7_adds_only_the_edges_is_default_column() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute_batch(
            "ALTER TABLE edges ADD COLUMN condition TEXT;
             ALTER TABLE edges ADD COLUMN modifier TEXT;
             ALTER TABLE mutation_log ADD COLUMN error_code TEXT;
             ALTER TABLE mutation_log ADD COLUMN trace_id TEXT;
             ALTER TABLE registry ADD COLUMN template TEXT;
             ALTER TABLE registry ADD COLUMN template_version TEXT;
             ALTER TABLE registry ADD COLUMN instantiated_at INTEGER;
             ALTER TABLE registry ADD COLUMN template_chain TEXT;
             UPDATE meta SET value='6' WHERE key='schema_version';",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let cols = columns(&conn);
        assert!(
            cols.contains(&"is_default".to_string()),
            "is_default missing, got {cols:?}"
        );
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrate_v1_to_v3_adds_condition_and_modifier_columns() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        migrate(&conn).unwrap();
        let cols = columns(&conn);
        assert!(cols.contains(&"condition".to_string()));
        assert!(cols.contains(&"modifier".to_string()));
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    /// Phase-16 W3 (A6): the v1→v3 chain also adds the two mutation_log reject
    /// columns (`error_code`, `trace_id`) — one pass migrates both stages.
    #[test]
    fn migrate_v1_to_v3_adds_mutation_log_reject_columns() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        migrate(&conn).unwrap();
        let cols = mutation_log_columns(&conn);
        assert!(
            cols.contains(&"error_code".to_string()),
            "error_code missing"
        );
        assert!(cols.contains(&"trace_id".to_string()), "trace_id missing");
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    /// Phase-16 W3 (A6): a v2 colony.db (edges already migrated) migrates to v3,
    /// adding only the two mutation_log reject columns. Idempotent on re-run.
    #[test]
    fn migrate_v2_to_v3_adds_only_mutation_log_columns_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        // Bring it to a v2 state: edge columns present, version bumped.
        conn.execute_batch(
            "ALTER TABLE edges ADD COLUMN condition TEXT;
             ALTER TABLE edges ADD COLUMN modifier TEXT;
             UPDATE meta SET value='2' WHERE key='schema_version';",
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent second pass
        let cols = mutation_log_columns(&conn);
        assert!(cols.contains(&"error_code".to_string()));
        assert!(cols.contains(&"trace_id".to_string()));
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    /// W6d (A6): the v1→v4 chain also creates the persistent `dead_letters`
    /// table (DLQ durability) — one pass migrates all stages.
    #[test]
    fn migrate_v1_to_v4_creates_dead_letters_table() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        migrate(&conn).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='dead_letters'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "dead_letters table missing after v1→v4 migration");
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    /// W6d (A6): a v3 colony.db (mutation_log already migrated) migrates to v4,
    /// creating the `dead_letters` table. Idempotent on re-run.
    #[test]
    fn migrate_v3_to_v4_creates_dead_letters_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute_batch(
            "ALTER TABLE edges ADD COLUMN condition TEXT;
             ALTER TABLE edges ADD COLUMN modifier TEXT;
             ALTER TABLE mutation_log ADD COLUMN error_code TEXT;
             ALTER TABLE mutation_log ADD COLUMN trace_id TEXT;
             UPDATE meta SET value='3' WHERE key='schema_version';",
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent second pass
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='dead_letters'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrate_v4_to_v4_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrate_reports_unknown_version_as_error() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', '99');",
        )
        .unwrap();
        assert!(matches!(
            migrate(&conn),
            Err(MigrationError::UnknownVersion(99))
        ));
    }

    #[test]
    fn migrate_recovers_when_condition_column_already_present() {
        let conn = Connection::open_in_memory().unwrap();
        open_v1(&conn);
        conn.execute("ALTER TABLE edges ADD COLUMN condition TEXT", [])
            .unwrap();
        migrate(&conn).unwrap();
        let cols = columns(&conn);
        assert!(cols.contains(&"condition".to_string()));
        assert!(cols.contains(&"modifier".to_string()));
        assert_eq!(
            super::super::schema::read_schema_version(&conn).unwrap(),
            TARGET_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrate_failed_step_leaves_version_at_1_no_partial() {
        // Since GH #90 an absent table is a legal skip (the DDL batch creates
        // it at the target shape afterwards), so the failure is forced at the
        // final version write instead — AFTER the edges ALTERs ran. The pin is
        // the same all-or-nothing promise, now proven against real step work.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', '1');
             CREATE TABLE edges (
               id TEXT PRIMARY KEY, from_path TEXT NOT NULL,
               to_path TEXT NOT NULL, created_at INTEGER NOT NULL);
             CREATE TRIGGER meta_locked BEFORE UPDATE ON meta
             BEGIN SELECT RAISE(ABORT, 'meta locked by test'); END;",
        )
        .unwrap();
        assert!(migrate(&conn).is_err());
        assert_eq!(super::super::schema::read_schema_version(&conn).unwrap(), 1);
        assert!(
            !column_exists(&conn, "edges", "condition").unwrap(),
            "the edges ALTER must roll back with the failed version write"
        );
    }
}
