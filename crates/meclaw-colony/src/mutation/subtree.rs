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
    /// GH #277: the `(name, version)` of every template `ref` traversed **below**
    /// the outer template root to reach this node, outermost first.
    ///
    /// Empty for a node that lives literally in the outer template's directory.
    /// This is the raw material of the provenance chain and of the ref-override
    /// map.
    pub ref_chain: Vec<(String, Option<String>)>,
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
    /// GH #283: the edge's routing PHASE as the template declares it
    /// (`"default": true`). A default edge is consulted only after every
    /// ordinary out-edge of the same sender declined.
    ///
    /// Read here rather than at instantiation because this struct is the only
    /// thing that survives the template walk — including the walk through a
    /// `ref`'d sub-template, which reaches the same
    /// [`edge_spec_from_config`]. Without it a composition template's default
    /// edge would be right in the template and an ordinary edge in every
    /// instance.
    pub is_default: bool,
    /// GH #559: the lane a template's own edge declares (`lane`), read here for
    /// exactly the reason `is_default` above is read here — this struct is the
    /// only thing that survives the template walk, including the walk through a
    /// `ref`'d sub-template.
    ///
    /// It closes a divergence rather than opening a door: the BOOT path already
    /// carries the key (`config::EdgeSpec::lane` → `PlannedEdge::lane`), so
    /// without this field the same template declared a v-lane when it was booted
    /// and an ordinary edge when it was instantiated — the quieter half of the
    /// GH #196 class. Subtree-internal edges are not judged by stage 6 in either
    /// door (a template's statement about ITSELF, ruling 2026-08-15); what they
    /// owe is to mean the same thing through both.
    pub lane: Option<String>,
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
    /// GH #277: the `override_params` carried by the `ref`s traversed on the way
    /// in, re-addressed from *their* rel-paths to **this** template's
    /// (`{"proxy": …}` inside a ref at `child` becomes `{"child/proxy": …}`,
    /// `""` becomes the ref's own position).
    ///
    /// A ref parameterises the template it pulls in — so this is the **default**
    /// layer under whatever the instantiating mutation says
    /// ([`SubtreeOverrides::with_ref_defaults`]). Empty for a ref-free template,
    /// which therefore stages exactly as it did before.
    pub ref_overrides: HashMap<String, JsonValue>,
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
/// - A cell whose `config.cell.type == "ref"` is NOT a node of its own: the
///   referenced template is resolved through `templates` and its whole content
///   takes the ref's position, re-anchored under it. Nothing below the ref
///   directory on disk is read (GH #277).
/// - `template.json` is ignored entirely (it carries metadata only).
///
/// `templates` is the registry snapshot a `cell.type: "ref"` sub-unit resolves
/// against — a ref names another template, and only the registry knows where it
/// lives on disk (GH #277).
///
/// # Errors
/// - [`MutationError::Schema`] if any `config.json` cannot be parsed as JSON, or
///   if a ref directory is malformed (stray entry, no string `cell.template`).
/// - [`MutationError::TemplateMissing`] if a ref names a template the registry
///   does not hold, or names a malformed `@<version>`.
/// - [`MutationError::TemplateRefCycle`] if the refs close a ring. The
///   resolution stack is the guard, so no depth cap is needed.
pub fn parse_subtree(
    template_root: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
) -> Result<SubtreeTemplate, MutationError> {
    let mut cells: Vec<CellNode> = Vec::new();
    let mut hives: Vec<String> = Vec::new();
    let mut edges: Vec<EdgeSpec> = Vec::new();
    let mut ref_overrides: HashMap<String, JsonValue> = HashMap::new();

    collect_cells(
        template_root,
        template_root,
        &mut cells,
        &mut hives,
        &mut edges,
        &mut ref_overrides,
        &[],
        templates,
    )?;

    Ok(SubtreeTemplate {
        cells,
        hives,
        edges,
        ref_overrides,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Recursively collect cell nodes starting at `dir`.
///
/// `root` is the original template root, used to compute relative paths.
/// `chain` is the `(name, version)` sequence of template refs already traversed
/// to reach `dir`; every node pushed here records it as
/// [`CellNode::ref_chain`].
/// `templates` is the registry a `cell.type: "ref"` sub-unit resolves against;
/// `chain` doubles as the resolution stack the cycle guard reads (GH #277).
/// `ref_overrides` collects every traversed ref's `override_params`, addressed
/// in `root`'s rel-paths.
#[allow(clippy::too_many_arguments)]
fn collect_cells(
    root: &std::path::Path,
    dir: &std::path::Path,
    cells: &mut Vec<CellNode>,
    hives: &mut Vec<String>,
    edges: &mut Vec<EdgeSpec>,
    ref_overrides: &mut HashMap<String, JsonValue>,
    chain: &[(String, Option<String>)],
    templates: &crate::templates::TemplatesRegistry,
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

        // GH #353 (fix round 1): the closed `cell` key list is a barrier HERE
        // too. Everything below keeps the config as a raw `serde_json::Value`
        // (`CellNode::config`), and so do the readers downstream
        // (`mutation/stage.rs`), so an unknown key inside a subtree node used to
        // travel all the way through validation into the written tree — where
        // the boot walk, which DOES parse `CellHeader`, then refused it. A
        // mutation that commits a tree the next restart rejects is the failure
        // this closes. Same struct, same serde message, same naming as the
        // single-cell path in `mutation/header_views.rs`: the key comes from
        // serde, the file from here.
        if let Some(cell_block) = config.get("cell") {
            serde_json::from_value::<crate::config::CellHeader>(cell_block.clone()).map_err(
                |e| MutationError::Schema(format!("parse {}: cell: {e}", config_path.display())),
            )?;
        }

        // Determine if this is a hive cell.
        let cell_type = config
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        // GH #277: a `ref` directory is not a cell — the referenced template's
        // whole content takes its position. Nothing below it on disk is read.
        if cell_type == "ref" {
            return expand_ref(
                dir,
                &rel_path,
                &config,
                cells,
                hives,
                edges,
                ref_overrides,
                chain,
                templates,
            );
        }

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

        cells.push(CellNode {
            rel_path,
            config,
            ref_chain: chain.to_vec(),
        });
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
        collect_cells(
            root,
            &sub,
            cells,
            hives,
            edges,
            ref_overrides,
            chain,
            templates,
        )?;
    }

    Ok(())
}

/// Expand a `cell.type: "ref"` directory at `rel_path`: resolve `cell.template`
/// through the registry and walk the referenced template as if its content had
/// been written at the ref's position.
///
/// The referenced template is walked with its OWN root, then every rel-path it
/// produced is re-anchored under `rel_path` — that keeps each hive's
/// `params.graph.edges` relative to the hive that declared them, which is
/// exactly what the ref position requires.
///
/// # Errors
/// [`MutationError::Schema`] if the ref directory holds anything besides
/// `config.json` (one address, two sources) or declares no string
/// `cell.template`; [`MutationError::TemplateMissing`] if the reference resolves
/// to nothing or names a malformed `@<version>`;
/// [`MutationError::TemplateRefCycle`] if the referenced template is already on
/// the resolution stack.
#[allow(clippy::too_many_arguments)]
fn expand_ref(
    dir: &std::path::Path,
    rel_path: &str,
    config: &serde_json::Value,
    cells: &mut Vec<CellNode>,
    hives: &mut Vec<String>,
    edges: &mut Vec<EdgeSpec>,
    ref_overrides: &mut HashMap<String, JsonValue>,
    chain: &[(String, Option<String>)],
    templates: &crate::templates::TemplatesRegistry,
) -> Result<(), MutationError> {
    reject_stray_ref_entries(dir)?;

    let reference = config
        .get("cell")
        .and_then(|c| c.get("template"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| {
            MutationError::Schema("a ref cell must declare cell.template".to_string())
        })?;
    let entry = templates
        .resolve(reference)
        .map_err(|e| unresolvable_ref(reference, rel_path, &e, templates))?;

    // The resolution stack IS the cycle guard: a template that is already on the
    // way in cannot be entered a second time. That is also why composition needs
    // no depth cap (spec `:110`) — a ring is refused at its first repetition, and
    // without a ring the chain cannot outgrow the finite registry.
    let hop = (entry.name.clone(), entry.version.clone());
    if chain.contains(&hop) {
        return Err(MutationError::TemplateRefCycle(format!(
            "template ref cycle at {rel_path:?}: {}",
            render_ref_cycle(chain, &hop)
        )));
    }

    let mut sub_chain = chain.to_vec();
    sub_chain.push(hop);

    let (mut sub_cells, mut sub_hives) = (Vec::new(), Vec::new());
    let mut sub_overrides: HashMap<String, JsonValue> = HashMap::new();
    collect_cells(
        &entry.filesystem_path,
        &entry.filesystem_path,
        &mut sub_cells,
        &mut sub_hives,
        edges,
        &mut sub_overrides,
        &sub_chain,
        templates,
    )?;

    // This ref speaks AFTER the refs it pulled in: its own `override_params`
    // layer on top of theirs, param key by param key.
    collect_ref_overrides(config, reference, &sub_cells, &mut sub_overrides)?;

    for mut node in sub_cells {
        node.rel_path = anchor_rel(rel_path, &node.rel_path);
        cells.push(node);
    }
    for hive in sub_hives {
        hives.push(anchor_rel(rel_path, &hive));
    }
    for (key, params) in sub_overrides {
        ref_overrides.insert(anchor_rel(rel_path, &key), params);
    }
    Ok(())
}

/// Read `override_params` off a ref marker, check every key against the cells
/// the referenced template actually has, and layer the entries onto
/// `sub_overrides` (which already holds whatever the refs *below* declared).
///
/// The key check is the same protection GH #140 gives the mutation form: a key
/// that addresses nothing must not become a silent no-op by a different route.
///
/// # Errors
/// [`MutationError::Schema`] if `override_params` is not an object keyed by the
/// referenced template's cell paths, if a key names no such cell (the message
/// lists the ones that exist), or if an entry is not a params object.
fn collect_ref_overrides(
    config: &serde_json::Value,
    reference: &str,
    sub_cells: &[CellNode],
    sub_overrides: &mut HashMap<String, JsonValue>,
) -> Result<(), MutationError> {
    let Some(over) = config.get("override_params") else {
        return Ok(());
    };
    let obj = ref_override_block(over, reference)?;
    for (key, params) in obj {
        ref_override_target(key, params, sub_cells, reference)?;
        layer_params(
            sub_overrides
                .entry(key.clone())
                .or_insert_with(|| JsonValue::Object(Default::default())),
            params,
        );
    }
    Ok(())
}

/// The `override_params` block of a `ref` as a keyed object, or the refusal that
/// says what shape it owes.
///
/// Extracted (GH #586) so the two readers of a ref's block — the template walk
/// ([`collect_ref_overrides`]) and the root-tree marker's pre-flight
/// ([`check_ref_marker_overrides`]) — cannot word one cause twice.
///
/// # Errors
/// [`MutationError::Schema`] if the block is not an object.
fn ref_override_block<'a>(
    over: &'a JsonValue,
    reference: &str,
) -> Result<&'a serde_json::Map<String, JsonValue>, MutationError> {
    over.as_object().ok_or_else(|| {
        MutationError::Schema(format!(
            "override_params on the ref to '{reference}' must be an object keyed by the cells' \
             paths inside the referenced template (\"\" is its root)"
        ))
    })
}

/// The cell one `override_params` entry of a `ref` addresses — the CELL half of
/// the check (GH #140), shared by both readers of a ref's block for the reason
/// [`ref_override_block`] gives.
///
/// # Errors
/// [`MutationError::Schema`] if `key` names no cell of the referenced template
/// (the message lists the ones that exist), or if the entry is not a params
/// object.
fn ref_override_target<'a>(
    key: &str,
    params: &JsonValue,
    cells: &'a [CellNode],
    reference: &str,
) -> Result<&'a CellNode, MutationError> {
    let Some(cell) = cells.iter().find(|c| c.rel_path == key) else {
        let known: Vec<&str> = cells.iter().map(|c| c.rel_path.as_str()).collect();
        return Err(MutationError::Schema(format!(
            "override_params['{key}'] names no cell of the referenced template \
             '{reference}'. Its cells are: {}",
            render_cell_list(&known)
        )));
    };
    if !params.is_object() {
        return Err(MutationError::Schema(format!(
            "override_params['{key}'] must be a params object"
        )));
    }
    Ok(cell)
}

/// GH #586 — ask a root-tree `ref` MARKER's top-level `override_params` both
/// halves the mutation door asks `add_nodes[].override_params`: the CELL half
/// (GH #140 — a key that names no cell of the referenced template) and the
/// PARAM half (GH #294 — a key inside an entry that names no param of the cell
/// it reached).
///
/// The marker is the one place where a template's params are reachable before a
/// builder exists, and it was the one place nobody asked: a typo committed as a
/// silent no-op (wrong cell path) or surfaced a whole boot cycle later at the
/// spawning cell (wrong param key). `docs/config.md` § Spezialfall
/// Template-Referenz has claimed the cell half since GH #277 — for a marker in
/// the ROOT tree it was true of neither half.
///
/// A marker's block is ALWAYS the ADDRESSED form (`""` is the referenced
/// template's root); the flat single-cell form GH #436 disambiguates belongs to
/// `add_nodes` and never reaches here.
///
/// This is a pure question about a parsed template — no filesystem, no writes —
/// which is what lets `meclaw --validate` ask it without growing anything.
///
/// **Every** offending key is reported, not the first — the door collects for
/// the same reason ([`crate::mutation::validate::collect_post_state_addresses`],
/// GH #293: a diff whose override misspells four keys names four keys). A
/// pre-flight check that surrendered on the first typo would cost one boot's
/// worth of round trips per typo, which is the cost this whole check exists to
/// remove. Within ONE addressed entry the door's own
/// [`check_override_params`] still stops at that entry's first unknown param,
/// and this keeps that granularity rather than growing a second opinion about
/// it.
///
/// Returns the refusals in the block's key order; an empty vector means every
/// entry addresses a cell and a param that exist. Each is a
/// [`MutationError::Schema`] (`error_code` `schema`, the door's own — never a
/// new code, README § Stability). A block that is not an object at all is one
/// refusal about the block, and there is nothing further to ask.
pub fn check_ref_marker_overrides(
    template: &SubtreeTemplate,
    reference: &str,
    override_params: &JsonValue,
) -> Vec<MutationError> {
    let obj = match ref_override_block(override_params, reference) {
        Ok(obj) => obj,
        Err(e) => return vec![e],
    };
    let mut findings = Vec::new();
    for (key, params) in obj {
        match ref_override_target(key, params, &template.cells, reference) {
            // The entry addresses nothing, or is not a params object — asking
            // it about param names would be asking about a shape that is not
            // there. Next key.
            Err(e) => findings.push(e),
            Ok(cell) => {
                if let Err(e) = check_override_params(cell, Some(key), reference, params) {
                    findings.push(e);
                }
            }
        }
    }
    findings
}

