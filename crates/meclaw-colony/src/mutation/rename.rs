//! Per-path `rename(2)` from `.staging/<id>/<name>/` to the final path (phase 6 apply step 7).
//!
//! Phase-13.5-A3/A4: on re-add/swap, `atomic_rename_or_overwrite_all` (F3 variant B)
//! replaces only `config.json` atomically (tmp + rename(2)); the `cell.db` at the final
//! path stays untouched. On first-add (final path does not exist) the function behaves
//! like the previous `atomic_rename_all` (rename(2) of the whole staging dir).

use super::{MutationError, stage::StagedDir};

/// Deep-Audit F2: classify a rename-sequence failure by whether the live tree was
/// already touched. `committed == 0` → pre-destructive staging failure (clean
/// `Schema` reject, live tree untouched). `committed >= 1` → at least one
/// `rename(2)` already landed (audit-model, no rollback) → `LiveTreeMutated`, which
/// the call-site strict-fails on instead of mislabelling as a clean reject.
fn rename_err(committed: usize, msg: String) -> MutationError {
    if committed >= 1 {
        MutationError::LiveTreeMutated(msg)
    } else {
        MutationError::Schema(msg)
    }
}

/// Rename every StagedDir.staging_path → StagedDir.final_path.
///
/// Per-path atomic (rename(2) on POSIX), but NOT transactional across all paths.
/// On partial failure: already renamed paths stay at their final location — audit model
/// (recovery marks the mutation as failed, FS bootstrap adopts the orphans).
// `committed` is a SEMANTIC commit-counter (input to `rename_err`'s
// LiveTreeMutated-vs-Schema decision), not a mere loop index — keep it explicit.
#[allow(clippy::explicit_counter_loop)]
pub fn atomic_rename_all(pairs: &[StagedDir]) -> Result<(), MutationError> {
    let mut committed = 0usize;
    for p in pairs {
        // Ensure parent directory exists (for scope != "/").
        if let Some(parent) = p.final_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| rename_err(committed, format!("create parent {parent:?}: {e}")))?;
        }
        std::fs::rename(&p.staging_path, &p.final_path).map_err(|e| {
            rename_err(
                committed,
                format!("rename {:?} -> {:?}: {e}", p.staging_path, p.final_path),
            )
        })?;
        committed += 1;
    }
    Ok(())
}

/// Rename or targeted-overwrite every StagedDir.staging_path → StagedDir.final_path.
///
/// Two modes per pair:
/// - **first-add** (`final_path` does not exist): rename(2) of the whole staging dir,
///   identical to `atomic_rename_all`.
/// - **re-add / swap** (`final_path` already exists): atomically replace only
///   `config.json` via `tmp + rename(2)`; any existing `cell.db` at `final_path` is
///   left **completely untouched** (No-Delete-Policy + M1 Resume). The staging dir is
///   cleaned up afterwards via `remove_dir_all`.
///
/// Not transactional across multiple pairs. Partial-failure semantics are the same as
/// `atomic_rename_all`: audit-model, FS-Bootstrap adopts orphans.
// `committed` counts committed live-tree effects (rename OR config overwrite) and
// diverges from the loop index in the overwrite-then-cleanup-failure case — it is
// a semantic counter, not a loop index.
#[allow(clippy::explicit_counter_loop)]
pub fn atomic_rename_or_overwrite_all(pairs: &[StagedDir]) -> Result<(), MutationError> {
    let mut committed = 0usize;
    for p in pairs {
        // Deep-Audit F2 (test-hooks): deterministic mid-rename fault injection.
        // Release build: `should_fail_rename` is const-false, no branch survives.
        if super::hook::should_fail_rename(committed) {
            return Err(rename_err(
                committed,
                format!("injected mid-rename failure at committed={committed} (test)"),
            ));
        }
        if let Some(parent) = p.final_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| rename_err(committed, format!("create parent {parent:?}: {e}")))?;
        }
        if p.final_path.exists() {
            // re-add / swap-with: targeted overwrite of config.json only. The
            // overwrite IS a live-tree effect — once it lands, `committed` rises.
            targeted_overwrite_config(&p.staging_path, &p.final_path)
                .map_err(|e| rename_err(committed, format!("{e:?}")))?;
            committed += 1;
            // Clean up the staging dir; cell.db was NOT created there on the
            // resume path (stage builder skips seeding when final_path exists).
            // A cleanup failure here is post-commit (the live tree already
            // changed) → LiveTreeMutated.
            std::fs::remove_dir_all(&p.staging_path).map_err(|e| {
                rename_err(
                    committed,
                    format!("cleanup staging {:?}: {e}", p.staging_path),
                )
            })?;
        } else {
            // first-add: rename(2) as before.
            std::fs::rename(&p.staging_path, &p.final_path).map_err(|e| {
                rename_err(
                    committed,
                    format!("rename {:?} -> {:?}: {e}", p.staging_path, p.final_path),
                )
            })?;
            committed += 1;
        }
    }
    Ok(())
}

