//! Schema-DDL + PRAGMA-Setup für cell.db und colony.db.
//!
//! **Pflicht-Helper**: kein Schreib-Pfad darf rusqlite direkt öffnen — immer über
//! `setup_cell_db(conn)` (Phase 5) oder `setup_colony_db(conn)` (T6) gehen.
//! Vergessener Aufruf = silent FULL-Sync = 10× langsamer.

const CELL_DB_DDL: &str = "
CREATE TABLE IF NOT EXISTS system (
  slot_path TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS last_input (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  message_json TEXT NOT NULL,
  received_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS params (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '1');
";

/// Setup a cell.db connection: PRAGMAs + DDL.
///
/// PRAGMA-Reihenfolge: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`.
/// DDL ist idempotent (`CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE`).
pub fn setup_cell_db(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(CELL_DB_DDL)?;
    Ok(())
}

const COLONY_DB_DDL: &str = "
CREATE TABLE IF NOT EXISTS registry (
  path        TEXT PRIMARY KEY,
  cell_id     TEXT NOT NULL,
  cell_type   TEXT NOT NULL,
  status      TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS edges (
  id          TEXT PRIMARY KEY,
  from_path   TEXT NOT NULL,
  to_path     TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  condition   TEXT,
  modifier    TEXT
);
CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_path);
CREATE TABLE IF NOT EXISTS hive_scopes (
  path        TEXT PRIMARY KEY,
  created_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS message_log (
  id                  TEXT PRIMARY KEY,
  trace_id            TEXT NOT NULL,
  parent_message_id   TEXT NULL,
  correlation_id      TEXT NULL,
  ttl                 INTEGER NOT NULL,
  from_path           TEXT NOT NULL,
  to_path             TEXT NOT NULL,
  reply_to            TEXT NULL,
  headers             TEXT NOT NULL,
  body_kind           TEXT NOT NULL,
  body_payload        TEXT NULL,
  created_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_msglog_parent  ON message_log(parent_message_id);
CREATE INDEX IF NOT EXISTS idx_msglog_trace   ON message_log(trace_id);
CREATE INDEX IF NOT EXISTS idx_msglog_to      ON message_log(to_path);
CREATE INDEX IF NOT EXISTS idx_msglog_created ON message_log(created_at);
CREATE TABLE IF NOT EXISTS mutation_log (
  id              TEXT PRIMARY KEY,
  scope           TEXT NOT NULL,
  payload_json    TEXT NOT NULL,
  status          TEXT NOT NULL,
  failure_reason  TEXT NULL,
  created_at      INTEGER NOT NULL,
  committed_at    INTEGER NULL,
  error_code      TEXT NULL,
  trace_id        TEXT NULL
);
CREATE INDEX IF NOT EXISTS idx_mutlog_status ON mutation_log(status);
CREATE TABLE IF NOT EXISTS templates (
  template_id      TEXT PRIMARY KEY,
  name             TEXT NOT NULL,
  version          TEXT,
  filesystem_path  TEXT NOT NULL,
  description_json TEXT NOT NULL,
  tags_json        TEXT NOT NULL,
  author           TEXT,
  scanned_at       INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_templates_name_version ON templates(name, COALESCE(version, ''));
CREATE INDEX IF NOT EXISTS idx_templates_name ON templates(name);
CREATE TABLE IF NOT EXISTS dead_letters (
  id              INTEGER PRIMARY KEY,
  sender_path     TEXT NOT NULL,
  original_target TEXT NOT NULL,
  resolved_target TEXT NOT NULL,
  error_code      TEXT NOT NULL,
  trace_id        TEXT NOT NULL,
  created_at      INTEGER NOT NULL,
  message_json    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dlq_created ON dead_letters(created_at);
CREATE INDEX IF NOT EXISTS idx_dlq_error_code ON dead_letters(error_code);
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '4');
";

/// Setup a colony.db connection: PRAGMAs + DDL (alle Phase-5-Tabellen + Indizes).
///
/// PRAGMA-Reihenfolge analog `setup_cell_db`: `journal_mode=WAL`, `synchronous=NORMAL`,
/// `foreign_keys=ON`. DDL ist idempotent.
///
/// **FIX 1 (Marcus-Review 2026-05-20)**: `message_log` enthält `correlation_id`,
/// `ttl`, `reply_to` — lasttragend für Phase 8/10-Request/Response-Korrelation.
pub fn setup_colony_db(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(COLONY_DB_DDL)?;
    super::migrations::migrate(conn).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some(format!("colony.db migration failed: {e}")),
        )
    })?;
    Ok(())
}

/// Read the schema_version from the `meta` table.
///
/// Pflicht-Helper für integrity-Probes in `plan_bootstrap` (T19) und für
/// `probe_boot_state` (T20). Erwartet eine via `setup_cell_db` oder
/// `setup_colony_db` initialisierte DB.
pub fn read_schema_version(conn: &rusqlite::Connection) -> rusqlite::Result<u32> {
    let s: String = conn.query_row(
        "SELECT value FROM meta WHERE key='schema_version'",
        [],
        |r| r.get(0),
    )?;
    s.parse::<u32>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_cell_db_creates_system_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='system'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn setup_cell_db_creates_last_input_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='last_input'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn setup_cell_db_creates_params_table() {
        // W4b: runtime-param overlay table (last-write-wins, keyed by param
        // name). Additive — schema_version stays 1.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='params'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn setup_cell_db_params_table_does_not_bump_schema_version() {
        // W4b invariant: the additive params table must NOT change the cell.db
        // schema_version — existing cell.dbs (no params table) reopen cleanly
        // and `check_schema_version`==1 keeps passing.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        assert_eq!(read_schema_version(&conn).unwrap(), 1);
    }

    #[test]
    fn setup_cell_db_creates_meta_table_with_schema_version() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "1");
    }

    #[test]
    fn setup_cell_db_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        setup_cell_db(&conn).unwrap(); // zweiter Aufruf darf nicht fehlschlagen
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "meta INSERT OR IGNORE bleibt single-row");
    }

    #[test]
    fn setup_colony_db_creates_message_log_with_fix1_columns() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(message_log)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        // FIX 1: correlation_id, ttl, reply_to müssen Spalten sein.
        assert!(
            cols.contains(&"correlation_id".to_string()),
            "correlation_id missing"
        );
        assert!(cols.contains(&"ttl".to_string()), "ttl missing");
        assert!(cols.contains(&"reply_to".to_string()), "reply_to missing");
        // Plus alle anderen Pflicht-Spalten.
        for required in &[
            "id",
            "trace_id",
            "parent_message_id",
            "from_path",
            "to_path",
            "headers",
            "body_kind",
            "body_payload",
            "created_at",
        ] {
            assert!(cols.contains(&required.to_string()), "{} missing", required);
        }
    }

    #[test]
    fn setup_colony_db_creates_registry_with_cell_id_and_status() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(registry)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for required in &[
            "path",
            "cell_id",
            "cell_type",
            "status",
            "created_at",
            "updated_at",
        ] {
            assert!(cols.contains(&required.to_string()), "{} missing", required);
        }
    }

    #[test]
    fn setup_colony_db_creates_edges_hive_scopes_meta() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        for table in &["edges", "hive_scopes", "meta"] {
            let cnt: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(cnt, 1, "{} missing", table);
        }
    }

    #[test]
    fn setup_colony_db_creates_message_log_indices() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        for idx in &[
            "idx_msglog_parent",
            "idx_msglog_trace",
            "idx_msglog_to",
            "idx_msglog_created",
        ] {
            let cnt: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(cnt, 1, "{} missing", idx);
        }
    }

    #[test]
    fn setup_colony_db_seeds_schema_version_4() {
        // Phase-16 W6d (A6): the persistent dead_letters table → schema v4.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "4");
    }

    #[test]
    fn setup_colony_db_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        setup_colony_db(&conn).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn read_schema_version_returns_1_after_cell_setup() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_cell_db(&conn).unwrap();
        assert_eq!(read_schema_version(&conn).unwrap(), 1);
    }

    #[test]
    fn read_schema_version_returns_4_after_colony_setup() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        assert_eq!(read_schema_version(&conn).unwrap(), 4);
    }

    #[test]
    fn setup_colony_db_creates_mutation_log_with_required_columns() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(mutation_log)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for required in &[
            "id",
            "scope",
            "payload_json",
            "status",
            "failure_reason",
            "created_at",
            "committed_at",
        ] {
            assert!(cols.contains(&required.to_string()), "{} missing", required);
        }
    }

    /// Phase-16 W3 (A6): the v3 schema adds `error_code` + `trace_id` columns to
    /// `mutation_log` — the two reject-row fields that have no home in v2
    /// (status/failure_reason/created_at already exist). A fresh colony.db must
    /// create them.
    #[test]
    fn setup_colony_db_creates_mutation_log_with_reject_columns() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(mutation_log)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            cols.contains(&"error_code".to_string()),
            "error_code missing"
        );
        assert!(cols.contains(&"trace_id".to_string()), "trace_id missing");
    }

    #[test]
    fn setup_colony_db_creates_mutation_log_status_index() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_mutlog_status'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "idx_mutlog_status missing");
    }

    #[test]
    fn setup_colony_db_creates_templates_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(templates)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for required in &[
            "template_id",
            "name",
            "version",
            "filesystem_path",
            "description_json",
            "tags_json",
            "author",
            "scanned_at",
        ] {
            assert!(
                cols.iter().any(|c| c == required),
                "missing column: {required} (have: {cols:?})"
            );
        }
    }

    #[test]
    fn setup_colony_db_templates_unique_name_version() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO templates (template_id, name, version, filesystem_path, description_json, tags_json, author, scanned_at) \
             VALUES ('t1', 'echo', '1.0', '/tmp/a', '{}', '[]', NULL, 0)",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO templates (template_id, name, version, filesystem_path, description_json, tags_json, author, scanned_at) \
             VALUES ('t2', 'echo', '1.0', '/tmp/b', '{}', '[]', NULL, 0)",
            [],
        );
        assert!(dup.is_err(), "UNIQUE(name, version) must reject duplicate");
    }

    #[test]
    fn setup_colony_db_on_fresh_db_creates_v2_edges_schema() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(edges)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(cols.contains(&"condition".to_string()));
        assert!(cols.contains(&"modifier".to_string()));
        assert_eq!(read_schema_version(&conn).unwrap(), 4);
    }

    #[test]
    fn setup_colony_db_on_v1_db_migrates_to_v3() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema_version', '1');
             CREATE TABLE edges (
               id TEXT PRIMARY KEY, from_path TEXT NOT NULL,
               to_path TEXT NOT NULL, created_at INTEGER NOT NULL);",
        )
        .unwrap();
        setup_colony_db(&conn).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(edges)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(cols.contains(&"condition".to_string()));
        assert!(cols.contains(&"modifier".to_string()));
        assert_eq!(read_schema_version(&conn).unwrap(), 4);
    }

    /// W6d (A6): a fresh colony.db has the persistent `dead_letters` table with
    /// all six DTO columns (sender/original/resolved path + error_code + trace_id
    /// + created_at). Pin against accidental schema drift.
    #[test]
    fn setup_colony_db_creates_dead_letters_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        setup_colony_db(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(dead_letters)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for required in &[
            "sender_path",
            "original_target",
            "resolved_target",
            "error_code",
            "trace_id",
            "created_at",
        ] {
            assert!(
                cols.iter().any(|c| c == required),
                "missing column: {required} (have: {cols:?})"
            );
        }
    }
}