/// GH #294 (ruling Q6, 2026-08-21) — refuse an `override_params` entry that
/// names a param the addressed cell does not have.
///
/// GH #140 built the CELL half: a key that names no cell of a subtree template
/// is refused instead of committing as a silent no-op. This is the PARAM half,
/// one nesting level down — a typo inside the entry commits today and the cell
/// spawns with its default.
///
/// "Exists" is an EXISTENCE check, nothing more: the cell's template
/// `config.json` either carries the key under `params` or it does not. Types and
/// a `because` may arrive later as declarations, incrementally. The key set does
/// not depend on the values, so instance substitution is irrelevant here and the
/// RAW `params` object is what is read. A cell with **no** `params` block has the
/// EMPTY set — and is refused naming that empty list rather than swallowing the
/// override, which is what
/// [`crate::mutation::stage::patch_and_substitute_config`]'s
/// `if let Some(params)` merge does on its own.
///
/// `cell_key` is the address the override was written at: `Some(rel_path)` for
/// the ADDRESSED form of a subtree template, `None` for the FLAT form of a
/// single-cell template, which has no cell coordinate to name. Both forms call
/// this one function, from validation, so they cannot drift apart.
///
/// A non-object `params` argument carries no keys and passes; the shape of the
/// entry itself is the caller's check.
///
/// # Errors
/// [`MutationError::Schema`] for the first key the cell's `params` does not
/// carry, naming the key, the cell, its `cell.type`, the template, and the
/// params that do exist.
pub fn check_override_params(
    cell: &CellNode,
    cell_key: Option<&str>,
    template: &str,
    params: &JsonValue,
) -> Result<(), MutationError> {
    let Some(over) = params.as_object() else {
        return Ok(());
    };
    let known: Vec<&str> = cell
        .config
        .get("params")
        .and_then(|p| p.as_object())
        .map(|p| p.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();
    let cell_type = cell
        .config
        .get("cell")
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("cell");
    for key in over.keys() {
        if known.contains(&key.as_str()) {
            continue;
        }
        let addressed = match cell_key {
            Some(k) => format!("override_params['{k}']['{key}']"),
            None => format!("override_params['{key}']"),
        };
        let at = match cell_key {
            Some(k) => format!(" at '{k}'"),
            None => String::new(),
        };
        return Err(MutationError::Schema(format!(
            "{addressed} names no param of {cell_type}{at} in template '{template}'. Its params \
             are: {}",
            render_param_list(&known)
        )));
    }
    Ok(())
}

/// GH #661 — the params this cell's contract declares `operator_set`.
///
/// Read off [`CellNode::config`], which is the fully parsed `config.json` of the
/// cell — the declaration is therefore reachable at the checking place without
/// reading a file twice. It lives in `contract.settings` and not in the
/// `description` block: the latter is dropped in silence by `ParsedConfig`
/// (*specified, not built* — GH #254), so the door could not read it even if it
/// wanted to, while `contract.settings` is substrate-enforced, is broken out per
/// param, and with `secret` already carries this form of statement.
pub fn operator_set_keys(cell: &CellNode) -> Vec<&str> {
    cell.config
        .get("contract")
        .and_then(|c| c.get("settings"))
        .and_then(|s| s.as_object())
        .map(|s| {
            s.iter()
                .filter(|(_, v)| v.get("operator_set").and_then(|b| b.as_bool()) == Some(true))
                .map(|(k, _)| k.as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// GH #661 (ADR-0032) — every `operator_set` param of `cell` the entry brings no
/// value for, collected.
///
/// "Brings no value" means exactly one thing: the entry does not write the key
/// into `override_params` (flat on a single-cell template, addressed on a
/// subtree), and no `ref` marker on the way in set it as a default
/// ([`SubtreeTemplate::ref_overrides`]). What is checked is the ACT and not the
/// value — an entry setting the param to exactly the shipped default comes
/// through, because that is the one honest thing an operator can say about such
/// a param, and reading it as an omission would make the declaration
/// unanswerable (OR-A2).
///
/// COLLECTING rather than `Result`, because stage 4 collects: three unset params
/// are three named violations in one refusal rather than three round trips.
///
/// `cell_key` is the address the entry would have written at: `Some(rel_path)`
/// for the addressed form of a subtree template, `None` for the flat form of a
/// single-cell template — the same distinction
/// [`check_override_params`] already makes.
pub fn check_operator_set_params(
    cell: &CellNode,
    cell_key: Option<&str>,
    template: &str,
    params: Option<&JsonValue>,
    ref_defaults: Option<&JsonValue>,
    out: &mut Vec<MutationError>,
) {
    let named = |v: Option<&JsonValue>, key: &str| {
        v.and_then(|p| p.as_object())
            .is_some_and(|o| o.contains_key(key))
    };
    for key in operator_set_keys(cell) {
        if named(params, key) || named(ref_defaults, key) {
            continue;
        }
        let addressed = match cell_key {
            Some(k) => format!("override_params['{k}']['{key}']"),
            None => format!("override_params['{key}']"),
        };
        let shipped = cell
            .config
            .get("contract")
            .and_then(|c| c.get("settings"))
            .and_then(|s| s.get(key))
            .and_then(|spec| spec.get("default"))
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| "none".to_string());
        out.push(MutationError::OperatorParamUnset(format!(
            "`{addressed}` is missing: `{template}` declares `{key}` as `operator_set`, so its \
             shipped default (`{shipped}`) is a shape and not a working value. Set it on this \
             entry, or grow the node somewhere it can mean something."
        )));
    }
}

/// Render a cell's param names for an error message: `'a', 'b'`, or the literal
/// `none` for a cell that declares no params at all.
fn render_param_list(known: &[&str]) -> String {
    if known.is_empty() {
        return "none".to_string();
    }
    let mut listed: Vec<String> = known.iter().map(|k| format!("'{k}'")).collect();
    listed.sort();
    listed.join(", ")
}

/// Render a template's cell paths for an error message: `"" (root), 'a', 'b'`.
///
/// `pub(crate)` so the mutation validator's addressed-`override_params` refusal
/// (`validate::validate_post_state_with_templates_scoped`) renders its cell list
/// the same way this module's ref-override refusal does — the two had the same
/// listing logic written out twice (GH #294 review).
pub(crate) fn render_cell_list(known: &[&str]) -> String {
    let mut listed: Vec<String> = known
        .iter()
        .map(|k| {
            if k.is_empty() {
                "\"\" (root)".to_string()
            } else {
                format!("'{k}'")
            }
        })
        .collect();
    listed.sort();
    listed.join(", ")
}

/// Layer `over`'s param keys onto `under`, key by key — `over` wins every
/// contest, and a key only `under` holds survives untouched. A non-object on
/// either side has no keys to merge and leaves `under` as it was.
fn layer_params(under: &mut JsonValue, over: &JsonValue) {
    let (Some(u), Some(o)) = (under.as_object_mut(), over.as_object()) else {
        return;
    };
    for (k, v) in o {
        u.insert(k.clone(), v.clone());
    }
}

/// Render a resolution chain plus the hop that closes it as
/// `a@1.0.0 -> b@1.0.0 -> a@1.0.0` (a hop without a version reads as its bare
/// name).
fn render_ref_cycle(
    chain: &[(String, Option<String>)],
    repeated: &(String, Option<String>),
) -> String {
    chain
        .iter()
        .chain(std::iter::once(repeated))
        .map(|(name, version)| match version {
            Some(v) => format!("{name}@{v}"),
            None => name.clone(),
        })
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Turn a registry [`crate::templates::ResolveError`] into the
/// [`MutationError::TemplateMissing`] a reader can act on.
///
/// Two shapes, two sentences: a reference the registry simply does not hold
/// resolves to nothing and is answered with the versions it DOES hold under that
/// name (or the literal `none`), because a version typo is the common case. A
/// malformed `@<version>` never asked an answerable question at all — it names
/// the offending token instead, and must not claim the template is absent.
fn unresolvable_ref(
    reference: &str,
    rel_path: &str,
    error: &crate::templates::ResolveError,
    templates: &crate::templates::TemplatesRegistry,
) -> MutationError {
    if let crate::templates::ResolveError::InvalidVersionRef(_, reason) = error {
        let token = reference.split_once('@').map(|(_, v)| v).unwrap_or("");
        return MutationError::TemplateMissing(format!(
            "template reference {reference:?} in the ref at {rel_path:?} names a malformed version {token:?}: {reason}"
        ));
    }
    let name = reference
        .split_once('@')
        .map(|(n, _)| n)
        .unwrap_or(reference);
    let mut known: Vec<String> = templates
        .entries_iter()
        .filter(|e| e.name == name)
        .map(|e| {
            e.version
                .clone()
                .unwrap_or_else(|| "unversioned".to_string())
        })
        .collect();
    known.sort();
    let list = if known.is_empty() {
        "none".to_string()
    } else {
        known.join(", ")
    };
    MutationError::TemplateMissing(format!(
        "template reference {reference:?} in the ref at {rel_path:?} resolves to nothing; known versions of {name:?}: {list}"
    ))
}

/// Reject every entry of a `ref` directory other than `config.json`.
///
/// A ref names its content; a file next to the marker would give one address two
/// sources, which is exactly the ambiguity the reference removes.
fn reject_stray_ref_entries(dir: &std::path::Path) -> Result<(), MutationError> {
    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| MutationError::Schema(format!("read_dir {}: {e}", dir.display())))?;
    let mut stray: Vec<String> = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| {
            MutationError::Schema(format!("read_dir entry in {}: {e}", dir.display()))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != "config.json" {
            stray.push(format!("{name:?}"));
        }
    }
    if stray.is_empty() {
        return Ok(());
    }
    stray.sort();
    Err(MutationError::Schema(format!(
        "a ref cell directory must contain nothing but config.json; {} also contains: {}",
        dir.display(),
        stray.join(", ")
    )))
}

/// Re-anchor a rel-path produced inside a referenced template under the ref's
/// own position. The referenced root (`""`) lands exactly ON the ref position.
///
/// `ref_rel_path` is never empty: a template ROOT cannot be a ref. Every
/// template root carries a `template.json` next to its `config.json`, and
/// [`reject_stray_ref_entries`] refuses a ref directory that holds anything
/// besides `config.json` — so a root marked `cell.type: "ref"` is rejected
/// before it ever reaches this function. A ref is always a nested position.
fn anchor_rel(ref_rel_path: &str, inner_rel_path: &str) -> String {
    if inner_rel_path.is_empty() {
        return ref_rel_path.to_string();
    }
    format!("{ref_rel_path}/{inner_rel_path}")
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
        // GH #283: the `default` key of the config edge, carried like any other
        // edge property. `ConfigEdgeSpec` already type-checked it.
        is_default: spec.is_default,
        // GH #559: and the `lane` key, same discipline. `ConfigEdgeSpec` is
        // `deny_unknown_fields`, so a misspelling is already a boot error and
        // this read cannot silently invent a lane.
        lane: spec.lane,
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
// GH #277: the ref-aware on-disk copy (Task 7)
// ──────────────────────────────────────────────────────────────────────────────

/// The `(name, version)` sequence of template refs traversed so far — the
/// resolution stack, which doubles as the cycle guard. Same shape as
/// [`CellNode::ref_chain`], which the parse side records.
type RefChain = Vec<(String, Option<String>)>;

/// If `dir` is a `cell.type: "ref"` marker, resolve it and return the referenced
/// template's root plus the chain extended by that hop; otherwise `None`.
///
/// A `seed` directory is data, never a cell — exactly as [`collect_cells`] reads
/// it, so both derivations agree on what a ref is.
///
/// # Errors
/// [`MutationError::Schema`] if the marker's `config.json` is unreadable or
/// declares no string `cell.template`; [`MutationError::TemplateMissing`] if the
/// reference resolves to nothing; [`MutationError::TemplateRefCycle`] if the
/// referenced template is already on `chain` — the same guard, the same error as
/// the parse side.
fn ref_target(
    dir: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
    chain: &[(String, Option<String>)],
) -> Result<Option<(PathBuf, RefChain)>, MutationError> {
    if dir.file_name().map(|n| n == "seed").unwrap_or(false) {
        return Ok(None);
    }
    let config_path = dir.join("config.json");
    if !config_path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| MutationError::Schema(format!("read {}: {e}", config_path.display())))?;
    let config: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| MutationError::Schema(format!("parse {}: {e}", config_path.display())))?;
    let cell = config.get("cell");
    if cell.and_then(|c| c.get("type")).and_then(|t| t.as_str()) != Some("ref") {
        return Ok(None);
    }
    let reference = cell
        .and_then(|c| c.get("template"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| {
            MutationError::Schema("a ref cell must declare cell.template".to_string())
        })?;
    let rel = dir.display().to_string();
    let entry = templates
        .resolve(reference)
        .map_err(|e| unresolvable_ref(reference, &rel, &e, templates))?;
    let hop = (entry.name.clone(), entry.version.clone());
    if chain.contains(&hop) {
        return Err(MutationError::TemplateRefCycle(format!(
            "template ref cycle at {rel:?}: {}",
            render_ref_cycle(chain, &hop)
        )));
    }
    let mut sub_chain = chain.to_vec();
    sub_chain.push(hop);
    Ok(Some((entry.filesystem_path.clone(), sub_chain)))
}

/// Follow `dir` through every ref it names until a non-ref directory is reached,
/// carrying the resolution stack along.
///
/// # Errors
/// Whatever [`ref_target`] reports.
fn follow_refs(
    dir: PathBuf,
    templates: &crate::templates::TemplatesRegistry,
    chain: RefChain,
) -> Result<(PathBuf, RefChain), MutationError> {
    let (mut dir, mut chain) = (dir, chain);
    while let Some((next, sub_chain)) = ref_target(&dir, templates, &chain)? {
        dir = next;
        chain = sub_chain;
    }
    Ok((dir, chain))
}

/// Recursively copy the template tree at `src` into `dst`, ref-aware.
///
/// [`crate::mutation::stage::copy_dir_recursive`]'s semantics (`template.json`
/// stripped at every level) plus one rule: a directory whose `config.json`
/// declares `cell.type == "ref"` contributes the **referenced template root's
/// content** at its position instead of the marker, recursively. `chain` is the
/// resolution stack and doubles as the cycle guard — the same guard and the same
/// [`MutationError::TemplateRefCycle`] the parse side raises (GH #277).
///
/// At a resolved ref the root's `README.md` is stripped too. `template.json` and
/// `README.md` are the descriptor pair of a STANDALONE template — its registry
/// entry and its page — and neither is part of the instance the ref places: the
/// byte copies a ref replaces never carried one (`talky/collector/` held
/// `config.json` files and nothing else). The composite's OWN README at the top
/// of the tree is not affected: nothing was followed to reach it.
///
/// A ref-free tree takes the identical path as before: no shipped template holds
/// a ref, so every one of them is copied byte-for-byte as it was.
///
/// # Errors
/// [`MutationError::Schema`] on any filesystem failure, plus whatever
/// [`ref_target`] reports for a malformed, unresolvable or cyclic reference.
fn copy_template_tree(
    src: &std::path::Path,
    dst: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
    chain: &[(String, Option<String>)],
) -> Result<(), MutationError> {
    let depth_before = chain.len();
    let (src, chain) = follow_refs(src.to_path_buf(), templates, chain.to_vec())?;
    // True exactly when this directory WAS a ref marker: `src` is now some other
    // template's root, and that root's descriptor pair is not ours.
    let via_ref = chain.len() > depth_before;
    std::fs::create_dir_all(dst)
        .map_err(|e| MutationError::Schema(format!("create staging cell dir: {e}")))?;
    for entry in std::fs::read_dir(&src)
        .map_err(|e| MutationError::Schema(format!("read template dir: {e}")))?
    {
        let entry = entry.map_err(|e| MutationError::Schema(format!("read entry: {e}")))?;
        let path = entry.path();
        let fname = entry.file_name();
        if fname == "template.json" {
            continue; // template-meta never goes into the instance.
        }
        if via_ref && fname == "README.md" {
            continue; // the referenced template's own page, see the fn docs.
        }
        let target = dst.join(&fname);
        if path.is_dir() {
            if fname == "seed" {
                // A `seed/` subtree is DATA, not cells: `collect_cells` never
                // descends into one, so neither may the copy path resolve refs
                // down there — a `config.json` in a seed fixture is a row's
                // payload, and expanding it would put files on disk that no
                // parsed cell claims. The whole subtree goes through the
                // ref-blind copy (GH #277).
                crate::mutation::stage::copy_dir_recursive(&path, &target)?;
            } else {
                copy_template_tree(&path, &target, templates, &chain)?;
            }
        } else {
            std::fs::copy(&path, &target)
                .map_err(|e| MutationError::Schema(format!("copy {fname:?}: {e}")))?;
        }
    }
    Ok(())
}

/// The on-disk source directory for `rel_path` of the **expanded** tree.
///
/// A rel-path produced by [`parse_subtree`] may only exist after ref expansion
/// (`child/a` where `child` is a ref), so it cannot be joined onto the template
/// root blindly. This walks it segment by segment, following a ref into its
/// resolved root wherever one is met.
///
/// The returned directory is handed to [`copy_template_tree`] with a fresh
/// chain; the ring it would otherwise miss has already been refused by
/// [`parse_subtree`], which runs before any caller of this reaches staging.
///
/// # Errors
/// Whatever [`ref_target`] reports for a malformed, unresolvable or cyclic
/// reference on the way down.
fn template_dir_for_rel(
    template_root: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
    rel_path: &str,
) -> Result<PathBuf, MutationError> {
    let (mut dir, mut chain) = follow_refs(template_root.to_path_buf(), templates, Vec::new())?;
    for segment in rel_path.split('/').filter(|s| !s.is_empty()) {
        let (next, next_chain) = follow_refs(dir.join(segment), templates, chain)?;
        dir = next;
        chain = next_chain;
    }
    Ok(dir)
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
    /// GH #682, [`DiffMode::Replace`] only: spawnable cells the template names
    /// that stand on disk at another version or with other bytes → replaced
    /// under their own name, the old one renamed beside it. Always empty in
    /// [`DiffMode::Resume`], where such a cell is simply [`Self::existing`].
    pub changed: Vec<ChangedNode>,
    /// GH #682, [`DiffMode::Replace`] only: nodes of the OLD tree — registry
    /// rows under the hive path or cell directories on disk — that the new
    /// template does not name → left standing, disconnected by the new inner
    /// edges (No-Delete). Always empty in [`DiffMode::Resume`].
    pub left: Vec<LeftNode>,
}

/// A child the new template names, standing on disk at another version or with
/// other bytes: replaced under its own name, the old one renamed beside it.
#[derive(Debug, Clone)]
pub struct ChangedNode {
    /// The template's child, verbatim — what the lift instantiates under the
    /// child's own name, together with everything the template names below
    /// it: a changed node is the rename root of its whole branch, and the
    /// partition lists none of its descendants on their own.
    pub node: CellNode,
    /// On-disk final directory the child occupies today (and will occupy again
    /// once the old one is renamed beside it).
    pub final_path: std::path::PathBuf,
    /// The standing child's `cell.provenance.template_version`, or
    /// [`VERSION_UNKNOWN`] when it carries no stamp (an older instantiation) —
    /// the suffix the old directory is renamed with.
    pub from_version: String,
    /// The version the template child is cut from: the last `ref` hop's for a
    /// child that comes in through a ref, the template's own otherwise;
    /// [`VERSION_UNVERSIONED`] when that template declares none.
    pub to_version: String,
}

/// A child the old tree has and the new template does not: left standing,
/// disconnected by the new inner edges (No-Delete).
#[derive(Debug, Clone)]
pub struct LeftNode {
    /// Path relative to the hive, e.g. `gone` or `bump~1.0.0` — the top-most
    /// unnamed node only; whatever stands below it is left with it.
    pub rel_path: String,
    /// Absolute logical path, e.g. `/alex/display/gone`.
    pub abs_path: String,
    /// The node's `cell.provenance.template_version` as it stands on disk, or
    /// [`VERSION_UNKNOWN`] when there is no stamp or no directory (a registry
    /// row alone).
    pub version: String,
}

/// How [`classify_subtree_nodes_in`] reads a template child that already stands
/// on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    /// Per-node subtree resume (Paket-5): what stands is `existing`, full stop.
    /// `changed` and `left` stay empty.
    Resume,
    /// GH #682 `replace_nodes`: what stands is `existing` only when it is what
    /// the template would write again; otherwise `changed`. Nodes the template
    /// does not name are `left`.
    Replace,
}

/// The version label of a standing node that carries no provenance stamp —
/// an instantiation older than GH #62. Version-unknown is never "the same
/// version", so such a child is always [`SubtreePartition::changed`].
pub const VERSION_UNKNOWN: &str = "unknown";

/// The version label of a template (or `ref` hop) that declares no version.
/// Distinct from [`VERSION_UNKNOWN`]: "this template has no version" is a
/// fact, "the version is unknown" is a gap.
pub const VERSION_UNVERSIONED: &str = "unversioned";

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
/// `templates` is handed straight to [`parse_subtree`]: it is the registry a
/// `cell.type: "ref"` sub-unit resolves against — the referenced template's
/// content takes the ref's position (GH #277).
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed, or
/// [`MutationError::TemplateMissing`] / [`MutationError::TemplateRefCycle`] if a
/// `ref` in it names nothing or closes a ring.
pub fn classify_subtree_nodes(
    root: &std::path::Path,
    scope: &str,
    name: &str,
    template_root: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
) -> Result<SubtreePartition, MutationError> {
    classify_subtree_nodes_in(
        root,
        scope,
        name,
        template_root,
        templates,
        DiffMode::Resume,
        &SubtreeOverrides::default(),
        &[],
    )
}

/// [`classify_subtree_nodes`] with a [`DiffMode`] — the GH #682 entry point.
///
/// In [`DiffMode::Resume`] this IS `classify_subtree_nodes`: `overrides` and
/// `registered` are not read, `changed` and `left` come back empty.
///
/// In [`DiffMode::Replace`] every spawnable template child that stands on disk
/// is read once more and sorted into `existing` (kept) or `changed`:
///
/// - **Version.** The standing child's `cell.provenance.template_version` is
///   the `from`; the `to` is the version the template child is cut from — the
///   last `ref` hop's for a child that comes in through a ref (the same hop
///   [`provenance_for`] stamps), the template's own otherwise. A child with no
///   stamp at all is version-unknown, and version-unknown is `changed`. For a
///   ref'd child the stamp must name the same template at the same version;
///   for a child that lives literally in the template the outer version is
///   deliberately NOT compared — it is the version being lifted, so it differs
///   for every child of every lift, and a kept child keeps its old stamp
///   (F1: not written), so the comparison would never again say "same". What
///   such a child is, is its bytes.
/// - **Bytes.** The standing `config.json` minus what instantiation minted
///   (`cell.id`, `cell.provenance`) against the template child's `config.json`
///   rendered the way [`crate::mutation::stage::patch_and_substitute_config`]
///   would write it again: the ref markers' `override_params` and this lift's
///   `overrides` layered onto `params` (top-level key, last wins), the default
///   sandbox block for a cell type that enforces one. A template value that
///   carries an instance token (`${ctx.…}`, `${uuid7:…}`) was rendered per
///   instance and cannot be re-derived here, so the token's span matches
///   whatever stands there — the literal segments around it must stand, in
///   order ([`template_string_matches`], which also states the limit of that).
///   Everything else is compared as JSON values — key order and whitespace
///   are not bytes that instantiation owns.
/// - **Only `config.json` is compared.** A version that changes a child's
///   `seed/` alone, or the declaration of a NESTED hive the child stands
///   under, is invisible here: such a child is kept, and a nested hive that
///   stands is `existing_hives` as before (spec § 2.3 renews the lifted hive's
///   own declaration, not those below it).
/// - **A changed node owns its subtree.** Whatever the template names below a
///   `changed` node — kept, other, new — is neither `existing` nor `missing`
///   nor `changed` on its own, and a hive marker below it is in neither hive
///   list: the changed node is the rename root, and [`ChangedNode::node`]
///   re-instantiates the whole branch (the same top-most rule `left` has).
///
/// `overrides` is the lift's own `with.params` (the `override_params` map of
/// a `swap_nodes`/`replace_nodes` entry, [`SubtreeOverrides::from_add_node`]).
/// A param the original instantiation overrode has no record on disk beyond
/// the merged value, so a lift that does not re-supply it finds the child
/// `changed` — which is right: the instance the lift would write is another
/// one.
///
/// `left` is the union of `registered` — absolute logical paths of the
/// registry rows under the hive, active or inactive; the caller passes what it
/// holds, `&[]` when it holds none — and the cell directories on disk below the
/// hive (a directory with a `config.json`, `seed/` excluded, the same rule
/// [`parse_subtree`] reads a template by), minus every rel-path the template
/// names. Only the top-most unnamed node is listed; a row or directory that
/// already carries a `~<version>` suffix from an earlier lift is simply one
/// more node the template does not name.
///
/// # Errors
/// As [`classify_subtree_nodes`]; additionally [`MutationError::Schema`] when
/// the hive's own directory cannot be walked for `left`.
#[allow(clippy::too_many_arguments)]
pub fn classify_subtree_nodes_in(
    root: &std::path::Path,
    scope: &str,
    name: &str,
    template_root: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
    mode: DiffMode,
    overrides: &SubtreeOverrides,
    registered: &[Path],
) -> Result<SubtreePartition, MutationError> {
    let template = parse_subtree(template_root, templates)?;
    let subtree_root_abs = crate::mutation::resolve_scoped_path(scope, name);

    let hive_set: std::collections::HashSet<&str> =
        template.hives.iter().map(|s| s.as_str()).collect();

    let mut partition = SubtreePartition {
        missing: Vec::new(),
        existing: Vec::new(),
        missing_hives: Vec::new(),
        existing_hives: Vec::new(),
        changed: Vec::new(),
        left: Vec::new(),
    };

    let outer = match mode {
        DiffMode::Resume => None,
        DiffMode::Replace => Some(outer_template_identity(template_root, templates)),
    };

    // GH #682: a changed node owns its subtree — nothing below it is
    // classified on its own. `template.cells` lists a parent before its
    // children ([`collect_cells`] pushes the node, then walks its
    // sub-directories; a ref's content is anchored the same way), so the
    // owners are known by the time a descendant comes up.
    let mut changed_roots: Vec<String> = Vec::new();

    for node in &template.cells {
        if changed_roots
            .iter()
            .any(|owner| is_self_or_rel_descendant(&node.rel_path, owner))
        {
            continue;
        }
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
                if let Some(outer) = &outer
                    && let Some(changed) =
                        version_diff(node, &final_path, outer, &template.ref_overrides, overrides)
                {
                    changed_roots.push(node.rel_path.clone());
                    partition.changed.push(changed);
                    continue;
                }
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

    if mode == DiffMode::Replace {
        partition.left = left_nodes(
            root,
            scope,
            name,
            &template,
            &subtree_root_abs,
            registered,
            &changed_roots,
        )?;
    }

    Ok(partition)
}

/// The `(name, version)` a direct child of `template_root` is stamped with —
/// the registry entry standing at that directory (what [`provenance_of`]
/// reads), else the directory's own `template.json`, else a nameless,
/// versionless template.
///
/// [`provenance_of`]: crate::mutation::stage::provenance_of
fn outer_template_identity(
    template_root: &std::path::Path,
    templates: &crate::templates::TemplatesRegistry,
) -> (String, Option<String>) {
    if let Some(entry) = templates
        .entries_iter()
        .find(|e| e.filesystem_path == template_root)
    {
        return (entry.name.clone(), entry.version.clone());
    }
    match crate::templates::parse_template_json(&template_root.join("template.json")) {
        Ok(t) => (t.name, t.version),
        Err(_) => (String::new(), None),
    }
}

/// The version label of an `Option<String>` version: the version itself, or
/// [`VERSION_UNVERSIONED`].
fn version_label(version: Option<&str>) -> String {
    version.unwrap_or(VERSION_UNVERSIONED).to_string()
}

/// Read a standing cell directory's `config.json`. Lenient like
/// [`read_on_disk_cell_type`]: `None` for a missing/unreadable/unparseable file.
fn read_on_disk_config(cell_dir: &std::path::Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(cell_dir.join("config.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// The provenance stamp a standing `config.json` carries — `None` when there
/// is no `cell.provenance` or it does not parse.
fn provenance_of_config(cfg: &serde_json::Value) -> Option<crate::config::NodeProvenance> {
    let stamp = cfg.get("cell")?.get("provenance")?;
    serde_json::from_value(stamp.clone()).ok()
}

/// GH #682: the verdict for one template child that stands on disk — `Some`
/// when it is `changed`, `None` when it is kept. The rule is the one
/// [`classify_subtree_nodes_in`] documents.
fn version_diff(
    node: &CellNode,
    final_path: &std::path::Path,
    outer: &(String, Option<String>),
    ref_defaults: &HashMap<String, JsonValue>,
    overrides: &SubtreeOverrides,
) -> Option<ChangedNode> {
    let (to_template, to_version) = node.ref_chain.last().cloned().unwrap_or(outer.clone());
    let changed = |from_version: String| ChangedNode {
        node: node.clone(),
        final_path: final_path.to_path_buf(),
        from_version,
        to_version: version_label(to_version.as_deref()),
    };

    let Some(on_disk) = read_on_disk_config(final_path) else {
        return Some(changed(VERSION_UNKNOWN.to_string()));
    };
    let Some(stamp) = provenance_of_config(&on_disk) else {
        return Some(changed(VERSION_UNKNOWN.to_string()));
    };
    let from_version = version_label(stamp.template_version.as_deref());

    // A ref'd child is an instance of the template the ref names, at the
    // version the ref resolved to; another name or version is another child.
    if !node.ref_chain.is_empty()
        && (stamp.template != to_template || stamp.template_version != to_version)
    {
        return Some(changed(from_version));
    }

    let rendered = render_child_like_instantiation(node, ref_defaults, overrides);
    if config_matches_rendered(&on_disk, &rendered) {
        None
    } else {
        Some(changed(from_version))
    }
}

/// The template child's `config.json` as
/// [`crate::mutation::stage::patch_and_substitute_config`] would write it
/// again, minus what cannot be re-derived: the ref markers' and this lift's
/// `override_params` layered onto `params` (top-level key, last wins — and, as
/// there, only when the template has a `params` object at all), the default
/// sandbox block for a cell type that enforces one. `cell.id` and
/// `cell.provenance` are not added; the comparison strips them from the other
/// side instead.
fn render_child_like_instantiation(
    node: &CellNode,
    ref_defaults: &HashMap<String, JsonValue>,
    overrides: &SubtreeOverrides,
) -> serde_json::Value {
    let mut cfg = node.config.clone();
    let mut layer = ref_defaults
        .get(&node.rel_path)
        .cloned()
        .unwrap_or_else(|| JsonValue::Object(Default::default()));
    if let Some(over) = overrides.by_rel_path.get(&node.rel_path) {
        layer_params(&mut layer, over);
    }
    if let Some(params) = cfg.get_mut("params").and_then(|v| v.as_object_mut())
        && let Some(over) = layer.as_object()
    {
        for (k, v) in over {
            params.insert(k.clone(), v.clone());
        }
    }
    let cell_type = cfg
        .get("cell")
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if crate::mutation::stage::sandbox_enforcing(cell_type)
        && let Some(obj) = cfg.as_object_mut()
        && let Some(params) = obj
            .entry("params")
            .or_insert_with(|| JsonValue::Object(Default::default()))
            .as_object_mut()
        && params.get("sandbox").is_none()
    {
        params.insert(
            "sandbox".into(),
            crate::mutation::stage::default_sandbox_block(),
        );
    }
    cfg
}

/// Whether a standing `config.json` is what the rendered template child would
/// be written as: `cell.id` and `cell.provenance` stripped off the standing
/// side, then [`values_match`] over the rest.
fn config_matches_rendered(on_disk: &serde_json::Value, rendered: &serde_json::Value) -> bool {
    let mut on_disk = on_disk.clone();
    if let Some(cell) = on_disk.get_mut("cell").and_then(|c| c.as_object_mut()) {
        cell.remove("id");
        cell.remove("provenance");
    }
    values_match(rendered, &on_disk)
}

/// Structural equality between a template-side value and a standing one, with
/// one tolerance: an instance token (`${ctx.…}`, `${uuid7:…}`) inside a
/// template string was rendered per instance and matches any span of the
/// standing string — the literal segments around it must stand, in order
/// ([`template_string_matches`]). Objects need the same key set, arrays the
/// same length.
fn values_match(template: &serde_json::Value, standing: &serde_json::Value) -> bool {
    match (template, standing) {
        (JsonValue::String(t), JsonValue::String(s)) => template_string_matches(t, s),
        (JsonValue::Object(t), JsonValue::Object(s)) => {
            t.len() == s.len()
                && t.iter()
                    .all(|(k, tv)| s.get(k).is_some_and(|sv| values_match(tv, sv)))
        }
        (JsonValue::Array(t), JsonValue::Array(s)) => {
            t.len() == s.len() && t.iter().zip(s).all(|(tv, sv)| values_match(tv, sv))
        }
        (t, s) => t == s,
    }
}

/// Whether a standing string is a rendering of a template string: the
/// template is split at its instance tokens (`${ctx.…}`, `${uuid7:…}`), and
/// the literal segments have to stand in the standing string in order — the
/// first as prefix, the last as suffix, the ones between somewhere after each
/// other; only the token spans are wildcards. A template without a token has
/// to stand literally. The `$${…}` escape and every environment token
/// (`${VAR}`, `${VAR:-x}`) survive the instance pass literally and so are
/// literal segments here.
///
/// The stated limit: a template that IS one token (`"${ctx.model}"`) matches
/// every standing string, so a template that turned a literal into a token
/// (1.0.0 `"sol"`, 1.1.0 `"${ctx.model}"`) reads as kept when the colony's
/// `ctx.model` happens to render to the same. That is the reproduction the
/// partition can do without the colony's `ctx` — which is the same at
/// instantiation and at the lift, so the token wildcard is the right shape
/// and a `ctx` parameter would buy nothing for the common case.
fn template_string_matches(template: &str, standing: &str) -> bool {
    let segments = literal_segments(template);
    if segments.len() == 1 {
        return segments[0] == standing;
    }
    let last = segments.len() - 1;
    let mut rest = standing;
    for (i, seg) in segments.iter().enumerate() {
        if i == 0 {
            let Some(after) = rest.strip_prefix(seg.as_str()) else {
                return false;
            };
            rest = after;
        } else if i == last {
            return rest.ends_with(seg.as_str());
        } else if let Some(at) = rest.find(seg.as_str()) {
            rest = &rest[at + seg.len()..];
        } else {
            return false;
        }
    }
    true
}

/// The literal segments of a template string, split at its instance tokens
/// (`${ctx.…}`, `${uuid7:…}`) — one segment more than tokens, so a string
/// without a token is one segment: itself. `$${` opens no token, and neither
/// does an environment token or an unclosed `${`.
fn literal_segments(template: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut rest = template;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("$${") {
            current.push_str("$${");
            rest = after;
            continue;
        }
        if let Some(inner) = rest.strip_prefix("${")
            && (inner.starts_with("ctx.") || inner.starts_with("uuid7:"))
            && let Some(close) = inner.find('}')
        {
            segments.push(std::mem::take(&mut current));
            rest = &inner[close + 1..];
            continue;
        }
        current.push(c);
        rest = &rest[c.len_utf8()..];
    }
    segments.push(current);
    segments
}

/// GH #682: the nodes of the old tree the template does not name — see
/// [`classify_subtree_nodes_in`]. `changed_roots` are the rel-paths of the
/// `changed` nodes: whatever stands below one of them, named or not, moves
/// along with its rename and is nobody's `left`.
fn left_nodes(
    root: &std::path::Path,
    scope: &str,
    name: &str,
    template: &SubtreeTemplate,
    subtree_root_abs: &Path,
    registered: &[Path],
    changed_roots: &[String],
) -> Result<Vec<LeftNode>, MutationError> {
    let named: std::collections::HashSet<&str> =
        template.cells.iter().map(|c| c.rel_path.as_str()).collect();
    let mut candidates: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let prefix = format!("{}/", subtree_root_abs.as_str());
    for path in registered {
        if let Some(rel) = path.as_str().strip_prefix(&prefix)
            && !rel.is_empty()
        {
            candidates.insert(rel.to_string());
        }
    }

    let hive_dir = final_path_for(root, scope, name, "");
    let mut on_disk = Vec::new();
    collect_on_disk_cells(&hive_dir, &hive_dir, &mut on_disk)?;
    candidates.extend(on_disk);

    let mut left = Vec::new();
    for rel in &candidates {
        if named.contains(rel.as_str()) {
            continue;
        }
        // Below a changed node: carried along by its rename, not left.
        if changed_roots
            .iter()
            .any(|owner| is_self_or_rel_descendant(rel, owner))
        {
            continue;
        }
        // Only the top-most unnamed node: an ancestor that is itself unnamed
        // carries everything below it.
        let under_a_left_ancestor = rel
            .match_indices('/')
            .any(|(i, _)| !named.contains(&rel[..i]) && candidates.contains(&rel[..i]));
        if under_a_left_ancestor {
            continue;
        }
        let dir = final_path_for(root, scope, name, rel);
        let version = stamped_version(&dir);
        left.push(LeftNode {
            rel_path: rel.clone(),
            abs_path: absolute_for(subtree_root_abs, rel).as_str().to_string(),
            version,
        });
    }
    Ok(left)
}

/// GH #682 — the version a standing cell directory is stamped with: its
/// `cell.provenance.template_version` ([`VERSION_UNVERSIONED`] for a stamp
/// without one), or [`VERSION_UNKNOWN`] when there is no readable stamp or
/// no directory. What a left child and a kept child report in the receipt.
pub(crate) fn stamped_version(dir: &std::path::Path) -> String {
    read_on_disk_config(dir)
        .as_ref()
        .and_then(provenance_of_config)
        .map(|p| version_label(p.template_version.as_deref()))
        .unwrap_or_else(|| VERSION_UNKNOWN.to_string())
}

/// Every cell directory below `dir` (a directory holding a `config.json`,
/// `seed/` and dot-directories excluded), as rel-paths from `hive_dir` — the
/// on-disk mirror of the rule [`collect_cells`] reads a template by. `hive_dir`
/// itself is not listed. A hive that does not stand on disk yields nothing.
fn collect_on_disk_cells(
    hive_dir: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<String>,
) -> Result<(), MutationError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| MutationError::Schema(format!("read_dir {}: {e}", dir.display())))?;
    let mut sub_dirs: Vec<PathBuf> = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| {
            MutationError::Schema(format!("read_dir entry in {}: {e}", dir.display()))
        })?;
        let path = entry.path();
        let skip = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_none_or(|n| n == "seed" || n.starts_with('.'));
        if path.is_dir() && !skip {
            sub_dirs.push(path);
        }
    }
    sub_dirs.sort();
    for sub in sub_dirs {
        if sub.join("config.json").is_file() {
            out.push(relative_path(hive_dir, &sub));
        }
        collect_on_disk_cells(hive_dir, &sub, out)?;
    }
    Ok(())
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
/// This is the RESUME guard, called on the `add_nodes` subtree path only. The
/// `replace_nodes` path (GH #682, [`DiffMode::Replace`]) does not use it: a
/// lift resumes nothing — a `kept` child's task is left exactly as it runs
/// (nothing is written under it), and a `changed` child's old task is stopped
/// through the mutation's own stop mechanics (recompute → peace-stop →
/// death-ack), which is precisely what a running child is allowed to go
/// through. Refusing a lift because its children are awake would refuse
/// every lift of a working hive.
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
/// `templates` is handed straight to [`parse_subtree`]: it is the registry a
/// `cell.type: "ref"` sub-unit resolves against — the referenced template's
/// content takes the ref's position (GH #277).
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed or if any
/// resolved internal edge endpoint escapes the subtree root (containment);
/// [`MutationError::TemplateMissing`] / [`MutationError::TemplateRefCycle`] if a
/// `ref` in the template names nothing or closes a ring.
pub fn resolve_subtree(
    template_root: &std::path::Path,
    scope: &str,
    name: &str,
    templates: &crate::templates::TemplatesRegistry,
) -> Result<ResolvedSubtree, MutationError> {
    let template = parse_subtree(template_root, templates)?;
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
pub(crate) fn resolve_internal_edges(
    template: &SubtreeTemplate,
    subtree_root_abs: &Path,
) -> Result<Vec<ResolvedEdge>, MutationError> {
    let mut internal_edges: Vec<ResolvedEdge> = Vec::new();
    for hive_rel in &template.hives {
        let hive_abs = absolute_for(subtree_root_abs, hive_rel);
        for spec in hive_edges(template, hive_rel) {
            let from = Path::resolve(&hive_abs, &spec.from);
            let to = Path::resolve(&hive_abs, &spec.to);
            // GH #163, GH #267: a template's own lane to one of the colony's
            // three read-only endpoints — `/colony/graph` (topology),
            // `/colony/registry` (its own bookkeeping about its own cells) and
            // `/colony/ledger` (counts) — is in bounds; it addresses the
            // authority, not a cell outside the subtree (see
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
                // GH #283: the phase travels with the edge, like its condition.
                is_default: spec.is_default,
                // GH #559: and so does the lane.
                lane: spec.lane,
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
    /// GH #437: the instantiation activity the `add_nodes` entry declared for
    /// the subtree as a whole. A unit is born whole, so every cell of one
    /// instantiation carries the same value.
    pub birth: crate::mutation::Birth,
    /// GH #62 / GH #277: this node's OWN template identity — the template it is
    /// an instance of, which is the one a bump addresses — plus
    /// [`template_chain`](crate::config::NodeProvenance::template_chain), the
    /// composites that placed it, outermost first. Identical to what the same
    /// node's `config.json` carries under `cell.provenance`.
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
    /// GH #283: the declared routing phase, kept verbatim from the hive config
    /// (`"default": true`). Resolution changes an edge's ENDPOINTS, never what
    /// it means.
    pub is_default: bool,
    /// GH #559: the declared lane, kept verbatim for the same reason — an
    /// instance of a template that declares a v-lane has to BE one.
    pub lane: Option<String>,
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
/// `templates` is handed straight to [`parse_subtree`]: it is the registry a
/// `cell.type: "ref"` sub-unit resolves against — the referenced template's
/// content takes the ref's position (GH #277).
///
/// `factories` answers one question per staged cell: does its type own the
/// schema of its `cell.db` ([`crate::CellFactory::owns_schema`])? A type that
/// does is left to build and seed its own database at first spawn — see
/// [`crate::mutation::stage::seed_cell_db_if_present`] (GH #398).
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed, copied,
/// patched or seeded, or if any resolved edge endpoint escapes the subtree root;
/// [`MutationError::TemplateMissing`] / [`MutationError::TemplateRefCycle`] if a
/// `ref` in the template names nothing or closes a ring.
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
    templates: &crate::templates::TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    pulse: &crate::watchdog::WorkPulse, // GH #439 — one beat per staged cell
    birth: crate::mutation::Birth,      // GH #437 — stamped on every cell of this subtree
) -> Result<StagedSubtree, MutationError> {
    // 1. Parse the template tree (reuse T3).
    let template = parse_subtree(template_root, templates)?;

    // GH #277: whatever the traversed `ref`s parameterised is the default layer;
    // this mutation's own `override_params` sit on top of it, param key by param
    // key. Built once, used for every cell below.
    // GH #516: the ref layer is read off a template tree, so it takes the
    // instance pass here — `ctx` reaches the cells a ref marker parameterises.
    let overrides = overrides.with_ref_defaults(&template.ref_overrides, ctx)?;

    // 2. Copy the whole nested tree into `.staging/<mid>/<name>/`.
    let root_staging_path = root.join(".staging").join(mutation_id).join(name);
    copy_template_tree(template_root, &root_staging_path, templates, &[])?;

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
        // GH #439: same reason as the merge path — one beat per staged cell.
        pulse.tick();
        let cell_staging = staging_dir_for(&root_staging_path, &node.rel_path);
        let abs = absolute_for(&subtree_root_abs, &node.rel_path);

        // Fresh UUID v7 per cell (`cell_id_override = None`) + substitution.
        // The empty `add_node` carries no `override_params` (templated subtree).
        // GH #140: the per-cell override, addressed by the cell's path inside
        // the template. `patch_and_substitute_config` takes an `add_nodes`
        // entry, so the override is handed over in the shape that call already
        // understands — one code path merges params, not two.
        let node_override = overrides.for_cell(&node.rel_path);
        // GH #277: this node's OWN stamp — the same value goes into the file and
        // into the staged meta, so the disk view and the registry index agree.
        let node_provenance = provenance.map(|outer| provenance_for(outer, node));
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
            node_provenance.as_ref(),
            factories,
        )?;
        // Seed inner cells where a `seed/` dir is present — unless the cell type
        // owns its schema, in which case it seeds itself at first spawn (GH #398).
        crate::mutation::stage::seed_cell_db_if_present(&cell_staging, &cell_type, factories)?;

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
                // GH #437: every cell of one instantiation carries the entry's
                // declaration — the declaration addresses the instantiation,
                // and a unit is born whole.
                birth,
                provenance: node_provenance,
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

/// GH #277: the provenance stamp for ONE node of a subtree instance.
///
/// `outer` is the stamp of the instantiation as a whole (the template the
/// mutation named); `node.ref_chain` are the `ref` hops traversed below it to
/// reach this node. The chain is `outer`'s chain followed by those hops —
/// outermost first, the node's own template last — and `template` /
/// `template_version` are that last element: a node is an instance of the
/// template it was cut from, not of the composite that placed it.
/// `instantiated_at` stays the outer's, so one instance carries one timestamp.
///
/// `outer`'s own chain is EXTENDED, not re-derived from its projection: today
/// the outer is always a direct instantiation whose chain is the one hop, so
/// both read the same — but a stamp that already names hops above it must not
/// lose them here.
fn provenance_for(
    outer: &crate::config::NodeProvenance,
    node: &CellNode,
) -> crate::config::NodeProvenance {
    let outer_hop = (outer.template.clone(), outer.template_version.clone());
    let mut chain = outer
        .template_chain
        .clone()
        .unwrap_or_else(|| vec![outer_hop.clone()]);
    chain.extend(node.ref_chain.iter().cloned());
    let (template, template_version) = chain.last().cloned().unwrap_or(outer_hop);
    crate::config::NodeProvenance {
        template,
        template_version,
        template_chain: Some(chain),
        instantiated_at: outer.instantiated_at,
    }
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
pub(crate) fn absolute_for(subtree_root_abs: &Path, rel_path: &str) -> Path {
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

    /// GH #424 — the same map, read off a root-tree `ref` MARKER's top-level
    /// `override_params` instead of an `add_nodes` entry.
    ///
    /// A thin constructor beside [`Self::from_add_node`], and deliberately not
    /// a call to [`Self::with_ref_defaults`]: at boot there is no mutation
    /// layer above the marker, so the marker's own layer IS the layer.
    /// `with_ref_defaults` stays for the mutation path, where both exist and
    /// have to be merged param key by param key.
    pub fn from_ref_marker(override_params: &JsonValue) -> Self {
        let mut by_rel_path = HashMap::new();
        if let Some(obj) = override_params.as_object() {
            for (k, v) in obj {
                by_rel_path.insert(k.clone(), v.clone());
            }
        }
        Self { by_rel_path }
    }

    /// This mutation's `override_params` layered **on top of** `defaults` — a
    /// [`SubtreeTemplate::ref_overrides`] map, already re-addressed to the outer
    /// template's rel-paths (GH #277).
    ///
    /// Merged **param key by param key**, not per cell: a ref parameterises the
    /// template it pulls in, and the mutation that instantiates the outer
    /// template amends that rather than replacing it. Both name the same param
    /// ⇒ the mutation wins; only the ref names it ⇒ it survives.
    ///
    /// Empty `defaults` (every ref-free template) returns this set unchanged, so
    /// such a template stages byte-identically to before.
    ///
    /// GH #516 — `defaults` takes the INSTANCE-CLASS pass first
    /// ([`crate::mutation::substitute::substitute_instance_only`]), and `ctx` is
    /// the only reason this function needs an argument it does not merge. The
    /// two sides of the merge are read from two different places and only one of
    /// them had been substituted: the mutation's own `override_params` arrive
    /// pre-substituted from `substitute_mutation_diff`, while `defaults` is read
    /// off a template TREE, where every other value gets the instance pass in
    /// `patch_and_substitute_config`. Without this a ref marker could hand a
    /// LITERAL down into the template it names but not the outer instantiation's
    /// own `${ctx.X}` — that token survived into the env pass and was refused
    /// there as an environment variable called `ctx.X`. A composite that
    /// references one template twice (a conversation surface and a reasoning
    /// core, both `talky`-shaped, both reading `${ctx.model}`) then had no way to
    /// give the two different models, which is the defect GH #516 was opened for.
    ///
    /// Environment tokens keep surviving literally, exactly as in every other
    /// value read off a template tree (GH #20): a secret a ref marker passes down
    /// is still never materialised on disk.
    ///
    /// # Errors
    /// [`MutationError`] for whatever the instance pass reports about `defaults`
    /// — a `${ctx.X}` the mutation does not supply, or an operator form that can
    /// never resolve. Both are authoring errors in the TEMPLATE, and both are
    /// raised here, before the first byte of the tree is copied.
    pub fn with_ref_defaults(
        &self,
        defaults: &HashMap<String, JsonValue>,
        ctx: &HashMap<String, String>,
    ) -> Result<Self, MutationError> {
        let mut by_rel_path = HashMap::with_capacity(defaults.len());
        for (rel_path, params) in defaults {
            by_rel_path.insert(
                rel_path.clone(),
                crate::mutation::substitute::substitute_instance_only(params, ctx)?,
            );
        }
        for (rel_path, params) in &self.by_rel_path {
            match by_rel_path.get_mut(rel_path) {
                Some(under) => layer_params(under, params),
                None => {
                    by_rel_path.insert(rel_path.clone(), params.clone());
                }
            }
        }
        Ok(Self { by_rel_path })
    }

    /// The synthetic `add_nodes` entry for one cell — the shape
    /// `patch_and_substitute_config` already merges. A cell with no entry gets
    /// an empty object and is byte-identical to the pre-#140 staging.
    ///
    /// `pub(crate)` since GH #682: the lift renders the lifted hive's own
    /// declaration through `patch_and_substitute_config` too, and hands it the
    /// entry addressed at `""` in this same shape.
    pub(crate) fn for_cell(&self, rel_path: &str) -> JsonValue {
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
/// `templates` is handed to [`parse_subtree`] — directly and once more through
/// [`classify_subtree_nodes`]: it is the registry a `cell.type: "ref"` sub-unit
/// resolves against, and the referenced template's content takes the ref's
/// position (GH #277). [`stage_rename_root`] takes it too — its rename-root
/// rel-path is a path of the EXPANDED tree, so the copy has to walk the same
/// refs the parse did.
///
/// `factories` travels the same way and for the same reason as in
/// [`stage_subtree`]: a cell type that owns its schema seeds itself at first
/// spawn instead of being seeded here (GH #398).
///
/// # Errors
/// Returns [`MutationError::Schema`] if the template cannot be parsed, copied,
/// patched or seeded, or if any resolved edge endpoint escapes the subtree root;
/// [`MutationError::TemplateMissing`] / [`MutationError::TemplateRefCycle`] if a
/// `ref` in the template names nothing or closes a ring.
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
    templates: &crate::templates::TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    pulse: &crate::watchdog::WorkPulse, // GH #439 — one beat per staged cell
    birth: crate::mutation::Birth,      // GH #437 — the entry's declared instantiation activity
) -> Result<StagedSubtreeMerge, MutationError> {
    let template = parse_subtree(template_root, templates)?;
    let partition = classify_subtree_nodes(root, scope, name, template_root, templates)?;
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
            templates,
            factories,
            pulse,
            birth,
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
/// fresh UUID-v7 `cell.id` + full substitution, seeds an inner `cell.db` if a
/// seed is present and the cell type does not own its schema (GH #398), and
/// classifies it into spawnable cells vs. hive scope markers — identical
/// per-cell semantics to [`stage_subtree`].
///
/// `pub(crate)` since GH #682: a lift ([`crate::mutation::stage_replace`])
/// stages its added and its changed children in exactly this form — a changed
/// child is the rename root of its whole branch, instantiated under the
/// template name while the old directory is renamed beside it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_rename_root(
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
    templates: &crate::templates::TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    pulse: &crate::watchdog::WorkPulse, // GH #439 — one beat per staged cell
    birth: crate::mutation::Birth,      // GH #437 — stamped on every cell of this root
) -> Result<StagedRenameRoot, MutationError> {
    // Copy the rename-root's template sub-path into staging (drops template.json).
    // GH #277: `root_rel` is a rel-path of the EXPANDED tree, so it may only
    // exist behind a ref — `template_dir_for_rel` walks it the way the parse
    // side did.
    let template_subdir = template_dir_for_rel(template_root, templates, root_rel)?;
    let root_staging_path = if root_rel.is_empty() {
        root.join(".staging").join(mutation_id).join(name)
    } else {
        root.join(".staging")
            .join(mutation_id)
            .join(format!("{name}/{root_rel}"))
    };
    copy_template_tree(&template_subdir, &root_staging_path, templates, &[])?;

    let root_final_path = final_path_for(root, scope, name, root_rel);

    // GH #277: same layering as the fresh path — a subtree that grows a missing
    // branch through a ref gets that ref's params as its default, and (GH #516)
    // the same instance pass over it: a merge stages its missing children
    // exactly like a fresh instantiation, so it substitutes exactly like one.
    let overrides = overrides.with_ref_defaults(&template.ref_overrides, ctx)?;

    let mut cells: Vec<StagedCellMeta> = Vec::new();
    let mut hive_scopes: Vec<Path> = Vec::new();

    // Every template node at or below the rename-root belongs to this fresh
    // sub-tree (a missing node has no existing descendants).
    for node in &template.cells {
        if !is_self_or_rel_descendant(&node.rel_path, root_rel) {
            continue;
        }
        // GH #439: this is where a 65-cell instantiation actually spends its
        // time — a copy, a config rewrite and a seed per cell. Beat before each.
        pulse.tick();
        // Staging dir of this node = root_staging_path + (node.rel_path minus root_rel).
        let sub_rel = rel_under(&node.rel_path, root_rel);
        let cell_staging = staging_dir_for(&root_staging_path, &sub_rel);
        let abs = absolute_for(subtree_root_abs, &node.rel_path);

        // GH #140: same per-cell addressing on the merge path — a subtree that
        // grows a missing branch parameterises it exactly like a fresh one.
        let node_override = overrides.for_cell(&node.rel_path);
        // GH #277: same per-node stamp as the fresh path — a branch that grows
        // in through a ref names the referenced template, not the composite.
        let node_provenance = provenance.map(|outer| provenance_for(outer, node));
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
            node_provenance.as_ref(),
            factories,
        )?;
        crate::mutation::stage::seed_cell_db_if_present(&cell_staging, &cell_type, factories)?;

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
                // GH #437: every cell of one instantiation carries the entry's
                // declaration — the declaration addresses the instantiation,
                // and a unit is born whole.
                birth,
                provenance: node_provenance,
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
pub(crate) fn is_self_or_rel_descendant(rel_path: &str, root_rel: &str) -> bool {
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

        let resolved = resolve_subtree(
            root,
            "/main",
            "m1",
            &crate::templates::TemplatesRegistry::default(),
        )
        .expect("resolve_subtree");

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

        let result = parse_subtree(root, &crate::templates::TemplatesRegistry::default())
            .expect("parse_subtree should succeed");

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

    /// GH #277 Task 2: the registry parameter is threaded through the parser so
    /// a later task can resolve `cell.type: "ref"` sub-units against it. A
    /// template that contains no such ref must parse IDENTICALLY no matter what
    /// the registry holds — the parameter is a capability, not a behaviour
    /// change.
    #[test]
    fn parse_subtree_of_a_ref_free_template_is_unchanged_by_the_registry_parameter() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Same three-cell layout as
        // `parse_subtree_multi_cell_hive_collects_cells_hives_edges`.
        write_json(&root.join("template.json"), r#"{"name":"multi_subtree"}"#);
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
        write_json(
            &root.join("inner_a").join("config.json"),
            r#"{"cell": {"type": "echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &root.join("inner_b").join("config.json"),
            r#"{"cell": {"type": "echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let empty = crate::templates::TemplatesRegistry::default();
        let populated = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "unrelated-id".into(),
                name: "unrelated".into(),
                version: Some("1.0.0".into()),
                filesystem_path: root.join("does_not_exist"),
            },
        ]);

        let with_empty = parse_subtree(root, &empty).expect("parse_subtree (empty registry)");
        let with_entry =
            parse_subtree(root, &populated).expect("parse_subtree (populated registry)");

        let rels = |t: &SubtreeTemplate| -> Vec<String> {
            t.cells.iter().map(|c| c.rel_path.clone()).collect()
        };
        assert_eq!(
            rels(&with_empty),
            rels(&with_entry),
            "cells rel-paths differ"
        );
        assert_eq!(with_empty.hives, with_entry.hives, "hives differ");

        let edges = |t: &SubtreeTemplate| -> Vec<(String, String, Option<String>)> {
            t.edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone(), e.condition.clone()))
                .collect()
        };
        assert_eq!(edges(&with_empty), edges(&with_entry), "edges differ");
        assert_eq!(with_empty.cells.len(), 3, "sanity: three cells parsed");
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

        let result = parse_subtree(root, &crate::templates::TemplatesRegistry::default())
            .expect("parse_subtree should succeed");

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

    /// GH #62 + Korrektur GH #277: every node — hive markers included — is
    /// stamped with the template it is an INSTANCE of. This template declares no
    /// `cell.type: "ref"` anywhere, so for every one of its nodes that template
    /// IS the subtree template, and each chain is that single hop. The test that
    /// separates the two cases is
    /// `a_cell_behind_a_ref_is_stamped_with_the_referenced_template_and_names_the_composite_above_it`.
    #[test]
    fn stage_subtree_stamps_every_node_with_its_own_template_ref_free_that_is_the_subtree_template()
    {
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
            // GH #277: no `ref` anywhere in this template, so every node's chain
            // is the one hop the mutation named.
            template_chain: Some(vec![("sub".into(), Some("3.1.0".into()))]),
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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

        let part = classify_subtree_nodes(
            colony_root,
            "/main",
            "m1",
            &tpl,
            &crate::templates::TemplatesRegistry::default(),
        )
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

        let part = classify_subtree_nodes(
            colony_root,
            "/main",
            "m1",
            &tpl,
            &crate::templates::TemplatesRegistry::default(),
        )
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

        let part = classify_subtree_nodes(
            colony_root,
            "/main",
            "m1",
            &tpl,
            &crate::templates::TemplatesRegistry::default(),
        )
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

        let part = classify_subtree_nodes(
            colony_root,
            "/main",
            "m1",
            &tpl,
            &crate::templates::TemplatesRegistry::default(),
        )
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

        let part = classify_subtree_nodes(
            colony_root,
            "/main",
            "m1",
            &tpl,
            &crate::templates::TemplatesRegistry::default(),
        )
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
        let part = classify_subtree_nodes(
            colony_root,
            scope,
            name,
            tpl,
            &crate::templates::TemplatesRegistry::default(),
        )
        .unwrap();
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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

    // ──────────────────────────────────────────────────────────────────────
    // GH #682: the version diff per child (DiffMode::Replace)
    // ──────────────────────────────────────────────────────────────────────

    /// A child's `config.json` as the `screen` fixture writes it — the same
    /// string for both versions of `keep`, so "byte-identical" is guaranteed
    /// and not merely apparent.
    fn screen_child(contract_version: &str) -> String {
        format!(
            r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/capture"}},"contract":{{"version":"{contract_version}","settings":{{}},"consumes":{{}}}}}}"#
        )
    }

    /// The `screen` class in two versions, as siblings under `library/`, plus a
    /// registry that knows both. 1.0.0 has `keep`, `bump`, `gone`; 1.1.0 has
    /// `keep` (byte-identical), `bump` (contract 1.1.0) and `fresh` — and no
    /// `gone`.
    fn screen_library(
        colony_root: &std::path::Path,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        crate::templates::TemplatesRegistry,
    ) {
        let lib = colony_root.join("library");
        let v100 = lib.join("screen@1.0.0");
        let v110 = lib.join("screen@1.1.0");
        write_json(
            &v100.join("template.json"),
            r#"{"name":"screen","version":"1.0.0"}"#,
        );
        write_json(
            &v100.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":".","to":"./keep"},{"from":"./keep","to":"./bump"},{"from":"./bump","to":"./gone"},{"from":"./gone","to":"."}]}}}"#,
        );
        for child in ["keep", "bump", "gone"] {
            write_json(
                &v100.join(child).join("config.json"),
                &screen_child("1.0.0"),
            );
        }
        write_json(
            &v110.join("template.json"),
            r#"{"name":"screen","version":"1.1.0"}"#,
        );
        write_json(
            &v110.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":".","to":"./keep"},{"from":".","to":"./fresh"},{"from":"./keep","to":"./bump"},{"from":"./bump","to":"."},{"from":"./fresh","to":"."}]}}}"#,
        );
        write_json(
            &v110.join("keep").join("config.json"),
            &screen_child("1.0.0"),
        );
        write_json(
            &v110.join("bump").join("config.json"),
            &screen_child("1.1.0"),
        );
        write_json(
            &v110.join("fresh").join("config.json"),
            &screen_child("1.0.0"),
        );
        let registry = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t-screen-100".into(),
                name: "screen".into(),
                version: Some("1.0.0".into()),
                filesystem_path: v100.clone(),
            },
            crate::templates::TemplateEntry {
                template_id: "t-screen-110".into(),
                name: "screen".into(),
                version: Some("1.1.0".into()),
                filesystem_path: v110.clone(),
            },
        ]);
        (v100, v110, registry)
    }

    /// Grow a subtree instance the way the mutation path does — the real
    /// `stage_subtree` (fresh `cell.id`, substitution, provenance stamp), then
    /// the rename into its final place — so the on-disk children carry exactly
    /// what an instantiation leaves behind, not a hand-written imitation.
    #[allow(clippy::too_many_arguments)]
    fn grow_for_real(
        colony_root: &std::path::Path,
        mutation_id: &str,
        scope: &str,
        name: &str,
        template_root: &std::path::Path,
        template_name: &str,
        template_version: &str,
        ctx: &HashMap<String, String>,
        overrides: &SubtreeOverrides,
        registry: &crate::templates::TemplatesRegistry,
    ) {
        let provenance = crate::config::NodeProvenance {
            template: template_name.to_string(),
            template_version: Some(template_version.to_string()),
            template_chain: Some(vec![(
                template_name.to_string(),
                Some(template_version.to_string()),
            )]),
            instantiated_at: 1_700_000_000,
        };
        let staged = stage_subtree(
            colony_root,
            mutation_id,
            scope,
            name,
            template_root,
            &HashMap::new(),
            ctx,
            Some(&provenance),
            overrides,
            registry,
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("stage_subtree");
        if let Some(parent) = staged.root_final_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::rename(&staged.root_staging_path, &staged.root_final_path).unwrap();
    }

    fn rel_paths(nodes: &[CellNode]) -> Vec<String> {
        nodes.iter().map(|n| n.rel_path.clone()).collect()
    }

    fn existing_paths(nodes: &[ResolvedExistingNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| n.absolute_path.as_str().to_string())
            .collect()
    }

    /// GH #682 (Task 3, the brief's partition): a screen grown at 1.0.0,
    /// partitioned against 1.1.0 in `Replace` mode — `keep` is kept, `bump`
    /// changed 1.0.0 → 1.1.0, `fresh` added, `gone` left.
    #[test]
    fn a_lift_partitions_kept_changed_added_and_left() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let (v100, v110, registry) = screen_library(colony_root);
        grow_for_real(
            colony_root,
            "mid-grow",
            "/alex",
            "display",
            &v100,
            "screen",
            "1.0.0",
            &HashMap::new(),
            &SubtreeOverrides::default(),
            &registry,
        );

        let part = classify_subtree_nodes_in(
            colony_root,
            "/alex",
            "display",
            &v110,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &[],
        )
        .expect("partition in Replace mode");

        assert_eq!(
            existing_paths(&part.existing),
            vec!["/alex/display/keep"],
            "keep stands at the same bytes: kept"
        );
        assert_eq!(
            part.changed.len(),
            1,
            "exactly bump changed: {:?}",
            part.changed
        );
        let bump = &part.changed[0];
        assert_eq!(bump.node.rel_path, "bump");
        assert_eq!(bump.from_version, "1.0.0");
        assert_eq!(bump.to_version, "1.1.0");
        assert_eq!(
            bump.final_path,
            crate::path_truth::resolve_cell_dir(colony_root, "/alex", "display/bump")
        );
        assert_eq!(rel_paths(&part.missing), vec!["fresh"], "fresh is added");
        assert_eq!(part.left.len(), 1, "exactly gone left: {:?}", part.left);
        assert_eq!(part.left[0].rel_path, "gone");
        assert_eq!(part.left[0].abs_path, "/alex/display/gone");
        assert_eq!(part.left[0].version, "1.0.0");
        assert!(part.missing_hives.is_empty());
        assert_eq!(part.existing_hives.len(), 1, "the hive itself stands");
        assert_eq!(
            part.existing_hives[0].absolute_path.as_str(),
            "/alex/display"
        );
    }

    /// GH #682 (orchestrator ruling): a child instantiated unchanged from the
    /// same template version compares EQUAL — the normalisation strips exactly
    /// what instantiation adds. Partitioning the grown 1.0.0 against 1.0.0
    /// itself keeps every child and changes none.
    #[test]
    fn a_child_instantiated_unchanged_compares_equal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let (v100, _v110, registry) = screen_library(colony_root);
        grow_for_real(
            colony_root,
            "mid-grow",
            "/alex",
            "display",
            &v100,
            "screen",
            "1.0.0",
            &HashMap::new(),
            &SubtreeOverrides::default(),
            &registry,
        );
        // The instantiation really did add what the comparison has to strip.
        let keep_cfg: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(
                crate::path_truth::resolve_cell_dir(colony_root, "/alex", "display/keep")
                    .join("config.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(keep_cfg["cell"]["id"].is_string(), "cell.id minted");
        assert_eq!(keep_cfg["cell"]["provenance"]["template_version"], "1.0.0");

        let part = classify_subtree_nodes_in(
            colony_root,
            "/alex",
            "display",
            &v100,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &[],
        )
        .expect("partition against the same version");

        assert_eq!(
            existing_paths(&part.existing),
            vec![
                "/alex/display/bump",
                "/alex/display/gone",
                "/alex/display/keep"
            ],
            "every child is kept"
        );
        assert!(
            part.changed.is_empty(),
            "nothing changed: {:?}",
            part.changed
        );
        assert!(part.missing.is_empty());
        assert!(part.left.is_empty(), "nothing left: {:?}", part.left);
    }

    /// GH #682: the comparison mirrors the rest of what instantiation writes —
    /// an instance token rendered from `ctx`, an `override_params` value merged
    /// into `params`, and the default sandbox block a `code` cell receives. With
    /// the lift re-supplying the same overrides the child is kept; without them
    /// it is changed, because the instance the lift would write is another one.
    /// A child with no provenance stamp is changed with its version unknown.
    #[test]
    fn the_comparison_mirrors_what_instantiation_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let tpl = colony_root.join("library").join("tuned@1.0.0");
        write_json(
            &tpl.join("template.json"),
            r#"{"name":"tuned","version":"1.0.0"}"#,
        );
        write_json(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &tpl.join("worker").join("config.json"),
            r#"{"cell":{"type":"code"},"params":{"model":"${ctx.model}","tick":1,"script_inline":"pass"},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &tpl.join("plain").join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#,
        );
        let registry = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t-tuned".into(),
                name: "tuned".into(),
                version: Some("1.0.0".into()),
                filesystem_path: tpl.clone(),
            },
        ]);
        let mut ctx = HashMap::new();
        ctx.insert("model".to_string(), "sol".to_string());
        let overrides = SubtreeOverrides::from_add_node(
            &serde_json::json!({"override_params": {"worker": {"tick": 5}}}),
        );
        grow_for_real(
            colony_root,
            "mid-grow",
            "/main",
            "t1",
            &tpl,
            "tuned",
            "1.0.0",
            &ctx,
            &overrides,
            &registry,
        );
        let worker_dir = crate::path_truth::resolve_cell_dir(colony_root, "/main", "t1/worker");
        let worker_cfg: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(worker_dir.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(worker_cfg["params"]["model"], "sol", "ctx rendered");
        assert_eq!(worker_cfg["params"]["tick"], 5, "override merged");
        assert!(
            worker_cfg["params"]["sandbox"].is_object(),
            "default sandbox block written"
        );

        let same = classify_subtree_nodes_in(
            colony_root,
            "/main",
            "t1",
            &tpl,
            &registry,
            DiffMode::Replace,
            &overrides,
            &[],
        )
        .expect("partition with the same overrides");
        assert_eq!(
            existing_paths(&same.existing),
            vec!["/main/t1/plain", "/main/t1/worker"],
            "kept: {:?}",
            same.changed
        );
        assert!(same.changed.is_empty());

        let without = classify_subtree_nodes_in(
            colony_root,
            "/main",
            "t1",
            &tpl,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &[],
        )
        .expect("partition without the overrides");
        assert_eq!(existing_paths(&without.existing), vec!["/main/t1/plain"]);
        assert_eq!(without.changed.len(), 1);
        assert_eq!(without.changed[0].node.rel_path, "worker");
        assert_eq!(without.changed[0].from_version, "1.0.0");
        assert_eq!(without.changed[0].to_version, "1.0.0");

        // Strip the stamp off `plain`: an older instantiation without
        // provenance is version-unknown, and version-unknown is changed.
        let plain_cfg_path = crate::path_truth::resolve_cell_dir(colony_root, "/main", "t1/plain")
            .join("config.json");
        let mut plain_cfg: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&plain_cfg_path).unwrap()).unwrap();
        plain_cfg["cell"]
            .as_object_mut()
            .unwrap()
            .remove("provenance");
        fs::write(&plain_cfg_path, plain_cfg.to_string()).unwrap();
        let unstamped = classify_subtree_nodes_in(
            colony_root,
            "/main",
            "t1",
            &tpl,
            &registry,
            DiffMode::Replace,
            &overrides,
            &[],
        )
        .expect("partition with an unstamped child");
        assert_eq!(existing_paths(&unstamped.existing), vec!["/main/t1/worker"]);
        assert_eq!(unstamped.changed.len(), 1);
        assert_eq!(unstamped.changed[0].node.rel_path, "plain");
        assert_eq!(unstamped.changed[0].from_version, VERSION_UNKNOWN);
    }

    /// GH #682: `left` is the union of the registry rows under the hive and the
    /// cell directories on disk that the template does not name — a row without
    /// a directory counts, a directory without a row counts, an earlier lift's
    /// `~<version>` sibling counts, and only the top-most left node is named.
    #[test]
    fn left_reads_registry_rows_and_disk_alike() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let (v100, v110, registry) = screen_library(colony_root);
        grow_for_real(
            colony_root,
            "mid-grow",
            "/alex",
            "display",
            &v100,
            "screen",
            "1.0.0",
            &HashMap::new(),
            &SubtreeOverrides::default(),
            &registry,
        );
        // An earlier lift's renamed sibling, stamped at its old version, with a
        // nested cell below it that must not be named on its own.
        let display_dir = crate::path_truth::resolve_cell_dir(colony_root, "/alex", "display");
        write_json(
            &display_dir.join("bump~0.9.0").join("config.json"),
            r#"{"cell":{"type":"echo_sub","provenance":{"template":"screen","template_version":"0.9.0","instantiated_at":1}},"params":{},"contract":{"version":"0.9.0","settings":{},"consumes":{}}}"#,
        );
        write_json(
            &display_dir
                .join("bump~0.9.0")
                .join("inner")
                .join("config.json"),
            r#"{"cell":{"type":"echo_sub"},"params":{},"contract":{"version":"0.9.0","settings":{},"consumes":{}}}"#,
        );
        // A seed directory is data, never a cell.
        write_json(
            &display_dir.join("keep").join("seed").join("config.json"),
            r#"{}"#,
        );
        let registered = vec![
            Path::new("/alex/display/keep"),
            Path::new("/alex/display/rowonly"),
            Path::new("/alex/display"),
            Path::new("/alex/elsewhere"),
        ];

        let part = classify_subtree_nodes_in(
            colony_root,
            "/alex",
            "display",
            &v110,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &registered,
        )
        .expect("partition");

        let mut left: Vec<(String, String, String)> = part
            .left
            .iter()
            .map(|l| (l.rel_path.clone(), l.abs_path.clone(), l.version.clone()))
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                (
                    "bump~0.9.0".to_string(),
                    "/alex/display/bump~0.9.0".to_string(),
                    "0.9.0".to_string()
                ),
                (
                    "gone".to_string(),
                    "/alex/display/gone".to_string(),
                    "1.0.0".to_string()
                ),
                (
                    "rowonly".to_string(),
                    "/alex/display/rowonly".to_string(),
                    VERSION_UNKNOWN.to_string()
                ),
            ]
        );
    }

    /// GH #682 (fix round 1): only the token spans of a template string are
    /// wildcards — the literal segments around them must stand in order. A
    /// token the whole string consists of matches any rendering (the stated
    /// limit: `ctx` is the colony's and the same at both moments), and the
    /// `$${…}` escape is literal.
    #[test]
    fn a_template_string_matches_on_its_literal_segments() {
        let m = |t: &str, s: &str| values_match(&serde_json::json!(t), &serde_json::json!(s));
        // 1.0.0 `${ctx.model}` → 1.1.0 `${ctx.model}-mini`, disk `sol`: changed.
        assert!(!m("${ctx.model}-mini", "sol"));
        // Positive: the literal suffix stands after the rendered token.
        assert!(m("${ctx.model}-x", "sol-x"));
        assert!(m("a-${uuid7:s}-${ctx.k}-z", "a-0190-v-z"));
        assert!(!m("a-${uuid7:s}-${ctx.k}-z", "b-0190-v-z"));
        assert!(!m("a-${ctx.k}-z", "a-v-y"));
        // The whole string is one token: any rendering, by ruling.
        assert!(m("${ctx.model}", "sol"));
        // Escaped token: literal on both sides, never a wildcard.
        assert!(m("$${ctx.x}", "$${ctx.x}"));
        assert!(!m("$${ctx.x}", "sol"));
        // Environment tokens survive literally and compare literally.
        assert!(m("${API_KEY}", "${API_KEY}"));
        assert!(!m("${API_KEY}", "sol"));
        // No token, no tolerance.
        assert!(!m("sol", "sol-mini"));
    }

    /// GH #682 (fix round 1): a `changed` node owns its subtree — what stands
    /// below it is neither `existing` nor `missing`; the changed node is the
    /// rename root and re-instantiates the whole branch.
    #[test]
    fn a_changed_node_owns_its_subtree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let lib = colony_root.join("library");
        let v100 = lib.join("branch@1.0.0");
        let v110 = lib.join("branch@1.1.0");
        let leaf = r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#;
        for (dir, version, a_params) in [
            (&v100, "1.0.0", r#"{"v":1}"#),
            (&v110, "1.1.0", r#"{"v":2}"#),
        ] {
            write_json(
                &dir.join("template.json"),
                &format!(r#"{{"name":"branch","version":"{version}"}}"#),
            );
            write_json(
                &dir.join("config.json"),
                r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./a/b"}]}}}"#,
            );
            write_json(
                &dir.join("a").join("config.json"),
                &format!(
                    r#"{{"cell":{{"type":"echo"}},"params":{a_params},"contract":{{"version":"1.0.0","settings":{{}},"consumes":{{}}}}}}"#
                ),
            );
            write_json(&dir.join("a").join("b").join("config.json"), leaf);
        }
        write_json(&v110.join("a").join("c").join("config.json"), leaf);
        let registry = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t-branch-100".into(),
                name: "branch".into(),
                version: Some("1.0.0".into()),
                filesystem_path: v100.clone(),
            },
            crate::templates::TemplateEntry {
                template_id: "t-branch-110".into(),
                name: "branch".into(),
                version: Some("1.1.0".into()),
                filesystem_path: v110.clone(),
            },
        ]);
        grow_for_real(
            colony_root,
            "mid-grow",
            "/main",
            "br",
            &v100,
            "branch",
            "1.0.0",
            &HashMap::new(),
            &SubtreeOverrides::default(),
            &registry,
        );
        // What stands below `a` and is not named — a hand-grown `a/old`, an
        // earlier lift's `a/x~0.9.0`, a registry row `a/rowonly` — moves along
        // with the rename of `a` and is nobody's `left`.
        let a_dir = crate::path_truth::resolve_cell_dir(colony_root, "/main", "br/a");
        write_json(&a_dir.join("old").join("config.json"), leaf);
        write_json(&a_dir.join("x~0.9.0").join("config.json"), leaf);

        let part = classify_subtree_nodes_in(
            colony_root,
            "/main",
            "br",
            &v110,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &[Path::new("/main/br/a/rowonly")],
        )
        .expect("partition");

        let changed: Vec<&str> = part
            .changed
            .iter()
            .map(|c| c.node.rel_path.as_str())
            .collect();
        assert_eq!(changed, vec!["a"], "a is the rename root");
        assert!(
            part.existing.is_empty(),
            "a/b stands byte-equal but belongs to a: {:?}",
            existing_paths(&part.existing)
        );
        assert!(
            part.missing.is_empty(),
            "a/c is new but belongs to a: {:?}",
            rel_paths(&part.missing)
        );
        assert!(
            part.left.is_empty(),
            "a/old, a/x~0.9.0 and the row a/rowonly go with a: {:?}",
            part.left
        );
    }

    /// GH #682 (fix round 1): the ref branch of the version rule. A child that
    /// comes in through a `ref` is kept when its stamp names the same hop
    /// (template, version) and the bytes stand; another hop version with the
    /// same bytes is changed, from/to being the hop versions.
    #[test]
    fn a_ref_child_is_judged_by_its_hop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let lib = colony_root.join("library");
        let widget = r#"{"cell":{"type":"echo"},"params":{"k":"v"},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#;
        let mut entries = Vec::new();
        for version in ["1.0.0", "1.1.0"] {
            let dir = lib.join(format!("widget@{version}"));
            write_json(
                &dir.join("template.json"),
                &format!(r#"{{"name":"widget","version":"{version}"}}"#),
            );
            write_json(&dir.join("config.json"), widget);
            entries.push(crate::templates::TemplateEntry {
                template_id: format!("t-widget-{version}"),
                name: "widget".into(),
                version: Some(version.into()),
                filesystem_path: dir,
            });
            let panel = lib.join(format!("panel@{version}"));
            write_json(
                &panel.join("template.json"),
                &format!(r#"{{"name":"panel","version":"{version}"}}"#),
            );
            write_json(
                &panel.join("config.json"),
                r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
            );
            write_json(
                &panel.join("w").join("config.json"),
                &format!(r#"{{"cell":{{"type":"ref","template":"widget@{version}"}}}}"#),
            );
            entries.push(crate::templates::TemplateEntry {
                template_id: format!("t-panel-{version}"),
                name: "panel".into(),
                version: Some(version.into()),
                filesystem_path: panel,
            });
        }
        let registry = crate::templates::TemplatesRegistry::from_entries(entries);
        let panel_100 = lib.join("panel@1.0.0");
        let panel_110 = lib.join("panel@1.1.0");
        grow_for_real(
            colony_root,
            "mid-grow",
            "/main",
            "p",
            &panel_100,
            "panel",
            "1.0.0",
            &HashMap::new(),
            &SubtreeOverrides::default(),
            &registry,
        );
        let w_cfg: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(
                crate::path_truth::resolve_cell_dir(colony_root, "/main", "p/w")
                    .join("config.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(w_cfg["cell"]["provenance"]["template"], "widget");
        assert_eq!(w_cfg["cell"]["provenance"]["template_version"], "1.0.0");

        let same_hop = classify_subtree_nodes_in(
            colony_root,
            "/main",
            "p",
            &panel_100,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &[],
        )
        .expect("partition against the same hop");
        assert_eq!(existing_paths(&same_hop.existing), vec!["/main/p/w"]);
        assert!(same_hop.changed.is_empty());

        let other_hop = classify_subtree_nodes_in(
            colony_root,
            "/main",
            "p",
            &panel_110,
            &registry,
            DiffMode::Replace,
            &SubtreeOverrides::default(),
            &[],
        )
        .expect("partition against the other hop");
        assert!(other_hop.existing.is_empty());
        assert_eq!(other_hop.changed.len(), 1);
        assert_eq!(other_hop.changed[0].node.rel_path, "w");
        assert_eq!(other_hop.changed[0].from_version, "1.0.0");
        assert_eq!(other_hop.changed[0].to_version, "1.1.0");
    }

    /// GH #682: `Resume` mode is the old behaviour byte for byte — a child
    /// standing at other bytes is `existing`, and `changed`/`left` stay empty.
    #[test]
    fn resume_mode_knows_neither_changed_nor_left() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let (v100, v110, registry) = screen_library(colony_root);
        grow_for_real(
            colony_root,
            "mid-grow",
            "/alex",
            "display",
            &v100,
            "screen",
            "1.0.0",
            &HashMap::new(),
            &SubtreeOverrides::default(),
            &registry,
        );

        let resumed = classify_subtree_nodes_in(
            colony_root,
            "/alex",
            "display",
            &v110,
            &registry,
            DiffMode::Resume,
            &SubtreeOverrides::default(),
            &[],
        )
        .expect("partition in Resume mode");
        let plain = classify_subtree_nodes(colony_root, "/alex", "display", &v110, &registry)
            .expect("the old entry point");

        assert_eq!(
            existing_paths(&resumed.existing),
            vec!["/alex/display/bump", "/alex/display/keep"]
        );
        assert_eq!(
            existing_paths(&resumed.existing),
            existing_paths(&plain.existing)
        );
        assert_eq!(rel_paths(&resumed.missing), vec!["fresh"]);
        assert_eq!(rel_paths(&resumed.missing), rel_paths(&plain.missing));
        assert!(resumed.changed.is_empty() && plain.changed.is_empty());
        assert!(resumed.left.is_empty() && plain.left.is_empty());
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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
            &crate::templates::TemplatesRegistry::default(),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
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

    // ──────────────────────────────────────────────────────────────────────
    // GH #277 Task 4: `cell.type: "ref"` resolves in memory
    // ──────────────────────────────────────────────────────────────────────

    const ECHO_CFG: &str =
        r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

    /// Build the `inner` template used by the ref tests and return its root.
    ///
    /// ```text
    /// inner/template.json        {"name":"inner","version":"1.0.0"}
    /// inner/config.json          hive with edge ./a → ./b
    /// inner/a/config.json        echo
    /// inner/b/config.json        echo
    /// ```
    fn write_inner_template(templates_dir: &std::path::Path) -> PathBuf {
        let inner = templates_dir.join("inner");
        write_json(
            &inner.join("template.json"),
            r#"{"name":"inner","version":"1.0.0"}"#,
        );
        write_json(
            &inner.join("config.json"),
            r#"{"cell":{"type":"hive"},
                "params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write_json(&inner.join("a/config.json"), ECHO_CFG);
        write_json(&inner.join("b/config.json"), ECHO_CFG);
        inner
    }

    /// Registry snapshot holding exactly the `inner` template above.
    fn inner_registry(inner: &std::path::Path) -> crate::templates::TemplatesRegistry {
        crate::templates::TemplatesRegistry::from_entries(vec![crate::templates::TemplateEntry {
            template_id: "tmpl-inner".to_string(),
            name: "inner".to_string(),
            version: Some("1.0.0".to_string()),
            filesystem_path: inner.to_path_buf(),
        }])
    }

    /// GH #277 Task 4: a `cell.type: "ref"` directory is not a cell of its own —
    /// the referenced template's whole content takes its position, and every node
    /// that came through the ref carries the ref chain.
    #[test]
    fn parse_subtree_places_the_referenced_templates_content_at_the_ref_position() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);

        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("template.json"),
            r#"{"name":"outer","version":"1.0.0"}"#,
        );
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#,
        );

        let registry = inner_registry(&inner);
        let parsed = parse_subtree(&outer, &registry).expect("parse_subtree");

        let mut rels: Vec<&str> = parsed.cells.iter().map(|c| c.rel_path.as_str()).collect();
        rels.sort();
        assert_eq!(rels, vec!["", "child", "child/a", "child/b"]);
        assert!(
            parsed.hives.contains(&"child".to_string()),
            "the referenced template's root is a hive AT the ref position, hives={:?}",
            parsed.hives
        );

        // The `child` node carries `inner`'s ROOT config, not the ref marker.
        let child = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == "child")
            .expect("child node");
        assert_eq!(child.config["cell"]["type"], "hive");
        assert!(
            child.config.get("params").is_some(),
            "the ref marker must be replaced by inner's config: {:?}",
            child.config
        );

        // The edge is read from inner's config and stays relative to the ref
        // position, so it resolves under `child`.
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.edges[0].from, "./a");
        assert_eq!(parsed.edges[0].to, "./b");
        let resolved = resolve_subtree(&outer, "/main", "m1", &registry).expect("resolve_subtree");
        assert_eq!(
            resolved.internal_edges,
            vec![(
                "/main/m1/child/a".to_string(),
                "/main/m1/child/b".to_string()
            )]
        );

        // The ref chain: empty for what lives literally in the outer template,
        // `[(inner, 1.0.0)]` for everything that came through the ref.
        let chain_of = |rel: &str| {
            parsed
                .cells
                .iter()
                .find(|c| c.rel_path == rel)
                .unwrap_or_else(|| panic!("node {rel:?}"))
                .ref_chain
                .clone()
        };
        let inner_chain = vec![("inner".to_string(), Some("1.0.0".to_string()))];
        assert!(chain_of("").is_empty());
        assert_eq!(chain_of("child"), inner_chain);
        assert_eq!(chain_of("child/a"), inner_chain);
        assert_eq!(chain_of("child/b"), inner_chain);
    }

    /// GH #277 Task 8: a node that came in through a `ref` is an instance of the
    /// REFERENCED template — that is the template a bump addresses — and the
    /// composites it sits inside are recorded above it, outermost first.
    #[test]
    fn a_cell_behind_a_ref_is_stamped_with_the_referenced_template_and_names_the_composite_above_it()
     {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        colony_root_with_root_cell(root, "main");

        let templates_dir = root.join("templates");
        let inner = write_inner_template(&templates_dir);
        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("template.json"),
            r#"{"name":"outer","version":"1.0.0"}"#,
        );
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#,
        );

        let outer_entry = crate::templates::TemplateEntry {
            template_id: "tmpl-outer".to_string(),
            name: "outer".to_string(),
            version: Some("1.0.0".to_string()),
            filesystem_path: outer.clone(),
        };
        let prov = crate::mutation::stage::provenance_of(&outer_entry);
        let staged = stage_subtree(
            root,
            "mid-ref-prov",
            "/main",
            "m1",
            &outer,
            &HashMap::new(),
            &HashMap::new(),
            Some(&prov),
            &SubtreeOverrides::default(),
            &inner_registry(&inner),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("stage_subtree should succeed");

        let outer_hop = ("outer".to_string(), Some("1.0.0".to_string()));
        let inner_hop = ("inner".to_string(), Some("1.0.0".to_string()));

        let a = staged
            .cells
            .iter()
            .find(|c| c.absolute_path.as_str() == "/main/m1/child/a")
            .expect("child/a is staged");
        let a_prov = a.provenance.as_ref().expect("child/a carries a stamp");
        assert_eq!(
            a_prov.template, "inner",
            "a cell behind a ref is an instance of the REFERENCED template: {a_prov:?}"
        );
        assert_eq!(a_prov.template_version.as_deref(), Some("1.0.0"));
        assert_eq!(
            a_prov.template_chain,
            Some(vec![outer_hop.clone(), inner_hop.clone()]),
            "the chain names the composite above it, outermost first, leaf included"
        );

        // The root lives literally in `outer`, so its chain is that one element.
        let root_cfg: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(staged.root_staging_path.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            root_cfg["cell"]["provenance"]["template_chain"],
            serde_json::json!([["outer", "1.0.0"]]),
            "a node instantiated from a ref-free position gets a one-element chain: {root_cfg}"
        );

        // The disk view and the staged meta say the same thing.
        let a_cfg: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(staged.root_staging_path.join("child/a/config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(a_cfg["cell"]["provenance"]["template"], "inner");
        assert_eq!(a_cfg["cell"]["provenance"]["template_version"], "1.0.0");
        assert_eq!(
            a_cfg["cell"]["provenance"]["template_chain"],
            serde_json::json!([["outer", "1.0.0"], ["inner", "1.0.0"]]),
            "on-disk cell.provenance must agree with StagedCellMeta: {a_cfg}"
        );
    }

    /// A ref directory carrying anything besides `config.json` would give one
    /// address two sources — exactly the ambiguity the ref removes.
    #[test]
    fn a_ref_directory_with_a_stray_entry_is_refused_naming_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);

        let outer = templates_dir.join("outer");
        write_json(&outer.join("config.json"), r#"{"cell":{"type":"hive"}}"#);
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#,
        );
        write_json(&outer.join("child/a/config.json"), ECHO_CFG);

        let err = parse_subtree(&outer, &inner_registry(&inner)).expect_err("stray entry");
        let MutationError::Schema(msg) = err else {
            panic!("expected Schema, got {err:?}");
        };
        assert!(
            msg.contains("\"a\""),
            "message must name the stray entry: {msg}"
        );
        assert!(
            msg.contains("config.json"),
            "message must state the rule: {msg}"
        );
    }

    /// Write a template `name` whose root is a hive and whose only child is a
    /// `ref` to `reference`. Unversioned on purpose: the cycle render then reads
    /// as bare names.
    fn write_ref_only_template(
        templates_dir: &std::path::Path,
        name: &str,
        reference: &str,
    ) -> PathBuf {
        let root = templates_dir.join(name);
        write_json(
            &root.join("template.json"),
            &format!(r#"{{"name":"{name}"}}"#),
        );
        write_json(
            &root.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &root.join("child/config.json"),
            &format!(r#"{{"cell":{{"type":"ref","template":"{reference}"}}}}"#),
        );
        root
    }

    /// An unversioned registry entry for `name` pointing at `path`.
    fn unversioned_entry(name: &str, path: &std::path::Path) -> crate::templates::TemplateEntry {
        crate::templates::TemplateEntry {
            template_id: format!("tmpl-{name}"),
            name: name.to_string(),
            version: None,
            filesystem_path: path.to_path_buf(),
        }
    }

    /// GH #277 Task 5 (spec acceptance 6): `x` refs `y`, `y` refs `x` — the
    /// resolution stack catches the second entry into `x` and names the ring.
    ///
    /// The parse root is not itself a registry entry (the chain only records
    /// what was traversed through a ref), so the ring is entered through a third
    /// template — that is also the shape a mutation has: it names ONE template,
    /// and the ring lives below it.
    #[test]
    fn a_ref_cycle_is_refused_naming_the_cycle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let x = write_ref_only_template(&templates_dir, "x", "y");
        let y = write_ref_only_template(&templates_dir, "y", "x");
        let outer = write_ref_only_template(&templates_dir, "outer", "x");

        let registry = crate::templates::TemplatesRegistry::from_entries(vec![
            unversioned_entry("x", &x),
            unversioned_entry("y", &y),
        ]);

        let err = parse_subtree(&outer, &registry).expect_err("a ref cycle must be refused");
        let MutationError::TemplateRefCycle(msg) = err else {
            panic!("expected TemplateRefCycle, got {err:?}");
        };
        assert!(
            msg.contains("x -> y -> x"),
            "the message must render the ring: {msg}"
        );
    }

    /// GH #277 Task 5: a reference that resolves to nothing names itself AND
    /// what the registry does hold under that name — a version typo is the
    /// common case and the list is what makes it obvious.
    #[test]
    fn an_unresolvable_ref_is_refused_naming_the_reference_and_what_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");

        let collector = templates_dir.join("collector");
        write_json(
            &collector.join("template.json"),
            r#"{"name":"collector","version":"2.0.6"}"#,
        );
        write_json(&collector.join("config.json"), ECHO_CFG);

        let outer = templates_dir.join("outer");
        write_json(&outer.join("config.json"), r#"{"cell":{"type":"hive"}}"#);
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"collector@9.9.9"}}"#,
        );

        let registry = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "tmpl-collector".to_string(),
                name: "collector".to_string(),
                version: Some("2.0.6".to_string()),
                filesystem_path: collector,
            },
        ]);

        let err = parse_subtree(&outer, &registry).expect_err("an absent version must be refused");
        let MutationError::TemplateMissing(msg) = err else {
            panic!("expected TemplateMissing, got {err:?}");
        };
        assert!(
            msg.contains("collector@9.9.9"),
            "the message must name the reference: {msg}"
        );
        assert!(
            msg.contains("2.0.6"),
            "the message must name what the registry holds: {msg}"
        );
    }

    /// A malformed `@<version>` is a broken reference, not an absent template:
    /// it names the token and does NOT claim the reference resolves to nothing.
    #[test]
    fn a_ref_with_a_malformed_version_is_refused_naming_the_token() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);

        let outer = templates_dir.join("outer");
        write_json(&outer.join("config.json"), r#"{"cell":{"type":"hive"}}"#);
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@abc"}}"#,
        );

        let err = parse_subtree(&outer, &inner_registry(&inner))
            .expect_err("a malformed version must be refused");
        let MutationError::TemplateMissing(msg) = err else {
            panic!("expected TemplateMissing, got {err:?}");
        };
        assert!(
            msg.contains("\"abc\""),
            "the message must name the malformed token: {msg}"
        );
        assert!(
            !msg.contains("resolves to nothing"),
            "a malformed version is not an absent template: {msg}"
        );
    }

    /// GH #277 Task 5: two ref hops compose — every level is re-anchored under
    /// the ref position above it, and the innermost cells carry BOTH hops.
    #[test]
    fn a_nested_ref_anchors_every_level_and_carries_both_hops() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);

        // `mid` refs `inner` at its own `deep` position.
        let mid = templates_dir.join("mid");
        write_json(
            &mid.join("template.json"),
            r#"{"name":"mid","version":"1.0.0"}"#,
        );
        write_json(
            &mid.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &mid.join("deep/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#,
        );

        // `outer` refs `mid` at its own `child` position.
        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("template.json"),
            r#"{"name":"outer","version":"1.0.0"}"#,
        );
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"mid@1.0.0"}}"#,
        );

        let registry = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "tmpl-inner".to_string(),
                name: "inner".to_string(),
                version: Some("1.0.0".to_string()),
                filesystem_path: inner,
            },
            crate::templates::TemplateEntry {
                template_id: "tmpl-mid".to_string(),
                name: "mid".to_string(),
                version: Some("1.0.0".to_string()),
                filesystem_path: mid,
            },
        ]);

        let parsed = parse_subtree(&outer, &registry).expect("parse_subtree");
        let mut rels: Vec<&str> = parsed.cells.iter().map(|c| c.rel_path.as_str()).collect();
        rels.sort();
        assert_eq!(
            rels,
            vec!["", "child", "child/deep", "child/deep/a", "child/deep/b"]
        );

        let chain_of = |rel: &str| {
            parsed
                .cells
                .iter()
                .find(|c| c.rel_path == rel)
                .unwrap_or_else(|| panic!("node {rel:?}"))
                .ref_chain
                .clone()
        };
        let mid_hop = ("mid".to_string(), Some("1.0.0".to_string()));
        let inner_hop = ("inner".to_string(), Some("1.0.0".to_string()));
        assert!(chain_of("").is_empty());
        assert_eq!(chain_of("child"), vec![mid_hop.clone()]);
        assert_eq!(
            chain_of("child/deep"),
            vec![mid_hop.clone(), inner_hop.clone()]
        );
        assert_eq!(
            chain_of("child/deep/a"),
            vec![mid_hop.clone(), inner_hop.clone()]
        );
        assert_eq!(
            chain_of("child/deep/b"),
            vec![mid_hop.clone(), inner_hop.clone()]
        );

        // GH #277 Task 8: the stamp the staging path derives from that chain —
        // the outer template in front, the innermost one as the leaf, three
        // elements deep.
        let outer_prov = crate::config::NodeProvenance {
            template: "outer".into(),
            template_version: Some("1.0.0".into()),
            template_chain: Some(vec![("outer".into(), Some("1.0.0".into()))]),
            instantiated_at: 1_700_000_000,
        };
        let deep_a = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == "child/deep/a")
            .expect("child/deep/a");
        let stamp = provenance_for(&outer_prov, deep_a);
        assert_eq!(stamp.template, "inner");
        assert_eq!(stamp.template_version.as_deref(), Some("1.0.0"));
        assert_eq!(
            stamp.template_chain,
            Some(vec![
                ("outer".into(), Some("1.0.0".into())),
                mid_hop,
                inner_hop
            ]),
            "two ref hops compose into a three-element chain, outermost first"
        );
        assert_eq!(
            stamp.instantiated_at, 1_700_000_000,
            "one instance, one timestamp"
        );

        // The innermost hive's edge is anchored twice over.
        let resolved = resolve_subtree(&outer, "/main", "m1", &registry).expect("resolve_subtree");
        assert_eq!(
            resolved.internal_edges,
            vec![(
                "/main/m1/child/deep/a".to_string(),
                "/main/m1/child/deep/b".to_string()
            )]
        );
    }

    /// A ref without a usable `cell.template` names nothing to resolve.
    ///
    /// Two shapes, two readers since GH #353: the closed key list
    /// ([`crate::config::CellHeader`]) is now the FIRST reader of every `cell`
    /// block in this walk, so a `template` of the wrong *type* is caught there
    /// — with the file named, which the ref-specific message never did. A
    /// `template` that is simply ABSENT still parses (the key is optional) and
    /// keeps reaching `expand_ref`'s own message.
    #[test]
    fn a_ref_without_a_template_string_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);

        let outer = templates_dir.join("outer");
        write_json(&outer.join("config.json"), r#"{"cell":{"type":"hive"}}"#);
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":7}}"#,
        );

        let err = parse_subtree(&outer, &inner_registry(&inner)).expect_err("no template");
        let MutationError::Schema(message) = &err else {
            panic!("a non-string cell.template is a schema refusal, got {err:?}");
        };
        assert!(
            message.contains("child/config.json") && message.contains("expected a string"),
            "the refusal must name the file and the type violation, got: {message}"
        );

        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref"}}"#,
        );
        let err = parse_subtree(&outer, &inner_registry(&inner)).expect_err("no template");
        assert_eq!(
            err,
            MutationError::Schema("a ref cell must declare cell.template".to_string())
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // GH #277 Task 7: staging copies THROUGH a ref
    // ──────────────────────────────────────────────────────────────────────

    /// Build the `outer` template whose `child/` is a ref to `inner@1.0.0`.
    fn write_outer_ref_template(templates_dir: &std::path::Path) -> PathBuf {
        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("template.json"),
            r#"{"name":"outer","version":"1.0.0"}"#,
        );
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#,
        );
        outer
    }

    /// The config at `path`, with the two per-instance stamps removed, so it can
    /// be compared against the template file it was cut from.
    fn config_without_instance_stamps(path: &std::path::Path) -> String {
        let mut v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        if let Some(cell) = v.get_mut("cell").and_then(|c| c.as_object_mut()) {
            cell.remove("id");
            cell.remove("provenance");
        }
        serde_json::to_string(&v).unwrap()
    }

    /// Collect every `config.json` under `dir`, as parsed JSON.
    fn all_configs(dir: &std::path::Path, out: &mut Vec<(PathBuf, serde_json::Value)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                all_configs(&path, out);
            } else if path
                .file_name()
                .map(|n| n == "config.json")
                .unwrap_or(false)
            {
                let v: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                out.push((path, v));
            }
        }
    }

    /// Every file under `dir`, relative-path strings.
    fn all_files(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                all_files(&path, base, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }

    #[test]
    fn staging_a_ref_produces_the_referenced_templates_files_at_the_ref_position() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let templates_dir = colony_root.join("templates");
        let inner = write_inner_template(&templates_dir);
        let outer = write_outer_ref_template(&templates_dir);

        let staged = stage_subtree(
            colony_root,
            "mid-ref",
            "/main",
            "m1",
            &outer,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
            &inner_registry(&inner),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("staging a ref template must succeed");

        let staging = &staged.root_staging_path;

        // The ref position carries INNER's root config, minus the per-instance
        // stamps the staging mints.
        assert_eq!(
            config_without_instance_stamps(&staging.join("child/config.json")),
            config_without_instance_stamps(&inner.join("config.json")),
            "the ref marker must be replaced by inner's root config"
        );

        // Inner's own children land under the ref position.
        assert!(
            staging.join("child/a/config.json").is_file(),
            "child/a staged"
        );
        assert!(
            staging.join("child/b/config.json").is_file(),
            "child/b staged"
        );

        // No ref marker survives anywhere in the staged tree.
        let mut configs = Vec::new();
        all_configs(staging, &mut configs);
        for (path, cfg) in &configs {
            assert_ne!(
                cfg.get("cell")
                    .and_then(|c| c.get("type"))
                    .and_then(|t| t.as_str()),
                Some("ref"),
                "a ref marker survived staging at {}",
                path.display()
            );
        }

        // `template.json` is stripped at EVERY level, the referenced root included.
        let mut files = Vec::new();
        all_files(staging, staging, &mut files);
        assert!(
            !files.iter().any(|f| f.ends_with("template.json")),
            "no template.json may survive: {files:?}"
        );
    }

    #[test]
    fn merge_staging_a_missing_branch_inside_a_ref_stages_from_the_referenced_template() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let templates_dir = colony_root.join("templates");
        let inner = write_inner_template(&templates_dir);
        let outer = write_outer_ref_template(&templates_dir);

        // Live: subtree root + the ref position + its `b` leaf exist, `a` does not.
        live_cell(colony_root, "/main", "m1", "hive");
        live_cell(colony_root, "/main", "m1/child", "hive");
        live_cell(colony_root, "/main", "m1/child/b", "echo");

        let merged = stage_subtree_merge(
            colony_root,
            "mid-ref-merge",
            "/main",
            "m1",
            &outer,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
            &inner_registry(&inner),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("merge-staging through a ref must succeed");

        assert_eq!(
            merged.rename_roots.len(),
            1,
            "only `child/a` is missing, so it is the sole rename-root: {:?}",
            merged
                .rename_roots
                .iter()
                .map(|r| r.root_final_path.clone())
                .collect::<Vec<_>>()
        );
        let rr = &merged.rename_roots[0];
        assert_eq!(rr.cells.len(), 1);
        assert_eq!(rr.cells[0].absolute_path.as_str(), "/main/m1/child/a");
        assert_eq!(
            rr.root_final_path,
            crate::path_truth::resolve_cell_dir(colony_root, "/main", "m1/child/a")
        );

        // The staged directory carries INNER's `a/config.json`, not something
        // read from the outer template (where `child/a` does not exist at all).
        assert_eq!(
            config_without_instance_stamps(&rr.root_staging_path.join("config.json")),
            config_without_instance_stamps(&inner.join("a/config.json")),
            "the missing branch is staged from the referenced template"
        );
    }

    /// A `seed/` subtree is DATA, and the parser never descends into one. The
    /// copy path must draw the same line: a `config.json` down there that looks
    /// like a ref is a seed row's payload, not a reference — expanding it would
    /// put files on disk that no parsed cell claims.
    #[test]
    fn a_ref_shaped_config_inside_a_seed_directory_is_copied_verbatim() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let templates_dir = colony_root.join("templates");
        let inner = write_inner_template(&templates_dir);

        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(&outer.join("leaf/config.json"), ECHO_CFG);
        let seed_cfg = r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#;
        // Not the `seed/` directory itself but a descendant of it: the whole
        // subtree is data, not just its top level.
        write_json(&outer.join("leaf/seed/fixture/config.json"), seed_cfg);

        let staged = stage_subtree(
            colony_root,
            "mid-seed-ref",
            "/main",
            "m1",
            &outer,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
            &inner_registry(&inner),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("stage_subtree");

        let staged_seed = staged.root_staging_path.join("leaf/seed/fixture");
        assert_eq!(
            fs::read_to_string(staged_seed.join("config.json")).unwrap(),
            seed_cfg,
            "a seed file is copied byte-for-byte, never resolved"
        );
        assert!(
            !staged_seed.join("a").exists() && !staged_seed.join("b").exists(),
            "the referenced template must not be expanded under seed/: {:?}",
            {
                let mut f = Vec::new();
                all_files(&staged.root_staging_path, &staged.root_staging_path, &mut f);
                f.sort();
                f
            }
        );
    }

    /// `template.json` and `README.md` are the descriptor pair of a STANDALONE
    /// template: its registry entry and its page. A ref places the template's
    /// CELLS at its position, not its documentation — the byte copies a ref
    /// replaces never carried either file. The composite's own README, reached
    /// without following anything, still travels (GH #277 Task 10: it is line 1
    /// of the `talky` golden manifest).
    #[test]
    fn a_referenced_templates_own_readme_stays_out_of_the_instance() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let templates_dir = colony_root.join("templates");
        let inner = write_inner_template(&templates_dir);
        fs::write(inner.join("README.md"), "# inner\n").unwrap();

        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &outer.join("child/config.json"),
            r#"{"cell":{"type":"ref","template":"inner@1.0.0"}}"#,
        );
        fs::write(outer.join("README.md"), "# outer\n").unwrap();

        let staged = stage_subtree(
            colony_root,
            "m-readme",
            "/main",
            "m1",
            &outer,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &SubtreeOverrides::default(),
            &inner_registry(&inner),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("stage_subtree");

        assert!(
            staged
                .root_staging_path
                .join("child/a/config.json")
                .is_file(),
            "the referenced template's cells DID travel"
        );
        assert!(
            !staged.root_staging_path.join("child/README.md").exists(),
            "the referenced template's page must not travel"
        );
        assert_eq!(
            fs::read_to_string(staged.root_staging_path.join("README.md")).unwrap(),
            "# outer\n",
            "the instantiated template's OWN page is unaffected"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // GH #277 Task 6: a ref's `override_params` is the default, the mutation's
    // own wins per param key
    // ──────────────────────────────────────────────────────────────────────

    /// An `outer` template whose `child/` is a ref to `inner@1.0.0` carrying
    /// `over_json` as its ref-level `override_params`.
    fn write_outer_ref_with_overrides(templates_dir: &std::path::Path, over_json: &str) -> PathBuf {
        let outer = templates_dir.join("outer");
        write_json(
            &outer.join("template.json"),
            r#"{"name":"outer","version":"1.0.0"}"#,
        );
        write_json(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_json(
            &outer.join("child/config.json"),
            &format!(
                r#"{{"cell":{{"type":"ref","template":"inner@1.0.0"}},"override_params":{over_json}}}"#
            ),
        );
        outer
    }

    /// The ref parameterises the template it pulls in; the mutation that
    /// instantiates the outer template overrides that — **per param key**, not
    /// per cell. `p` comes from the ref and survives, `q` is claimed by both and
    /// the mutation wins.
    #[test]
    fn a_ref_override_is_the_default_and_the_mutation_override_wins_per_key() {
        let tmp = tempfile::TempDir::new().unwrap();
        let colony_root = tmp.path();
        colony_root_with_root_cell(colony_root, "main");
        let templates_dir = colony_root.join("templates");
        let inner = write_inner_template(&templates_dir);
        // `patch_and_substitute_config` merges an override INTO an existing
        // `params` block (`stage.rs`), so the cell under test carries one — as
        // every parameterisable shipped template does.
        write_json(
            &inner.join("a/config.json"),
            r#"{"cell":{"type":"echo"},"params":{"p":0,"q":0},
                "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let outer = write_outer_ref_with_overrides(&templates_dir, r#"{"a":{"p":1,"q":2}}"#);

        let overrides = SubtreeOverrides::from_add_node(&serde_json::json!({
            "override_params": {"child/a": {"q": 99}}
        }));

        let staged = stage_subtree(
            colony_root,
            "mid-ref-over",
            "/main",
            "m1",
            &outer,
            &HashMap::new(),
            &HashMap::new(),
            None,
            &overrides,
            &inner_registry(&inner),
            &Default::default(),
            &crate::watchdog::WorkPulse::silent(),
            crate::mutation::Birth::Active,
        )
        .expect("stage_subtree");

        let cfg_path = staged.root_staging_path.join("child/a/config.json");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(
            v["params"]["p"], 1,
            "the ref's own key survives untouched: {v}"
        );
        assert_eq!(
            v["params"]["q"], 99,
            "the mutation wins on the contested key: {v}"
        );
    }

    /// The collected map speaks the OUTER template's addresses: a key of the
    /// referenced template is re-anchored under the ref's position, and its
    /// root key `""` becomes the ref position itself.
    #[test]
    fn a_ref_overrides_are_re_addressed_to_the_outer_templates_rel_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);
        let outer =
            write_outer_ref_with_overrides(&templates_dir, r#"{"":{"r":1},"a":{"p":1},"b":{}}"#);

        let parsed = parse_subtree(&outer, &inner_registry(&inner)).expect("parse_subtree");
        let mut keys: Vec<&str> = parsed.ref_overrides.keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["child", "child/a", "child/b"]);
        assert_eq!(parsed.ref_overrides["child"], serde_json::json!({"r": 1}));
        assert_eq!(parsed.ref_overrides["child/a"], serde_json::json!({"p": 1}));
    }

    /// A ref-free template collects nothing, so `with_ref_defaults` hands the
    /// mutation's own set straight back — that is why such a template still
    /// stages byte-identically.
    #[test]
    fn a_ref_free_template_collects_no_ref_overrides() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);

        let parsed = parse_subtree(&inner, &crate::templates::TemplatesRegistry::default())
            .expect("parse_subtree");
        assert!(parsed.ref_overrides.is_empty());

        let mutation = SubtreeOverrides::from_add_node(&serde_json::json!({
            "override_params": {"a": {"q": 7}}
        }));
        let layered = mutation
            .with_ref_defaults(&parsed.ref_overrides, &HashMap::new())
            .expect("a ref-free template substitutes nothing");
        assert_eq!(layered.for_cell("a"), mutation.for_cell("a"));
        assert_eq!(layered.for_cell("b"), mutation.for_cell("b"));
    }

    /// A ref-level key that addresses no cell of the referenced template must
    /// not become a silent no-op by a different route than the mutation form —
    /// the same protection GH #140 gives there.
    #[test]
    fn a_ref_override_addressing_no_cell_of_the_referenced_template_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates_dir = tmp.path().join("templates");
        let inner = write_inner_template(&templates_dir);
        let outer = write_outer_ref_with_overrides(&templates_dir, r#"{"nope":{"p":1}}"#);

        let err = parse_subtree(&outer, &inner_registry(&inner)).expect_err("unaddressable key");
        let MutationError::Schema(msg) = err else {
            panic!("expected Schema, got {err:?}");
        };
        assert!(msg.contains("'nope'"), "must name the key: {msg}");
        assert!(
            msg.contains("'a'") && msg.contains("'b'") && msg.contains("(root)"),
            "must list the referenced template's cells: {msg}"
        );
    }
}
