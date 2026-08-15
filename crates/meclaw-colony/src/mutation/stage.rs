//! `.staging/<mutation_id>/` tree construction for add_nodes (phase 6 apply sequence step 6).
//!
//! One directory with a `config.json` per cell; ready for the atomic rename(2) in T16.
//! Phase 11 adds `build_staging_tree_from_templates` (template copy + patch + substitution + seed).

use super::MutationError;
use crate::templates::TemplatesRegistry;
use meclaw_core::JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

/// A cell built inside `.staging/<mutation_id>/`, ready for the rename(2).
///
/// `template` and `params` are needed for the cell spawn after the rename
/// (T18 — apply sequence step 9). `contract_view` carries the view extracted
/// from `config.json::contract` for the spawn call (T23).
#[derive(Debug, Clone)]
pub struct StagedDir {
    pub staging_path: PathBuf,
    pub final_path: PathBuf,
    pub absolute_path: meclaw_core::Path,
    pub template: String,
    pub params: JsonValue,
    pub contract_view: crate::factory::ContractView,
    /// Phase-13.5 Lifecycle-3b Task 7 (A2): `cell.timeout` from the substituted
    /// `config.json` (default `0`). Drives the mutation-spawn idle/cell-timeout
    /// mapping (`0` → idle-default kind, `>0` one-shot, `-1` persistent),
    /// replacing the former hardcode. Same semantics as `PlannedCell.cell_timeout`
    /// on the bootstrap path.
    pub cell_timeout: i64,
    /// Phase-13.5 Lifecycle-3b Task 7 (A2): optional per-cell `cell.idle_timeout_ms`
    /// override from the substituted `config.json`. `None` → substrate
    /// `DEFAULT_IDLE_TIMEOUT_MS`. Only consulted when `cell_timeout == 0`.
    pub idle_timeout_ms: Option<u64>,
    /// P3-B-plumb-2: optional per-cell `cell.message_timeout` (B-backstop) override
    /// from the substituted `config.json`. `None` → colony
    /// `message_timeout_default_ms`; resolved at the spawn call-site via
    /// `resolve_message_timeout`. Same propagation shape as `idle_timeout_ms`.
    pub message_timeout: Option<i64>,
    /// Paket-1 T20: optional per-cell `cell.mailbox_size` override from the
    /// substituted `config.json`. `None` → `colony.json mailbox_default_capacity`.
    /// Drives the mutation-spawn bounded-mailbox capacity, replacing the former
    /// `1000` hardcode. Same semantics as `cell.mailbox_size` on the bootstrap
    /// path. A `0` is rejected pre-destructively during staging (no capacity
    /// semantics).
    pub mailbox_size: Option<usize>,
    /// Hardening Slice 1 (Task 1.4): 14-B header projection of the SAME parsed
    /// `contract` block that `contract_view` is compiled from. Carried so the
    /// mutation-spawn arm can register the cell in the colony's
    /// `node_contracts` map (the live source of the post-state locality check).
    pub header_view: crate::mutation::validate::HeaderNodeView,
    /// A5b 2b (Phase-16 W1b): `true` iff the `final_path` directory existed on
    /// disk BEFORE this mutation (an `adopt` entry — the dir is the builder's
    /// pre-placed content, staged only as a config.json overwrite). A fresh
    /// `add_nodes`/swap entry is `false` (its `final_path` was just renamed in
    /// from staging). The spawn-reject sweep (`sweep_reject_residue`) MUST NOT
    /// `remove_dir_all` a pre-existing target — that would delete the builder's
    /// directory + its `cell.db` (No-Delete-Policy violation). Only the freshly
    /// renamed-in residue of a non-adopt reject is swept.
    pub preexisting_target: bool,
    /// GH #62: the template identity stamped into this instance's
    /// `config.json`, carried on so the mutation-spawn arm can index it in
    /// `colony.db`'s `registry` row. `None` for an `adopt` entry — adopting an
    /// existing directory is not a template instantiation and invents no origin.
    pub provenance: Option<crate::config::NodeProvenance>,
}

/// Unix seconds, the one time unit `colony.db` speaks.
///
/// GH #62 needs an instantiation timestamp in `meclaw-colony`, which
/// deliberately carries no date/time crate (see `blob/disk.rs`), so the stamp
/// is seconds since the epoch — the same unit as every `created_at` column.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Phase-11 Slice 11-F T15: build staging tree from templates.
///
/// For each `add_nodes` entry in `diff`, resolves the template from the
/// `TemplatesRegistry`, copies the template dir (excluding `template.json`),
/// patches `config.json` with substitution + UUID-v7 `cell.id`, and seeds
/// `cell.db` from `seed/*.jsonl` if present.
///
/// Phase-13.5 a5-subtree T8b-1: an `add_nodes` entry whose template is a SUBTREE
/// (a multi-cell template — `parse_subtree(...).cells.len() > 1`, i.e. it has
/// nested cell directories / hive markers) is staged via
/// [`crate::mutation::subtree::stage_subtree`] into the second return vec instead
/// of the single-cell path. Single-cell templates keep their existing behaviour
/// verbatim. The subtree-reject (`reject_if_subtree_template`) is therefore gone
/// from THIS function — subtrees are now first-class on the add_nodes path.
pub fn build_staging_tree_from_templates(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    diff: &JsonValue,
    templates: &TemplatesRegistry,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
) -> Result<
    (
        Vec<StagedDir>,
        Vec<crate::mutation::subtree::StagedSubtreeMerge>,
    ),
    MutationError,
