//! GH #682 — `replace_nodes`: the staging half of lifting a standing node in
//! place to another version of its template.
//!
//! ```json
//! {"replace_nodes": [{"match": {"name": "display"},
//!                     "with": {"template": "screen@1.1.0", "params": {...}}}]}
//! ```
//!
//! A lift keeps the node's path and re-reads its template. Against the
//! standing tree the new template's children fall into four classes
//! ([`crate::mutation::subtree::classify_subtree_nodes_in`],
//! [`DiffMode::Replace`]): **kept** (what stands is what the template would
//! write again — untouched, F1), **added** (the template names it, nothing
//! stands — instantiated), **changed** (it stands at another version or with
//! other bytes — instantiated anew under its own name, the old one renamed
//! beside it as `<child>~<from_version>`, whole subtree and identity kept,
//! No-Delete) and **left** (it stands, the template does not name it — left
//! where it is, for the apply to disconnect). The lifted hive's own
//! declaration is rendered afresh from the template as well, under the
//! hive's OLD `cell.id`, and staged beside the children as
//! `config.json.replace` — not as a `config.json`, which a rename would take
//! for a fresh hive.
//!
//! Everything here is pre-destructive: the plan is built under
//! `.staging/<mutation_id>/`, every refusal leaves the live tree
//! byte-identical. A second lift from the same version finds
//! `<child>~<from_version>` taken on disk and takes the next free name
//! (`<child>~<from_version>~2`, see `aside_of`) — the round trip is never
//! blocked by its own history. Applying the
//! plan — renaming the old children aside, renaming the staged ones in,
//! replacing the declaration, re-laying the inner edges, moving the registry
//! rows, recomputing — is the apply arm's, which consumes [`StagedReplace`].
//!
//! Lives beside `stage.rs` rather than in it: that file carries the
//! instantiation machinery this one reuses (`patch_and_substitute_config`,
//! `provenance_of`) and is long enough.

use super::MutationError;
use super::PlannedMove;
use super::stage::{patch_and_substitute_config, provenance_of, read_existing_cell_id};
use super::subtree::{
    ChangedNode, DiffMode, LeftNode, ResolvedEdge, ResolvedExistingHive, ResolvedExistingNode,
    StagedRenameRoot, SubtreeOverrides, SubtreeTemplate, VERSION_UNVERSIONED, absolute_for,
    classify_subtree_nodes_in, is_self_or_rel_descendant, parse_subtree, rename_roots,
    resolve_internal_edges, stage_rename_root, stamped_version,
};
use super::{NodeChange, NodeVerdict};
use crate::templates::TemplatesRegistry;
use meclaw_core::{JsonValue, Path};
use std::collections::HashMap;
use std::path::PathBuf;

/// The staged half of one `replace_nodes` entry — what the apply arm consumes.
///
/// Nothing in here has touched the live tree: the added and changed children
/// stand under [`Self::root_staging_path`], the renames are planned, and the
/// renewed declaration is a file beside them.
#[derive(Debug, Clone)]
pub struct StagedReplace {
    /// Absolute logical path of the lifted node (a hive, usually; a leaf when
    /// the template is a single cell).
    pub absolute_path: Path,
    /// The lifted node's on-disk directory today — and after the lift.
    pub final_path: PathBuf,
    /// `.staging/<mutation_id>/<name>/` — every staged child lies below it,
    /// the renewed declaration in it.
    pub root_staging_path: PathBuf,
    /// The template identity the lift moves the node to — what every
    /// instantiated child is stamped from and the declaration carries.
    pub provenance: crate::config::NodeProvenance,
    /// Children the template names and nothing stands for: each staged whole
    /// (rename-root form, fresh `cell.id`s, seeded), to be renamed in.
    pub added: Vec<StagedRenameRoot>,
    /// Children that stand at another version or with other bytes: the new
    /// instantiation and the rename of the old one beside it, per child.
    pub changed: Vec<StagedChange>,
    /// Children that stand as the template would write them — untouched (F1),
    /// listed so the recompute reaches them.
    pub kept: Vec<ResolvedExistingNode>,
    /// Hive markers below the lifted node that stand — untouched, listed for
    /// the same reason. The lifted hive itself is NOT in here; its renewal is
    /// [`Self::declaration`].
    pub kept_hives: Vec<ResolvedExistingHive>,
    /// Nodes of the old tree the template does not name — left standing, to be
    /// disconnected by the new inner edges. Receipt and recompute material;
    /// nothing is staged for them.
    pub left: Vec<LeftNode>,
    /// The lifted node's own renewed `config.json`, staged as
    /// `config.json.replace`; `None` when the lifted node is the template's
    /// single cell and therefore itself added, changed or kept.
    pub declaration: Option<StagedDeclaration>,
    /// The template's inner edges, resolved to absolute paths and
    /// containment-checked — what the apply lays in place of the old inner
    /// graph. Empty for a leaf template.
    pub internal_edges: Vec<ResolvedEdge>,
    /// GH #682 (OR-P4) — the receipt's material: one entry per child of the
    /// lifted node, sorted by path. Read here, pre-destructively, because a
    /// kept child's version is its stamp on disk and the apply arm has no
    /// other reason to open a kept `config.json`.
    pub changes: Vec<NodeChange>,
}

