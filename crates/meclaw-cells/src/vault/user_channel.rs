//! The user channel — storing, listing and revoking a secret **without a
//! colony**.
//!
//! This is the filling workflow from the design, and its whole point is what
//! does not happen: no message is built, no edge is travelled, no message log
//! row is written, no context window ever sees the value. The secret goes from
//! the operator's stdin into the vault's own database and stops there.
//!
//! It lives in this crate rather than in the CLI because it is vault
//! behaviour — the same sealing, the same salt, the same no-delete history the
//! cell uses. The CLI above it only decides where the bytes come from.

use super::crypto::MasterKey;
use super::store;
use std::path::{Path, PathBuf};

/// Where a cell's storage lives: its colony path, resolved under the root.
pub fn cell_dir(root: &Path, cell_path: &str) -> Result<PathBuf, String> {
    if !cell_path.starts_with('/') {
        return Err(format!(
            "expected an absolute cell path, e.g. /main/access/vault (got {cell_path:?})"
        ));
    }
    let mut dir = root.to_path_buf();
    for segment in cell_path.trim_start_matches('/').split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("{cell_path:?} is not a well-formed cell path"));
        }
        dir.push(segment);
    }
    Ok(dir)
}

/// Open an existing vault's database. Refuses to create one: a typo in the
/// cell path must not produce an empty vault that looks like it worked.
fn open(root: &Path, cell_path: &str) -> Result<rusqlite::Connection, String> {
    let db_file = cell_dir(root, cell_path)?.join("cell.db");
    if !db_file.is_file() {
        return Err(format!(
            "no vault at {cell_path} — {} does not exist. Check the cell path and that the \
             colony has booted once.",
            db_file.display()
        ));
    }
    let conn = rusqlite::Connection::open(&db_file)
        .map_err(|e| format!("opening {}: {e}", db_file.display()))?;
    store::apply_ddl(&conn)?;
    Ok(conn)
}

/// What the vault holds: names and their active version. Never content.
pub fn status(root: &Path, cell_path: &str) -> Result<Vec<(String, i64)>, String> {
    store::names(&open(root, cell_path)?)
}

/// Revoke every active version of `name`, and say how many that was.
///
/// Deliberately needs no passphrase. Being locked out of a vault must never
/// stop you from disabling a credential that leaked.
pub fn revoke(root: &Path, cell_path: &str, name: &str) -> Result<usize, String> {
    let conn = open(root, cell_path)?;
    let changed = store::revoke(&conn, name)?;
    store::audit(
        &conn,
        &store::now_iso(),
        "revoke",
        "user-channel",
        Some(name),
        "ok",
        None,
    )?;
    Ok(changed)
}