> {
    let staging_root = root.join(".staging").join(mutation_id);
    std::fs::create_dir_all(&staging_root)
        .map_err(|e| MutationError::Schema(format!("create staging root: {e}")))?;
    let mut out = Vec::new();
    let mut subtrees = Vec::new();
    let adds = diff.get("add_nodes").and_then(|v| v.as_array());
    for n in adds.into_iter().flatten() {
        let name = n
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("add_nodes[].name missing".into()))?;
        // A5b 2b (Phase-16 W1b, Ruling 2026-06-12): an `adopt` entry instantiates
        // from the EXISTING on-disk node (source = the final path), not a
        // template — the template-instantiation pipeline MINUS template
        // lookup/copy. `colony::handle_mutation` Step 1a already validated this
        // pre-destructively (final path exists, path unregistered, on-disk
        // `cell.type`/`contract.version` match the `adopt` expectation). We stage
        // ONLY `config.json`; the existing `cell.db` and any other files stay in
        // place and are preserved by the config-overwrite rename
        // (`atomic_rename_or_overwrite_all`, final_path exists). The shared
        // `patch_and_substitute_config` mints a FRESH `cell.id` (A5b ID-Vergabe),
        // runs `${VAR}`/`${ctx.*}`/`${uuid7:*}` substitution, and validates the
        // contract — same as a template instantiation.
        if n.get("adopt").is_some() {
            let final_path = crate::path_truth::resolve_cell_dir(root, scope, name);
            let staging_path = staging_root.join(name);
            std::fs::create_dir_all(&staging_path).map_err(|e| {
                MutationError::Schema(format!("create adopt staging {staging_path:?}: {e}"))
            })?;
            std::fs::copy(
                final_path.join("config.json"),
                staging_path.join("config.json"),
            )
            .map_err(|e| {
                MutationError::Schema(format!(
                    "adopt: read existing config.json at {final_path:?}: {e}"
                ))
            })?;
            let (
                cell_type,
                params,
                contract_view,
                cell_timeout,
                idle_timeout_ms,
                message_timeout,
                mailbox_size,
                header_view,
            ) = patch_and_substitute_config(&staging_path, env, ctx, n, None)?;
            let absolute_path = super::resolve_scoped_path(scope, name);
            out.push(StagedDir {
                staging_path,
                final_path,
                absolute_path,
                template: cell_type,
                params,
                contract_view,
                cell_timeout,
                idle_timeout_ms,
                message_timeout,
                mailbox_size,
                header_view,
                preexisting_target: true,
                // GH #62: an adopt names no template — the node's origin is
                // whatever its own config.json already said, carried through
                // by the copy above.
                provenance: None,
            });
            continue;
        }
        let tpl_ref = n
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("add_nodes[].template missing".into()))?;
        let tpl = templates
            .resolve(tpl_ref)
            .map_err(|_| MutationError::TemplateMissing(tpl_ref.into()))?;
        // Paket-5 T12 (P9): SUBTREE-dispatch — checked BEFORE the single-cell
        // existence-skip below, because a partially-existing subtree has an
        // existing root but missing children (the existence-skip would wrongly
        // drop the whole node). A template with nested cell directories / hive
        // markers (`cells.len() > 1`) is merge-staged via `stage_subtree_merge`:
        // ONLY the missing rename-roots are staged (fresh UUID per cell,
        // internal-edge remap, hive-scope collection); existing nodes are left
        // untouched (F1). The whole-fresh case (root absent) yields exactly one
        // rename-root equal to today's fresh-subtree staging.
        if crate::mutation::subtree::parse_subtree(&tpl.filesystem_path)?
            .cells
            .len()
            > 1
        {
            let staged_subtree = crate::mutation::subtree::stage_subtree_merge(
                root,
                mutation_id,
                scope,
                name,
                &tpl.filesystem_path,
                env,
                ctx,
                // GH #62: one stamp for the whole subtree instance — every
                // nested cell names the subtree template, which is the unit an
                // update addresses.
                Some(&provenance_of(tpl)),
            )?;
            subtrees.push(staged_subtree);
            continue;
        }
        // Phase-13.5 Lifecycle-3a (A1): if the final path already exists, this
        // single-cell `add_nodes` entry is a Reconnect/Resume (overview Z.170-180),
        // not an instantiation. Skip the entire staging build for this node — NO
        // template copy, NO `config.json` rewrite, NO seed. The live directory
        // (config.json + cell.db) stays byte-identical; the node produces no
        // `StagedDir` (no spawn output). The existing registry entry keeps its
        // `cell_id`, and `cell.db` resumes (M1) at next wake.
        // Spec overview Z.331: anchor the logical path under the single root cell
        // directory (root-cell-dir name stripped from logical paths). Shared with
        // the colony Resume-detect site via `path_truth` so the on-disk final
        // path matches the bootstrap-instantiated layout (`{root}/<root-cell>/…`).
        let final_path = crate::path_truth::resolve_cell_dir(root, scope, name);
        if final_path.exists() {
            continue;
        }
        let staging_path = staging_root.join(name);
        copy_dir_recursive(&tpl.filesystem_path, &staging_path)?;
        // GH #62: the RESOLVED template identity (not the reference string —
        // `echo` resolves to the highest version, and the instance has to name
        // the version it actually got).
        let provenance = provenance_of(tpl);
        // add_nodes: fresh `cell.id` minted inside patch_and_substitute_config.
        let (
            cell_type,
            params,
            contract_view,
            cell_timeout,
            idle_timeout_ms,
            message_timeout,
            mailbox_size,
            header_view,
        ) = patch_and_substitute_config(&staging_path, env, ctx, n, Some(&provenance))?;
        seed_cell_db_if_present(&staging_path)?;
        let absolute_path = super::resolve_scoped_path(scope, name);
        out.push(StagedDir {
            staging_path,
            final_path,
            absolute_path,
            template: cell_type,
            params,
            contract_view,
            cell_timeout,
            idle_timeout_ms,
            message_timeout,
            mailbox_size,
            header_view,
            preexisting_target: false,
            provenance: Some(provenance),
        });
    }
    // Paket-2 T4 (b1): graph-swap with-side template instantiation. Each
    // `swap_nodes[].with` that carries a `template` instantiates a FRESH t3 cell
    // through the SAME single-cell staging machinery as add_nodes (own fresh
    // `cell.id` via `None`, seed applied, atomic rename + registration downstream).
    // The with-side `name` is the new node's name; `with.params` maps onto the
    // shared `override_params` contract. The existing-node form (`{name}` only,
    // no `template`) needs NO staging — it references an already-live cell.
    let swaps = diff.get("swap_nodes").and_then(|v| v.as_array());
    for s in swaps.into_iter().flatten() {
        let with = match s.get("with").and_then(|v| v.as_object()) {
            Some(w) => w,
            None => continue, // validate covers a missing/malformed `with`.
        };
        let Some(tpl_ref) = with.get("template").and_then(|v| v.as_str()) else {
            continue; // existing-node form (`{name}` only) → no staging.
        };
        let name = with
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.name missing".into()))?;
        let final_path = crate::path_truth::resolve_cell_dir(root, scope, name);
        let tpl = templates
            .resolve(tpl_ref)
            .map_err(|_| MutationError::TemplateMissing(tpl_ref.into()))?;
        let staging_path = staging_root.join(name);
        copy_dir_recursive(&tpl.filesystem_path, &staging_path)?;
        // GH #62: the with-side is a fresh instantiation → same provenance stamp
        // as add_nodes.
        let provenance = provenance_of(tpl);
        // Map `with.params` onto the substitution helper's `override_params`
        // contract so a swap can override copied template params just like
        // add_nodes. Fresh `cell.id` (`None`) — t3 is a brand-new instance.
        let override_node = JsonValue::Object({
            let mut m = meclaw_core::serde_json::Map::new();
            if let Some(p) = with.get("params") {
                m.insert("override_params".into(), p.clone());
            }
            m
        });
        let (
            cell_type,
            params,
            contract_view,
            cell_timeout,
            idle_timeout_ms,
            message_timeout,
            mailbox_size,
            header_view,
        ) = patch_and_substitute_config(
            &staging_path,
            env,
            ctx,
            &override_node,
            Some(&provenance),
        )?;
        seed_cell_db_if_present(&staging_path)?;
        let absolute_path = super::resolve_scoped_path(scope, name);
        out.push(StagedDir {
            staging_path,
            final_path,
            absolute_path,
            template: cell_type,
            params,
            contract_view,
            cell_timeout,
            idle_timeout_ms,
            message_timeout,
            mailbox_size,
            header_view,
            preexisting_target: false,
            provenance: Some(provenance),
        });
    }
    Ok((out, subtrees))
}