/// One changed child of a lift: what goes in, and where the old one goes.
#[derive(Debug, Clone)]
pub struct StagedChange {
    /// The old child's `cell.provenance.template_version`, or
    /// [`crate::mutation::subtree::VERSION_UNKNOWN`] — the suffix it is
    /// renamed with.
    pub from_version: String,
    /// The version the new child is cut from.
    pub to_version: String,
    /// The rename of the old child beside itself: `<child>` →
    /// `<child>~<from_version>`, logical path and directory alike, identity
    /// kept. The whole old subtree goes with the directory.
    pub aside: PlannedMove,
    /// The new child, staged whole under its own name (rename-root form) and
    /// aimed at the directory [`Self::aside`] vacates.
    pub fresh: StagedRenameRoot,
}

/// The lifted hive's renewed declaration, rendered from the template's own
/// `config.json` the way an instantiation renders it, under the OLD
/// `cell.id` and the NEW provenance.
#[derive(Debug, Clone)]
pub struct StagedDeclaration {
    /// `<root_staging_path>/config.json.replace`.
    pub staging_path: PathBuf,
    /// `<final_path>/config.json` — the file it replaces.
    pub final_path: PathBuf,
}

/// Stage every `replace_nodes` entry of `diff` under `.staging/<mutation_id>/`.
///
/// Reads the same arguments as
/// [`crate::mutation::stage::build_staging_tree_from_templates`] and is called
/// right after it, before the first rename. A diff with no `replace_nodes`
/// yields an empty vector.
///
/// # Errors
/// [`MutationError::TemplateMissing`] for a `with.template` the registry does
/// not hold; [`MutationError::OperatorParamUnset`] for an `operator_set` param of an
/// added or changed child the entry brings no value for (ADR-0032 — the
/// standing instance's value is not read, so silence would be the shipped
/// default); [`MutationError::Schema`] when the lifted node does not stand on
/// disk, plus whatever instantiating a child reports.
#[allow(clippy::too_many_arguments)]
pub fn stage_replace_nodes(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    diff: &JsonValue,
    templates: &TemplatesRegistry,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    factories: &crate::CellFactoryRegistry,
    pulse: &crate::watchdog::WorkPulse,
) -> Result<Vec<StagedReplace>, MutationError> {
    let mut out = Vec::new();
    let replaces = diff.get("replace_nodes").and_then(|v| v.as_array());
    for r in replaces.into_iter().flatten() {
        pulse.tick();
        let name = r
            .get("match")
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("replace_nodes[].match.name missing".into()))?;
        let with = r
            .get("with")
            .and_then(|v| v.as_object())
            .ok_or_else(|| MutationError::Schema("replace_nodes[].with missing".into()))?;
        let tpl_ref = with
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("replace_nodes[].with.template missing".into()))?;
        let tpl = templates
            .resolve(tpl_ref)
            .map_err(|_| MutationError::TemplateMissing(tpl_ref.into()))?;
        out.push(stage_one_lift(
            root,
            mutation_id,
            scope,
            name,
            tpl,
            tpl_ref,
            with.get("params"),
            env,
            ctx,
            templates,
            factories,
            pulse,
        )?);
    }
    Ok(out)
}

