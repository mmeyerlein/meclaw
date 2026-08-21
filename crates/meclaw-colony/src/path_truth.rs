//! Single source of truth for the `logical_path → fs_path` mapping shared by
//! the bootstrap and the mutation paths.
//!
//! Spec § Filesystem-Layout (overview Z.331): the single top-level cell
//! directory under `{root}` is the root cell with logical path `/`; its
//! directory name is **stripped** from every logical path. `bootstrap.rs::
//! fs_to_meclaw_path` already encodes this direction (fs → logical). This
//! module provides the **inverse** (logical → fs), so the mutation Resume-detect
//! and staging-final-path computation anchor logical paths under the same root
//! cell directory bootstrap walked from.
//!
//! Before unification the mutation path computed `root.join(scope).join(name)`,
//! dropping the root-cell-dir segment (the F3 bug): a Resume `add_nodes` at a
//! bootstrap-instantiated path checked `{root}/<name>` instead of the real
//! `{root}/<root-cell>/<name>`, missed the existing directory, and rejected the
//! Resume as a naming-collision.

use std::path::{Path, PathBuf};

/// Resolve the on-disk cell directory for a logical (`scope`, `name`) pair,
/// anchored under the single root cell directory (spec § Filesystem-Layout,
/// overview Z.331 — the root cell dir name is stripped from logical paths, so
/// logical `/` maps back to that directory).
///
/// `scope` is the logical scope (`/`, `/sub`, …); `name` is the new node's
/// short name. Logical `/q` → fs `{root}/<root-cell>/q`; logical `/sub/q` → fs
/// `{root}/<root-cell>/sub/q`.
///
/// Three cases for the root-cell-dir lookup:
/// - **Exactly 1 candidate** — normal path; anchor under that directory.
/// - **0 candidates** — flat-layout fixture (no bootstrapped root cell); anchor
///   directly under `root` (pre-unification fallback, correct for fresh or
///   test-only layouts).
/// - **>1 candidates** — broken topology that cannot occur once bootstrap
///   succeeded; logs a `tracing::error!` with the root path and candidate count
///   (defense-in-depth, never silent), then falls back to `root` so the
///   function remains infallible.
pub fn resolve_cell_dir(root: &Path, scope: &str, name: &str) -> PathBuf {
    let anchor = find_root_cell_dir(root);
    let scope_clean = scope.trim_start_matches('/').trim_end_matches('/');
    if scope_clean.is_empty() {
        anchor.join(name)
    } else {
        anchor.join(scope_clean).join(name)
    }
}

/// Collect top-level subdirectories of `root` that contain a `config.json`
/// and are not on the top-level blacklist, then return the appropriate anchor:
/// - 1 candidate  → that directory
/// - 0 candidates → `root` itself (flat-layout fallback)
/// - >1 candidates → `tracing::error!` + `root` fallback (broken topology)
///
/// `pub(crate)` since GH #324: the surface resolver needs the anchor itself, not
/// a path built under it. It used to spell the anchor `root.join("main")` — a
/// literal for something boot never required to be called `main`, so a colony
/// rooted anywhere else served 404 for every surface. One resolver, one answer.
pub(crate) fn find_root_cell_dir(root: &Path) -> PathBuf {
    let Ok(entries) = std::fs::read_dir(root) else {
        return root.to_path_buf();
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if crate::bootstrap::is_blacklisted_top_level(&p) {
            continue;
        }
        if p.join("config.json").is_file() {
            candidates.push(p);
        }
    }
    match candidates.len() {
        1 => candidates.into_iter().next().unwrap(),
        0 => root.to_path_buf(),
        n => {
            tracing::error!(
                root = %root.display(),
                candidate_count = n,
                "resolve_cell_dir: >1 root cell directories found — broken topology; \
                 falling back to root (defense-in-depth)"
            );
            root.to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_root_cell(root: &Path, dir: &str) {
        let p = root.join(dir);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("config.json"), b"{}").unwrap();
    }

    #[test]
    fn anchors_scope_root_under_root_cell_dir() {
        // Spec overview Z.331: logical `/q` maps to `{root}/<root-cell>/q`.
        let td = TempDir::new().unwrap();
        write_root_cell(td.path(), "main");
        assert_eq!(
            resolve_cell_dir(td.path(), "/", "q"),
            td.path().join("main").join("q")
        );
    }

    #[test]
    fn anchors_sub_scope_under_root_cell_dir() {
        let td = TempDir::new().unwrap();
        write_root_cell(td.path(), "main");
        assert_eq!(
            resolve_cell_dir(td.path(), "/sub", "q"),
            td.path().join("main").join("sub").join("q")
        );
    }

    #[test]
    fn falls_back_to_root_when_no_root_cell_dir() {
        // Flat-layout fixture (no bootstrapped root cell) → anchor at `{root}`.
        let td = TempDir::new().unwrap();
        assert_eq!(resolve_cell_dir(td.path(), "/", "q"), td.path().join("q"));
    }
}