/// GH #62: the provenance stamp for an instantiation from `tpl`, timestamped now.
///
/// Records the RESOLVED identity of the template entry, not the reference the
/// mutation wrote: `template: "echo"` resolves to the highest known version, and
/// an instance that only remembered `"echo"` could not tell an app-store update
/// which version it is behind.
pub(crate) fn provenance_of(
    tpl: &crate::templates::TemplateEntry,
) -> crate::config::NodeProvenance {
    crate::config::NodeProvenance {
        template: tpl.name.clone(),
        template_version: tpl.version.clone(),
        instantiated_at: unix_now(),
    }
}

/// Read the persisted `cell.id` from a live cell directory's `config.json`.
///
/// Used on the Reconnect/Resume path (Phase-13.5 Lifecycle-3a): when
/// `add_nodes` targets an existing path, the cell keeps its original `cell_id`
/// — it is never re-minted. Returns `None` if the directory has no readable
/// `config.json`, the JSON is malformed, or `cell.id` is absent / not a valid
/// UUID. Production callers already hold the stable `cell_id` in the live
/// registry entry (Task-2 identity overlay); this helper is the file-system
/// source of truth for tests and any future re-hydration path.
pub fn read_existing_cell_id(cell_dir: &std::path::Path) -> Option<meclaw_core::Uuid> {
    let raw = std::fs::read_to_string(cell_dir.join("config.json")).ok()?;
    let cfg: JsonValue = meclaw_core::serde_json::from_str(&raw).ok()?;
    let id_str = cfg.get("cell")?.get("id")?.as_str()?;
    id_str.parse::<meclaw_core::Uuid>().ok()
}

/// Reject subtree-templates: any sub-directory that is NOT `seed/` AND contains
/// a `config.json` is a nested cell — not supported in Phase 11 (overview Z.1097,
/// deferred to Phase 14/15).
///
/// `pub(crate)` so the swap-validation path in `validate.rs` can reuse the same
/// tripwire (paket-2 T1 Rule A6).
pub(crate) fn reject_if_subtree_template(
    tpl_dir: &std::path::Path,
    tpl_ref: &str,
) -> Result<(), MutationError> {
    let mut nested: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(tpl_dir)
        .map_err(|e| MutationError::Schema(format!("read template dir: {e}")))?
    {
        let entry = entry.map_err(|e| MutationError::Schema(format!("read entry: {e}")))?;
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let fname = entry.file_name();
        if fname == "seed" {
            continue;
        }
        if p.join("config.json").is_file() {
            nested.push(fname.to_string_lossy().into_owned());
        }
    }
    if !nested.is_empty() {
        return Err(MutationError::Schema(format!(
            "subtree templates not supported in phase 11; template '{tpl_ref}' has \
             nested cell directories: {nested:?}. See overview Z.1097 — subtree \
             support is deferred to phase 14/15."
        )));
    }
    Ok(())
}

/// Recursively copy `src` to `dst`, skipping `template.json` at the top level.
///
/// `pub(crate)` so the subtree-staging path
/// ([`crate::mutation::subtree::stage_subtree`]) reuses the exact same copy
/// semantics (template.json stripped) instead of duplicating them.
pub(crate) fn copy_dir_recursive(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<(), MutationError> {
    std::fs::create_dir_all(dst)
        .map_err(|e| MutationError::Schema(format!("create staging cell dir: {e}")))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| MutationError::Schema(format!("read template dir: {e}")))?
    {
        let entry = entry.map_err(|e| MutationError::Schema(format!("read entry: {e}")))?;
        let path = entry.path();
        let fname = entry.file_name();
        if fname == "template.json" {
            continue; // template-meta never goes into the instance.
        }
        let target = dst.join(&fname);
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|e| MutationError::Schema(format!("copy {fname:?}: {e}")))?;
        }
    }
    Ok(())
}

