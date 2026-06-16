//! `apply_mutation` — Phase-6 Apply-Sequenz Schritte 6 + 7 zusammengefasst:
//! stage → rename. Substitute + Validate sind im colony_task::handle_mutation
//! VOR dem durable in_flight-Insert (T13). Spawn + Edges kommen NACH dieser fn
//! (T18 + T21).
//!
//! Phase 11 T16: Signatur erweitert um `templates`, `env`, `ctx`. Ruft
//! `build_staging_tree_from_templates` statt des alten `build_staging_tree`.

use super::{
    MutationError,
    rename::{atomic_rename_or_overwrite_all, rename_subtree_roots},
    stage::{StagedDir, build_staging_tree_from_templates},
    subtree::StagedSubtreeMerge,
};
use meclaw_core::JsonValue;
use std::collections::HashMap;

/// Apply-Phase NACH validate + in_flight-Insert.
///
/// Macht NUR `build_staging_tree_from_templates` + `atomic_rename_all`. Returnt
/// die `StagedDir`s (single-cell) UND `StagedSubtree`s (Phase-13.5 a5-subtree
/// T8b-1), damit der Mutation-Arm anschließend Cell-Spawn + Subtree-Registrierung
/// machen kann (T18 — Apply-Sequenz Schritt 9).
///
/// Phase 11 T16: drei neue Parameter:
/// - `templates`: Templates-Registry-Snapshot (aus `colony_db.read_templates()`).
/// - `env`: Root-`.env`-Map (schon in `handle_mutation` geladen via `load_env`).
/// - `ctx`: Mutation-Ctx-Map (aus `payload.ctx`).
///
/// Paket-5 T12 (P9 per-node subtree resume): each [`StagedSubtreeMerge`] carries
/// the MISSING rename-roots staged into `.staging`; [`rename_subtree_roots`]
/// renames each one into its final path with ONE atomic `rename(2)` per root.
/// Existing nodes are never staged or renamed (F1: untouched). The whole-fresh
/// case (root absent) is a single rename-root and reproduces today's fresh-subtree
/// rename. A missing child renaming into its existing parent works (parent on
/// disk, child's own final dir absent).
pub fn apply_mutation(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    diff_substituted: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
) -> Result<(Vec<StagedDir>, Vec<StagedSubtreeMerge>), MutationError> {
    let (staged, subtrees) = build_staging_tree_from_templates(
        root,
        mutation_id,
        scope,
        diff_substituted,
        templates,
        env,
        ctx,
    )?;
    // A5b 2b (Phase-16 W1b): use the overwrite-superset rename. For a normal
    // add_nodes / swap-with the final path does NOT exist → identical to the old
    // `atomic_rename_all` (full rename(2)). For an `adopt` entry the final path
    // EXISTS (the on-disk node being adopted) → targeted config.json overwrite
    // that preserves the existing `cell.db` (No-Delete). The pre-2b flows never
    // produced a StagedDir with an existing final path, so this switch is
    // behaviour-neutral for them.
    // Deep-Audit F2: track whether ANY live-tree-affecting rename has committed,
    // across BOTH rename calls. A failure in the second call that reports a clean
    // `Schema` (its own internal counter is 0) must still be a `LiveTreeMutated` if
    // the first call already committed — otherwise the call-site would mislabel a
    // half-applied mutation as a clean reject.
    atomic_rename_or_overwrite_all(&staged)?;
    let mut live_touched = !staged.is_empty();
    for sub in &subtrees {
        rename_subtree_roots(&sub.rename_roots).map_err(|e| upgrade_if_live(e, live_touched))?;
        if !sub.rename_roots.is_empty() {
            live_touched = true;
        }
    }
    Ok((staged, subtrees))
}

/// Deep-Audit F2: promote a clean `Schema` rename failure to `LiveTreeMutated`
/// once an earlier rename call has already committed a live-tree change. A
/// `Schema` on a still-untouched tree stays a clean pre-destructive reject; an
/// already-`LiveTreeMutated` error is preserved unchanged.
fn upgrade_if_live(e: MutationError, live_touched: bool) -> MutationError {
    match e {
        MutationError::Schema(m) if live_touched => MutationError::LiveTreeMutated(m),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::{TemplateEntry, TemplatesRegistry};
    use meclaw_core::serde_json::json;
    use tempfile::TempDir;

    /// Helper: prepares a minimal single-cell template directory and returns a
    /// `TemplatesRegistry` with one entry pointing to it.
    fn setup_echo_template(td: &TempDir) -> TemplatesRegistry {
        let tpl = td.path().join("templates/echo");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"echo"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: "echo".into(),
            version: None,
            filesystem_path: tpl,
        }])
    }

    /// Deep-Audit F2: once any rename call has committed a live-tree change, a
    /// later call's clean `Schema` failure (its own internal `committed == 0`)
    /// must be upgraded to `LiveTreeMutated` — the live tree IS partially mutated
    /// across the call boundary. An untouched-tree `Schema` stays clean; an
    /// already-`LiveTreeMutated` error is preserved.
    #[test]
    fn upgrade_if_live_promotes_schema_once_tree_touched() {
        let promoted = upgrade_if_live(MutationError::Schema("x".into()), true);
        assert!(
            matches!(promoted, MutationError::LiveTreeMutated(_)),
            "Schema after a committed rename must become LiveTreeMutated, got {promoted:?}"
        );
        let clean = upgrade_if_live(MutationError::Schema("y".into()), false);
        assert!(
            matches!(clean, MutationError::Schema(_)),
            "Schema with an untouched tree stays a clean reject, got {clean:?}"
        );
        let preserved = upgrade_if_live(MutationError::LiveTreeMutated("z".into()), false);
        assert!(
            matches!(preserved, MutationError::LiveTreeMutated(_)),
            "an existing LiveTreeMutated is preserved, got {preserved:?}"
        );
    }

    #[test]
    fn apply_mutation_stages_and_renames_add_node() {
        let td = TempDir::new().unwrap();
        // Phase-11 T16: braucht ein Template-Verzeichnis + TemplatesRegistry-Stub.
        let templates = setup_echo_template(&td);
        let diff = json!({"add_nodes": [{"name": "n_apply", "template": "echo"}]});
        let (staged, _subtrees) = apply_mutation(
            td.path(),
            "mid-17",
            "/",
            &diff,
            &templates,
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(staged.len(), 1);
        // cell_type from template config.json, not raw "echo" anymore.
        assert_eq!(staged[0].template, "echo_type");
        // Final directory exists (renamed from staging).
        assert!(td.path().join("n_apply/config.json").exists());
        // Staging directory cleaned up by rename.
        assert!(!td.path().join(".staging/mid-17/n_apply").exists());
    }

    #[test]
    fn apply_mutation_empty_diff_returns_empty_vec() {
        let td = TempDir::new().unwrap();
        let diff = json!({});
        let (staged, subtrees) = apply_mutation(
            td.path(),
            "mid-17-empty",
            "/",
            &diff,
            &TemplatesRegistry::default(),
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        assert!(staged.is_empty());
        assert!(subtrees.is_empty());
    }
}
