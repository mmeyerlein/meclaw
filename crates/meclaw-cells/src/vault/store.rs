//! The vault's own storage: sealed secrets, their history, and an audit trail.
//!
//! Three tables, all append-only in the meclaw sense — no row is ever deleted:
//!
//! - `vault_meta` holds the salt the master key is derived against. It is not
//!   secret; it exists so that two vaults with the same passphrase do not share
//!   a key.
//! - `vault_secrets` holds one row per (name, version). A rotation appends a
//!   version, a revocation flips a status. Yesterday's ciphertext stays on
//!   disk, which is what makes a revocation auditable rather than a hole.
//! - `vault_audit` records every operation the cell performed, including the
//!   refusals. A vault whose refusals are invisible cannot be reviewed.

use rusqlite::{Connection, OptionalExtension, params};

/// The timestamp format every vault row carries. One helper so the cell and
/// the user channel stamp identically — a store whose two writers disagree on
/// time format is a store whose history cannot be read in order.
pub fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Apply the vault DDL. Idempotent — runs on every wake and respawn.
pub fn apply_ddl(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vault_meta (
             k TEXT PRIMARY KEY,
             v BLOB NOT NULL
         );
         CREATE TABLE IF NOT EXISTS vault_secrets (
             name       TEXT NOT NULL,
             version    INTEGER NOT NULL,
             nonce      BLOB NOT NULL,
             ciphertext BLOB NOT NULL,
             status     TEXT NOT NULL DEFAULT 'active',
             created_at TEXT NOT NULL,
             PRIMARY KEY (name, version)
         );
         CREATE TABLE IF NOT EXISTS vault_audit (
             id      INTEGER PRIMARY KEY AUTOINCREMENT,
             at      TEXT NOT NULL,
             op      TEXT NOT NULL,
             actor   TEXT NOT NULL,
             name    TEXT,
             outcome TEXT NOT NULL,
             reason  TEXT
         );",
    )
    .map_err(|e| format!("vault: DDL failed: {e}"))
}

/// Read the store's salt, creating one on first use.
///
/// The salt is born with the store, not with the key: rotating a passphrase
/// must not orphan the secrets sealed under the old one, so re-sealing is an
/// explicit operation and the salt stays put.
pub fn salt_or_create(conn: &Connection) -> Result<Vec<u8>, String> {
    let existing: Option<Vec<u8>> = conn
        .query_row("SELECT v FROM vault_meta WHERE k = 'salt'", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| format!("vault: reading salt failed: {e}"))?;
    if let Some(s) = existing {
        return Ok(s);
    }
    let fresh = super::crypto::new_salt()
        .map_err(|e| e.to_string())?
        .to_vec();
    conn.execute(
        "INSERT INTO vault_meta (k, v) VALUES ('salt', ?1)",
        params![fresh],
    )
    .map_err(|e| format!("vault: writing salt failed: {e}"))?;
    Ok(fresh)
}

/// One stored version of a secret.
pub struct SealedSecret {
    /// Version number; the highest active one is what `use` reaches.
    pub version: i64,
    /// The nonce this version was sealed with.
    pub nonce: Vec<u8>,
    /// The sealed bytes.
    pub ciphertext: Vec<u8>,
}

/// Append a new version of `name`. Returns the version written.
pub fn put(
    conn: &Connection,
    name: &str,
    nonce: &[u8],
    ciphertext: &[u8],
    at: &str,
) -> Result<i64, String> {
    let next: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM vault_secrets WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .map_err(|e| format!("vault: version lookup failed: {e}"))?;
    conn.execute(
        "INSERT INTO vault_secrets (name, version, nonce, ciphertext, status, created_at)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5)",
        params![name, next, nonce, ciphertext, at],
    )
    .map_err(|e| format!("vault: storing secret failed: {e}"))?;
    Ok(next)
}