/// Read + substitute + UUID-patch `config.json` inside the staging dir.
///
/// Two views of the same config (GH #20): the DISK view resolves the instance
/// class only (`${ctx.<key>}`, `${uuid7:label}`) and is what gets written, so
/// environment placeholders -- secrets included -- stay tokens on the filesystem.
/// The RUNTIME view applies the env pass in memory on top of it, exactly as the
/// boot path does, and every returned value is derived from THAT. A cell born
/// from a mutation therefore sees the same params as after a reboot, while the
/// file on disk carries none of them.
///
/// Returns `(cell_type, params, contract_view, cell_timeout, idle_timeout_ms,
/// message_timeout, mailbox_size, header_view)` for Factory-Lookup + the timeout
/// mapping after rename. `contract_view` is extracted from the post-substitution
/// `config.json` (T23); `cell_timeout` / `idle_timeout_ms` from `cell.timeout` /
/// `cell.idle_timeout_ms` (A2, Task 7); `message_timeout` from
/// `cell.message_timeout` (P3-B-plumb-2); `mailbox_size` from `cell.mailbox_size`
/// (paket-1 T20); `header_view` is the 14-B projection of the SAME parsed
/// `contract` block (Hardening Slice 1, Task 1.4).
///
/// Every call mints a FRESH UUID v7 as `cell.id` — this helper is only used
/// for new instantiations (add_nodes, swap_nodes with-side, subtree nodes),
/// never for in-place identity preservation.
///
/// GH #62: `provenance` is the template identity of THIS instantiation. `Some`
/// stamps `cell.provenance` into the written file (the disk view, next to the
/// freshly minted `cell.id`); `None` writes nothing and leaves whatever the
/// source config carried — the `adopt` case, which re-instantiates an existing
/// on-disk node and therefore has no template to name.
///
/// `pub(crate)` so the subtree-staging path
/// ([`crate::mutation::subtree::stage_subtree`]) reuses the identical
/// config-patch + substitution + UUID-mint logic per nested cell.
#[allow(clippy::type_complexity)]
pub(crate) fn patch_and_substitute_config(
    staging_path: &std::path::Path,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    add_node: &JsonValue,
    provenance: Option<&crate::config::NodeProvenance>,
) -> Result<
    (
        String,
        JsonValue,
        crate::factory::ContractView,
        i64,
        Option<u64>,
        Option<i64>,
        Option<usize>,
        crate::mutation::validate::HeaderNodeView,
    ),
    MutationError,
> {
    let cfg_path = staging_path.join("config.json");
    let raw = std::fs::read_to_string(&cfg_path)
        .map_err(|e| MutationError::Schema(format!("read config.json: {e}")))?;
    let cfg: JsonValue = meclaw_core::serde_json::from_str(&raw)
        .map_err(|e| MutationError::Schema(format!("parse config.json: {e}")))?;
    // GH #20 -- the DISK view: instance-class substitution only (`${ctx.<key>}`,
    // `${uuid7:label}`). Environment-class tokens (`${VAR}`, `${VAR:-default}`)
    // survive literally into the instance config and bind late, at every read.
    // A secret referenced by a template therefore never reaches the filesystem.
    let mut cfg = super::substitute::substitute_instance_only(&cfg, ctx)?;
    // override_params merge (last-write-wins over copied values). The diff-side
    // values arrive already instance-substituted and env-literal
    // (`substitute_mutation_diff`), so this merge stays inside the disk view.
    if let Some(over) = add_node.get("override_params").and_then(|v| v.as_object())
        && let Some(params) = cfg.get_mut("params").and_then(|v| v.as_object_mut())
    {
        for (k, v) in over {
            params.insert(k.clone(), v.clone());
        }
    }
    // cell.id: always mint a fresh UUID v7 — every staged cell is a new
    // instance (add_nodes, swap_nodes with-side, subtree nodes).
    let cell_block = cfg
        .get_mut("cell")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| MutationError::Schema("config.json: cell-block missing".into()))?;
    let cell_id = meclaw_core::Uuid::now_v7();
    cell_block.insert(
        "id".into(),
        meclaw_core::serde_json::Value::String(cell_id.to_string()),
    );
    // GH #20 -- the RUNTIME view: the same late binding the boot path applies
    // (`plan_bootstrap` → `substitute_env_only`), in memory, over the disk view.
    // Everything below is derived from it, so a cell spawned by a mutation sees
    // exactly what it would see after a reboot -- and a `${VAR}` that cannot
    // resolve rejects the mutation here, pre-destructively, as before.
    let runtime = super::substitute::substitute_env_only(&cfg, env)?;
    let cell = runtime
        .get("cell")
        .and_then(|v| v.as_object())
        .ok_or_else(|| MutationError::Schema("config.json: cell-block missing".into()))?;
    let cell_type = cell
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MutationError::Schema("config.json: cell.type missing".into()))?
        .to_string();
    // A2 (Task 7): timeout mapping inputs, mirroring the bootstrap `CellHeader`
    // read (`cfg.cell.timeout` / `cfg.cell.idle_timeout_ms`). Absent `timeout`
    // → 0 (idle-default kind); absent `idle_timeout_ms` → None.
    let cell_timeout = cell.get("timeout").and_then(|v| v.as_i64()).unwrap_or(0);
    let idle_timeout_ms = cell.get("idle_timeout_ms").and_then(|v| v.as_u64());
    // P3-B-plumb-2: per-cell B-backstop override. Absent → None (resolved against
    // `colony.json message_timeout_default_ms` at the spawn call-site).
    let message_timeout = cell.get("message_timeout").and_then(|v| v.as_i64());
    // Paket-1 T20: per-cell mailbox capacity override. Absent → None (uses
    // `colony.json mailbox_default_capacity` at the spawn call-site).
    let mailbox_size = cell
        .get("mailbox_size")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    // Pre-destructive 0-reject: a `0` capacity has no bounded-mailbox semantics
    // (a `mpsc::channel(0)` would have zero buffer). This runs during staging,
    // BEFORE the atomic rename — the `.staging` dir is discarded, no live change.
    if mailbox_size == Some(0) {
        return Err(MutationError::Schema(
            "cell.mailbox_size must be >= 1 (0 has no capacity semantics)".into(),
        ));
    }
    let mut params = runtime
        .get("params")
        .cloned()
        .unwrap_or(JsonValue::Object(Default::default()));
    // GH #85 -- the default-deny cut, and it is PROSPECTIVE. `provenance` is
    // `Some` exactly on the paths that instantiate a node from a template, so
    // this fills the hole in the node being BORN and touches nothing that is
    // already on disk. Same shape as the GH #20 secret cut: instantiation
    // starts writing something new, existing instances keep running unchanged.
    //
    // Written into both views because the block is a literal: the runtime view
    // is what the cell about to spawn sees, and the disk view is what the
    // operator can read back and edit.
    if provenance.is_some() && sandbox_enforcing(&cell_type) && params.get("sandbox").is_none() {
        let block = default_sandbox_block();
        if let Some(obj) = params.as_object_mut() {
            obj.insert("sandbox".into(), block.clone());
        }
        cfg.as_object_mut()
            .and_then(|c| {
                c.entry("params")
                    .or_insert_with(|| JsonValue::Object(Default::default()))
                    .as_object_mut()
            })
            .map(|p| p.insert("sandbox".into(), block));
    }
    // Extract the contract block from the post-substitution config (T23) and
    // compile it via the shared `compile_contract_view` helper (paket-7 B4).
    // A malformed `contract.emits` schema yields `MutationError::Schema` HERE —
    // during staging, BEFORE the atomic rename — so the `.staging` dir is
    // discarded and the live filesystem is unchanged (handle_mutation step 1/2
    // discipline: reject pre-destructively, analog the mailbox_size:0 reject above).
    let contract_block: crate::config::ContractBlock = match runtime.get("contract") {
        Some(c) => meclaw_core::serde_json::from_value(c.clone())
            .map_err(|e| MutationError::Schema(format!("config.json: invalid contract: {e}")))?,
        None => crate::config::ContractBlock::default(),
    };
    // Hardening Slice 4 (Task 4.2): presence-enforce the builder-mandatory
    // contract keys (`version`/`settings`/`consumes`) on every staged NON-hive
    // config — pre-destructively during staging, BEFORE the atomic rename, so
    // the `.staging` dir is discarded and the live tree stays untouched. Hive
    // markers are exempt (scope markers, their contract block is not
    // evaluated). This single site covers add_nodes, the swap_nodes with-side
    // AND the subtree staging paths (`stage_subtree`/`stage_rename_root`),
    // which all parse their per-cell config right here. The staging path in
    // the error message carries the mutation id + node name (builder feedback).
    if cell_type != "hive"
        && let Err(reason) = crate::config::validate_contract_presence(&contract_block)
    {
        return Err(MutationError::ContractIncomplete(format!(
            "{}: {reason}",
            cfg_path.display()
        )));
    }
    let contract_view = crate::bootstrap::compile_contract_view(&contract_block)
        .map_err(|reason| MutationError::Schema(format!("contract.emits: {reason}")))?;
    // Hardening Slice 1 (Task 1.4): 14-B projection of the SAME parsed block —
    // the mutation-spawn arm registers it in the colony's `node_contracts` map.
    let header_view = crate::mutation::validate::header_view_from_contract(&contract_block);
    // GH #62 -- the provenance stamp, written into the DISK view only, and
    // deliberately AFTER the runtime view was derived: the template identity is
    // minted by the colony, carries no placeholders, and must not be walked by
    // a substitution pass that could choke on a `${`-shaped template name. Same
    // write, same once-only guarantee as the `cell.id` mint above.
    if let Some(prov) = provenance
        && let Some(cell_block) = cfg.get_mut("cell").and_then(|v| v.as_object_mut())
    {
        cell_block.insert(
            "provenance".into(),
            meclaw_core::serde_json::to_value(prov)
                .map_err(|e| MutationError::Schema(format!("serialize provenance: {e}")))?,
        );
    }
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .map_err(|e| MutationError::Schema(format!("write patched config.json: {e}")))?;
    Ok((
        cell_type,
        params,
        contract_view,
        cell_timeout,
        idle_timeout_ms,
        message_timeout,
        mailbox_size,
        header_view,
    ))
}

