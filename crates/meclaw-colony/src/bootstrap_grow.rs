//! GH #424 — the first boot fulfils the `ref` markers its root tree declares.
//!
//! WHAT THIS IS
//! ============
//! Ruling R4: a root tree that references a composite template grows itself on
//! the first `meclaw --root` start, through the very resolution and staging a
//! mutation takes. This module is that growth, and it is deliberately thin:
//! everything below `stage_subtree` is the mutation path, unchanged. What lives
//! here is only the part a mutation does not need — turning a marker DIRECTORY
//! into the tree it names, in an order that leaves a readable state at every
//! moment.
//!
//! THE MARKER CONSUMES ITSELF
//! ==========================
//! A marker is a declaration of what shall stand here. Replacing it with what
//! it names is its FULFILMENT, not a deletion — and it is why two questions the
//! design sketch left open need no bookkeeping at all:
//!
//! * **Idempotence over reboots**: after the growth there is no marker, so the
//!   second boot plans no growth. No `mutation_id` ledger, no "already grown"
//!   flag.
//! * **A removed node stays removed**: `remove_nodes` unhooks a cell, and no
//!   standing declaration is left anywhere to demand it back.
//!
//! The No-Delete-Policy is untouched by this: it protects CELL STATE, and a
//! marker has none — no `cell.db`, no `cell_id`, no registry row. Nothing is
//! moved and nothing is lost.
//!
//! THE ORDER, AND WHY
//! ==================
//! 1. `stage_subtree` builds the whole tree under `{root}/.staging/boot-<id>/`
//!    — fresh UUIDs, substitution, seeds: everything a mutation does.
//! 2. every CHILD directory is `rename(2)`d into the marker directory;
//! 3. LAST, the staged `config.json` is renamed over the marker's own.
//!
//! Step 3 is the commit point, and `rename(2)` over an existing file is atomic
//! on POSIX. A process that dies before it leaves the marker standing:
//! `bootstrap_in_flight` is in `colony.db`, the next boot classifies as
//! FirstBoot, and `classify_subtree_nodes` skips exactly the children that
//! already lie there.

use crate::bootstrap::{BootstrapError, BootstrapErrors, PlannedGrowth};
use crate::mutation::MutationError;

/// Materialise every planned growth, in walk order.
///
/// One `stage_subtree` per marker, then the rename sequence above. Returns the
/// first refusal as a `BootstrapErrors` — a boot that cannot fulfil a
/// declaration must not start half a tree.
///
/// GH #437: the meclaw paths of the growth roots that declared
/// `birth: "inactive"` travel back to the caller. The re-plan that follows a
/// growth has no memory of the marker (the marker consumed itself), so the
/// declaration has to be carried by hand across that boundary.
pub(crate) fn grow_planned_refs(
    root: &std::path::Path,
    growths: &[PlannedGrowth],
    templates: &crate::templates::TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    env_path: Option<&std::path::Path>,
) -> Result<Vec<meclaw_core::Path>, BootstrapErrors> {
    let env_file = env_path
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| root.join(".env"));
    let env = crate::env_file::load_env(&env_file).unwrap_or_default();
    let ctx = std::collections::HashMap::new();

    let mut born_inactive = Vec::new();
    for g in growths {
        if let Err(e) = grow_one(root, g, templates, factories, &env, &ctx) {
            let mut errors = BootstrapErrors::new();
            errors.push(e);
            return Err(errors);
        }
        if g.birth == crate::mutation::Birth::Inactive {
            born_inactive.push(g.path.clone());
        }
    }
    Ok(born_inactive)
}

fn grow_one(
    root: &std::path::Path,
    g: &PlannedGrowth,
    templates: &crate::templates::TemplatesRegistry,
    factories: &crate::CellFactoryRegistry,
    env: &std::collections::HashMap<String, String>,
    ctx: &std::collections::HashMap<String, String>,
) -> Result<(), BootstrapError> {
    let entry = templates.resolve(&g.reference).map_err(|e| {
        growth_error(
            g,
            &MutationError::TemplateMissing(format!("{}: {e}", g.reference)),
        )
    })?;
    let provenance = crate::mutation::stage::provenance_of(entry);

    // The marker's path split into the scope it hangs in and its own last
    // segment — the two `stage_subtree` addresses a subtree by.
    let (scope, name) = split_scope_and_name(&g.path);

    let mutation_id = format!("boot-{}", meclaw_core::Uuid::now_v7());
    let staged = crate::mutation::subtree::stage_subtree(
        root,
        &mutation_id,
        &scope,
        &name,
        &entry.filesystem_path,
        env,
        ctx,
        Some(&provenance),
        &crate::mutation::subtree::SubtreeOverrides::from_ref_marker(&g.override_params),
        templates,
        factories,
        // GH #439: the boot growth runs BEFORE the watchdog is armed, so there
        // is nothing to beat to.
        &crate::watchdog::WorkPulse::silent(),
        // GH #437: the marker's own declaration, stamped on every cell of the
        // tree it grows.
        g.birth,
    )
    .map_err(|e| growth_error(g, &e))?;

    consume_marker(&staged.root_staging_path, &g.fs_path).map_err(|reason| {
        BootstrapError::GrowthFailed {
            path: g.fs_path.clone(),
            reference: g.reference.clone(),
            reason,
        }
    })
}

