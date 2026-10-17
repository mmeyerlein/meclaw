//! GH #1034 — an `llm` cell that a `swap_nodes` puts in place of an `llm` cell
//! is its TWIN: what it does not bring itself, it takes from the original —
//! the instructions, the durable slots, the reasoning budget.
//!
//! A model swap is two diff entries: a template for the twin (the original's
//! standing `config.json` with another model) and a `swap_nodes` that swings
//! the original's edges onto it. Before this module the twin was born from its
//! template alone. Measured in a lab colony (06.10.2026):
//!
//! * twins cut from the standing `config.json` had no `seed/`, so a judge whose
//!   instructions live in its `system` table answered like a chat assistant,
//!   and every state downstream of it stayed empty (107/582 artefacts, 0 needs
//!   rows) — while the colony reported no error;
//! * every brain swapped in behind a curator held 0 of the 47–57 tools its
//!   curator ledger listed (talky 0/52, talky-chat 0/47, cogny 0/57). A curator
//!   hands its durable slots (`system.tools`, the sidecar contract, …) to its
//!   brain only when a slot CHANGES (`templates/collector/README.md`, lane
//!   `menu`); the new brain started with none and the curator believed it held
//!   them, so the model ran without tools and answered in prose;
//! * a twin that ran without the original's reasoning params measured another
//!   product: one spent 2048 of 2048 output tokens on reasoning and wrote
//!   nothing.
//!
//! The door COMPLETES the twin and never refuses it, because a deliberate
//! change is the main case of a swap: another model often needs other
//! reasoning params (a provider refuses an effort beside a reasoning limit,
//! GH #854), and a twin may be cut to carry other instructions on purpose.
//!
//! 1. **handover** ([`arm_handover`], before the twin's `config.json` is
//!    patched) — the twin is born with its type's wait param (`llm`:
//!    `handover_wait_ms`, the A timeout of the wait) unless its template or
//!    the entry's override params name it: the bound of one inference turn its
//!    own staged config states, its backstop resolved as the spawn resolves it
//!    (for `llm` the effective backstop; without one `external_timeout_ms` x
//!    (1 + `length_continuations`) plus the margin), so ONE turn of the
//!    original still running at the swap is covered. More than one turn queued
//!    in the original's mailbox, or a twin swapped again while it still waits
//!    (A→B→C), can outlast the wait: the twin then runs on without the
//!    inheritance and says so in a `WARN` line (the documented bound,
//!    `docs/rewiring.en.md`). Before the swap
//!    makes the original inactive the colony asks it for its handover
//!    (`{"handover": {"operation": "export"}}`, `reply_to` the twin, GH #1236). The original answers on its own
//!    connection, the twin imports on its own and parks its turns until then
//!    (GH #1232): the original's `system` rows, a newer row winning per slot,
//!    and its running budget — birth params with every run-time params update
//!    on top — as a set, taken only by a twin that names none of
//!    [`BUDGET_KEYS`]. A twin that declares one keeps its own, and nothing of
//!    the original's is mixed in — an inherited effort beside a declared limit
//!    is exactly the request a provider refuses.
//! 2. **seed** ([`carry_seed`], before the twin's `cell.db` is seeded) — a twin
//!    born without `seed/` gets the one of the library package the original
//!    was instantiated from (its provenance stamp in the colony's registry
//!    book, GH #62), byte for byte, not
//!    the one in the original's directory, which a hand edit may have moved
//!    away from the package (GH #1250) -- but only where that `seed/` is
//!    provably the original's: a single-cell package (its one cell at the
//!    root, reached without a ref) with a `seed/` of its own, under a stamp of
//!    one hop. Everywhere else -- a nested cell, a ref node, the twin of a
//!    twin, an original whose package the library does not hold -- the
//!    original's directory hands on its seed, as before. One that brings its
//!    own keeps it.
//! 3. **system rows** — no longer copied at birth: they arrive with the
//!    handover of step 1. A twin without a seed of its own holds the
//!    original's seed rows from step 2, and the original's newer rows replace
//!    them; a twin with a seed of its own keeps its own rows where they are at
//!    least as new, and takes every slot it does not hold. The curator's
//!    belief "my brain holds these" stays true across the swap, and the twin's
//!    first call offers the model the tools the original's last call offered.
//!
//! Only `llm` → `llm`: the `system` table is where an `llm` cell keeps what it
//! IS, so a successor without it is the same cell broken. Every other type's
//! seed is data (a `store`'s rows, a policy), and swapping it for another
//! implementation with other data is what a swap is for. A swap onto a cell
//! that already stands (the `{name}` form without staging — the documented
//! swing-back) is no birth and keeps its own table.
//!
//! **The colony opens neither the original's `cell.db` nor its `config.json`**
//! (GH #1236, the owner's ruling of 07.10.2026: cell boundaries are hard, no
//! exception, not even a reader). Until then this module read the original's
//! `system` and `params` tables `SQLITE_OPEN_READ_ONLY` on the swap path, and
//! its `config.json` for the type check and the provenance; the handover by
//! message replaces the first, the colony's own books the second: the type
//! comes from the registry (`RegistryEntry.cell_type`), the stamp from the
//! registry book (`colony.db` `registry`, `SetRegistryProvenance`, kept on the
//! mutation's templates snapshot by [`registry_book`]). What staging still
//! reads is one `seed/` directory: the library package's where it is provably
//! the original's, otherwise the one in the original's directory -- item 4 of
//! the cell-boundary inventory of 07.10.2026, left as it is until the owner
//! re-assesses it. Everything here runs while the twin is still in
//! `.staging`, so a failure leaves the live tree untouched.