/// The cell types that read `params.sandbox` and enforce it (GH #85).
///
/// A closed list rather than "every cell type", because a block nobody reads
/// is noise in a `store`'s params and meaningless on a `hive` scope marker.
/// The coupling is deliberate and one-directional: `meclaw-colony` cannot see
/// `meclaw-cells`, so a fourth cell type that grows a sandbox has to be added
/// here as well, and `docs/cell-types.md` says so at each of the three.
const SANDBOX_ENFORCING_CELL_TYPES: [&str; 3] = ["bash", "code", "harness"];

/// Whether `cell_type` is one of the cell types that enforce a sandbox.
fn sandbox_enforcing(cell_type: &str) -> bool {
    SANDBOX_ENFORCING_CELL_TYPES.contains(&cell_type)
}

/// The profile a template-sourced cell gets when it declares none (GH #85).
///
/// Deliberately path-free. A default naming the cell's own directory would
/// bake an absolute host path into the instantiated `config.json`, and an
/// exported tree would carry a boundary that points at a directory on somebody
/// else's machine -- the same class of failure GH #20 was opened about. What
/// stays reachable is the runtime set (`/usr`, `/lib`, `/etc`, `/proc`, the
/// usual device nodes), which is what an interpreter needs to start at all;
/// everything else the template has to declare, and `trust: "trusted"` remains
/// the explicit escape hatch.
fn default_sandbox_block() -> JsonValue {
    meclaw_core::serde_json::json!({
        "trust": "restricted",
        "network": "deny",
        "filesystem": {"runtime": true}
    })
}

/// If a `seed/` directory exists in the staging path, create a fresh `cell.db`
/// and populate it from each `seed/<table>.jsonl` file.
///
/// Reconciliation with Phase-9 store (Spec overview Z.1215): store sees
/// `OpenStatus::Resumed` at spawn-time → skips its own `load_seed_if_present`
/// → no double-seed. store's `apply_schema_ddl` is idempotent (CREATE TABLE IF
/// NOT EXISTS) → no DDL conflict.
pub(crate) fn seed_cell_db_if_present(staging_path: &std::path::Path) -> Result<(), MutationError> {
    let seed_dir = staging_path.join("seed");
    if !seed_dir.is_dir() {
        return Ok(());
    }
    let cell_db_path = staging_path.join("cell.db");
    let conn = rusqlite::Connection::open(&cell_db_path)
        .map_err(|e| MutationError::Schema(format!("open seed cell.db: {e}")))?;
    crate::persist::setup_cell_db(&conn)
        .map_err(|e| MutationError::Schema(format!("setup_cell_db: {e}")))?;
    for entry in std::fs::read_dir(&seed_dir)
        .map_err(|e| MutationError::Schema(format!("read seed dir: {e}")))?
    {
        let entry = entry.map_err(|e| MutationError::Schema(format!("seed entry: {e}")))?;
        let path = entry.path();
        if path.extension().map(|x| x == "jsonl").unwrap_or(false) {
            apply_seed_jsonl(&conn, &path)?;
        }
    }
    Ok(())
}