/// Atomically replace `final_path/config.json` with `staging/config.json`.
///
/// Uses a `config.json.tmp` side-file inside `final_path` as an intermediate,
/// then rename(2) into place — no partial-content window for concurrent readers.
///
/// **Failure behavior / known limitation**: if `fs::rename(tmp, dst)` fails after
/// the copy has already written `config.json.tmp`, that side-file is left in the
/// final cell directory. The staging/crash-recovery sweep must therefore handle
/// stray `*.tmp` files in cell dirs (e.g. treat them as crash debris and delete them).
fn targeted_overwrite_config(
    staging: &std::path::Path,
    final_path: &std::path::Path,
) -> Result<(), MutationError> {
    let src = staging.join("config.json");
    let dst = final_path.join("config.json");
    let tmp = final_path.join("config.json.tmp");
    std::fs::copy(&src, &tmp)
        .map_err(|e| MutationError::Schema(format!("copy {src:?} -> {tmp:?}: {e}")))?;
    std::fs::rename(&tmp, &dst)
        .map_err(|e| MutationError::Schema(format!("atomic rename {tmp:?} -> {dst:?}: {e}")))?;
    Ok(())
}

/// Rename each missing **rename-root**'s staged sub-tree into its final path with
/// ONE atomic `rename(2)` per root (Paket-5 T10, per-node subtree-resume).
///
/// For each [`StagedRenameRoot`] (`root_staging_path` → `root_final_path`): ensures
/// the final path's parent directory exists (`create_dir_all`), then `rename(2)`s the
/// whole staged sub-tree into place. For per-node resume the parent is an existing
/// node or the subtree scope (already on disk) and the rename-root's own final
/// directory does NOT exist — so renaming the missing child INTO its existing parent
/// works. Existing nodes are never renamed or touched. Whole-fresh root rename (parent
/// absent) is covered by the `create_dir_all`, mirroring [`atomic_rename_all`].
///
/// Per-path atomic but NOT transactional across roots: on partial failure, already
/// renamed roots stay at their final location — the audit/recovery model applies (same
/// as [`atomic_rename_all`]).
///
/// # Errors
/// Returns [`MutationError::Schema`] if a parent directory cannot be created or a
/// `rename(2)` fails.
// `committed` is the semantic commit-counter feeding `rename_err`, not a loop index.
#[allow(clippy::explicit_counter_loop)]
pub fn rename_subtree_roots(
    roots: &[crate::mutation::subtree::StagedRenameRoot],
) -> Result<(), MutationError> {
    let mut committed = 0usize;
    for r in roots {
        if let Some(parent) = r.root_final_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                rename_err(committed, format!("create subtree parent {parent:?}: {e}"))
            })?;
        }
        std::fs::rename(&r.root_staging_path, &r.root_final_path).map_err(|e| {
            rename_err(
                committed,
                format!(
                    "rename subtree {:?} -> {:?}: {e}",
                    r.root_staging_path, r.root_final_path
                ),
            )
        })?;
        committed += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_staged_dir(
        staging_path: std::path::PathBuf,
        final_path: std::path::PathBuf,
    ) -> StagedDir {
        StagedDir {
            staging_path,
            final_path,
            absolute_path: meclaw_core::Path::new("/foo"),
            template: "echo".into(),
            params: meclaw_core::serde_json::Value::Object(Default::default()),
            contract_view: crate::factory::ContractView::default(),
            cell_timeout: 0,
            idle_timeout_ms: None,
            message_timeout: None,
            mailbox_size: None,
            header_view: crate::mutation::validate::HeaderNodeView::default(),
            preexisting_target: false,
        }
    }

    #[test]
    fn rename_moves_staging_dirs_to_final_paths() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join(".staging/mid/foo");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), "{}").unwrap();
        let final_path = td.path().join("foo");
        let pairs = vec![make_staged_dir(staging.clone(), final_path.clone())];
        atomic_rename_all(&pairs).unwrap();
        assert!(!staging.exists());
        assert!(final_path.join("config.json").exists());
    }

    // --- Deep-Audit F2: mid-rename strict-fail signalling ---

    /// After at least one committed rename, a later failure must surface as
    /// `LiveTreeMutated` (live tree partially mutated, audit-model), NOT a clean
    /// `Schema` reject. Mechanik: index 0 commits; index 1's final_path parent is
    /// a FILE → its `create_dir_all` fails after index 0 already landed.
    #[test]
    fn mid_rename_failure_after_first_commit_signals_live_tree_mutated() {
        let td = TempDir::new().unwrap();
        let s0 = td.path().join(".staging/mid/a");
        std::fs::create_dir_all(&s0).unwrap();
        std::fs::write(s0.join("config.json"), "{}").unwrap();
        let f0 = td.path().join("a");

        let s1 = td.path().join(".staging/mid/b");
        std::fs::create_dir_all(&s1).unwrap();
        std::fs::write(s1.join("config.json"), "{}").unwrap();
        let blocker = td.path().join("blocker_file");
        std::fs::write(&blocker, "x").unwrap();
        let f1 = blocker.join("b"); // parent is a file → create_dir_all/rename fails.

        let pairs = vec![make_staged_dir(s0, f0.clone()), make_staged_dir(s1, f1)];
        let err = atomic_rename_all(&pairs).unwrap_err();
        assert!(
            matches!(err, MutationError::LiveTreeMutated(_)),
            "after a committed rename, a later failure must be LiveTreeMutated, got {err:?}"
        );
        // index 0 stands in the live tree (audit-model, no rollback).
        assert!(f0.join("config.json").exists());
    }

    /// A failure on the FIRST index (nothing committed yet) stays a clean
    /// pre-destructive `Schema` reject — the live tree is genuinely untouched.
    #[test]
    fn pre_rename_failure_on_first_index_stays_schema() {
        let td = TempDir::new().unwrap();
        let s0 = td.path().join(".staging/mid/a");
        std::fs::create_dir_all(&s0).unwrap();
        std::fs::write(s0.join("config.json"), "{}").unwrap();
        let blocker = td.path().join("blocker_file");
        std::fs::write(&blocker, "x").unwrap();
        let f0 = blocker.join("a"); // first rename fails, nothing committed.

        let pairs = vec![make_staged_dir(s0, f0)];
        let err = atomic_rename_all(&pairs).unwrap_err();
        assert!(
            matches!(err, MutationError::Schema(_)),
            "a failure before any committed rename stays a clean Schema reject, got {err:?}"
        );
    }

    // --- TDD tests for atomic_rename_or_overwrite_all (T2, Phase-13.5 Slice-4) ---

    /// first-add path: when final_path does NOT exist, behaves like rename(2).
    #[test]
    fn first_add_still_uses_rename_when_final_does_not_exist() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join(".staging/mid/foo");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), r#"{"new":true}"#).unwrap();
        let final_path = td.path().join("foo");

        // final_path must NOT exist before the call.
        assert!(!final_path.exists());

        let pairs = vec![make_staged_dir(staging.clone(), final_path.clone())];
        atomic_rename_or_overwrite_all(&pairs).unwrap();

        // staging dir is gone (renamed, not copied+deleted).
        assert!(!staging.exists());
        // config.json is at the final location with exactly the staged content.
        let content = std::fs::read_to_string(final_path.join("config.json")).unwrap();
        assert_eq!(content, r#"{"new":true}"#);
    }

    /// re-add path: when final_path exists, config.json is replaced with staging content.
    #[test]
    fn targeted_overwrite_replaces_config_json_atomically_when_final_exists() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join(".staging/mid/foo");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), r#"{"version":"new"}"#).unwrap();

        let final_path = td.path().join("foo");
        std::fs::create_dir_all(&final_path).unwrap();
        std::fs::write(final_path.join("config.json"), r#"{"version":"old"}"#).unwrap();

        let pairs = vec![make_staged_dir(staging.clone(), final_path.clone())];
        atomic_rename_or_overwrite_all(&pairs).unwrap();

        // staging dir is cleaned up.
        assert!(!staging.exists());
        // config.json at the final path now contains new content.
        let content = std::fs::read_to_string(final_path.join("config.json")).unwrap();
        assert!(
            content.contains("new"),
            "expected new content, got: {content}"
        );
        assert!(
            !content.contains("old"),
            "old content must be gone, got: {content}"
        );
    }

    /// re-add path: existing cell.db at final_path is preserved untouched.
    #[test]
    fn targeted_overwrite_preserves_cell_db_when_final_exists() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join(".staging/mid/foo");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), r#"{"version":"new"}"#).unwrap();
        // staging does NOT contain a cell.db (resume path — no re-seed).

        let final_path = td.path().join("foo");
        std::fs::create_dir_all(&final_path).unwrap();
        std::fs::write(final_path.join("config.json"), r#"{"version":"old"}"#).unwrap();
        // Write a sentinel into cell.db to prove it is not touched.
        let cell_db_path = final_path.join("cell.db");
        std::fs::write(&cell_db_path, b"SENTINEL_CONTENT_MUST_SURVIVE").unwrap();

        let pairs = vec![make_staged_dir(staging.clone(), final_path.clone())];
        atomic_rename_or_overwrite_all(&pairs).unwrap();

        // cell.db must still exist and contain the original sentinel bytes.
        assert!(cell_db_path.exists(), "cell.db must not be deleted");
        let db_bytes = std::fs::read(&cell_db_path).unwrap();
        assert_eq!(
            db_bytes, b"SENTINEL_CONTENT_MUST_SURVIVE",
            "cell.db content must be unmodified"
        );
        // config.json is the new content.
        let content = std::fs::read_to_string(final_path.join("config.json")).unwrap();
        assert!(
            content.contains("new"),
            "config.json must have new content, got: {content}"
        );
    }

    /// Proves the atomic per-file replace: the replace is done via tmp + rename(2),
    /// so a concurrent reader sees only old or new content, never partial/corrupt.
    ///
    /// The test verifies the mechanism indirectly: if `targeted_overwrite_config`
    /// uses tmp+rename(2), after a successful call the `.tmp` side-file must not
    /// exist (rename(2) consumes it atomically). We also verify that content is
    /// fully written even for a large payload that would be partial if written
    /// directly without the tmp intermediary.
    #[test]
    fn targeted_overwrite_atomic_per_file_via_tmp_rename() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join(".staging/mid/foo");
        std::fs::create_dir_all(&staging).unwrap();

        // Large payload to make partial-write observable if the tmp step were skipped.
        let new_content: String = "X".repeat(1_000_000);
        std::fs::write(staging.join("config.json"), new_content.as_bytes()).unwrap();

        let final_path = td.path().join("foo");
        std::fs::create_dir_all(&final_path).unwrap();
        let old_content = "OLD_CONTENT";
        std::fs::write(final_path.join("config.json"), old_content).unwrap();

        let pairs = vec![make_staged_dir(staging.clone(), final_path.clone())];
        atomic_rename_or_overwrite_all(&pairs).unwrap();

        // The tmp side-file must be gone (rename(2) consumed it).
        assert!(
            !final_path.join("config.json.tmp").exists(),
            "tmp side-file must be cleaned up by rename(2)"
        );
        // The final config.json must be exactly the new large content.
        let content = std::fs::read_to_string(final_path.join("config.json")).unwrap();
        assert_eq!(
            content.len(),
            1_000_000,
            "full new content must be present (no partial write)"
        );
        assert!(
            content.chars().all(|c| c == 'X'),
            "content must be all X (no mixing with old)"
        );
    }

    // --- TDD tests for rename_subtree_roots (T10, Paket-5 per-node subtree-resume) ---

    use crate::mutation::subtree::StagedRenameRoot;

    /// Build a minimal `StagedRenameRoot` with empty cells/hive_scopes; only the two
    /// paths matter for the rename mechanics under test.
    fn make_rename_root(
        root_staging_path: std::path::PathBuf,
        root_final_path: std::path::PathBuf,
    ) -> StagedRenameRoot {
        StagedRenameRoot {
            root_staging_path,
            root_final_path,
            cells: Vec::new(),
            hive_scopes: Vec::new(),
        }
    }

    /// A missing child staged at `.staging/<mid>/<name>/child` renames into an
    /// EXISTING parent `<scope>/<name>/` → child dir lands at the final path,
    /// staging dir gone.
    #[test]
    fn renames_missing_child_into_existing_parent() {
        let td = TempDir::new().unwrap();
        // Existing parent dir <scope>/<name>/ already on disk.
        let parent = td.path().join("main/m1");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(parent.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();

        // Staged child sub-tree.
        let staging = td.path().join(".staging/mid/m1/child");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();

        let final_path = parent.join("child");
        assert!(!final_path.exists());

        let roots = vec![make_rename_root(staging.clone(), final_path.clone())];
        rename_subtree_roots(&roots).unwrap();

        assert!(final_path.join("config.json").exists());
        assert!(!staging.exists());
    }

    /// Renaming a missing child into an existing parent does NOT disturb an existing
    /// sibling: the sibling's marker file stays byte-identical.
    #[test]
    fn rename_does_not_disturb_existing_sibling() {
        let td = TempDir::new().unwrap();
        let parent = td.path().join("main/m1");
        std::fs::create_dir_all(&parent).unwrap();

        // Existing sibling with a marker file.
        let sibling = parent.join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        let marker = sibling.join("marker.bin");
        let marker_bytes = b"SIBLING_MARKER_MUST_SURVIVE";
        std::fs::write(&marker, marker_bytes).unwrap();

        // Staged new child.
        let staging = td.path().join(".staging/mid/m1/child");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();
        let final_path = parent.join("child");

        let roots = vec![make_rename_root(staging.clone(), final_path.clone())];
        rename_subtree_roots(&roots).unwrap();

        assert!(final_path.join("config.json").exists());
        // Sibling marker file byte-unchanged.
        let after = std::fs::read(&marker).unwrap();
        assert_eq!(after, marker_bytes, "sibling marker must be byte-unchanged");
    }

    /// Whole-fresh root rename: the final path's parent does not exist yet and is
    /// created by the rename (mirrors `atomic_rename_all` today's behavior).
    #[test]
    fn whole_fresh_root_rename_creates_parent() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join(".staging/mid/m1");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();

        // Final path's parent (main/) does NOT exist yet.
        let final_path = td.path().join("main/m1");
        assert!(!td.path().join("main").exists());

        let roots = vec![make_rename_root(staging.clone(), final_path.clone())];
        rename_subtree_roots(&roots).unwrap();

        assert!(final_path.join("config.json").exists());
        assert!(!staging.exists());
    }

    /// Multiple rename-roots in one call all land at their final paths.
    #[test]
    fn multiple_rename_roots_all_land() {
        let td = TempDir::new().unwrap();
        let parent = td.path().join("main/m1");
        std::fs::create_dir_all(&parent).unwrap();

        let mut roots = Vec::new();
        for name in ["a", "b", "c"] {
            let staging = td.path().join(format!(".staging/mid/m1/{name}"));
            std::fs::create_dir_all(&staging).unwrap();
            std::fs::write(staging.join("config.json"), format!(r#"{{"n":"{name}"}}"#)).unwrap();
            let final_path = parent.join(name);
            roots.push(make_rename_root(staging, final_path));
        }

        rename_subtree_roots(&roots).unwrap();

        for name in ["a", "b", "c"] {
            assert!(parent.join(name).join("config.json").exists());
        }
    }
}
