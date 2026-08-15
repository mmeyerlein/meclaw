//! Phase-7 slice 2 — shared security-boundary resolvers for tool cells.
//!
//! Extracted from FileCell (slice 1) and reused for EditCell.
//! Freie Funktionen + `ResolveErr` + `resolve_error_code`. FileCells
//! `resolve_existing`/`resolve_write_parent`-Methoden delegieren intern
//! to these free functions — signature unchanged, behaviour byte-for-byte
//! identical (FileCell's T4 security tests are the regression guard).

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
    /// GH #79: the write path could not use the parent directory. Carries the
    /// parent **as the caller wrote it** — that string is the argument a repair
    /// (`mkdir -p`, a retry one level up) takes, so it is the part of the
    /// failure worth naming.
    WriteParent {
        /// Relative parent path from the caller's `path`, `"."` for the base.
        parent: String,
        /// Why the parent is unusable.
        why: WriteParentErr,
    },
}

/// GH #79: the ways a write parent can be unusable. One repair per variant —
/// that is the reason they are separate at all.
#[derive(Debug)]
pub(crate) enum WriteParentErr {
    /// No such directory. Repair: create it.
    Missing,
    /// Something else stands in that position. Repair: aim elsewhere.
    NotADirectory,
    /// It exists but this process may not traverse it. No repair from here.
    Unreachable,
    /// Anything else `canonicalize` reported, verbatim.
    Other(String),
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
            ResolveErr::WriteParent { parent, why } => match why {
                WriteParentErr::Missing => write!(
                    f,
                    "parent directory does not exist: {parent} \
                     (write does not create directories)"
                ),
                WriteParentErr::NotADirectory => {
                    write!(f, "parent path is not a directory: {parent}")
                }
                WriteParentErr::Unreachable => {
                    write!(
                        f,
                        "parent directory not accessible: {parent} (permission denied)"
                    )
                }
                WriteParentErr::Other(e) => {
                    write!(f, "parent directory could not be resolved: {parent} ({e})")
                }
            },
        }
    }
}

/// Maps a resolve-Error to an error_code string. Caller emits via `build_error_body`.
///
/// GH #79 note: `WriteParent` is `io_error` like the `Io` it split off from —
/// the issue changed the message, deliberately not the taxonomy.
pub(crate) fn resolve_error_code(reason: &ResolveErr) -> &'static str {
    match reason {
        ResolveErr::AbsoluteRel => ERR_INVALID_INPUT,
        ResolveErr::OutsideBoundary => ERR_PATH_OUTSIDE_BOUNDARY,
        ResolveErr::NotFound(_) => ERR_NOT_FOUND,
        ResolveErr::Io(_) | ResolveErr::WriteParent { .. } => ERR_IO_ERROR,
    }
}