/// Generic JSONL-Seed-Loader (Spec overview Z.1206–1216).
///
/// Line 1: `{"schema": {"<col>": "<type>", ...}}` (text | int | json).
/// Line 2+: `{"<col>": <value>, ...}` data rows.
/// Table name is derived from the file stem (`items.jsonl` → `items`).
fn apply_seed_jsonl(
    conn: &rusqlite::Connection,
    path: &std::path::Path,
) -> Result<(), MutationError> {
    let table = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            MutationError::Schema(format!("seed file stem missing: {}", path.display()))
        })?
        .to_string();
    let content = std::fs::read_to_string(path)
        .map_err(|e| MutationError::Schema(format!("read seed {}: {e}", path.display())))?;
    let mut lines = content.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| MutationError::Schema(format!("empty seed {}", path.display())))?;
    let header: JsonValue = meclaw_core::serde_json::from_str(header_line)
        .map_err(|e| MutationError::Schema(format!("seed {} header parse: {e}", path.display())))?;
    let schema_obj = header
        .get("schema")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            MutationError::Schema(format!(
                "seed {} header: missing schema object",
                path.display()
            ))
        })?;
    // CREATE TABLE IF NOT EXISTS from schema line. Type-mapping per Spec.
    let cols: Vec<(String, String)> = schema_obj
        .iter()
        .map(|(k, v)| {
            let sql_type = match v.as_str().unwrap_or("text") {
                "int" => "INTEGER",
                "json" => "TEXT",
                _ => "TEXT", // text + fallback.
            };
            (k.clone(), sql_type.to_string())
        })
        .collect();
    let col_defs = cols
        .iter()
        .map(|(c, t)| format!("\"{c}\" {t}"))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS \"{table}\" ({col_defs})"),
        [],
    )
    .map_err(|e| MutationError::Schema(format!("seed {} CREATE TABLE: {e}", path.display())))?;
    // INSERT rows with param-bind.
    let col_names: Vec<&String> = cols.iter().map(|(c, _)| c).collect();
    let placeholders = col_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let col_list = col_names
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let stmt = format!("INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})");
    for (idx, raw) in lines.enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let row: JsonValue = meclaw_core::serde_json::from_str(raw).map_err(|e| {
            MutationError::Schema(format!("seed {} line {}: {e}", path.display(), idx + 2))
        })?;
        let row_obj = row.as_object().ok_or_else(|| {
            MutationError::Schema(format!(
                "seed {} line {}: not an object",
                path.display(),
                idx + 2
            ))
        })?;
        let params: Vec<rusqlite::types::Value> = col_names
            .iter()
            .map(|c| json_to_sql_value(row_obj.get(c.as_str())))
            .collect();
        let bind: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        conn.execute(&stmt, bind.as_slice()).map_err(|e| {
            MutationError::Schema(format!(
                "seed {} line {} INSERT: {e}",
                path.display(),
                idx + 2
            ))
        })?;
    }
    Ok(())
}

