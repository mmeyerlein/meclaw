//! Pure parser for SUBTREE template directories.
//!
//! A SUBTREE template may contain multiple cell directories (the root cell plus nested
//! cells), some of which may be hive markers. This module reads such a directory tree
//! into a [`SubtreeTemplate`] without performing any filesystem writes, UUID generation,
//! or staging — it is read-only parsing.
//!
//! Graph information is extracted exclusively from the `params.graph.edges` field of
//! each hive cell's `config.json`. The `template.json` file at the root is intentionally
//! ignored for graph purposes (it is pure metadata).

use crate::config::{EdgeSpec as ConfigEdgeSpec, HiveParams};
use crate::mutation::MutationError;
use meclaw_core::{JsonValue, Path, serde_json};
use std::collections::HashMap;
use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────────────────────
// Public data types
// ──────────────────────────────────────────────────────────────────────────────

/// A single cell directory found inside a SUBTREE template.
#[derive(Debug, Clone)]
pub struct CellNode {
    /// Path of the cell directory relative to the template root.
    ///
    /// The template root itself uses `""` (empty string) as its relative path.
    /// Nested cells use their relative path from the root, e.g. `"inner_a"`.
    pub rel_path: String,
    /// Parsed contents of the cell's `config.json`.
    pub config: serde_json::Value,
}

/// A directed edge declared inside a hive cell's `params.graph.edges`.
///
/// The `from`/`to` values are kept **exactly as written** in the hive's
/// `config.json` (e.g. `"./inner_a"`). Resolution to absolute paths is a
/// later task (T5) and is intentionally NOT performed here.
#[derive(Debug, Clone)]
pub struct EdgeSpec {
    /// Source cell path, relative as written in the template (e.g. `"./inner_a"`).
    pub from: String,
    /// Destination cell path, relative as written in the template (e.g. `"./inner_b"`).
    pub to: String,
    /// Optional CEL boolean expression controlling when the edge is taken.
    pub condition: Option<String>,
    /// Optional header modifier spec for this edge, kept as raw JSON.
    pub modifier: Option<serde_json::Value>,
}

/// Parsed representation of a SUBTREE template directory.
///
/// Contains every cell found in the subtree, the subset of those that are
/// hive markers, and all intra-subtree edges declared inside hive configs.
#[derive(Debug, Clone)]
pub struct SubtreeTemplate {
    /// Every cell directory in the subtree (root + all nested), indexed by their
    /// path relative to the template root. Root itself has `rel_path == ""`.
    pub cells: Vec<CellNode>,
    /// Relative paths (same convention as `CellNode::rel_path`) of those cells
    /// whose `config.json` has `cell.type == "hive"`.
    pub hives: Vec<String>,
    /// Subtree-internal edges collected from every hive's `params.graph.edges`.
    /// `from`/`to` are kept relative as written — no path resolution is applied.
    pub edges: Vec<EdgeSpec>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Parser
// ──────────────────────────────────────────────────────────────────────────────

/// Recursively walk `template_root` and parse every cell directory into a
/// [`SubtreeTemplate`].
///
/// # Rules
/// - A directory that contains a `config.json` file is a cell node, **except**
///   if the directory is named `seed` (seed directories are data, not cells).
/// - The template root itself is always included as a cell node with `rel_path == ""`.
/// - A cell whose `config.cell.type == "hive"` is additionally listed in
///   [`SubtreeTemplate::hives`].
/// - For each hive, its `params.graph.edges` are extracted and stored in
///   [`SubtreeTemplate::edges`] with `from`/`to` left as written.
/// - `template.json` is ignored entirely (it carries metadata only).
///
/// # Errors
/// Returns [`MutationError::Schema`] if any `config.json` cannot be parsed as JSON.
pub fn parse_subtree(template_root: &std::path::Path) -> Result<SubtreeTemplate, MutationError> {
    let mut cells: Vec<CellNode> = Vec::new();
    let mut hives: Vec<String> = Vec::new();
    let mut edges: Vec<EdgeSpec> = Vec::new();

    collect_cells(
        template_root,
        template_root,
        &mut cells,
        &mut hives,
        &mut edges,
    )?;

    Ok(SubtreeTemplate {
        cells,
        hives,
        edges,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Recursively collect cell nodes starting at `dir`.
///
/// `root` is the original template root, used to compute relative paths.
fn collect_cells(
    root: &std::path::Path,
    dir: &std::path::Path,
    cells: &mut Vec<CellNode>,
    hives: &mut Vec<String>,
    edges: &mut Vec<EdgeSpec>,
) -> Result<(), MutationError> {
    let config_path = dir.join("config.json");

    // If this directory contains a config.json AND is not a seed/ directory,
    // treat it as a cell node.
    let is_seed = dir.file_name().map(|n| n == "seed").unwrap_or(false);

    if config_path.is_file() && !is_seed {
        let rel_path = relative_path(root, dir);
        let raw = std::fs::read_to_string(&config_path)
            .map_err(|e| MutationError::Schema(format!("read {}: {e}", config_path.display())))?;
        let config: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| MutationError::Schema(format!("parse {}: {e}", config_path.display())))?;

        // Determine if this is a hive cell.
        let cell_type = config
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        if cell_type == "hive" {
            hives.push(rel_path.clone());

            // Extract edges from params.graph.edges (reusing HiveParams).
            let params_value = config
                .get("params")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let hp: HiveParams = serde_json::from_value(params_value).map_err(|e| {
                MutationError::Schema(format!("parse params in {}: {e}", config_path.display()))
            })?;

            for spec in hp.graph.edges {
                edges.push(edge_spec_from_config(spec));
            }
        }

        cells.push(CellNode { rel_path, config });
    }

    // Recurse into subdirectories (skip seed/ regardless of whether it had config.json).
    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| MutationError::Schema(format!("read_dir {}: {e}", dir.display())))?;

    let mut sub_dirs: Vec<std::path::PathBuf> = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| {
            MutationError::Schema(format!("read_dir entry in {}: {e}", dir.display()))
        })?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().map(|n| n == "seed").unwrap_or(false);
            if !name {
                sub_dirs.push(path);
            }
        }
    }

    // Sort for deterministic order.
    sub_dirs.sort();

    for sub in sub_dirs {
        collect_cells(root, &sub, cells, hives, edges)?;
    }

    Ok(())
}

/// Convert a [`ConfigEdgeSpec`] (from `crate::config`) into our [`EdgeSpec`].
fn edge_spec_from_config(spec: ConfigEdgeSpec) -> EdgeSpec {
    EdgeSpec {
        from: spec.from,
        to: spec.to,
        condition: spec.condition,
        modifier: spec
            .modifier
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null)),
    }
}

/// Compute the path of `target` relative to `root`, using `""` for the root itself.
fn relative_path(root: &std::path::Path, target: &std::path::Path) -> String {
    if target == root {
        return String::new();
    }
    target
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| target.to_string_lossy().into_owned())
}

// ──────────────────────────────────────────────────────────────────────────────
// Per-node subtree classification (missing vs existing) — Paket-5 T6
// ──────────────────────────────────────────────────────────────────────────────

/// A subtree cell node that already exists on disk and will therefore be
/// *resumed* (left untouched) rather than instantiated.
///
/// Captured by [`classify_subtree_nodes`] for every template cell whose resolved
/// final filesystem directory already exists. The on-disk `cell.type` is read
/// from the existing directory's `config.json` so a later F2 type-compatibility
/// check (deferred task) can compare it against the template's `cell.type`
/// without re-reading the filesystem.
#[derive(Debug, Clone)]
pub struct ResolvedExistingNode {
    /// Absolute logical path of the node, e.g. `/main/m1/inner_a`.
    pub absolute_path: Path,
    /// On-disk final directory the node already occupies.
    pub final_path: PathBuf,
    /// `cell.type` read from the EXISTING on-disk `config.json`, or `None` if the
    /// config could not be read/parsed or carried no `cell.type` string.
    pub on_disk_cell_type: Option<String>,
    /// `cell.type` declared in the TEMPLATE node at this node's rel-path, or
    /// `None` if the template config carried no `cell.type` string. The F2
    /// resume-type-compat check (Paket-5 T12) compares this against
    /// [`Self::on_disk_cell_type`] pre-destructively.
    pub template_cell_type: Option<String>,
}

/// A subtree hive marker that already exists on disk (its scope directory is
/// present), so no `InsertHiveScope` is needed for it.
#[derive(Debug, Clone)]
pub struct ResolvedExistingHive {
    /// Absolute logical path of the hive marker, e.g. `/main/m1/sub_h`.
    pub absolute_path: Path,
    /// On-disk final directory the hive marker already occupies.
    pub final_path: PathBuf,
}

/// Partition of a SUBTREE template's nodes against the LIVE filesystem.
///
/// Produced by [`classify_subtree_nodes`]. Splits both spawnable cells and hive
/// markers into those still *missing* on disk (must be instantiated) and those
/// already *present* (will be resumed / left untouched). This is the pure
/// foundation for per-node subtree resume; it performs no writes.
#[derive(Debug, Clone)]
pub struct SubtreePartition {
    /// Spawnable (NON-hive) cells whose final fs directory does NOT exist →
    /// instantiate (copy + patch + seed). Carries the parsed template
    /// [`CellNode`] verbatim.
    pub missing: Vec<CellNode>,
    /// Spawnable (NON-hive) cells whose final fs directory already exists →
    /// resume (left untouched).
    pub existing: Vec<ResolvedExistingNode>,
    /// Hive markers whose final fs directory does NOT exist → need an
    /// `InsertHiveScope`. Carries the parsed template [`CellNode`] verbatim.
    pub missing_hives: Vec<CellNode>,
    /// Hive markers whose final fs directory already exists → already scoped.
    pub existing_hives: Vec<ResolvedExistingHive>,
}