use crate::mutation::MutationError;
use crate::templates::TemplatesRegistry;
use meclaw_core::JsonValue;
use meclaw_core::serde_json::Map;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The cell type the twin rule binds (see the module docs for why only it).
pub const TWIN_CELL_TYPE: &str = "llm";

/// The params that make up an `llm` cell's reasoning budget, inherited as ONE
/// set or not at all: how hard it deliberates (`reasoning_effort`, and
/// `reasoning`, which outranks it), how many tokens it may deliberate with
/// (`thinking_budget`) and how many it may spend on the whole answer
/// (`max_tokens`, the output cap a reasoning model fills with its reasoning
/// first — the 2048/2048 measurement above).
pub const BUDGET_KEYS: &[&str] = &[
    "reasoning_effort",
    "reasoning",
    "thinking_budget",
    "max_tokens",
];

/// The original of a twin: its node path, where it stands and the `seed/` of
/// the library package its provenance names (`None` when the library holds no
/// such package).
#[derive(Debug, Clone)]
pub(crate) struct Original {
    pub(crate) path: meclaw_core::Path,
    dir: PathBuf,
    library_seed: Option<PathBuf>,
}

/// What staging needs from the colony's own books to recognise a twin
/// (re-check H2b C-1, I-1): the registered type of every node a `swap_nodes`
/// of the diff names as an original (`RegistryEntry.cell_type`), and the
/// colony's `message_timeout_default_ms`, so a twin's wait resolves its
/// backstop exactly as its spawn will. The default knows no original: a diff
/// staged without it stages no twin.
#[derive(Debug, Clone, Default)]
pub struct TwinFacts {
    originals: HashMap<meclaw_core::Path, String>,
    message_timeout_default_ms: u64,
}

impl TwinFacts {
    /// The facts for `diff` at `scope`: the registered type of each swap
    /// original `registry` holds, beside the colony's backstop default.
    pub(crate) fn of(
        diff: &JsonValue,
        scope: &str,
        registry: &HashMap<meclaw_core::Path, crate::colony::RegistryEntry>,
        message_timeout_default_ms: u64,
    ) -> Self {
        let originals = diff
            .get("swap_nodes")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("match")?.get("name")?.as_str())
            .filter_map(|name| {
                let path = super::resolve_scoped_path(scope, name);
                let cell_type = registry.get(&path)?.cell_type.clone();
                Some((path, cell_type))
            })
            .collect();
        Self {
            originals,
            message_timeout_default_ms,
        }
    }
}

/// GH #1236 (C-1): every provenance stamp the colony's registry book holds,
/// read once per mutation snapshot from the colony's own `colony.db`. A
/// book that cannot be read keeps none: the original's directory then hands
/// on its seed, as for any original whose package the library does not hold.
pub(crate) fn registry_book(
    colony_db: &crate::persist::colony_db::ColonyDb,
) -> Vec<(meclaw_core::Path, crate::config::NodeProvenance)> {
    match colony_db.read_registry() {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| Some((r.path, r.provenance?)))
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "swap_nodes twin (GH #1236): registry book unreadable");
            Vec::new()
        }
    }
}