/// Local duplicate of `meclaw_cells::store::ops::json_to_sql_value`.
///
/// Layering-Invariante: meclaw-colony MUST NOT import meclaw-cells.
/// Phase-16-Cleanup-Backlog: move to `meclaw-core` when a third consumer appears.
fn json_to_sql_value(v: Option<&JsonValue>) -> rusqlite::types::Value {
    use rusqlite::types::Value as SqlV;
    match v {
        None | Some(JsonValue::Null) => SqlV::Null,
        Some(JsonValue::Bool(b)) => SqlV::Integer(if *b { 1 } else { 0 }),
        Some(JsonValue::Number(n)) => n
            .as_i64()
            .map(SqlV::Integer)
            .or_else(|| n.as_f64().map(SqlV::Real))
            .unwrap_or(SqlV::Null),
        Some(JsonValue::String(s)) => SqlV::Text(s.clone()),
        Some(v @ (JsonValue::Array(_) | JsonValue::Object(_))) => {
            SqlV::Text(meclaw_core::serde_json::to_string(v).unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::{TemplateEntry, TemplatesRegistry};
    use meclaw_core::serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_registry(td: &TempDir, dir: &str, name: &str) -> (PathBuf, TemplatesRegistry) {
        let tpl = td.path().join("templates").join(dir);
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
        let registry = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: name.into(),
            version: None,
            filesystem_path: tpl.clone(),
        }]);
        (tpl, registry)
    }

    /// Like [`make_registry`], but the template declares a version — the shape
    /// GH #62 has to record, because an app-store update addresses
    /// `<name>@<version>`, not a bare name.
    fn make_versioned_registry(
        td: &TempDir,
        dir: &str,
        name: &str,
        version: &str,
    ) -> (PathBuf, TemplatesRegistry) {
        let tpl = td.path().join("templates").join(dir);
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(
            tpl.join("template.json"),
            format!(r#"{{"name":"{name}","version":"{version}"}}"#),
        )
        .unwrap();
        let registry = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: name.into(),
            version: Some(version.into()),
            filesystem_path: tpl.clone(),
        }]);
        (tpl, registry)
    }

    // ── GH #62: provenance ──────────────────────────────────────────────

    /// GH #62: an instantiated node names the template it came from.
    ///
    /// The template identity goes into the DISK view (`cell.provenance` of the
    /// written `config.json`), because the instance is a detached copy: an
    /// exported, backed-up or moved tree has to carry its own origin. The same
    /// value is handed on in `StagedDir.provenance`, so the mutation-spawn arm
    /// can index it in `colony.db`.
    #[test]
    fn build_staging_tree_records_template_provenance_in_the_instance_config() {
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_versioned_registry(&td, "echo@1.2.3", "echo", "1.2.3");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let before = crate::mutation::stage::unix_now();
        let diff = json!({"add_nodes": [{"name":"e1","template":"echo@1.2.3"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-prov",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        let cfg: JsonValue = meclaw_core::serde_json::from_str(
            &std::fs::read_to_string(staged[0].staging_path.join("config.json")).unwrap(),
        )
        .unwrap();
        let prov = &cfg["cell"]["provenance"];
        assert_eq!(
            prov["template"], "echo",
            "the instance records the RESOLVED template name: {cfg}"
        );
        assert_eq!(
            prov["template_version"], "1.2.3",
            "the instance records the RESOLVED template version: {cfg}"
        );
        let at = prov["instantiated_at"]
            .as_i64()
            .unwrap_or_else(|| panic!("instantiated_at must be unix seconds: {cfg}"));
        assert!(
            at >= before,
            "instantiated_at must be the time of THIS instantiation, got {at} < {before}"
        );
        let carried = staged[0]
            .provenance
            .as_ref()
            .expect("StagedDir carries the provenance for the registry index");
        assert_eq!(carried.template, "echo");
        assert_eq!(carried.template_version.as_deref(), Some("1.2.3"));
        assert_eq!(carried.instantiated_at, at);
    }

    /// An unversioned template records its name and NO version key — absence
    /// means "this template declares no version", which is a different fact
    /// from "version unknown".
    #[test]
    fn build_staging_tree_provenance_omits_the_version_of_an_unversioned_template() {
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_registry(&td, "echo", "echo");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let diff = json!({"add_nodes": [{"name":"e1","template":"echo"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-prov-nover",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        let cfg: JsonValue = meclaw_core::serde_json::from_str(
            &std::fs::read_to_string(staged[0].staging_path.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["cell"]["provenance"]["template"], "echo");
        assert!(
            cfg["cell"]["provenance"].get("template_version").is_none(),
            "an unversioned template writes no template_version key: {cfg}"
        );
        assert_eq!(
            staged[0].provenance.as_ref().unwrap().template_version,
            None
        );
    }

    /// The swap with-side is an instantiation too — same machinery, same stamp.
    #[test]
    fn swap_with_side_records_template_provenance() {
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_versioned_registry(&td, "echo@2.0.0", "echo", "2.0.0");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let diff =
            json!({"swap_nodes": [{"name":"old","with":{"name":"new","template":"echo@2.0.0"}}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-prov-swap",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        let cfg: JsonValue = meclaw_core::serde_json::from_str(
            &std::fs::read_to_string(staged[0].staging_path.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["cell"]["provenance"]["template"], "echo");
        assert_eq!(cfg["cell"]["provenance"]["template_version"], "2.0.0");
    }

    /// An `adopt` entry has no template — it re-instantiates an existing
    /// on-disk node. It must NOT invent a provenance, and it must not destroy
    /// the one the adopted node already carried: the node's origin does not
    /// change by being adopted.
    #[test]
    fn adopt_neither_invents_nor_destroys_provenance() {
        let td = TempDir::new().unwrap();
        let root_cell = td.path().join("main");
        std::fs::create_dir_all(&root_cell).unwrap();
        std::fs::write(
            root_cell.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{}}"#,
        )
        .unwrap();
        let existing = root_cell.join("kept");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(
            existing.join("config.json"),
            r#"{"cell":{"type":"echo_type","provenance":{"template":"older","template_version":"0.9.0","instantiated_at":1000}},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let (_tpl, registry) = make_registry(&td, "echo", "echo");
        let diff = json!({"add_nodes": [{"name":"kept","adopt":{"cell_type":"echo_type"}}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-prov-adopt",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        let cfg: JsonValue = meclaw_core::serde_json::from_str(
            &std::fs::read_to_string(staged[0].staging_path.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            cfg["cell"]["provenance"]["template"], "older",
            "an adopt carries the node's existing provenance through verbatim: {cfg}"
        );
        assert_eq!(cfg["cell"]["provenance"]["instantiated_at"], 1000);
        assert!(
            staged[0].provenance.is_none(),
            "an adopt is not a template instantiation — it stamps nothing new"
        );
    }

    #[test]
    fn build_staging_tree_carries_cell_timeout_and_idle_override() {
        // Phase-13.5 Lifecycle-3b Task 7 (A2): `StagedDir` carries
        // `cell_timeout` + `idle_timeout_ms`, read from the substituted
        // `config.json` (`cell.timeout` / `cell.idle_timeout_ms`), replacing the
        // mutation-spawn hardcode. A persistent timer (`cell.timeout = -1`) with
        // a per-cell idle override must flow through verbatim.
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_registry(&td, "timer", "timer");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"timer","timeout":-1,"idle_timeout_ms":250,"message_timeout":5000},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let diff = json!({"add_nodes": [{"name":"t1","template":"timer"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-a2",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!(
            staged[0].cell_timeout, -1,
            "cell.timeout == -1 (persistent) must be carried into StagedDir"
        );
        assert_eq!(
            staged[0].idle_timeout_ms,
            Some(250),
            "cell.idle_timeout_ms override must be carried into StagedDir"
        );
        assert_eq!(
            staged[0].message_timeout,
            Some(5000),
            "cell.message_timeout override must be carried into StagedDir"
        );
    }

    #[test]
    fn build_staging_tree_cell_timeout_defaults_to_zero_and_idle_none() {
        // Absent `cell.timeout` → 0 (idle-default kind); absent
        // `cell.idle_timeout_ms` → None (uses substrate DEFAULT_IDLE_TIMEOUT_MS).
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_registry(&td, "echo", "echo");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let diff = json!({"add_nodes": [{"name":"e1","template":"echo"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-a2b",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            staged[0].cell_timeout, 0,
            "absent cell.timeout defaults to 0"
        );
        assert_eq!(
            staged[0].idle_timeout_ms, None,
            "absent cell.idle_timeout_ms defaults to None"
        );
        assert_eq!(
            staged[0].message_timeout, None,
            "absent cell.message_timeout defaults to None"
        );
    }

    #[test]
    fn build_staging_tree_copies_template_dir_and_patches_uuid() {
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_registry(&td, "echo", "echo");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{"greeting":"${GREET}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let env: HashMap<String, String> = [("GREET".to_string(), "hi".to_string())].into();
        let ctx: HashMap<String, String> = HashMap::new();
        let diff = json!({"add_nodes": [{"name":"e1","template":"echo"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-1",
            "/",
            &diff,
            &registry,
            &env,
            &ctx,
        )
        .unwrap();
        assert_eq!(staged.len(), 1);
        let cfg_raw = std::fs::read_to_string(staged[0].staging_path.join("config.json")).unwrap();
        // GH #20: the env token stays on disk, the value does not.
        assert!(
            cfg_raw.contains("${GREET}"),
            "the environment token survives instantiation literally: {cfg_raw}"
        );
        assert!(
            !cfg_raw.contains("\"hi\""),
            "the env VALUE is never written to disk: {cfg_raw}"
        );
        assert!(cfg_raw.contains("\"id\""), "cell.id (UUID v7) must be set");
        // The runtime view handed to the factory IS resolved (boot-equivalent).
        assert_eq!(
            staged[0].params["greeting"], "hi",
            "the spawned cell sees the resolved value"
        );
    }

    #[test]
    fn build_staging_tree_rejects_missing_env_var() {
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_registry(&td, "echo", "echo");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{"key":"${MISSING}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let diff = json!({"add_nodes": [{"name":"e1","template":"echo"}]});
        let err = build_staging_tree_from_templates(
            td.path(),
            "mid-2",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(
            matches!(err, MutationError::EnvVarMissing(_)),
            "expected EnvVarMissing, got {err:?}"
        );
    }

    #[test]
    fn build_staging_tree_seeds_cell_db_with_table_and_row_readback() {
        let td = TempDir::new().unwrap();
        let tpl = td.path().join("templates/seeded");
        std::fs::create_dir_all(tpl.join("seed")).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"seeded"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"store"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        std::fs::write(
            tpl.join("seed/items.jsonl"),
            "{\"schema\":{\"id\":\"int\",\"name\":\"text\"}}\n\
             {\"id\":1,\"name\":\"alice\"}\n\
             {\"id\":2,\"name\":\"bob\"}\n",
        )
        .unwrap();
        let registry = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: "seeded".into(),
            version: None,
            filesystem_path: tpl.clone(),
        }]);
        let diff = json!({"add_nodes": [{"name":"s1","template":"seeded"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-3",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        let cell_db = staged[0].staging_path.join("cell.db");
        assert!(cell_db.exists(), "cell.db must exist");
        let conn = rusqlite::Connection::open(&cell_db).unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 2, "seeded rows must be readable");
        let names: Vec<String> = conn
            .prepare("SELECT name FROM items ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(names, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn build_staging_tree_skips_existing_final_path_resume() {
        // Phase-13.5 Lifecycle-3a (A1): an `add_nodes` entry whose final_path
        // already exists is a Reconnect/Resume — NO staging build, the live
        // directory (config.json + cell.db) stays byte-identical, and the node
        // does NOT appear in the returned StagedDir vec (no spawn output).
        let td = TempDir::new().unwrap();
        let (tpl, registry) = make_registry(&td, "echo", "echo");
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        // Spec overview Z.331: the single top-level cell dir is the root cell
        // (logical `/`), its name stripped. A live cell at logical `/e1` lives on
        // disk under `{root}/<root-cell>/e1`. Set up that root-cell-dir so the
        // unified `resolve_cell_dir` anchors correctly.
        let root_cell = td.path().join("main");
        std::fs::create_dir_all(&root_cell).unwrap();
        std::fs::write(
            root_cell.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{}}"#,
        )
        .unwrap();
        // Pre-create the LIVE cell directory at the final path (scope "/", name
        // "e1") → on-disk `{root}/main/e1`.
        let live_dir = root_cell.join("e1");
        std::fs::create_dir_all(&live_dir).unwrap();
        let live_config = r#"{"cell":{"id":"0190aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee","type":"echo_type"},"params":{"k":"v"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
        std::fs::write(live_dir.join("config.json"), live_config).unwrap();
        // A cell.db with arbitrary bytes — must survive untouched.
        std::fs::write(live_dir.join("cell.db"), b"existing-db-bytes").unwrap();

        let diff = json!({"add_nodes": [{"name":"e1","template":"echo"}]});
        let (staged, _subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-resume",
            "/",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();

        // No staging output for the existing node.
        assert!(
            staged.is_empty(),
            "resume node must NOT produce a StagedDir (no spawn output)"
        );
        // config.json byte-identical (A1: no config rewrite).
        let after = std::fs::read_to_string(live_dir.join("config.json")).unwrap();
        assert_eq!(after, live_config, "config.json must be byte-identical");
        // cell.db untouched (Resume = M1, no re-seed).
        let db_after = std::fs::read(live_dir.join("cell.db")).unwrap();
        assert_eq!(db_after, b"existing-db-bytes", "cell.db must be untouched");
        // No staging directory was created for the resume node.
        assert!(
            !td.path().join(".staging/mid-resume/e1").exists(),
            "no staging dir for a resume node"
        );
    }

    #[test]
    fn read_existing_cell_id_reads_live_config_id() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("cell");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            r#"{"cell":{"id":"0190aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee","type":"x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let id = read_existing_cell_id(&dir);
        assert_eq!(
            id,
            Some(
                "0190aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee"
                    .parse::<meclaw_core::Uuid>()
                    .unwrap()
            )
        );
    }

    #[test]
    fn read_existing_cell_id_none_when_no_config() {
        let td = TempDir::new().unwrap();
        assert_eq!(read_existing_cell_id(td.path()), None);
    }

    #[test]
    fn build_staging_tree_dispatches_subtree_template() {
        // Phase-13.5 a5-subtree T8b-1: a multi-cell (SUBTREE) template on the
        // add_nodes path is NO LONGER rejected — it is staged into the second
        // return vec via `stage_subtree`. The single-cell vec stays empty.
        let td = TempDir::new().unwrap();
        // A single root-cell dir so `resolve_cell_dir` anchors final paths.
        let root_cell = td.path().join("main");
        std::fs::create_dir_all(&root_cell).unwrap();
        std::fs::write(root_cell.join("config.json"), b"{}").unwrap();

        let tpl = td.path().join("templates/multi");
        std::fs::create_dir_all(tpl.join("sub_cell")).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"multi"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        )
        .unwrap();
        std::fs::write(
            tpl.join("sub_cell/config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let registry = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: "multi".into(),
            version: None,
            filesystem_path: tpl.clone(),
        }]);
        let diff = json!({"add_nodes": [{"name":"m1","template":"multi"}]});
        let (single, subtrees) = build_staging_tree_from_templates(
            td.path(),
            "mid-4",
            "/main",
            &diff,
            &registry,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert!(
            single.is_empty(),
            "subtree template must NOT produce a single-cell StagedDir"
        );
        assert_eq!(subtrees.len(), 1, "subtree template must be staged");
        // Paket-5 T12: merge-staging. Whole-fresh root → exactly one rename-root
        // carrying the entire sub-tree. The root hive is a scope marker; `sub_cell`
        // is the only spawnable cell. No existing nodes/hives on a fresh path.
        assert_eq!(subtrees[0].rename_roots.len(), 1, "one fresh rename-root");
        assert!(subtrees[0].existing.is_empty());
        assert!(subtrees[0].existing_hives.is_empty());
        let rr = &subtrees[0].rename_roots[0];
        assert_eq!(rr.cells.len(), 1);
        assert_eq!(rr.cells[0].absolute_path.as_str(), "/main/m1/sub_cell");
        assert_eq!(rr.hive_scopes.len(), 1, "root hive scope");
        assert_eq!(rr.hive_scopes[0].as_str(), "/main/m1");
    }
}