/// Replace the marker directory's content with the staged tree.
///
/// A MERGE, not an overwrite, and that is the whole subtlety of the nesting
/// case: the template brings containers (`orgs/`), and the operator may have
/// written a deeper declaration inside one of them (`orgs/acme/config.json`).
/// Those two are not in conflict — they address different things. What IS a
/// conflict is one ADDRESS with two sources: the template and the root tree
/// both claiming the same leaf.
///
/// The rule, applied per directory level:
///
/// * the target has nothing by that name — rename the whole staged subtree in;
/// * both sides are directories — recurse, because a container is a place, not
///   a claim;
/// * anything else — a named refusal, raised BEFORE any rename at that level.
///
/// `config.json` is handled last at every level, and at the marker root it is
/// the COMMIT POINT: `rename(2)` over an existing file is atomic on POSIX, so
/// the marker is a marker until that call returns and the grown node
/// afterwards. Below the marker root a `config.json` that is already there is
/// the two-sources case, not a fulfilment.
fn consume_marker(staging: &std::path::Path, marker_dir: &std::path::Path) -> Result<(), String> {
    merge_level(staging, marker_dir, true)
}

fn merge_level(
    staging: &std::path::Path,
    target: &std::path::Path,
    is_marker_root: bool,
) -> Result<(), String> {
    let rd = std::fs::read_dir(staging)
        .map_err(|e| format!("staged tree unreadable at {}: {e}", staging.display()))?;

    // Two passes: decide everything first, act afterwards. A refusal must not
    // leave half a level merged.
    let mut renames: Vec<std::ffi::OsString> = Vec::new();
    let mut recurse: Vec<std::ffi::OsString> = Vec::new();
    let mut has_config = false;
    for entry in rd.flatten() {
        let name = entry.file_name();
        let from = staging.join(&name);
        let to = target.join(&name);
        if name == "config.json" {
            if !is_marker_root && to.exists() {
                return Err(two_sources(target, &name));
            }
            has_config = true;
            continue;
        }
        if !to.exists() {
            renames.push(name);
        } else if from.is_dir() && to.is_dir() {
            recurse.push(name);
        } else {
            return Err(two_sources(target, &name));
        }
    }

    for name in renames {
        std::fs::rename(staging.join(&name), target.join(&name)).map_err(|e| {
            format!(
                "moving `{}` into {} failed: {e}",
                name.to_string_lossy(),
                target.display()
            )
        })?;
    }
    for name in recurse {
        merge_level(&staging.join(&name), &target.join(&name), false)?;
    }
    if has_config {
        std::fs::create_dir_all(target)
            .map_err(|e| format!("creating {} failed: {e}", target.display()))?;
        std::fs::rename(staging.join("config.json"), target.join("config.json"))
            .map_err(|e| format!("replacing {}/config.json failed: {e}", target.display()))?;
    }
    Ok(())
}

fn two_sources(target: &std::path::Path, name: &std::ffi::OsStr) -> String {
    format!(
        "{}: the template brings `{}` and the root tree already declares it — \
         one address, two sources; rename one of them",
        target.display(),
        name.to_string_lossy()
    )
}

/// `/a/b/c` → (`/a/b`, `c`); `/x` → (`/`, `x`).
fn split_scope_and_name(path: &meclaw_core::Path) -> (String, String) {
    let s = path.as_str();
    match s.rsplit_once('/') {
        Some(("", name)) => ("/".to_string(), name.to_string()),
        Some((scope, name)) => (scope.to_string(), name.to_string()),
        None => ("/".to_string(), s.to_string()),
    }
}

/// Carry a mutation-path refusal into the boot's own error family — with the
/// reason WORD FOR WORD. Two formulations for one cause are two truths.
fn growth_error(g: &PlannedGrowth, e: &MutationError) -> BootstrapError {
    let reason = match e {
        MutationError::TemplateMissing(m) => format!("template_missing: {m}"),
        MutationError::TemplateRefCycle(m) => format!("template_ref_cycle: {m}"),
        MutationError::Schema(m) => format!("schema: {m}"),
        other => format!("{other:?}"),
    };
    BootstrapError::GrowthFailed {
        path: g.fs_path.clone(),
        reference: g.reference.clone(),
        reason,
    }
}