/// The `match.name` of the `swap_nodes` entry whose with-side is `name`, both
/// resolved against `scope` (`brain-b` and `./brain-b` are one node, GH #179).
/// `None` when no swap of this diff names the node as its successor.
pub(crate) fn original_of(diff: &JsonValue, scope: &str, name: &str) -> Option<String> {
    let me = super::resolve_scoped_path(scope, name);
    diff.get("swap_nodes")?.as_array()?.iter().find_map(|s| {
        let with = s.get("with")?.get("name")?.as_str()?;
        if super::resolve_scoped_path(scope, with) != me {
            return None;
        }
        s.get("match")?.get("name")?.as_str().map(str::to_string)
    })
}

/// Is the node staged at `staging_path` a twin? Only when a swap of this diff
/// names it as the successor of a cell, and both are `llm` cells: the twin by
/// the copied template, so it answers before the config is patched; the
/// original by its registered type ([`TwinFacts`]), never its `config.json`.
pub(crate) fn resolve(
    root: &Path,
    scope: &str,
    diff: &JsonValue,
    name: &str,
    staging_path: &Path,
    templates: &TemplatesRegistry,
    twins: &TwinFacts,
) -> Option<Original> {
    let original_name = original_of(diff, scope, name)?;
    let staged = read_json(&staging_path.join("config.json"))?;
    if cell_type(&staged) != Some(TWIN_CELL_TYPE) {
        return None;
    }
    let path = super::resolve_scoped_path(scope, &original_name);
    if twins.originals.get(&path).map(String::as_str) != Some(TWIN_CELL_TYPE) {
        return None;
    }
    let dir = crate::path_truth::resolve_cell_dir(root, scope, &original_name);
    let library_seed = templates
        .instance_provenance(&path)
        .and_then(|p| library_package(p, templates))
        .map(|p| p.join("seed"));
    Some(Original {
        path,
        dir,
        library_seed,
    })
}

/// The library package the original was instantiated from: the template its
/// registry-book stamp names, name and version exactly as stamped (GH #62), in
/// this mutation's registry, which holds the kept copies
/// `local/<name>@<version>/` -- only where the package's `seed/` is provably
/// the original's: a stamp of one hop, a single-cell package whose one cell
/// stands at its root without a ref, and a `seed/` of its own. `None`
/// otherwise, and for an original without a stamp or whose package the
/// library no longer holds; the original's directory decides then.
fn library_package<'a>(
    provenance: &crate::config::NodeProvenance,
    templates: &'a TemplatesRegistry,
) -> Option<&'a Path> {
    // Re-check C4 I1 (twin.rs `library_package`): the stamp names the package,
    // not the cell's place in it. A cell a subtree or a growth placed carries
    // the outer template's stamp (`subtree.rs` `provenance_for`,
    // `bootstrap_grow.rs`) while its seed lies at `<pkg>/<rel_path>/seed`; a
    // ref node (a chain of more than one hop) was born with the GH #1061
    // overlays of its marker; a twin's stamp names the seedless template
    // `twin` (gh1034 case 7, red before this guard: no seed/ at all). Taking
    // `<pkg>/seed` there handed the twin a foreign seed or none, without an
    // error -- so outside the one proven position the directory stays the
    // source, exactly as before GH #1250.
    if provenance
        .template_chain
        .as_ref()
        .is_some_and(|chain| chain.len() != 1)
    {
        return None;
    }
    let template = provenance.template.as_str();
    let version = provenance.template_version.as_deref();
    let package = templates
        .entries_iter()
        .find(|e| e.name == template && e.version.as_deref() == version)?
        .filesystem_path
        .as_path();
    let parsed = crate::mutation::subtree::parse_subtree(package, templates).ok()?;
    let [cell] = parsed.cells.as_slice() else {
        return None;
    };
    (cell.rel_path.is_empty() && cell.ref_chain.is_empty() && package.join("seed").is_dir())
        .then_some(package)
}