/// The active version of `name`, or `None` when there is none — either because
/// nothing was ever stored under that name or because every version is revoked.
pub fn active(conn: &Connection, name: &str) -> Result<Option<SealedSecret>, String> {
    conn.query_row(
        "SELECT version, nonce, ciphertext FROM vault_secrets
         WHERE name = ?1 AND status = 'active'
         ORDER BY version DESC LIMIT 1",
        params![name],
        |r| {
            Ok(SealedSecret {
                version: r.get(0)?,
                nonce: r.get(1)?,
                ciphertext: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("vault: secret lookup failed: {e}"))
}

/// Revoke every version of `name`. Returns how many rows changed status; zero
/// means there was nothing active to revoke.
pub fn revoke(conn: &Connection, name: &str) -> Result<usize, String> {
    conn.execute(
        "UPDATE vault_secrets SET status = 'revoked' WHERE name = ?1 AND status = 'active'",
        params![name],
    )
    .map_err(|e| format!("vault: revoke failed: {e}"))
}

/// The names the vault holds, with their active version. Metadata only — this
/// is what `status` may report, and it is deliberately the *most* a caller can
/// ever learn about the content.
pub fn names(conn: &Connection) -> Result<Vec<(String, i64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT name, MAX(version) FROM vault_secrets
             WHERE status = 'active' GROUP BY name ORDER BY name",
        )
        .map_err(|e| format!("vault: name listing failed: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| format!("vault: name listing failed: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("vault: name listing failed: {e}"))
}

/// Append an audit row. Every operation lands here, refusals included.
pub fn audit(
    conn: &Connection,
    at: &str,
    op: &str,
    actor: &str,
    name: Option<&str>,
    outcome: &str,
    reason: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO vault_audit (at, op, actor, name, outcome, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![at, op, actor, name, outcome, reason],
    )
    .map(|_| ())
    .map_err(|e| format!("vault: audit write failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::crypto::MasterKey;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        apply_ddl(&c).unwrap();
        c
    }

    #[test]
    fn the_salt_is_created_once_and_then_kept() {
        let c = db();
        let a = salt_or_create(&c).unwrap();
        let b = salt_or_create(&c).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), crate::vault::crypto::SALT_LEN);
    }

    #[test]
    fn putting_a_name_twice_appends_a_version_and_the_old_one_stays() {
        let c = db();
        let key = MasterKey::derive(b"pw", &salt_or_create(&c).unwrap()).unwrap();
        let (n1, c1) = key.seal(b"first").unwrap();
        assert_eq!(put(&c, "tg", &n1, &c1, "2026-08-16T00:00:00Z").unwrap(), 1);
        let (n2, c2) = key.seal(b"second").unwrap();
        assert_eq!(put(&c, "tg", &n2, &c2, "2026-08-16T00:01:00Z").unwrap(), 2);

        let live = active(&c, "tg").unwrap().unwrap();
        assert_eq!(live.version, 2);
        assert_eq!(key.open(&live.nonce, &live.ciphertext).unwrap(), b"second");

        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM vault_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "no-delete: the old version stays on disk");
    }

    #[test]
    fn a_revoked_secret_is_gone_from_use_but_not_from_the_record() {
        let c = db();
        let key = MasterKey::derive(b"pw", &salt_or_create(&c).unwrap()).unwrap();
        let (n, ct) = key.seal(b"s").unwrap();
        put(&c, "tg", &n, &ct, "2026-08-16T00:00:00Z").unwrap();

        assert_eq!(revoke(&c, "tg").unwrap(), 1);
        assert!(active(&c, "tg").unwrap().is_none());
        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM vault_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(revoke(&c, "tg").unwrap(), 0, "already revoked");
    }

    #[test]
    fn names_report_metadata_and_never_content() {
        let c = db();
        let key = MasterKey::derive(b"pw", &salt_or_create(&c).unwrap()).unwrap();
        for (name, secret) in [("a", "s1"), ("b", "s2")] {
            let (n, ct) = key.seal(secret.as_bytes()).unwrap();
            put(&c, name, &n, &ct, "2026-08-16T00:00:00Z").unwrap();
        }
        revoke(&c, "b").unwrap();
        assert_eq!(names(&c).unwrap(), vec![("a".to_string(), 1)]);
    }

    #[test]
    fn refusals_are_audited_too() {
        let c = db();
        audit(
            &c,
            "2026-08-16T00:00:00Z",
            "use",
            "/main/talky/brain",
            Some("tg"),
            "refused",
            Some("sender_not_broker"),
        )
        .unwrap();
        let (outcome, reason): (String, Option<String>) = c
            .query_row("SELECT outcome, reason FROM vault_audit", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(outcome, "refused");
        assert_eq!(reason.as_deref(), Some("sender_not_broker"));
    }
}