/// Seal `secret` under `name` and append it. Returns the version written.
///
/// If the vault already holds something, the passphrase is proven against it
/// first. Without that check a typo would quietly create a second half of the
/// store that the running vault can never open — and the operator would only
/// find out at the first `use`, in production, at night.
pub fn add(
    root: &Path,
    cell_path: &str,
    name: &str,
    secret: &[u8],
    passphrase: &[u8],
) -> Result<i64, String> {
    if secret.is_empty() {
        return Err("the secret is empty — nothing was stored".into());
    }
    let conn = open(root, cell_path)?;
    let salt = store::salt_or_create(&conn)?;
    let key = MasterKey::derive(passphrase, &salt).map_err(|e| e.to_string())?;

    if let Some((first, _)) = store::names(&conn)?.first()
        && let Some(sealed) = store::active(&conn, first)?
        && key.open(&sealed.nonce, &sealed.ciphertext).is_err()
    {
        store::audit(
            &conn,
            &store::now_iso(),
            "put",
            "user-channel",
            Some(name),
            "refused",
            Some("wrong_passphrase"),
        )?;
        return Err(
            "that passphrase does not open this vault — nothing was stored. \
                    (Storing under a second passphrase would leave half the vault unopenable.)"
                .into(),
        );
    }

    let (nonce, ciphertext) = key.seal(secret).map_err(|e| e.to_string())?;
    let version = store::put(&conn, name, &nonce, &ciphertext, &store::now_iso())?;
    store::audit(
        &conn,
        &store::now_iso(),
        "put",
        "user-channel",
        Some(name),
        "ok",
        None,
    )?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault that has booted once — which is what `open` insists on.
    fn booted_vault() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("main").join("access").join("vault");
        std::fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("cell.db")).unwrap();
        store::apply_ddl(&conn).unwrap();
        let path = root.path().to_path_buf();
        (root, path)
    }

    #[test]
    fn a_cell_path_becomes_its_directory_under_the_root() {
        assert_eq!(
            cell_dir(Path::new("/srv/colony"), "/main/access/vault").unwrap(),
            PathBuf::from("/srv/colony/main/access/vault")
        );
    }

    #[test]
    fn a_relative_or_traversing_cell_path_is_refused() {
        assert!(cell_dir(Path::new("/srv"), "main/vault").is_err());
        assert!(cell_dir(Path::new("/srv"), "/main/../../etc").is_err());
        assert!(cell_dir(Path::new("/srv"), "/main//vault").is_err());
    }

    #[test]
    fn a_missing_vault_says_so_instead_of_creating_one() {
        let root = tempfile::tempdir().unwrap();
        let err = status(root.path(), "/main/access/vault").unwrap_err();
        assert!(err.contains("no vault at /main/access/vault"), "{err}");
        assert!(
            !root.path().join("main").exists(),
            "a typo must not leave a directory tree behind"
        );
    }

    #[test]
    fn a_stored_secret_shows_up_as_metadata_only() {
        let (_root, path) = booted_vault();
        assert_eq!(
            add(&path, "/main/access/vault", "tg", b"tok", b"pw").unwrap(),
            1
        );
        assert_eq!(
            status(&path, "/main/access/vault").unwrap(),
            vec![("tg".to_string(), 1)]
        );
    }

    #[test]
    fn a_second_put_appends_a_version_rather_than_overwriting() {
        let (_root, path) = booted_vault();
        add(&path, "/main/access/vault", "tg", b"first", b"pw").unwrap();
        assert_eq!(
            add(&path, "/main/access/vault", "tg", b"second", b"pw").unwrap(),
            2
        );
    }

    #[test]
    fn a_wrong_passphrase_is_caught_before_anything_is_written() {
        let (_root, path) = booted_vault();
        add(&path, "/main/access/vault", "tg", b"first", b"pw").unwrap();
        let err = add(&path, "/main/access/vault", "other", b"x", b"typo").unwrap_err();
        assert!(err.contains("does not open this vault"), "{err}");
        assert_eq!(
            status(&path, "/main/access/vault").unwrap(),
            vec![("tg".to_string(), 1)],
            "the second name must not exist"
        );
    }

    #[test]
    fn revoking_needs_no_passphrase_and_keeps_the_row() {
        let (_root, path) = booted_vault();
        add(&path, "/main/access/vault", "tg", b"tok", b"pw").unwrap();
        assert_eq!(revoke(&path, "/main/access/vault", "tg").unwrap(), 1);
        assert!(status(&path, "/main/access/vault").unwrap().is_empty());
        assert_eq!(revoke(&path, "/main/access/vault", "tg").unwrap(), 0);

        let conn = rusqlite::Connection::open(path.join("main/access/vault/cell.db")).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM vault_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "no-delete holds on the user channel too");
    }

    #[test]
    fn an_empty_secret_is_refused() {
        let (_root, path) = booted_vault();
        assert!(add(&path, "/main/access/vault", "tg", b"", b"pw").is_err());
    }

    #[test]
    fn the_user_channel_is_recorded_as_the_actor() {
        let (_root, path) = booted_vault();
        add(&path, "/main/access/vault", "tg", b"tok", b"pw").unwrap();
        let conn = rusqlite::Connection::open(path.join("main/access/vault/cell.db")).unwrap();
        let actor: String = conn
            .query_row("SELECT actor FROM vault_audit LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(actor, "user-channel");
    }
}