/// Before the twin's config is patched: the twin is born waiting for its
/// original's handover under its type's wait param
/// ([`crate::CellFactory::handover_wait`]), written into its staged
/// `config.json` so the patch validates it like any other param. A value its
/// template params or the entry's `override_params` (`with.params` for the
/// instantiate form) name stands. Otherwise the wait comes from the twin's
/// OWN staged config, never the original's (OR-programm-tm.H2b.orch15-1):
/// its backstop resolved as its spawn resolves it (`cell.message_timeout`
/// over the colony default, [`crate::resolve_message_timeout`]; re-check H2b
/// I-1) and its params with the override on top -- for `llm` the effective
/// backstop of one inference turn of the same model package, which covers
/// the original's turn still running at the swap. When the wait elapses the
/// twin runs on without the inheritance and says so in a `WARN` line.
///
/// `true` iff the twin waits (a wait above `0`): only then does the colony
/// ask the original for its handover (re-check H2b M-1).
pub(crate) fn arm_handover(
    staging_path: &Path,
    node: &JsonValue,
    factories: &crate::CellFactoryRegistry,
    twins: &TwinFacts,
) -> Result<bool, MutationError> {
    let cfg_path = staging_path.join("config.json");
    let mut cfg = read_json(&cfg_path).ok_or_else(|| {
        MutationError::Schema(format!(
            "swap_nodes twin (GH #1034): staged config.json does not parse ({})",
            cfg_path.display()
        ))
    })?;
    let mut declared = cfg
        .get("params")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(over) = node.get("override_params").and_then(JsonValue::as_object) {
        declared.extend(over.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let message_timeout = cfg
        .get("cell")
        .and_then(|c| c.get("message_timeout"))
        .and_then(JsonValue::as_i64);
    let backstop_ms =
        crate::resolve_message_timeout(message_timeout, twins.message_timeout_default_ms)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    let wait = cfg
        .get("cell")
        .and_then(|c| c.get("type"))
        .and_then(JsonValue::as_str)
        .and_then(|t| factories.get(t))
        .and_then(|f| f.handover_wait(backstop_ms, &JsonValue::Object(declared)));
    let Some((key, wait_ms)) = wait else {
        return Ok(false);
    };
    let Some(obj) = cfg.as_object_mut() else {
        return Ok(false);
    };
    let params = obj
        .entry("params")
        .or_insert_with(|| JsonValue::Object(Map::new()));
    let Some(params) = params.as_object_mut() else {
        return Ok(false);
    };
    if params.contains_key(key) {
        return Ok(wait_ms > 0);
    }
    params.insert(key.into(), JsonValue::from(wait_ms));
    let text = meclaw_core::serde_json::to_string_pretty(&cfg).map_err(|e| {
        MutationError::Schema(format!("swap_nodes twin (GH #1034): config.json: {e}"))
    })?;
    std::fs::write(&cfg_path, text).map_err(|e| {
        MutationError::Schema(format!(
            "swap_nodes twin (GH #1034): write {}: {e}",
            cfg_path.display()
        ))
    })?;
    Ok(wait_ms > 0)
}

/// Before the twin's `cell.db` is seeded: a twin without `seed/` gets the
/// `seed/` of the original's library package, byte for byte, where
/// [`library_package`] proves it the original's — the directory of the
/// running original everywhere else. One that brings its own keeps it.
pub(crate) fn carry_seed(original: &Original, staging_path: &Path) -> Result<(), MutationError> {
    let twin_seed = staging_path.join("seed");
    if twin_seed.exists() {
        return Ok(());
    }
    let original_seed = original
        .library_seed
        .clone()
        .unwrap_or_else(|| original.dir.join("seed"));
    if original_seed.is_dir() {
        crate::mutation::stage::copy_dir_recursive(&original_seed, &twin_seed)?;
    }
    Ok(())
}

fn read_json(path: &Path) -> Option<JsonValue> {
    let text = std::fs::read_to_string(path).ok()?;
    meclaw_core::serde_json::from_str(&text).ok()
}

fn cell_type(cfg: &JsonValue) -> Option<&str> {
    cfg.get("cell")?.get("type")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn the_successor_is_found_in_either_spelling() {
        let diff =
            json!({"swap_nodes": [{"match": {"name": "brain"}, "with": {"name": "./brain-b"}}]});
        assert_eq!(
            original_of(&diff, "/m", "brain-b").as_deref(),
            Some("brain")
        );
        assert_eq!(
            original_of(&diff, "/m", "./brain-b").as_deref(),
            Some("brain")
        );
        assert_eq!(original_of(&diff, "/m", "other"), None);
        assert_eq!(original_of(&json!({}), "/m", "brain-b"), None);
    }
}