/// One entry: partition, refuse what cannot be lifted, stage the children,
/// render the declaration.
#[allow(clippy::too_many_arguments)]
fn stage_one_lift(
    root: &std::path::Path,
    mutation_id: &str,
    scope: &str,
    name: &str,
    tpl: &crate::templates::TemplateEntry,
    tpl_ref: &str,
    params: Option<&JsonValue>,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    templates: &TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    pulse: &crate::watchdog::WorkPulse,
) -> Result<StagedReplace, MutationError> {
    let template_root = &tpl.filesystem_path;
    let template = parse_subtree(template_root, templates)?;
    let final_path = crate::path_truth::resolve_cell_dir(root, scope, name);
    if !final_path.exists() {
        return Err(MutationError::Schema(format!(
            "replace_nodes[] '{name}': nothing stands at {} to lift — a node that is not \
             there is grown with add_nodes, not replaced",
            final_path.display()
        )));
    }
    refuse_class_mismatch(name, &final_path, &template, tpl_ref)?;
    // Ruling T3-C2: the ONE override set — the entry's `with.params` in the
    // `override_params` shape every door merges — read by the partition (to
    // tell kept from changed) and by the instantiation (to write) alike. On a
    // leaf template the object IS the params and addresses the root.
    let overrides = SubtreeOverrides::from_add_node(&override_entry(params, template.cells.len()));
    // `registered` is empty: staging holds no registry handle. The disk walk
    // covers every node that has a directory, which under No-Delete is every
    // node there is.
    let partition = classify_subtree_nodes_in(
        root,
        scope,
        name,
        template_root,
        templates,
        DiffMode::Replace,
        &overrides,
        &[],
    )?;
    let subtree_root_abs = crate::mutation::resolve_scoped_path(scope, name);
    let hive_set: std::collections::HashSet<&str> =
        template.hives.iter().map(|s| s.as_str()).collect();

    let added_roots = rename_roots(&partition, &subtree_root_abs);
    let changed_roots: Vec<&str> = partition
        .changed
        .iter()
        .map(|c| c.node.rel_path.as_str())
        .collect();
    refuse_unset_operator_params(
        &template,
        tpl_ref,
        params,
        added_roots.iter().map(|(rel, _)| rel.as_str()),
        changed_roots.iter().copied(),
    )?;

    let provenance = provenance_of(tpl);
    let stage = |root_rel: &str| {
        stage_rename_root(
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
            Some(&provenance),
            &overrides,
            templates,
            factories,
            pulse,
            crate::mutation::Birth::Active,
        )
    };
    let mut added = Vec::with_capacity(added_roots.len());
    for (rel, _is_hive) in &added_roots {
        added.push(stage(rel)?);
    }
    let mut changed = Vec::with_capacity(partition.changed.len());
    for c in &partition.changed {
        let fresh = stage(&c.node.rel_path)?;
        changed.push(StagedChange {
            from_version: c.from_version.clone(),
            to_version: c.to_version.clone(),
            aside: aside_of(&subtree_root_abs, c),
            fresh,
        });
    }

    let root_staging_path = root.join(".staging").join(mutation_id).join(name);
    let declaration = if hive_set.contains("") {
        Some(stage_declaration(
            &root_staging_path,
            &final_path,
            template_root,
            &overrides,
            &provenance,
            env,
            ctx,
            factories,
        )?)
    } else {
        None
    };
    let internal_edges = resolve_internal_edges(&template, &subtree_root_abs)?;
    // The lifted hive itself stands in `existing_hives` (its directory is
    // there); its renewal is the declaration above, not a kept marker.
    let kept_hives = partition
        .existing_hives
        .into_iter()
        .filter(|h| h.absolute_path != subtree_root_abs)
        .collect();
    let changes = node_changes(
        &template,
        templates,
        &subtree_root_abs,
        &final_path,
        &provenance,
        &added_roots,
        &changed,
        &partition.existing,
        &partition.left,
    );

    Ok(StagedReplace {
        absolute_path: subtree_root_abs,
        final_path,
        root_staging_path,
        provenance,
        added,
        changed,
        kept: partition.existing,
        kept_hives,
        left: partition.left,
        declaration,
        internal_edges,
        changes,
    })
}

