//! Phase-7 Slice 2 — geteilte Security-Boundary-Resolver für Tool-Cells.
//!
//! Aus FileCell extrahiert (Slice 1) und für EditCell wiederverwendet.
//! Freie Funktionen + `ResolveErr` + `resolve_error_code`. FileCells
//! `resolve_existing`/`resolve_write_parent`-Methoden delegieren intern
//! an diese freien Funktionen — Signatur unverändert, Verhalten
//! byte-für-byte identisch (FileCells T4-Security-Tests sind der
//! Regressions-Wächter).

use crate::tool::{ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_NOT_FOUND, ERR_PATH_OUTSIDE_BOUNDARY};
use std::path::{Path as StdPath, PathBuf};

/// Error variants for path resolution.
#[derive(Debug)]
pub(crate) enum ResolveErr {
    /// Caller supplied an absolute path where a relative path is required.
    AbsoluteRel,
    /// Resolved path escapes the `base_path` boundary.
    OutsideBoundary,
    /// The path (or a component) does not exist on disk.
    NotFound(String),
    /// I/O error during canonicalize or metadata check (other than NotFound).
    Io(String),
}

impl std::fmt::Display for ResolveErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveErr::AbsoluteRel => write!(f, "path must be relative, not absolute"),
            ResolveErr::OutsideBoundary => {
                write!(f, "path resolves outside base_path boundary")
            }
            ResolveErr::NotFound(e) => write!(f, "path not found: {e}"),
            ResolveErr::Io(e) => write!(f, "io error during resolve: {e}"),
        }
    }
}

/// Maps a resolve-Error to an error_code string. Caller emits via `build_error_body`.
pub(crate) fn resolve_error_code(reason: &ResolveErr) -> &'static str {
    match reason {
        ResolveErr::AbsoluteRel => ERR_INVALID_INPUT,
        ResolveErr::OutsideBoundary => ERR_PATH_OUTSIDE_BOUNDARY,
        ResolveErr::NotFound(_) => ERR_NOT_FOUND,
        ResolveErr::Io(_) => ERR_IO_ERROR,
    }
}

fn io_err_to_resolve(e: std::io::Error) -> ResolveErr {
    if e.kind() == std::io::ErrorKind::NotFound {
        ResolveErr::NotFound(e.to_string())
    } else {
        ResolveErr::Io(e.to_string())
    }
}

/// Resolve `rel` for read/list/stat: must exist, canonicalize follows
/// symlinks; final canonical path must be under `base.canonicalize()`.
pub(crate) fn resolve_existing(base: &StdPath, rel: &str) -> Result<PathBuf, ResolveErr> {
    let rel_path = StdPath::new(rel);
    if rel_path.is_absolute() {
        return Err(ResolveErr::AbsoluteRel);
    }
    let canon_base = base.canonicalize().map_err(io_err_to_resolve)?;
    let joined = canon_base.join(rel_path);
    let resolved = joined.canonicalize().map_err(io_err_to_resolve)?;
    if !resolved.starts_with(&canon_base) {
        return Err(ResolveErr::OutsideBoundary);
    }
    Ok(resolved)
}

/// Resolve `rel` for write: parent MUST exist (Entscheidung 1.1, no
/// auto-`create_dir_all`); file itself may be new. Parent is canonicalized
/// (symlink-safe); final path = canon_parent.join(file_name).
pub(crate) fn resolve_write_parent(base: &StdPath, rel: &str) -> Result<PathBuf, ResolveErr> {
    let rel_path = StdPath::new(rel);
    if rel_path.is_absolute() {
        return Err(ResolveErr::AbsoluteRel);
    }
    let canon_base = base.canonicalize().map_err(io_err_to_resolve)?;
    let joined = canon_base.join(rel_path);
    let parent = joined
        .parent()
        .ok_or_else(|| ResolveErr::Io("no parent path component".into()))?;
    let file_name = joined
        .file_name()
        .ok_or_else(|| ResolveErr::Io("no file_name component".into()))?
        .to_os_string();
    let canon_parent = parent
        .canonicalize()
        .map_err(|e| ResolveErr::Io(e.to_string()))?;
    if !canon_parent.starts_with(&canon_base) {
        return Err(ResolveErr::OutsideBoundary);
    }
    Ok(canon_parent.join(file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_existing_accepts_inside() {
        let td = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("a.txt"), b"hi").unwrap();
        let r = resolve_existing(td.path(), "a.txt").unwrap();
        assert!(r.ends_with("a.txt"));
        assert!(r.starts_with(td.path().canonicalize().unwrap()));
    }

    #[test]
    fn resolve_existing_rejects_absolute_rel() {
        let td = tempfile::TempDir::new().unwrap();
        assert!(matches!(
            resolve_existing(td.path(), "/etc/passwd"),
            Err(ResolveErr::AbsoluteRel)
        ));
    }

    #[test]
    fn resolve_existing_rejects_traversal() {
        let td = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("a.txt"), b"hi").unwrap();
        assert!(resolve_existing(td.path(), "../../../etc/passwd").is_err());
    }

    #[test]
    fn resolve_existing_missing_path_returns_not_found() {
        let td = tempfile::TempDir::new().unwrap();
        let err = resolve_existing(td.path(), "nope.txt").unwrap_err();
        assert!(matches!(err, ResolveErr::NotFound(_)));
        assert_eq!(resolve_error_code(&err), "not_found");
    }

    #[test]
    fn resolve_write_parent_accepts_existing_parent() {
        let td = tempfile::TempDir::new().unwrap();
        let r = resolve_write_parent(td.path(), "new.txt").unwrap();
        assert!(r.ends_with("new.txt"));
    }

    #[test]
    fn resolve_write_parent_rejects_missing_parent() {
        let td = tempfile::TempDir::new().unwrap();
        let err = resolve_write_parent(td.path(), "subdir/new.txt").unwrap_err();
        assert!(matches!(err, ResolveErr::Io(_) | ResolveErr::NotFound(_)));
    }

    #[test]
    fn resolve_existing_rejects_symlink_escape() {
        let td = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path().join("secret.txt"), td.path().join("escape"))
                .unwrap();
            let err = resolve_existing(td.path(), "escape").unwrap_err();
            assert!(matches!(err, ResolveErr::OutsideBoundary));
        }
        #[cfg(not(unix))]
        {
            let _ = (td, outside);
        }
    }
}