/// Partition a SUBTREE template's cells and hive markers into *missing* (absent
/// on disk → instantiate) vs *existing* (present on disk → resume), based on the
/// filesystem existence of each node's resolved final directory.
///
/// PURE: no filesystem writes, no UUID minting, no staging. It reuses
/// [`parse_subtree`] for the template structure and resolves each node's final
/// fs path the SAME way [`stage_subtree`] does ([`final_path_for`] →
/// [`crate::path_truth::resolve_cell_dir`]), so classification agrees with
/// staging on every path.
///
/// `root` is the colony root; `scope`/`name` give the subtree's logical anchor
/// (`resolve_scoped_path` → e.g. `/main/m1`); `template_root` is the on-disk
/// template directory (read-only).
///
/// For an EXISTING cell, [`ResolvedExistingNode::on_disk_cell_type`] is read
/// from the cell's EXISTING on-disk `config.json` (`cell.type`), mirroring how
/// [`parse_subtree`] reads `cell.type` from a template config — `None` if that
/// config is unreadable/unparsable or carries no `cell.type` string.
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed.
pub fn classify_subtree_nodes(
    root: &std::path::Path,
    scope: &str,
    name: &str,
    template_root: &std::path::Path,
) -> Result<SubtreePartition, MutationError> {
    let template = parse_subtree(template_root)?;
    let subtree_root_abs = crate::mutation::resolve_scoped_path(scope, name);

    let hive_set: std::collections::HashSet<&str> =
        template.hives.iter().map(|s| s.as_str()).collect();

    let mut partition = SubtreePartition {
        missing: Vec::new(),
        existing: Vec::new(),
        missing_hives: Vec::new(),
        existing_hives: Vec::new(),
    };

    for node in &template.cells {
        let abs = absolute_for(&subtree_root_abs, &node.rel_path);
        let final_path = final_path_for(root, scope, name, &node.rel_path);
        let is_hive = hive_set.contains(node.rel_path.as_str());
        let exists = final_path.exists();

        match (is_hive, exists) {
            (true, true) => partition.existing_hives.push(ResolvedExistingHive {
                absolute_path: abs,
                final_path,
            }),
            (true, false) => partition.missing_hives.push(node.clone()),
            (false, true) => {
                let on_disk_cell_type = read_on_disk_cell_type(&final_path);
                let template_cell_type = node
                    .config
                    .get("cell")
                    .and_then(|c| c.get("type"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
                partition.existing.push(ResolvedExistingNode {
                    absolute_path: abs,
                    final_path,
                    on_disk_cell_type,
                    template_cell_type,
                });
            }
            (false, false) => partition.missing.push(node.clone()),
        }
    }

    Ok(partition)
}

/// Read `cell.type` from an EXISTING on-disk cell directory's `config.json`.
///
/// Returns `None` if the file is missing, unreadable, not valid JSON, or does
/// not carry a `cell.type` string — the same lenient `cell.type` extraction
/// [`collect_cells`] uses for templates.
///
/// `pub(crate)` so the single-cell resume path (`colony::handle_mutation`
/// Step 1a, Paket-5 T11) reuses the same lenient extraction for the F2
/// resume-type-compat check, instead of duplicating it.
pub(crate) fn read_on_disk_cell_type(cell_dir: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(cell_dir.join("config.json")).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&raw).ok()?;
    cfg.get("cell")
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

/// Read the on-disk `contract.version` string from a cell directory's
/// `config.json`. Lenient mirror of [`read_on_disk_cell_type`]: returns `None`
/// on a missing/unreadable/unparseable file or absent `contract.version`. Used
/// by the A5b 2b adoption path (`colony::handle_mutation` Step 1a) for the
/// optional `adopt.version` provenance match against the existing node.
pub(crate) fn read_on_disk_contract_version(cell_dir: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(cell_dir.join("config.json")).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&raw).ok()?;
    cfg.get("contract")
        .and_then(|c| c.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// Awake-Schranke for resumed subtree nodes — Paket-5 T7
// ──────────────────────────────────────────────────────────────────────────────

/// Minimal status discriminant the subtree resume-guard cares about.
///
/// The only distinction relevant to the awake-reject is whether a node's cell is
/// currently running (`Awake`) or not. Keeping this decoupled from
/// `colony::CellStatus` (which carries non-`Copy`, non-`Send` parked
/// `mailbox::Receiver` payloads) lets [`subtree_resume_awake_check`] stay a pure,
/// unit-testable helper without building a full Registry. The caller (T12) maps
/// `Some(CellStatus::Awake) → AwakeState::Awake`, everything else (other statuses
/// or no registry entry) → `AwakeState::NotAwake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwakeState {
    /// The node's cell task is currently running — cannot be resumed.
    Awake,
    /// The node's cell is stopped (Asleep / NotYetSpawned) or has no registry
    /// entry — eligible for resume.
    NotAwake,
}

/// Reject the FIRST existing subtree node that is currently `Awake`.
///
/// When per-node subtree resume reconnects an existing subtree node, an EXISTING
/// node whose cell is currently `Awake` (running task) cannot be resumed: a
/// running task cannot race-free release its live `cell.db` (spec Z.296
/// `resume_requires_stopped_cell`). This mirrors the single-cell resume
/// awake-reject in `colony.rs` (which rejects with
/// [`MutationError::ResumeRequiresStoppedCell`] when the target's status is
/// `CellStatus::Awake`).
///
/// PURE: takes the existing-nodes partition (from [`classify_subtree_nodes`] →
/// [`SubtreePartition::existing`]) plus a `status_of` lookup closure, and returns
/// `Err(MutationError::ResumeRequiresStoppedCell(path))` for the first node whose
/// status is [`AwakeState::Awake`] (in iteration order), else `Ok(())`. Nodes
/// absent from the registry (closure returns `None`) are treated as not-awake.
///
/// The error carries the node's absolute logical path string, exactly as the
/// single-cell resume site does (`target.as_str().to_string()`).
///
/// # Errors
/// Returns [`MutationError::ResumeRequiresStoppedCell`] for the first existing
/// node reported as [`AwakeState::Awake`] by `status_of`.
pub fn subtree_resume_awake_check(
    existing: &[ResolvedExistingNode],
    status_of: impl Fn(&Path) -> Option<AwakeState>,
) -> Result<(), MutationError> {
    for node in existing {
        if matches!(status_of(&node.absolute_path), Some(AwakeState::Awake)) {
            return Err(MutationError::ResumeRequiresStoppedCell(
                node.absolute_path.as_str().to_string(),
            ));
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Pure resolution helper (no FS writes) — T8b-2
// ──────────────────────────────────────────────────────────────────────────────

/// Pure (FS-write-free) resolution of a SUBTREE template, sufficient to feed the
/// subtree-aware mutation validator BEFORE any staging happens.
///
/// Holds the absolute logical endpoint representation the validator expects for a
/// subtree instantiation, in the SAME representation the apply side
/// ([`stage_subtree`]) uses.
#[derive(Debug, Clone)]
pub struct ResolvedSubtree {
    /// Absolute logical paths of every subtree cell AND hive marker (root +
    /// nested), e.g. `/main/m1`, `/main/m1/inner_a`. These are the valid edge
    /// endpoints contributed by the subtree.
    pub node_endpoints: Vec<String>,
    /// Subtree-internal edges, resolved to absolute logical `(from, to)` strings
    /// and containment-checked against the subtree root.
    pub internal_edges: Vec<(String, String)>,
    /// The SAME internal edges in their full resolved form (absolute endpoint
    /// [`Path`]s + condition source + raw modifier JSON). Superset of
    /// [`Self::internal_edges`], which stays for the existing validator
    /// consumers. Added in Task 1.3 so the post-state header-view builder can
    /// project modifier key-sets without duplicating the resolution logic.
    pub internal_edges_resolved: Vec<ResolvedEdge>,
}

/// Resolve a SUBTREE template to its absolute node endpoints + internal edges
/// WITHOUT touching the filesystem (no copy, no UUID-patch, no seed).
///
/// Does the SAME resolution [`stage_subtree`] performs, MINUS the FS staging:
/// `parse_subtree` → resolve each cell/hive `rel_path` to its absolute logical
/// path → resolve each hive's `params.graph` edges relative to that hive +
/// containment-check both endpoints against the subtree root.
///
/// `scope`/`name` give the subtree's logical anchor (`resolve_scoped_path` →
/// e.g. `/main/m1`); `template_root` is the on-disk template directory (read-only).
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed or if any
/// resolved internal edge endpoint escapes the subtree root (containment).
pub fn resolve_subtree(
    template_root: &std::path::Path,
    scope: &str,
    name: &str,
) -> Result<ResolvedSubtree, MutationError> {
    let template = parse_subtree(template_root)?;
    let subtree_root_abs = crate::mutation::resolve_scoped_path(scope, name);

    // Every cell (spawnable + hive marker) is a valid edge endpoint.
    let node_endpoints: Vec<String> = template
        .cells
        .iter()
        .map(|c| {
            absolute_for(&subtree_root_abs, &c.rel_path)
                .as_str()
                .to_string()
        })
        .collect();

    let internal_edges_resolved = resolve_internal_edges(&template, &subtree_root_abs)?;
    let internal_edges = internal_edges_resolved
        .iter()
        .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
        .collect();

    Ok(ResolvedSubtree {
        node_endpoints,
        internal_edges,
        internal_edges_resolved,
    })
}

/// Resolve every hive's `params.graph` edges to absolute [`ResolvedEdge`]s,
/// containment-checked against `subtree_root_abs`.
///
/// This is the single resolution truth shared between [`resolve_subtree`] (pure,
/// validation) and [`stage_subtree`] (staging) so both agree on the absolute
/// edge representation and the containment rule.
///
/// # Errors
/// Returns [`MutationError::Schema`] if a resolved endpoint escapes the subtree
/// root.
fn resolve_internal_edges(
    template: &SubtreeTemplate,
    subtree_root_abs: &Path,
) -> Result<Vec<ResolvedEdge>, MutationError> {
    let mut internal_edges: Vec<ResolvedEdge> = Vec::new();
    for hive_rel in &template.hives {
        let hive_abs = absolute_for(subtree_root_abs, hive_rel);
        for spec in hive_edges(template, hive_rel) {
            let from = Path::resolve(&hive_abs, &spec.from);
            let to = Path::resolve(&hive_abs, &spec.to);
            // GH #163: a template's own lane to the colony's read-only topology
            // endpoint is in bounds — it addresses the authority, not a cell
            // outside the subtree (see
            // `crate::mutation::MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`). Only as a
            // target: `from` is always a node of the subtree.
            let exempt_target = crate::mutation::is_mutation_drawable_virtual_target(to.as_str());
            let contained: &[&Path] = if exempt_target {
                &[&from]
            } else {
                &[&from, &to]
            };
            for endpoint in contained {
                if !crate::connectivity::is_self_or_descendant(endpoint, subtree_root_abs) {
                    return Err(MutationError::Schema(format!(
                        "subtree edge endpoint {} escapes subtree root {}",
                        endpoint.as_str(),
                        subtree_root_abs.as_str()
                    )));
                }
            }
            internal_edges.push(ResolvedEdge {
                from,
                to,
                condition: spec.condition,
                modifier: spec.modifier,
            });
        }
    }
    Ok(internal_edges)
}

// ──────────────────────────────────────────────────────────────────────────────
// Staging types + entry point (T4+T5)
// ──────────────────────────────────────────────────────────────────────────────

/// A spawnable (NON-hive) cell staged out of a SUBTREE template.
///
/// Hive markers are scope markers (no actor) and never appear here — they are
/// listed separately in [`StagedSubtree::hive_scopes`]. The fields mirror what
/// the single-cell [`crate::mutation::stage::StagedDir`] carries, so the later
/// integration (T8b) can spawn each nested cell identically.
#[derive(Debug, Clone)]
pub struct StagedCellMeta {
    /// Absolute logical path of this cell, e.g. `/main/m1/inner_a`.
    pub absolute_path: Path,
    /// On-disk directory the staged cell will be renamed into.
    pub final_path: PathBuf,
    /// Resolved `cell.type` from the substituted `config.json`.
    pub cell_type: String,
    /// Resolved `params` block from the substituted `config.json`.
    pub params: serde_json::Value,
    /// Contract view (currently `multi_send_capable`) for the spawn call.
    pub contract_view: crate::factory::ContractView,
    /// `cell.timeout` from the substituted `config.json` (default `0`).
    pub cell_timeout: i64,
    /// Optional per-cell `cell.idle_timeout_ms` override (`None` → substrate default).
    pub idle_timeout_ms: Option<u64>,
    /// P3-B-plumb-2: optional per-cell `cell.message_timeout` (B-backstop) override
    /// (`None` → colony `message_timeout_default_ms`). Same shape as `idle_timeout_ms`.
    pub message_timeout: Option<i64>,
    /// Paket-1 T20: optional per-cell `cell.mailbox_size` override (`None` →
    /// `colony.json mailbox_default_capacity`).
    pub mailbox_size: Option<usize>,
    /// Hardening Slice 1 (Task 1.4): 14-B header projection of the SAME parsed
    /// `contract` block as `contract_view` — registered into the colony's
    /// `node_contracts` map by the subtree registration arm.
    pub header_view: crate::mutation::validate::HeaderNodeView,
    /// GH #62: the SUBTREE template's identity, stamped identically into every
    /// nested cell of this instance. The subtree template is the unit an
    /// app-store update addresses, so every cell it produced names it — not the
    /// per-cell config it happened to be cut from.
    pub provenance: Option<crate::config::NodeProvenance>,
}

/// A subtree-internal edge, RESOLVED to absolute logical paths and verified to
/// stay inside the subtree (containment-checked, was T5).
#[derive(Debug, Clone)]
pub struct ResolvedEdge {
    /// Absolute logical source path.
    pub from: Path,
    /// Absolute logical destination path.
    pub to: Path,
    /// Optional CEL boolean condition, kept verbatim from the hive config.
    pub condition: Option<String>,
    /// Optional header modifier spec (raw JSON), kept verbatim.
    pub modifier: Option<serde_json::Value>,
}

/// Result of staging one SUBTREE template instance into `.staging`.
///
/// Holds the staging root, the resolved final directory for the subtree root,
/// the spawnable cells (NON-hive, root + nested), every hive marker's absolute
/// path, and all subtree-internal edges resolved to absolute logical paths.
#[derive(Debug, Clone)]
pub struct StagedSubtree {
    /// `.staging/<mutation_id>/<name>/` — the staged subtree tree root.
    pub root_staging_path: PathBuf,
    /// On-disk final directory the subtree root renames into.
    pub root_final_path: PathBuf,
    /// SPAWNABLE (NON-hive) cells — root + nested.
    pub cells: Vec<StagedCellMeta>,
    /// Absolute logical paths of every hive marker (root + nested).
    pub hive_scopes: Vec<Path>,
    /// Subtree-internal edges, resolved to absolute logical paths and
    /// containment-checked.
    pub internal_edges: Vec<ResolvedEdge>,
}

/// Stage a multi-cell SUBTREE template instance into `.staging`.
///
/// Builds the full nested tree under `.staging/<mutation_id>/<name>/` by
/// recursively copying `template_root` (dropping `template.json`), patches every
/// cell's `config.json` with a fresh UUID-v7 `cell.id` plus full substitution,
/// seeds inner store `cell.db`s, then resolves each hive's `params.graph.edges`
/// (relative to the owning hive) to absolute logical paths with a subtree
/// containment check.
///
/// `scope`/`name` give the subtree's logical anchor (`resolve_scoped_path` →
/// e.g. `/main/m1`); `template_root` is the on-disk template directory.
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed, copied,
/// patched or seeded, or if any resolved edge endpoint escapes the subtree root.
#[allow(clippy::too_many_arguments)]
pub fn stage_subtree(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    name: &str,
    template_root: &std::path::Path,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    provenance: Option<&crate::config::NodeProvenance>,
    overrides: &SubtreeOverrides,
) -> Result<StagedSubtree, MutationError> {
    // 1. Parse the template tree (reuse T3).
    let template = parse_subtree(template_root)?;

    // 2. Copy the whole nested tree into `.staging/<mid>/<name>/`.
    let root_staging_path = root.join(".staging").join(mutation_id).join(name);
    crate::mutation::stage::copy_dir_recursive(template_root, &root_staging_path)?;

    // Absolute logical subtree root + its on-disk final directory.
    let subtree_root_abs = crate::mutation::resolve_scoped_path(scope, name);
    let root_final_path = crate::path_truth::resolve_cell_dir(root, scope, name);

    // Index hives for the spawnable/hive split.
    let hive_set: std::collections::HashSet<&str> =
        template.hives.iter().map(|s| s.as_str()).collect();

    let mut cells: Vec<StagedCellMeta> = Vec::new();
    let mut hive_scopes: Vec<Path> = Vec::new();

    // 3.–5. Per cell node: patch config (fresh UUID + substitution), seed,
    // and classify into spawnable cells vs. hive scope markers.
    for node in &template.cells {
        let cell_staging = staging_dir_for(&root_staging_path, &node.rel_path);
        let abs = absolute_for(&subtree_root_abs, &node.rel_path);

        // Fresh UUID v7 per cell (`cell_id_override = None`) + substitution.
        // The empty `add_node` carries no `override_params` (templated subtree).
        // GH #140: the per-cell override, addressed by the cell's path inside
        // the template. `patch_and_substitute_config` takes an `add_nodes`
        // entry, so the override is handed over in the shape that call already
        // understands — one code path merges params, not two.
        let node_override = overrides.for_cell(&node.rel_path);
        let (
            cell_type,
            params,
            contract_view,
            cell_timeout,
            idle_timeout_ms,
            message_timeout,
            mailbox_size,
            header_view,
        ) = crate::mutation::stage::patch_and_substitute_config(
            &cell_staging,
            env,
            ctx,
            &node_override,
            provenance,
        )?;
        // Seed inner store cells where a `seed/` dir is present.
        crate::mutation::stage::seed_cell_db_if_present(&cell_staging)?;

        if hive_set.contains(node.rel_path.as_str()) {
            hive_scopes.push(abs);
        } else {
            let final_path = final_path_for(root, scope, name, &node.rel_path);
            cells.push(StagedCellMeta {
                absolute_path: abs,
                final_path,
                cell_type,
                params,
                contract_view,
                cell_timeout,
                idle_timeout_ms,
                message_timeout,
                mailbox_size,
                header_view,
                provenance: provenance.cloned(),
            });
        }
    }

    // 6.+7. Edge-remap + containment via the shared resolution truth (T8b-2):
    // each hive's edges are relative to THAT hive's absolute path; both endpoints
    // are verified to stay inside the subtree root. Identical to what
    // `resolve_subtree` does in the validation phase (one resolution truth).
    let internal_edges = resolve_internal_edges(&template, &subtree_root_abs)?;

    Ok(StagedSubtree {
        root_staging_path,
        root_final_path,
        cells,
        hive_scopes,
        internal_edges,
    })
}

/// Staging directory of a cell node: the root staging dir for `""`, else the
/// `rel_path` joined onto it.
fn staging_dir_for(root_staging_path: &std::path::Path, rel_path: &str) -> PathBuf {
    if rel_path.is_empty() {
        root_staging_path.to_path_buf()
    } else {
        root_staging_path.join(rel_path)
    }
}

/// Absolute logical path of a cell node: the subtree root itself for `""`, else
/// `resolve_scoped_path(subtree_root, rel_path)`.
fn absolute_for(subtree_root_abs: &Path, rel_path: &str) -> Path {
    if rel_path.is_empty() {
        subtree_root_abs.clone()
    } else {
        crate::mutation::resolve_scoped_path(subtree_root_abs.as_str(), rel_path)
    }
}

/// On-disk final directory of a cell node, mirroring how `stage.rs` derives a
/// single cell's `final_path`: `resolve_cell_dir(root, scope, <name>[/rel_path])`.
fn final_path_for(root: &std::path::Path, scope: &str, name: &str, rel_path: &str) -> PathBuf {
    if rel_path.is_empty() {
        crate::path_truth::resolve_cell_dir(root, scope, name)
    } else {
        crate::path_truth::resolve_cell_dir(root, scope, &format!("{name}/{rel_path}"))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Per-node subtree resume: merge-staging (missing rename-roots) — Paket-5 T9
// ──────────────────────────────────────────────────────────────────────────────

/// One missing **rename-root** staged as a complete fresh sub-tree.
///
/// A rename-root is a `missing` template node whose parent directory already
/// exists on disk (the parent is an `existing` node/hive, or the parent is the
/// subtree scope root itself). Because a missing node can have no existing
/// descendants (its directory is absent, so no child directory can exist under
/// it), the rename-root's WHOLE sub-tree is entirely fresh — it is staged with
/// the identical copy/patch(fresh-UUID)/seed machinery [`stage_subtree`] uses,
/// then handed to T10 for a single atomic rename ([`root_staging_path`] →
/// [`root_final_path`]).
///
/// [`root_staging_path`]: StagedRenameRoot::root_staging_path
/// [`root_final_path`]: StagedRenameRoot::root_final_path
#[derive(Debug, Clone)]
pub struct StagedRenameRoot {
    /// `.staging/<mutation_id>/<root-rel-path>/` — the staged sub-tree root,
    /// ready for a single atomic rename in T10.
    pub root_staging_path: PathBuf,
    /// On-disk final directory the rename-root's sub-tree renames into.
    pub root_final_path: PathBuf,
    /// SPAWNABLE (NON-hive) cells inside this rename-root's sub-tree (root +
    /// nested), each with a fresh UUID-v7 `cell.id` and full substitution.
    pub cells: Vec<StagedCellMeta>,
    /// Absolute logical paths of every hive marker inside this rename-root's
    /// sub-tree (root + nested).
    pub hive_scopes: Vec<Path>,
}

/// Result of merge-staging a SUBTREE template against a partial live tree.
///
/// Produced by [`stage_subtree_merge`]. Stages ONLY the `missing` subset — one
/// staged sub-tree per missing rename-root — and passes the `existing` resume
/// info plus ALL resolved subtree-internal edges through untouched. Existing
/// nodes are NEVER copied, patched or seeded (F1: untouched).
#[derive(Debug, Clone)]
pub struct StagedSubtreeMerge {
    /// One staged sub-tree per missing rename-root, each ready for a single
    /// atomic rename in T10.
    pub rename_roots: Vec<StagedRenameRoot>,
    /// Existing spawnable nodes (resume — no FS), passthrough from the partition.
    pub existing: Vec<ResolvedExistingNode>,
    /// Existing hive markers (resume — no FS), passthrough from the partition.
    pub existing_hives: Vec<ResolvedExistingHive>,
    /// ALL subtree-internal edges resolved to absolute logical paths
    /// (existing-referencing + new), containment-checked against the subtree
    /// root. Dedup against the live edge table happens later at insert time
    /// (T8) — here every internal edge is resolved, none filtered.
    pub internal_edges: Vec<ResolvedEdge>,
}

/// Compute the **rename-roots** of a [`SubtreePartition`]: the `missing` nodes
/// (spawnable cells AND hive markers) whose parent directory already exists on
/// disk.
///
/// A node's parent "exists on disk" when the parent's absolute logical path is
/// the subtree scope root itself OR is the path of some `existing` /
/// `existing_hives` node in the partition. Such a missing node roots a
/// completely fresh sub-tree (a missing node can have no existing descendants),
/// so each rename-root's whole sub-tree is staged independently.
///
/// Returned as `(rel_path, is_hive)` pairs in template order. Edge cases:
/// * whole-root-missing → the single rename-root is the subtree root
///   (`rel_path == ""`), reproducing today's fresh-subtree behavior;
/// * root-exists-but-child-missing → the rename-root is the missing child.
///
/// PURE: reads only the partition + the subtree root, no filesystem access.
pub fn rename_roots(partition: &SubtreePartition, subtree_root_abs: &Path) -> Vec<(String, bool)> {
    // Absolute paths whose directory is known to exist on disk: every
    // classified-existing node/hive. (The subtree root itself is handled
    // separately below — when it is MISSING it is the sole rename-root, when it
    // EXISTS it is already in `partition.existing`/`existing_hives`.)
    let mut existing_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &partition.existing {
        existing_dirs.insert(e.absolute_path.as_str().to_string());
    }
    for h in &partition.existing_hives {
        existing_dirs.insert(h.absolute_path.as_str().to_string());
    }

    // A missing node roots a fresh sub-tree iff its parent directory exists:
    // either it IS the subtree root (`rel_path == ""`, parent is the scope,
    // which always exists since that is where we instantiate), or its parent's
    // absolute path is a classified-existing node/hive.
    let is_root = |node: &CellNode| -> bool {
        node.rel_path.is_empty() || {
            let abs = absolute_for(subtree_root_abs, &node.rel_path);
            existing_dirs.contains(abs.parent().as_str())
        }
    };

    let mut roots: Vec<(String, bool)> = Vec::new();
    for node in &partition.missing {
        if is_root(node) {
            roots.push((node.rel_path.clone(), false));
        }
    }
    for node in &partition.missing_hives {
        if is_root(node) {
            roots.push((node.rel_path.clone(), true));
        }
    }
    roots
}

/// GH #140 — `override_params` for the cells INSIDE a subtree template.
///
/// A subtree template is instantiated as a whole, and until now its sub-cells
/// could not be parameterised at that moment: `override_params` was rejected
/// outright (R10, 2026-06-11), because the flat form has no addressing and used
/// to commit as a silent no-op. The protection was right and the closure was
/// collateral — `collector`, `cogny` and `talky` are subtree templates, so the
/// params surface they gained in #136 had no way to be set at birth. An
/// operator edited the instance config afterwards, which is a fork of the
/// template by hand.
///
/// The addressing is the cell's path inside the template:
///
/// ```json
/// {"name": "coll", "template": "collector",
///  "override_params": {"assemble": {"max_turns": 40},
///                      "window": {"retention_days": 7}}}
/// ```
///
/// `""` addresses the subtree root. A key that names no cell in the template is
/// a `schema` reject that lists what the template actually contains — R10's
/// original complaint was a silent no-op, and an unaddressable key must not
/// become one again by a different route.
#[derive(Debug, Default)]
pub struct SubtreeOverrides {
    by_rel_path: HashMap<String, JsonValue>,
}

impl SubtreeOverrides {
    /// Read the map out of an `add_nodes` entry. An absent or empty
    /// `override_params` yields an empty set, which changes nothing anywhere.
    pub fn from_add_node(add_node: &JsonValue) -> Self {
        let mut by_rel_path = HashMap::new();
        if let Some(obj) = add_node.get("override_params").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                by_rel_path.insert(k.clone(), v.clone());
            }
        }
        Self { by_rel_path }
    }

    /// The synthetic `add_nodes` entry for one cell — the shape
    /// `patch_and_substitute_config` already merges. A cell with no entry gets
    /// an empty object and is byte-identical to the pre-#140 staging.
    fn for_cell(&self, rel_path: &str) -> JsonValue {
        match self.by_rel_path.get(rel_path) {
            None => JsonValue::Object(Default::default()),
            Some(params) => {
                let mut o = serde_json::Map::new();
                o.insert("override_params".to_string(), params.clone());
                JsonValue::Object(o)
            }
        }
    }

    /// Every addressed path, for the validator that has to reject an unknown one.
    pub fn addressed(&self) -> impl Iterator<Item = &String> {
        self.by_rel_path.keys()
    }

    /// True when nothing was addressed — the state of every mutation written
    /// before this existed.
    pub fn is_empty(&self) -> bool {
        self.by_rel_path.is_empty()
    }
}

/// Merge-stage a SUBTREE template against a partial live tree: stage ONLY the
/// `missing` subset (one fresh sub-tree per rename-root), leaving existing nodes
/// untouched, and resolve ALL subtree-internal edges.
///
/// Decomposition (T9): classify the template against the live FS
/// ([`classify_subtree_nodes`]) → compute [`rename_roots`] → stage each
/// rename-root's whole sub-tree with the SAME copy/patch(fresh-UUID)/seed
/// machinery as [`stage_subtree`], sub-rooted at the rename-root's template
/// sub-path and targeting its nested final path. Existing cells are NOT copied,
/// NOT patched, NOT seeded (F1). Internal edges come from the shared resolution
/// truth ([`resolve_internal_edges`]) — every internal edge is resolved here,
/// none filtered (live-table dedup is T8, later).
///
/// Backwards-compat: when the whole root is missing there is exactly one
/// rename-root (`rel_path == ""`) and its staged sub-tree equals what
/// [`stage_subtree`] produces today (same cells, same final paths, same edges).
///
/// `scope`/`name` give the subtree's logical anchor (`resolve_scoped_path` →
/// e.g. `/main/m1`); `template_root` is the on-disk template directory.
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed, copied,
/// patched or seeded, or if any resolved edge endpoint escapes the subtree root.
#[allow(clippy::too_many_arguments)]
pub fn stage_subtree_merge(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    name: &str,
    template_root: &std::path::Path,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    provenance: Option<&crate::config::NodeProvenance>,
    overrides: &SubtreeOverrides,
) -> Result<StagedSubtreeMerge, MutationError> {
    let template = parse_subtree(template_root)?;
    let partition = classify_subtree_nodes(root, scope, name, template_root)?;
    let subtree_root_abs = crate::mutation::resolve_scoped_path(scope, name);

    let roots = rename_roots(&partition, &subtree_root_abs);
    let hive_set: std::collections::HashSet<&str> =
        template.hives.iter().map(|s| s.as_str()).collect();

    let mut rename_root_stagings: Vec<StagedRenameRoot> = Vec::new();
    for (root_rel, _is_hive) in &roots {
        rename_root_stagings.push(stage_rename_root(
            root,
            mutation_id,
            scope,
            name,
            template_root,
            &template,
            &subtree_root_abs,
            &hive_set,
            root_rel,
            env,
            ctx,
            provenance,
            overrides,
        )?);
    }

    // All subtree-internal edges, resolved once via the shared truth. No
    // filtering against the live edge table (that is T8, at insert time).
    let internal_edges = resolve_internal_edges(&template, &subtree_root_abs)?;

    Ok(StagedSubtreeMerge {
        rename_roots: rename_root_stagings,
        existing: partition.existing,
        existing_hives: partition.existing_hives,
        internal_edges,
    })
}

/// Stage the complete fresh sub-tree rooted at the rename-root `root_rel`.
///
/// Copies `template_root/<root_rel>` into `.staging/<mid>/<root_rel>/`, then for
/// EVERY template cell at or below `root_rel`: patches its `config.json` with a
/// fresh UUID-v7 `cell.id` + full substitution, seeds an inner store `cell.db`
/// if present, and classifies it into spawnable cells vs. hive scope markers —
/// identical per-cell semantics to [`stage_subtree`].
#[allow(clippy::too_many_arguments)]
fn stage_rename_root(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    name: &str,
    template_root: &std::path::Path,
    template: &SubtreeTemplate,
    subtree_root_abs: &Path,
    hive_set: &std::collections::HashSet<&str>,
    root_rel: &str,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    provenance: Option<&crate::config::NodeProvenance>,
    overrides: &SubtreeOverrides,
) -> Result<StagedRenameRoot, MutationError> {
    // Copy the rename-root's template sub-path into staging (drops template.json).
    let template_subdir = if root_rel.is_empty() {
        template_root.to_path_buf()
    } else {
        template_root.join(root_rel)
    };
    let root_staging_path = if root_rel.is_empty() {
        root.join(".staging").join(mutation_id).join(name)
    } else {
        root.join(".staging")
            .join(mutation_id)
            .join(format!("{name}/{root_rel}"))
    };
    crate::mutation::stage::copy_dir_recursive(&template_subdir, &root_staging_path)?;

    let root_final_path = final_path_for(root, scope, name, root_rel);

    let mut cells: Vec<StagedCellMeta> = Vec::new();
    let mut hive_scopes: Vec<Path> = Vec::new();

    // Every template node at or below the rename-root belongs to this fresh
    // sub-tree (a missing node has no existing descendants).
    for node in &template.cells {
        if !is_self_or_rel_descendant(&node.rel_path, root_rel) {
            continue;
        }
        // Staging dir of this node = root_staging_path + (node.rel_path minus root_rel).
        let sub_rel = rel_under(&node.rel_path, root_rel);
        let cell_staging = staging_dir_for(&root_staging_path, &sub_rel);
        let abs = absolute_for(subtree_root_abs, &node.rel_path);

        // GH #140: same per-cell addressing on the merge path — a subtree that
        // grows a missing branch parameterises it exactly like a fresh one.
        let node_override = overrides.for_cell(&node.rel_path);
        let (
            cell_type,
            params,
            contract_view,
            cell_timeout,
            idle_timeout_ms,
            message_timeout,
            mailbox_size,
            header_view,
        ) = crate::mutation::stage::patch_and_substitute_config(
            &cell_staging,
            env,
            ctx,
            &node_override,
            provenance,
        )?;
        crate::mutation::stage::seed_cell_db_if_present(&cell_staging)?;

        if hive_set.contains(node.rel_path.as_str()) {
            hive_scopes.push(abs);
        } else {
            let final_path = final_path_for(root, scope, name, &node.rel_path);
            cells.push(StagedCellMeta {
                absolute_path: abs,
                final_path,
                cell_type,
                params,
                contract_view,
                cell_timeout,
                idle_timeout_ms,
                message_timeout,
                mailbox_size,
                header_view,
                provenance: provenance.cloned(),
            });
        }
    }

    Ok(StagedRenameRoot {
        root_staging_path,
        root_final_path,
        cells,
        hive_scopes,
    })
}

/// True if template `rel_path` is the rename-root `root_rel` itself or a
/// descendant of it. The empty `root_rel` (whole-root rename-root) matches every
/// node.
fn is_self_or_rel_descendant(rel_path: &str, root_rel: &str) -> bool {
    if root_rel.is_empty() {
        return true;
    }
    rel_path == root_rel || rel_path.starts_with(&format!("{root_rel}/"))
}

/// Template `rel_path` rewritten relative to the rename-root `root_rel`.
///
/// For `root_rel == ""` this is `rel_path` unchanged. For `rel_path == root_rel`
/// it is `""` (the rename-root's own staging dir). For a descendant it is the
/// suffix after `root_rel/`.
fn rel_under(rel_path: &str, root_rel: &str) -> String {
    if root_rel.is_empty() {
        return rel_path.to_string();
    }
    if rel_path == root_rel {
        return String::new();
    }
    rel_path
        .strip_prefix(&format!("{root_rel}/"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| rel_path.to_string())
}

/// Re-extract the edges owned by the hive at `hive_rel` from the parsed tree.
///
/// [`SubtreeTemplate::edges`] flattens edges across all hives, losing owner
/// attribution; edge-remap needs each edge resolved relative to ITS hive, so we
/// re-read the owning hive's `params.graph.edges` here.
fn hive_edges(template: &SubtreeTemplate, hive_rel: &str) -> Vec<EdgeSpec> {
    let Some(node) = template.cells.iter().find(|c| c.rel_path == hive_rel) else {
        return Vec::new();
    };
    let params_value = node
        .config
        .get("params")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let Ok(hp) = serde_json::from_value::<HiveParams>(params_value) else {
        return Vec::new();
    };
    hp.graph
        .edges
        .into_iter()
        .map(edge_spec_from_config)
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_json(path: &std::path::Path, json: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, json).unwrap();
    }

    /// Task 1.3 (header-view builder): `resolve_subtree` must ALSO expose the
    /// internal edges in their full resolved form (condition + modifier JSON),
    /// consistent with the derived `(from, to)` tuple list.
    #[test]
    fn resolve_subtree_carries_resolved_internal_edges_with_modifier() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write_json(&root.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &root.join("config.json"),
            r#"{"cell":{"type":"hive"},
                "params":{"graph":{"edges":[
                    {"from":"./inner_a","to":"./inner_b",
                     "condition":"true",
                     "modifier":{"set_hop":{"h":"'1'"}}}
                ]}}}"#,
        );
        write_json(
            &root.join("inner_a/config.json"),
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &root.join("inner_b/config.json"),
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let resolved = resolve_subtree(root, "/main", "m1").expect("resolve_subtree");

        assert_eq!(resolved.internal_edges_resolved.len(), 1);
        let re = &resolved.internal_edges_resolved[0];
        assert_eq!(re.from.as_str(), "/main/m1/inner_a");
        assert_eq!(re.to.as_str(), "/main/m1/inner_b");
        assert_eq!(re.condition.as_deref(), Some("true"));
        let m = re.modifier.as_ref().expect("modifier JSON carried");
        assert!(m.get("set_hop").is_some());
        // Derived tuple list stays consistent with the resolved form.
        assert_eq!(resolved.internal_edges.len(), 1);
        assert_eq!(resolved.internal_edges[0].0, "/main/m1/inner_a");
        assert_eq!(resolved.internal_edges[0].1, "/main/m1/inner_b");
    }

    /// Build a multi-cell SUBTREE template layout and verify the parser
    /// produces the correct cells/hives/edges.
    ///
    /// Layout:
    /// ```text
    /// <root>/template.json          # metadata only — ignored for graph purposes
    /// <root>/config.json            # hive with one edge: ./inner_a → ./inner_b
    /// <root>/inner_a/config.json    # echo cell
    /// <root>/inner_b/config.json    # echo cell
    /// ```
    #[test]
    fn parse_subtree_multi_cell_hive_collects_cells_hives_edges() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // template.json — metadata only, no graph field
        write_json(&root.join("template.json"), r#"{"name":"multi_subtree"}"#);

        // root config.json — hive with internal edge
        write_json(
            &root.join("config.json"),
            r#"{
                "cell": {"type": "hive"},
                "params": {
                    "graph": {
                        "edges": [
                            {"from": "./inner_a", "to": "./inner_b"}
                        ]
                    }
                }
            }"#,
        );

        // inner_a/config.json — echo cell
        write_json(
            &root.join("inner_a").join("config.json"),
            r#"{"cell": {"type": "echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // inner_b/config.json — echo cell
        write_json(
            &root.join("inner_b").join("config.json"),
            r#"{"cell": {"type": "echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let result = parse_subtree(root).expect("parse_subtree should succeed");

        // 3 cells: root + inner_a + inner_b
        assert_eq!(
            result.cells.len(),
            3,
            "expected 3 cells, got {:?}",
            result.cells.iter().map(|c| &c.rel_path).collect::<Vec<_>>()
        );

        // hives: only the root (rel_path == "")
        assert_eq!(result.hives.len(), 1, "expected 1 hive");
        assert_eq!(
            result.hives[0], "",
            "root hive rel_path should be empty string"
        );

        // edges: one edge from ./inner_a to ./inner_b
        assert_eq!(result.edges.len(), 1, "expected 1 edge");
        assert_eq!(result.edges[0].from, "./inner_a");
        assert_eq!(result.edges[0].to, "./inner_b");
        assert!(result.edges[0].condition.is_none());
        assert!(result.edges[0].modifier.is_none());

        // rel_path of all cells
        let rel_paths: Vec<&str> = result.cells.iter().map(|c| c.rel_path.as_str()).collect();
        assert!(rel_paths.contains(&""), "root cell missing");
        assert!(rel_paths.contains(&"inner_a"), "inner_a missing");
        assert!(rel_paths.contains(&"inner_b"), "inner_b missing");
    }

    /// A `seed/` subdirectory with a `config.json` must NOT be treated as a cell node.
    #[test]
    fn parse_subtree_ignores_seed_subdirectory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // root config.json — simple echo cell (not a hive)
        write_json(
            &root.join("config.json"),
            r#"{"cell": {"type": "echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // seed/ subdirectory with config.json and an extra file — must be ignored
        write_json(
            &root.join("seed").join("config.json"),
            r#"{"cell": {"type": "echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(&root.join("seed").join("data.json"), r#"{"rows": []}"#);

        let result = parse_subtree(root).expect("parse_subtree should succeed");

        // Only 1 cell — the root; seed is excluded
        assert_eq!(
            result.cells.len(),
            1,
            "seed/ must not be treated as a cell; got {:?}",
            result.cells.iter().map(|c| &c.rel_path).collect::<Vec<_>>()
        );
        assert_eq!(result.cells[0].rel_path, "");
    }

    // ──────────────────────────────────────────────────────────────────────
    // stage_subtree tests (T4+T5)
    // ──────────────────────────────────────────────────────────────────────

    use std::collections::HashMap;

    /// Create a colony root with a single root-cell dir so `resolve_cell_dir`
    /// anchors final paths under it (spec § Filesystem-Layout).
    fn colony_root_with_root_cell(td: &std::path::Path, root_cell: &str) {
        let p = td.join(root_cell);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("config.json"), b"{}").unwrap();
    }

    /// Parse a staged cell's `config.json` and return its `cell.id` string.
    fn staged_cell_id(staging_path: &std::path::Path, rel: &str) -> String {
        let cfg_path = if rel.is_empty() {
            staging_path.join("config.json")
        } else {
            staging_path.join(rel).join("config.json")
        };
        let raw = fs::read_to_string(&cfg_path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        v.get("cell")
            .and_then(|c| c.get("id"))
            .and_then(|i| i.as_str())
            .unwrap()
            .to_string()
    }

    /// GH #62: a subtree instance stamps the SUBTREE template's identity into
    /// every nested node — hive markers included. The subtree template is the
    /// unit an update addresses, so a per-node origin that named something else
    /// would be a lie about who owns the node.
    #[test]
    fn stage_subtree_stamps_every_nested_node_with_the_subtree_template() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        let tpl = root.join("tpl");
        write_json(
            &tpl.join("template.json"),
            r#"{"name":"sub","version":"3.1.0"}"#,
        );
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("inner_a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("inner_a").join("inner_b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let prov = crate::config::NodeProvenance {
            template: "sub".into(),
            template_version: Some("3.1.0".into()),
            instantiated_at: 1_700_000_000,
        };
        let staged = stage_subtree(
            root,
            "mid-prov",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            Some(&prov),
            &SubtreeOverrides::default(),
        )
        .expect("stage_subtree should succeed");

        for rel in ["", "inner_a", "inner_a/inner_b"] {
            let cfg_path = if rel.is_empty() {
                staged.root_staging_path.join("config.json")
            } else {
                staged.root_staging_path.join(rel).join("config.json")
            };
            let v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
            assert_eq!(
                v["cell"]["provenance"]["template"], "sub",
                "node {rel:?} must name the subtree template: {v}"
            );
            assert_eq!(v["cell"]["provenance"]["template_version"], "3.1.0");
            assert_eq!(v["cell"]["provenance"]["instantiated_at"], 1_700_000_000i64);
        }
        for cell in &staged.cells {
            assert_eq!(
                cell.provenance.as_ref(),
                Some(&prov),
                "every spawnable cell carries the stamp on for the registry index"
            );
        }
    }

    #[test]
    fn stage_subtree_instantiates_nested_tree_with_fresh_uuids() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        // 3-level template: root hive → inner_a → inner_a/inner_b.
        let tpl = root.join("tpl");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("inner_a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("inner_a").join("inner_b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let staged = stage_subtree(
            root,
            "mid-x",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("stage_subtree should succeed");

        // Every staged config.json has a distinct, valid UUID v7 cell.id.
        let ids: Vec<String> = ["", "inner_a", "inner_a/inner_b"]
            .iter()
            .map(|rel| staged_cell_id(&staged.root_staging_path, rel))
            .collect();
        for id in &ids {
            let u: meclaw_core::Uuid = id.parse().expect("valid uuid");
            assert_eq!(u.get_version_num(), 7, "cell.id must be UUID v7");
        }
        // Pairwise-disjoint.
        let set: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(set.len(), ids.len(), "cell ids must be pairwise-disjoint");

        // cells lists only the NON-hive cells with correct absolute_paths.
        let mut abs: Vec<String> = staged
            .cells
            .iter()
            .map(|c| c.absolute_path.as_str().to_string())
            .collect();
        abs.sort();
        assert_eq!(abs, vec!["/main/m1/inner_a", "/main/m1/inner_a/inner_b"]);
        // The root hive is NOT a spawnable cell.
        assert_eq!(staged.cells.len(), 2);
    }

    #[test]
    fn stage_subtree_remaps_internal_edges_to_absolute() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        let tpl = root.join("tpl");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./a/b"}]}}}"#,
        );
        write_json(
            &tpl.join("a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("a").join("b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let staged = stage_subtree(
            root,
            "mid-e",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("stage_subtree should succeed");

        assert_eq!(staged.internal_edges.len(), 1);
        assert_eq!(staged.internal_edges[0].from.as_str(), "/main/m1/a");
        assert_eq!(staged.internal_edges[0].to.as_str(), "/main/m1/a/b");
    }

    /// T29 — Pinning test: root rename ⇒ complete edge remap.
    ///
    /// When a subtree template is instantiated under a name that differs from the
    /// template's own default name (the `"name"` field in `template.json`), EVERY
    /// internal edge must use the chosen instantiation name in its absolute path —
    /// no edge may contain the template-default name.
    ///
    /// Template default name: `"default_tpl_name"` (in `template.json`).
    /// Chosen instantiation name: `"renamed_root"`.
    /// Expected edge: `from=/main/renamed_root/a`, `to=/main/renamed_root/b`.
    #[test]
    fn stage_subtree_root_rename_remaps_all_internal_edges() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        // Template whose metadata name differs from the instantiation name chosen below.
        let tpl = root.join("tpl_rename");
        write_json(&tpl.join("template.json"), r#"{"name":"default_tpl_name"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write_json(
            &tpl.join("a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // Instantiate under "renamed_root" — deliberately different from
        // the template's own "default_tpl_name".
        let staged = stage_subtree(
            root,
            "mid-rename",
            "/main",
            "renamed_root",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("stage_subtree should succeed");

        assert_eq!(
            staged.internal_edges.len(),
            1,
            "expected exactly one internal edge"
        );

        let from = staged.internal_edges[0].from.as_str();
        let to = staged.internal_edges[0].to.as_str();

        // The chosen name must appear in both endpoints.
        assert_eq!(
            from, "/main/renamed_root/a",
            "edge 'from' must carry the chosen instantiation name, got: {from}"
        );
        assert_eq!(
            to, "/main/renamed_root/b",
            "edge 'to' must carry the chosen instantiation name, got: {to}"
        );

        // The template-default name must NOT appear anywhere in the edges.
        assert!(
            !from.contains("default_tpl_name"),
            "edge 'from' must not contain the template-default name, got: {from}"
        );
        assert!(
            !to.contains("default_tpl_name"),
            "edge 'to' must not contain the template-default name, got: {to}"
        );
    }

    #[test]
    fn stage_subtree_rejects_edge_escaping_subtree_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        let tpl = root.join("tpl");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"../sibling"}]}}}"#,
        );
        write_json(
            &tpl.join("a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let err = stage_subtree(
            root,
            "mid-esc",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect_err("escaping edge must be rejected");
        assert!(
            matches!(err, MutationError::Schema(_)),
            "expected Schema error, got {err:?}"
        );
    }

    #[test]
    fn stage_subtree_collects_hive_scopes_absolute() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        // Root hive + a nested hive marker `sub_h`.
        let tpl = root.join("tpl");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("sub_h").join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("sub_h").join("leaf").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let staged = stage_subtree(
            root,
            "mid-h",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("stage_subtree should succeed");

        let mut hs: Vec<String> = staged
            .hive_scopes
            .iter()
            .map(|p| p.as_str().to_string())
            .collect();
        hs.sort();
        assert_eq!(hs, vec!["/main/m1", "/main/m1/sub_h"]);
        // The leaf is the only spawnable cell.
        assert_eq!(staged.cells.len(), 1);
        assert_eq!(
            staged.cells[0].absolute_path.as_str(),
            "/main/m1/sub_h/leaf"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // classify_subtree_nodes tests (Paket-5 T6)
    // ──────────────────────────────────────────────────────────────────────

    /// Build a 3-cell template: root hive + two spawnable cells `inner_a`,
    /// `inner_b`. Returns the template root path.
    fn classify_template(colony_root: &std::path::Path, tpl_name: &str) -> std::path::PathBuf {
        let tpl = colony_root.join(tpl_name);
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("inner_a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("inner_b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        tpl
    }

    /// Materialize a live on-disk cell directory at the node's final fs path.
    fn live_cell(
        colony_root: &std::path::Path,
        scope: &str,
        name_rel: &str,
        cell_type: &str,
    ) -> std::path::PathBuf {
        let final_path = crate::path_truth::resolve_cell_dir(colony_root, scope, name_rel);
        write_json(
            &final_path.join("config.json"),
            &format!(
                r#"{{"cell":{{"type":"{cell_type}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
            ),
        );
        final_path
    }

    #[test]
    fn classify_subtree_nodes_partial_live_tree_splits_missing_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_partial");

        // Live tree: root hive dir (m1) + inner_a present, inner_b absent.
        // (Materializing m1/inner_a also creates the m1 parent dir, which is the
        // realistic partial-resume shape: subtree root present, some children
        // missing.)
        live_cell(colony_root, "/main", "m1", "hive");
        live_cell(colony_root, "/main", "m1/inner_a", "echo");

        let part = classify_subtree_nodes(colony_root, "/main", "m1", &tpl)
            .expect("classify should succeed");

        // missing spawnable cell: inner_b only.
        let missing_rel: Vec<&str> = part.missing.iter().map(|c| c.rel_path.as_str()).collect();
        assert_eq!(missing_rel, vec!["inner_b"], "missing cells");

        // existing spawnable cell: inner_a only.
        assert_eq!(part.existing.len(), 1, "one existing cell");
        assert_eq!(part.existing[0].absolute_path.as_str(), "/main/m1/inner_a");

        // root hive dir is present on disk → existing.
        assert!(part.missing_hives.is_empty(), "no missing hives");
        assert_eq!(part.existing_hives.len(), 1, "root hive existing");
        assert_eq!(part.existing_hives[0].absolute_path.as_str(), "/main/m1");
    }

    #[test]
    fn classify_subtree_nodes_fresh_tree_all_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_fresh");

        let part = classify_subtree_nodes(colony_root, "/main", "m1", &tpl)
            .expect("classify should succeed");

        assert_eq!(part.missing.len(), 2, "both spawnable cells missing");
        assert!(part.existing.is_empty(), "no existing cells");
        assert_eq!(part.missing_hives.len(), 1, "root hive missing");
        assert!(part.existing_hives.is_empty(), "no existing hives");
    }

    #[test]
    fn classify_subtree_nodes_full_tree_all_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_full");

        // Materialize the WHOLE tree on disk.
        live_cell(colony_root, "/main", "m1", "hive");
        live_cell(colony_root, "/main", "m1/inner_a", "echo");
        live_cell(colony_root, "/main", "m1/inner_b", "echo");

        let part = classify_subtree_nodes(colony_root, "/main", "m1", &tpl)
            .expect("classify should succeed");

        assert!(part.missing.is_empty(), "no missing cells");
        assert_eq!(part.existing.len(), 2, "both spawnable cells existing");
        assert!(part.missing_hives.is_empty(), "no missing hives");
        assert_eq!(part.existing_hives.len(), 1, "root hive existing");
        assert_eq!(part.existing_hives[0].absolute_path.as_str(), "/main/m1");
    }

    #[test]
    fn classify_subtree_nodes_captures_on_disk_cell_type() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_type");

        // Live inner_a exists but with a DIFFERENT on-disk type than template.
        live_cell(colony_root, "/main", "m1/inner_a", "store");

        let part = classify_subtree_nodes(colony_root, "/main", "m1", &tpl)
            .expect("classify should succeed");

        assert_eq!(part.existing.len(), 1);
        assert_eq!(
            part.existing[0].on_disk_cell_type.as_deref(),
            Some("store"),
            "on_disk_cell_type must come from the EXISTING config.json"
        );
    }

    #[test]
    fn classify_subtree_nodes_partitions_nested_hives() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");

        // Template: root hive + nested hive `sub_h` + leaf cell under sub_h.
        let tpl = colony_root.join("tpl_hives");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("sub_h").join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("sub_h").join("leaf").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // Live tree: root hive present, nested hive sub_h absent.
        live_cell(colony_root, "/main", "m1", "hive");

        let part = classify_subtree_nodes(colony_root, "/main", "m1", &tpl)
            .expect("classify should succeed");

        // Root hive existing, nested sub_h missing.
        assert_eq!(part.existing_hives.len(), 1, "root hive existing");
        assert_eq!(part.existing_hives[0].absolute_path.as_str(), "/main/m1");
        let missing_hive_rel: Vec<&str> = part
            .missing_hives
            .iter()
            .map(|c| c.rel_path.as_str())
            .collect();
        assert_eq!(missing_hive_rel, vec!["sub_h"], "nested hive missing");

        // The leaf is the only spawnable cell, and it is missing.
        assert_eq!(part.missing.len(), 1, "leaf missing");
        assert_eq!(part.missing[0].rel_path, "sub_h/leaf");
        assert!(part.existing.is_empty(), "no existing spawnable cells");
    }

    // ──────────────────────────────────────────────────────────────────────
    // subtree_resume_awake_check tests (Paket-5 T7)
    // ──────────────────────────────────────────────────────────────────────

    /// Build a `ResolvedExistingNode` with the given absolute logical path.
    fn existing_node(abs: &str) -> ResolvedExistingNode {
        ResolvedExistingNode {
            absolute_path: Path::new(abs),
            final_path: PathBuf::from(abs),
            on_disk_cell_type: None,
            template_cell_type: None,
        }
    }

    #[test]
    fn subtree_resume_awake_check_ok_when_no_node_awake() {
        let existing = vec![
            existing_node("/main/m1/inner_a"),
            existing_node("/main/m1/inner_b"),
        ];
        // inner_a is NotAwake, inner_b is absent from the registry (None).
        let status_of = |p: &Path| match p.as_str() {
            "/main/m1/inner_a" => Some(AwakeState::NotAwake),
            _ => None,
        };
        subtree_resume_awake_check(&existing, status_of)
            .expect("all non-awake (or absent) existing nodes must be Ok");
    }

    #[test]
    fn subtree_resume_awake_check_rejects_first_awake_node() {
        let existing = vec![
            existing_node("/main/m1/inner_a"),
            existing_node("/main/m1/inner_b"),
        ];
        // inner_a is NotAwake, inner_b is Awake → reject inner_b.
        let status_of = |p: &Path| match p.as_str() {
            "/main/m1/inner_b" => Some(AwakeState::Awake),
            _ => Some(AwakeState::NotAwake),
        };
        let err = subtree_resume_awake_check(&existing, status_of)
            .expect_err("an awake existing node must be rejected");
        match err {
            MutationError::ResumeRequiresStoppedCell(path) => {
                assert_eq!(path, "/main/m1/inner_b", "must carry the awake node's path");
            }
            other => panic!("expected ResumeRequiresStoppedCell, got {other:?}"),
        }
    }

    #[test]
    fn subtree_resume_awake_check_ok_for_empty_existing() {
        let existing: Vec<ResolvedExistingNode> = Vec::new();
        subtree_resume_awake_check(&existing, |_p| Some(AwakeState::Awake))
            .expect("empty existing must be Ok regardless of status closure");
    }

    // ──────────────────────────────────────────────────────────────────────
    // rename_roots + stage_subtree_merge tests (Paket-5 T9)
    // ──────────────────────────────────────────────────────────────────────

    /// Content hash of a file (for byte-unchanged assertions).
    fn file_hash(p: &std::path::Path) -> u64 {
        use std::hash::{Hash, Hasher};
        let bytes = fs::read(p).unwrap();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    }

    /// Sorted `(rel_path, is_hive)` of the rename-roots for the given subtree.
    fn sorted_roots(
        colony_root: &std::path::Path,
        scope: &str,
        name: &str,
        tpl: &std::path::Path,
    ) -> Vec<(String, bool)> {
        let part = classify_subtree_nodes(colony_root, scope, name, tpl).unwrap();
        let abs = crate::mutation::resolve_scoped_path(scope, name);
        let mut r = rename_roots(&part, &abs);
        r.sort();
        r
    }

    #[test]
    fn rename_roots_whole_fresh_is_single_subtree_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_rr_fresh");

        // Nothing on disk → whole tree missing → single rename-root == "".
        let roots = sorted_roots(colony_root, "/main", "m1", &tpl);
        assert_eq!(
            roots,
            vec![("".to_string(), true)],
            "single root == subtree root (hive)"
        );
    }

    #[test]
    fn rename_roots_root_exists_one_child_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_rr_child");

        // Root hive + inner_a present, inner_b missing → root = inner_b only.
        live_cell(colony_root, "/main", "m1", "hive");
        live_cell(colony_root, "/main", "m1/inner_a", "echo");

        let roots = sorted_roots(colony_root, "/main", "m1", &tpl);
        assert_eq!(roots, vec![("inner_b".to_string(), false)]);
    }

    #[test]
    fn rename_roots_two_missing_siblings_under_existing_parent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_rr_two");

        // Only root hive present → inner_a + inner_b both missing → 2 roots.
        live_cell(colony_root, "/main", "m1", "hive");

        let roots = sorted_roots(colony_root, "/main", "m1", &tpl);
        assert_eq!(
            roots,
            vec![
                ("inner_a".to_string(), false),
                ("inner_b".to_string(), false)
            ]
        );
    }

    #[test]
    fn rename_roots_missing_hive_with_missing_children_is_single_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");

        // Root hive + nested hive `sub_h` + leaf under sub_h.
        let tpl = colony_root.join("tpl_rr_hive");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("sub_h").join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("sub_h").join("leaf").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // Root hive present, sub_h (+ its leaf) missing → single root == sub_h.
        live_cell(colony_root, "/main", "m1", "hive");

        let roots = sorted_roots(colony_root, "/main", "m1", &tpl);
        assert_eq!(
            roots,
            vec![("sub_h".to_string(), true)],
            "the missing nested hive is the sole rename-root; its leaf comes inside it"
        );
    }

    #[test]
    fn stage_subtree_merge_stages_only_missing_and_leaves_existing_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = classify_template(colony_root, "tpl_merge_partial");

        // Live: root hive + inner_a present (with a cell.db), inner_b missing.
        live_cell(colony_root, "/main", "m1", "hive");
        let inner_a_dir = live_cell(colony_root, "/main", "m1/inner_a", "echo");
        let inner_a_cfg = inner_a_dir.join("config.json");
        let inner_a_db = inner_a_dir.join("cell.db");
        fs::write(&inner_a_db, b"existing-db-bytes").unwrap();
        let cfg_hash_before = file_hash(&inner_a_cfg);
        let db_hash_before = file_hash(&inner_a_db);

        let merged = stage_subtree_merge(
            colony_root,
            "mid-merge",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("merge-staging should succeed");

        // Exactly one rename-root: inner_b.
        assert_eq!(merged.rename_roots.len(), 1, "one rename-root");
        let rr = &merged.rename_roots[0];
        assert_eq!(rr.cells.len(), 1);
        assert_eq!(rr.cells[0].absolute_path.as_str(), "/main/m1/inner_b");
        assert!(rr.hive_scopes.is_empty(), "inner_b is not a hive");

        // The staged dir for inner_b exists; nothing staged for inner_a.
        assert!(
            rr.root_staging_path.join("config.json").exists(),
            "inner_b staged"
        );
        assert!(
            !colony_root.join(".staging/mid-merge/m1/inner_a").exists(),
            "inner_a must NOT be staged"
        );

        // existing passthrough: inner_a present.
        assert_eq!(merged.existing.len(), 1);
        assert_eq!(
            merged.existing[0].absolute_path.as_str(),
            "/main/m1/inner_a"
        );
        assert_eq!(merged.existing_hives.len(), 1);
        assert_eq!(merged.existing_hives[0].absolute_path.as_str(), "/main/m1");

        // Existing inner_a config.json + cell.db are byte-unchanged.
        assert_eq!(
            file_hash(&inner_a_cfg),
            cfg_hash_before,
            "config.json untouched"
        );
        assert_eq!(file_hash(&inner_a_db), db_hash_before, "cell.db untouched");
    }

    #[test]
    fn stage_subtree_merge_whole_fresh_equivalent_to_stage_subtree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");

        // 3-level template (root hive → a → a/b) with an internal edge.
        let tpl = colony_root.join("tpl_equiv");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./a/b"}]}}}"#,
        );
        write_json(
            &tpl.join("a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("a").join("b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // Whole tree missing → single rename-root == subtree root.
        let merged = stage_subtree_merge(
            colony_root,
            "mid-equiv",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("merge should succeed");

        assert_eq!(merged.rename_roots.len(), 1, "single fresh rename-root");
        let rr = &merged.rename_roots[0];

        // Same root_final_path + root_staging_path as stage_subtree would use.
        assert_eq!(
            rr.root_final_path,
            crate::path_truth::resolve_cell_dir(colony_root, "/main", "m1")
        );
        assert_eq!(
            rr.root_staging_path,
            colony_root.join(".staging").join("mid-equiv").join("m1")
        );

        // Same set of spawnable cells (absolute paths).
        let mut abs: Vec<String> = rr
            .cells
            .iter()
            .map(|c| c.absolute_path.as_str().to_string())
            .collect();
        abs.sort();
        assert_eq!(abs, vec!["/main/m1/a", "/main/m1/a/b"]);

        // Root hive collected as scope.
        assert_eq!(rr.hive_scopes.len(), 1);
        assert_eq!(rr.hive_scopes[0].as_str(), "/main/m1");

        // Internal edge resolved identically to stage_subtree.
        assert_eq!(merged.internal_edges.len(), 1);
        assert_eq!(merged.internal_edges[0].from.as_str(), "/main/m1/a");
        assert_eq!(merged.internal_edges[0].to.as_str(), "/main/m1/a/b");

        // No existing nodes for a whole-fresh subtree.
        assert!(merged.existing.is_empty());
        assert!(merged.existing_hives.is_empty());
    }

    #[test]
    fn stage_subtree_merge_internal_edges_contains_all_existing_and_new() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");

        // Root hive with two edges: one between existing nodes, one to a missing
        // node. Both must appear in internal_edges (no filtering at T9).
        let tpl = colony_root.join("tpl_edges_all");
        write_json(&tpl.join("template.json"), r#"{"name":"sub"}"#);
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
                {"from":"./inner_a","to":"./inner_b"},
                {"from":"./inner_a","to":"./inner_c"}
            ]}}}"#,
        );
        write_json(
            &tpl.join("inner_a").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("inner_b").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("inner_c").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // inner_a + inner_b exist (the first edge references existing nodes);
        // inner_c missing (the second edge references a new node).
        live_cell(colony_root, "/main", "m1", "hive");
        live_cell(colony_root, "/main", "m1/inner_a", "echo");
        live_cell(colony_root, "/main", "m1/inner_b", "echo");

        let merged = stage_subtree_merge(
            colony_root,
            "mid-edges",
            "/main",
            "m1",
            &tpl,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
        )
        .expect("merge should succeed");

        let mut edges: Vec<(String, String)> = merged
            .internal_edges
            .iter()
            .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
            .collect();
        edges.sort();
        assert_eq!(
            edges,
            vec![
                (
                    "/main/m1/inner_a".to_string(),
                    "/main/m1/inner_b".to_string()
                ),
                (
                    "/main/m1/inner_a".to_string(),
                    "/main/m1/inner_c".to_string()
                ),
            ],
            "both existing-referencing and new edges must be present, none filtered"
        );

        // Only inner_c staged.
        assert_eq!(merged.rename_roots.len(), 1);
        assert_eq!(
            merged.rename_roots[0].cells[0].absolute_path.as_str(),
            "/main/m1/inner_c"
        );
    }
}