/// GH #682 (OR-P4) — the receipt's entries for one lift, sorted by path.
///
/// One entry per node the partition acted on: every kept cell (at any
/// depth), every changed root, every added root (a whole grown branch is one
/// entry, as it is one rename), every left root. A kept hive marker is not
/// an entry — the lift never compared it, only walked through it to the
/// cells below, and those are listed. The versions follow the partition's
/// own rule: a standing child's is its provenance stamp
/// ([`stamped_version`]), a template child's is the last `ref` hop's or the
/// lifted template's own. A replaced entry also names its park path and
/// whether a `cell.db` went with it (GH #773) — read off the staged move, so
/// the path is the one the apply renames to, `~<n>` suffix included.
#[allow(clippy::too_many_arguments)]
fn node_changes(
    template: &SubtreeTemplate,
    templates: &TemplatesRegistry,
    subtree_root_abs: &Path,
    hive_dir: &std::path::Path,
    provenance: &crate::config::NodeProvenance,
    added_roots: &[(String, bool)],
    changed: &[StagedChange],
    kept: &[ResolvedExistingNode],
    left: &[LeftNode],
) -> Vec<NodeChange> {
    let abs = |rel: &str| absolute_for(subtree_root_abs, rel).as_str().to_string();
    let template_version = |rel: &str| -> String {
        template
            .cells
            .iter()
            .find(|n| n.rel_path == rel)
            .and_then(|n| n.ref_chain.last().map(|(_, v)| v.clone()))
            .unwrap_or_else(|| provenance.template_version.clone())
            .unwrap_or_else(|| VERSION_UNVERSIONED.to_string())
    };
    // GH #811: the row a `name@version` stands for in the library this lift
    // resolved against. `resolve` for a pinned version, the unversioned entry
    // otherwise — the same two answers a reference gets.
    let row_of = |name: &str, version: Option<&str>| -> Option<String> {
        match version {
            Some(v) => templates.resolve(&format!("{name}@{v}")).ok(),
            None => templates
                .entries_iter()
                .find(|e| e.name == name && e.version.is_none()),
        }
        .map(|e| e.template_id.clone())
    };
    let stamped_row = |dir: &std::path::Path| -> Option<String> {
        crate::mutation::subtree::stamped_template(dir)
            .and_then(|(name, version)| row_of(&name, version.as_deref()))
    };
    // What a child of the NEW version is cut from: the last `ref` hop that
    // placed it, the lifted template itself for an inline child — the same
    // rule `provenance_for` stamps it with.
    let new_row = |rel: &str| -> Option<String> {
        let hop = template
            .cells
            .iter()
            .find(|n| n.rel_path == rel)
            .and_then(|n| n.ref_chain.last().cloned())
            .unwrap_or_else(|| {
                (
                    provenance.template.clone(),
                    provenance.template_version.clone(),
                )
            });
        row_of(&hop.0, hop.1.as_deref())
    };
    let rel_of = |abs_path: &str| -> String {
        abs_path
            .strip_prefix(subtree_root_abs.as_str())
            .map(|r| r.trim_start_matches('/').to_string())
            .unwrap_or_default()
    };
    let mut out: Vec<NodeChange> = Vec::new();
    for k in kept {
        let version = stamped_version(&k.final_path);
        out.push(NodeChange {
            path: k.absolute_path.as_str().to_string(),
            verdict: NodeVerdict::Kept,
            from_version: Some(version.clone()),
            to_version: Some(version),
            parked_path: None,
            parked_store: None,
            from_template_id: stamped_row(&k.final_path),
            to_template_id: stamped_row(&k.final_path),
        });
    }
    for c in changed {
        out.push(NodeChange {
            path: c.aside.from.as_str().to_string(),
            verdict: NodeVerdict::Replaced,
            from_version: Some(c.from_version.clone()),
            to_version: Some(c.to_version.clone()),
            parked_path: Some(c.aside.to.as_str().to_string()),
            parked_store: Some(holds_a_store(&c.aside.from_dir)),
            from_template_id: stamped_row(&c.aside.from_dir),
            to_template_id: new_row(&rel_of(c.aside.from.as_str())),
        });
    }
    for (rel, _is_hive) in added_roots {
        out.push(NodeChange {
            path: abs(rel),
            verdict: NodeVerdict::Added,
            from_version: None,
            to_version: Some(template_version(rel)),
            parked_path: None,
            parked_store: None,
            from_template_id: None,
            to_template_id: new_row(rel),
        });
    }
    for l in left {
        out.push(NodeChange {
            path: l.abs_path.clone(),
            verdict: NodeVerdict::Left,
            from_version: Some(l.version.clone()),
            to_version: None,
            parked_path: None,
            parked_store: None,
            from_template_id: stamped_row(&hive_dir.join(&l.rel_path)),
            to_template_id: None,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// GH #773 — whether a standing directory (the whole subtree a replaced child
/// takes aside) holds a `cell.db` anywhere. The database is created lazily
/// (store, llm, timer, vault, mcp, subcolony, anything seeded), so only the
/// disk can say. Read at staging, before the rename, like the kept versions;
/// a cell that first wakes between staging and rename is reported `false` —
/// that window lies inside one mutation on the colony task (OR-T3). An
/// unreadable directory is `false`.
fn holds_a_store(dir: &std::path::Path) -> bool {
    if dir.join("cell.db").is_file() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .any(|e| holds_a_store(&e.path()))
}

/// Ruling T4-C4: the standing node and the template have to be of one class
/// — hive to hive, leaf to leaf. A hive "lifted" to a leaf template would
/// plan its whole tree aside as `changed` and put a single cell in its place;
/// a leaf lifted to a hive template would render a hive declaration over a
/// cell. Neither is a lift, and both are refused here, where the two classes
/// are first known together, before a byte is staged.
///
/// # Errors
/// [`MutationError::Schema`] naming both classes.
fn refuse_class_mismatch(
    name: &str,
    final_path: &std::path::Path,
    template: &SubtreeTemplate,
    tpl_ref: &str,
) -> Result<(), MutationError> {
    let standing_is_hive = std::fs::read_to_string(final_path.join("config.json"))
        .ok()
        .and_then(|raw| meclaw_core::serde_json::from_str::<JsonValue>(&raw).ok())
        .and_then(|cfg| {
            cfg.get("cell")
                .and_then(|c| c.get("type"))
                .and_then(|t| t.as_str())
                .map(|t| t == "hive")
        })
        .unwrap_or(false);
    let template_is_hive = template.hives.iter().any(|h| h.is_empty());
    if standing_is_hive == template_is_hive {
        return Ok(());
    }
    let class = |hive: bool| if hive { "a hive" } else { "a single cell" };
    Err(MutationError::Schema(format!(
        "replace_nodes[] '{name}': {} stands at {}, but '{tpl_ref}' is {} — a lift keeps \
         the node's class. Nothing was written.",
        class(standing_is_hive),
        final_path.display(),
        class(template_is_hive)
    )))
}

/// The entry's `with.params` in the `override_params` shape
/// [`SubtreeOverrides::from_add_node`] reads: path-keyed on a subtree
/// template, and on a leaf template — where the object IS the params — keyed
/// at the root `""`.
fn override_entry(params: Option<&JsonValue>, cells: usize) -> JsonValue {
    let mut entry = meclaw_core::serde_json::Map::new();
    if let Some(p) = params {
        let addressed = if cells > 1 {
            p.clone()
        } else {
            let mut m = meclaw_core::serde_json::Map::new();
            m.insert(String::new(), p.clone());
            JsonValue::Object(m)
        };
        entry.insert("override_params".into(), addressed);
    }
    JsonValue::Object(entry)
}

/// GH #661 (ADR-0032) at this door: every template cell that is about to be
/// instantiated — at or below an added or a changed root — must have a value
/// for each of its `operator_set` params, from the entry or from a `ref`
/// marker on the way in. The standing instance's value is not read (it is
/// merged into `params` on disk, not recorded as an override), so a param the
/// entry does not name would silently become the shipped default.
///
/// The same [`crate::mutation::subtree::check_operator_set_params`] the
/// `add_nodes` and `swap_nodes` doors ask, collected and joined the way the
/// swap door joins them.
fn refuse_unset_operator_params<'a>(
    template: &SubtreeTemplate,
    tpl_ref: &str,
    params: Option<&JsonValue>,
    added_roots: impl Iterator<Item = &'a str>,
    changed_roots: impl Iterator<Item = &'a str>,
) -> Result<(), MutationError> {
    let roots: Vec<&str> = added_roots.chain(changed_roots).collect();
    let flat = template.cells.len() == 1;
    let mut unset = Vec::new();
    for cell in &template.cells {
        if !roots
            .iter()
            .any(|r| is_self_or_rel_descendant(&cell.rel_path, r))
        {
            continue;
        }
        let (cell_key, cell_params) = if flat {
            (None, params)
        } else {
            (
                Some(cell.rel_path.as_str()),
                params.and_then(|p| p.get(&cell.rel_path)),
            )
        };
        crate::mutation::subtree::check_operator_set_params(
            cell,
            cell_key,
            tpl_ref,
            cell_params,
            template.ref_overrides.get(&cell.rel_path),
            &mut unset,
        );
    }
    if unset.is_empty() {
        return Ok(());
    }
    Err(MutationError::OperatorParamUnset(
        unset
            .iter()
            .map(MutationError::message)
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

/// Where the old child goes: `<child>~<from_version>` beside it, as a
/// logical path and as a directory — and when that name is taken on disk
/// (a lift from this version already happened here), `<child>~<from_version>~<n>`
/// with the smallest free `n >= 2`, so the round trip 1.0.0 → 1.1.0 → 1.0.0 →
/// 1.1.0 is never blocked by its own history (Spec OR-P2: the suffix scheme
/// is a constant). This is the ONE place the name is chosen; the apply
/// front checks the chosen name against the registry. The marker is
/// [`crate::mutation::validate::PARKED_MARKER`], which no operator-given name
/// may carry — so a `~` on a path always means "parked".
fn aside_of(subtree_root_abs: &Path, changed: &ChangedNode) -> PlannedMove {
    let from = if changed.node.rel_path.is_empty() {
        subtree_root_abs.clone()
    } else {
        crate::mutation::resolve_scoped_path(subtree_root_abs.as_str(), &changed.node.rel_path)
    };
    let from_dir = changed.final_path.clone();
    let base = from_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let marker = crate::mutation::validate::PARKED_MARKER;
    let first = format!("{base}{marker}{}", changed.from_version);
    let mut suffixed = first.clone();
    let mut n = 2u32;
    while from_dir.with_file_name(&suffixed).exists() {
        suffixed = format!("{first}{marker}{n}");
        n += 1;
    }
    let to_dir = from_dir.with_file_name(&suffixed);
    let to = Path::new(&format!("{}/{suffixed}", from.parent().as_str()));
    PlannedMove {
        from,
        to,
        from_dir,
        to_dir,
    }
}

/// The lifted hive's renewed declaration: the template's own `config.json`
/// rendered through the one instantiation path (`patch_and_substitute_config`
/// — instance substitution, the entry's params addressed at `""`, the new
/// provenance), then the minted `cell.id` put back to the hive's OLD one, and
/// the file moved to `config.json.replace` so no rename mistakes it for a
/// fresh hive.
///
/// A hive that carries no `cell.id` (placed by hand, never instantiated) has
/// none to keep; the minted one stands.
#[allow(clippy::too_many_arguments)]
fn stage_declaration(
    root_staging_path: &std::path::Path,
    final_path: &std::path::Path,
    template_root: &std::path::Path,
    overrides: &SubtreeOverrides,
    provenance: &crate::config::NodeProvenance,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    factories: &crate::CellFactoryRegistry,
) -> Result<StagedDeclaration, MutationError> {
    std::fs::create_dir_all(root_staging_path)
        .map_err(|e| MutationError::Schema(format!("create lift staging: {e}")))?;
    let rendered = root_staging_path.join("config.json");
    std::fs::copy(template_root.join("config.json"), &rendered).map_err(|e| {
        MutationError::Schema(format!(
            "replace_nodes: read the template's config.json at {}: {e}",
            template_root.display()
        ))
    })?;
    patch_and_substitute_config(
        root_staging_path,
        env,
        ctx,
        &overrides.for_cell(""),
        Some(provenance),
        factories,
    )?;
    if let Some(old_id) = read_existing_cell_id(final_path) {
        let raw = std::fs::read_to_string(&rendered)
            .map_err(|e| MutationError::Schema(format!("read rendered declaration: {e}")))?;
        let mut cfg: JsonValue = meclaw_core::serde_json::from_str(&raw)
            .map_err(|e| MutationError::Schema(format!("parse rendered declaration: {e}")))?;
        if let Some(cell) = cfg.get_mut("cell").and_then(|c| c.as_object_mut()) {
            cell.insert("id".into(), JsonValue::String(old_id.to_string()));
        }
        std::fs::write(
            &rendered,
            meclaw_core::serde_json::to_string_pretty(&cfg)
                .map_err(|e| MutationError::Schema(format!("serialize declaration: {e}")))?,
        )
        .map_err(|e| MutationError::Schema(format!("write renewed declaration: {e}")))?;
    }
    let staging_path = root_staging_path.join("config.json.replace");
    std::fs::rename(&rendered, &staging_path)
        .map_err(|e| MutationError::Schema(format!("stage config.json.replace: {e}")))?;
    Ok(StagedDeclaration {
        staging_path,
        final_path: final_path.join("config.json"),
    })
}