/// GH #107: does `rel` climb above the boundary **lexically** — without asking
/// the filesystem anything?
///
/// This runs BEFORE any `canonicalize`, and that ordering is the whole point:
/// `canonicalize` fails `not_found` on a missing target, so the old order let
/// `../missing` answer `not_found` while `../existing` answered
/// `path_outside_boundary` — a (weak) existence oracle for the world outside
/// the fence. With the pre-check every escape attempt answers identically,
/// whatever is or is not out there.
///
/// The rule is a plain component walk: `.` is nothing, a name descends, `..`
/// ascends, and popping past the base is an escape — **even if a later
/// component would come back in** (`../<base-name>/x`). Deciding that would
/// mean resolving names outside the fence, which is exactly the oracle being
/// closed. Symlinks are invisible here by design; the post-`canonicalize`
/// boundary comparison still runs and catches them.
fn lexically_escapes(rel: &StdPath) -> bool {
    use std::path::Component;
    let mut depth: usize = 0;
    for c in rel.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            // Unreachable after the `is_absolute` gate; refusing is the safe
            // reading either way.
            Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
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
///
/// GH #107: the lexical boundary pre-check runs FIRST — before the filesystem
/// is touched at all — so an escape never reaches the existence check.
pub(crate) fn resolve_existing(base: &StdPath, rel: &str) -> Result<PathBuf, ResolveErr> {
    let rel_path = StdPath::new(rel);
    if rel_path.is_absolute() {
        return Err(ResolveErr::AbsoluteRel);
    }
    if lexically_escapes(rel_path) {
        return Err(ResolveErr::OutsideBoundary);
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
///
/// GH #79: every way the parent can be unusable comes back as
/// [`ResolveErr::WriteParent`] naming that parent, so the caller's text says
/// what to repair. The boundary check runs BEFORE the is-a-directory check —
/// an escape must keep reporting itself as an escape, not as a shape problem.
///
/// GH #107: the write path carried the same existence oracle as the read path
/// (a missing parent OUTSIDE the fence answered `io_error`), so it gets the
/// same lexical pre-check, ahead of every filesystem call.
pub(crate) fn resolve_write_parent(base: &StdPath, rel: &str) -> Result<PathBuf, ResolveErr> {
    let rel_path = StdPath::new(rel);
    if rel_path.is_absolute() {
        return Err(ResolveErr::AbsoluteRel);
    }
    if lexically_escapes(rel_path) {
        return Err(ResolveErr::OutsideBoundary);
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
    // The parent as the CALLER wrote it — an absolute canonical path would name
    // the boundary root back at an agent that never sees it.
    let named_parent = match rel_path.parent().map(|p| p.to_string_lossy().into_owned()) {
        Some(p) if !p.is_empty() => p,
        _ => ".".to_string(),
    };
    let canon_parent = parent.canonicalize().map_err(|e| ResolveErr::WriteParent {
        parent: named_parent.clone(),
        why: match e.kind() {
            std::io::ErrorKind::NotFound => WriteParentErr::Missing,
            std::io::ErrorKind::NotADirectory => WriteParentErr::NotADirectory,
            std::io::ErrorKind::PermissionDenied => WriteParentErr::Unreachable,
            _ => WriteParentErr::Other(e.to_string()),
        },
    })?;
    if !canon_parent.starts_with(&canon_base) {
        return Err(ResolveErr::OutsideBoundary);
    }
    // `canonicalize` succeeds on a plain file too, so the ENOTDIR would only
    // surface at the write itself, one stage too late to name the parent.
    if !canon_parent.is_dir() {
        return Err(ResolveErr::WriteParent {
            parent: named_parent,
            why: WriteParentErr::NotADirectory,
        });
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

    /// GH #107: the pre-check is pure arithmetic over components — no
    /// filesystem, no existence, no symlinks.
    #[test]
    fn lexical_escape_detection_is_pure_component_arithmetic() {
        for inside in [
            "a.txt",
            "./a.txt",
            "sub/a.txt",
            "sub/../a.txt",
            "sub/./deep/../a.txt",
            "a/b/c/../../d.txt",
        ] {
            assert!(
                !lexically_escapes(StdPath::new(inside)),
                "{inside} stays inside"
            );
        }
        for escape in [
            "../a.txt",
            "../",
            "sub/../../a.txt",
            "a/b/../../../c",
            "../../../../etc/passwd",
            // Climbing out and back in is refused too — deciding it would mean
            // resolving names outside the fence.
            "../boundary/a.txt",
        ] {
            assert!(lexically_escapes(StdPath::new(escape)), "{escape} escapes");
        }
    }

    /// GH #107: the fence answers the same whether the target outside exists
    /// or not — that identity IS the closed oracle.
    #[test]
    fn escapes_answer_outside_boundary_regardless_of_existence() {
        let outer = tempfile::TempDir::new().unwrap();
        let base = outer.path().join("boundary");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(outer.path().join("exists.txt"), b"x").unwrap();

        for rel in ["../exists.txt", "../missing.txt"] {
            let err = resolve_existing(&base, rel).unwrap_err();
            assert!(matches!(err, ResolveErr::OutsideBoundary), "{rel}: {err:?}");
            assert_eq!(resolve_error_code(&err), "path_outside_boundary");

            let err = resolve_write_parent(&base, rel).unwrap_err();
            assert!(matches!(err, ResolveErr::OutsideBoundary), "{rel}: {err:?}");
            assert_eq!(resolve_error_code(&err), "path_outside_boundary");
        }
        // A missing parent OUTSIDE is an escape, not the GH #79 io_error.
        let err = resolve_write_parent(&base, "../nodir/x.txt").unwrap_err();
        assert!(matches!(err, ResolveErr::OutsideBoundary), "{err:?}");
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

    /// GH #79: the variant AND its wording are the contract — the caller's only
    /// feedback channel is this string.
    #[test]
    fn resolve_write_parent_rejects_missing_parent() {
        let td = tempfile::TempDir::new().unwrap();
        let err = resolve_write_parent(td.path(), "subdir/new.txt").unwrap_err();
        assert!(matches!(
            err,
            ResolveErr::WriteParent {
                why: WriteParentErr::Missing,
                ..
            }
        ));
        assert_eq!(
            err.to_string(),
            "parent directory does not exist: subdir (write does not create directories)"
        );
        assert_eq!(resolve_error_code(&err), "io_error");
    }

    /// GH #79: a file in the parent position is a different repair, hence a
    /// different variant.
    #[test]
    fn resolve_write_parent_rejects_a_file_as_parent() {
        let td = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("notes"), b"x").unwrap();
        let err = resolve_write_parent(td.path(), "notes/new.txt").unwrap_err();
        assert_eq!(err.to_string(), "parent path is not a directory: notes");
        assert_eq!(resolve_error_code(&err), "io_error");
    }

    /// GH #79: a boundary escape stays a boundary escape — the shape check runs
    /// after it, never instead of it.
    #[test]
    fn resolve_write_parent_reports_escape_before_shape() {
        let td = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), td.path().join("out")).unwrap();
            let err = resolve_write_parent(td.path(), "out/new.txt").unwrap_err();
            assert!(matches!(err, ResolveErr::OutsideBoundary), "got {err:?}");
        }
        #[cfg(not(unix))]
        {
            let _ = (td, outside);
        }
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
