//! Schema-Versions-Migration für `colony.db`.
//!
//! v1 (Phase-5-Baseline): edges-Tabelle ohne CEL-State.
//! v2 (Phase-13.5-Durable-Edges): edges erhält `condition` + `modifier`
//! als TEXT NULL Spalten. ALTER TABLE in-place, kein CREATE-new+COPY (F4).
//! v3 (Phase-16 W3 / A6): `mutation_log` erhält `error_code` + `trace_id`
//! als TEXT NULL Spalten — die zwei Reject-Row-Felder ohne Heimat in v2
//! (status/failure_reason/created_at existieren bereits). ALTER TABLE in-place.
//! v4 (Phase-16 W6d / A6): neue Tabelle `dead_letters` — die DLQ ist nicht mehr
//! ein flüchtiger In-Memory-`VecDeque`, sondern persistiert in `colony.db`
//! (Crash-/Shutdown-Survival der Diagnose-Wahrheit). Spalten: die 6 Lokalisierungs-
//! Felder (`DeadLetterDto`) PLUS `message_json` — der volle Message-Envelope,
//! serialisiert mit denselben Primitiven wie `message_log` (Marcus-Ruling W6d:
//! Envelope mitpersistieren → Drain rekonstruiert volle `DeadLetter` aus DB).
//! `CREATE TABLE IF NOT EXISTS`, additiv (keine Spalten-Änderung an Bestehendem).
//!
//! WICHTIG: betrifft ausschließlich `colony.db`. `cell.db` bleibt v1.

use rusqlite::Connection;

/// Ziel-Schema-Version für `colony.db` nach diesem Slice.
pub(crate) const TARGET_SCHEMA_VERSION: u32 = 4;

/// Fehler während der `colony.db`-Schema-Migration.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MigrationError {
    /// rusqlite-Fehler beim Lesen der Version oder beim ALTER TABLE.
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    /// `schema_version` ist weder die aktuelle noch eine bekannte Vorgänger-Version.
    #[error("unknown schema_version {0}; target is {}", TARGET_SCHEMA_VERSION)]
    UnknownVersion(u32),
}

/// Migriert `colony.db` idempotent auf [`TARGET_SCHEMA_VERSION`].
///
/// Atomar: alle `ALTER TABLE` + der Versions-Bump laufen in EINER Transaktion.
/// Schlägt ein Schritt fehl, rollt alles zurück → `schema_version` bleibt
/// unverändert. `ADD COLUMN` läuft nur, wenn die Spalte fehlt → ein abgebrochener
/// Vor-Lauf ist beim Re-Run sauber nachholbar (kein duplicate-column-Deadend).
///
/// Die Migration ist gestuft (v1→v2: edges-CEL-Spalten, v2→v3: mutation_log-
/// Reject-Spalten) und chained in einem Lauf: eine v1-DB erhält beide Stufen.
/// `mutation_log` existiert beim Aufruf immer — `setup_colony_db` führt das
/// `CREATE TABLE IF NOT EXISTS`-DDL VOR `migrate` aus.
pub(crate) fn migrate(conn: &Connection) -> Result<(), MigrationError> {
    let current = super::schema::read_schema_version(conn)?;
    match current {
        v if v == TARGET_SCHEMA_VERSION => Ok(()),
        1..=3 => {
            let tx = conn.unchecked_transaction()?;
            // v1→v2: durable-edges CEL-Spalten.
            if current <= 1 {
                if !column_exists(&tx, "edges", "condition")? {
                    tx.execute("ALTER TABLE edges ADD COLUMN condition TEXT", [])?;
                }
                if !column_exists(&tx, "edges", "modifier")? {
                    tx.execute("ALTER TABLE edges ADD COLUMN modifier TEXT", [])?;
                }
            }
            // v2→v3 (A6): mutation_log Reject-Spalten.
            if current <= 2 {
                if !column_exists(&tx, "mutation_log", "error_code")? {
                    tx.execute("ALTER TABLE mutation_log ADD COLUMN error_code TEXT", [])?;
                }
                if !column_exists(&tx, "mutation_log", "trace_id")? {
                    tx.execute("ALTER TABLE mutation_log ADD COLUMN trace_id TEXT", [])?;
                }
            }
            // v3→v4 (W6d/A6): persistente DLQ-Tabelle. `CREATE TABLE IF NOT EXISTS`
            // ist idempotent — eine via `setup_colony_db` (DDL) bereits angelegte
            // Tabelle bleibt unangetastet; eine alt-migrierte DB (oder der reine
            // `migrate`-Pfad ohne DDL) erhält sie hier. `id` = rowid (monotone
            // Einfüge-Reihenfolge); die 6 Felder spiegeln das `DeadLetterDto`.
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

/// True, wenn `table` die Spalte `col` bereits hat (PRAGMA table_info).
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
             CREATE TABLE mutation_log (
               id TEXT PRIMARY KEY, scope TEXT NOT NULL, payload_json TEXT NOT NULL,
               status TEXT NOT NULL, failure_reason TEXT NULL,
               created_at INTEGER NOT NULL, committed_at INTEGER NULL);",
        )
        .unwrap();
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
        assert_eq!(super::super::schema::read_schema_version(&conn).unwrap(), 4);
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
        assert_eq!(super::super::schema::read_schema_version(&conn).unwrap(), 4);
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
        assert_eq!(super::super::schema::read_schema_version(&conn).unwrap(), 4);
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
        assert_eq!(super::super::schema::read_schema_version(&conn).unwrap(), 4);
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
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', '1');",
        )
        .unwrap(); // edges table intentionally absent
        assert!(migrate(&conn).is_err());
        assert_eq!(super::super::schema::read_schema_version(&conn).unwrap(), 1);
    }
}
