//! Filesystem bootstrap. Two-phase discipline (validate then apply):
//! `plan_bootstrap` validates the entire tree, `apply_bootstrap_plan` executes a
//! fully validated plan. If 15a fails, nothing was
//! gespawnt.

use std::path::PathBuf;

use meclaw_core::Path as McPath;
use meclaw_core::serde_json;
use meclaw_core::{JsonValue, Uuid};

use crate::CellFactoryRegistry;
use crate::config::{HiveParams, ParsedConfig};
use crate::factory::ContractView;

/// Recursive walk under `root_dir`, yielding every directory that contains a
/// `config.json`. The top-level blacklist (`templates/`, `.staging/`,
/// `blobs/`, dot-prefixed) is applied **only** at the {root}-Ebene; deeper
/// in the tree, all directories with a `config.json` are valid.
pub fn walk_cell_directories(root_dir: &std::path::Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    walk_into(root_dir, true, &mut result);
    result
}

fn walk_into(dir: &std::path::Path, top_level: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if top_level && is_blacklisted_top_level(&path) {
            continue;
        }
        if path.join("config.json").is_file() {
            out.push(path.clone());
        }
        walk_into(&path, false, out);
    }
}

pub(crate) fn is_blacklisted_top_level(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    matches!(name, "templates" | ".staging" | "blobs") || name.starts_with('.')
}

/// Map a filesystem path under `root_dir` to a meclaw Path, where `root_dir`
/// itself maps to `/`. The root directory name is **stripped** — it does not
/// appear in the meclaw path (spec § Filesystem-Layout Z. 331: root cell has
/// path `/`).
pub fn fs_to_meclaw_path(
    root_dir: &std::path::Path,
    fs_path: &std::path::Path,
) -> Result<McPath, String> {
    let rel = fs_path
        .strip_prefix(root_dir)
        .map_err(|e| format!("fs path {:?} not under root {:?}: {e}", fs_path, root_dir))?;
    if rel.as_os_str().is_empty() {
        return Ok(McPath::new("/"));
    }
    let mut s = String::from("/");
    for (i, comp) in rel.components().enumerate() {
        if i > 0 {
            s.push('/');
        }
        let std::path::Component::Normal(seg) = comp else {
            return Err(format!("non-normal component in {:?}", rel));
        };
        s.push_str(
            seg.to_str()
                .ok_or_else(|| "non-utf8 path component".to_string())?,
        );
    }
    Ok(McPath::new(&s))
}

/// Find the single top-level cell directory under `{root}` (after blacklist).
/// Returns error if 0 or >1 such directories exist.
pub fn assert_single_root_dir(root: &std::path::Path) -> Result<PathBuf, String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Err(format!("cannot read {:?}", root));
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if is_blacklisted_top_level(&p) {
            continue;
        }
        if p.join("config.json").is_file() {
            candidates.push(p);
        }
    }
    match candidates.len() {
        0 => Err(format!("no root cell directory found under {:?}", root)),
        1 => Ok(candidates.into_iter().next().unwrap()),
        n => Err(format!("expected exactly 1 root cell directory, found {n}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(dir: &std::path::Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"{}").unwrap();
    }

    #[test]
    fn walk_yields_top_level_cell_dirs() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "main/config.json");
        let r = walk_cell_directories(td.path());
        let names: Vec<_> = r
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"main".to_string()));
    }

    #[test]
    fn walk_skips_top_level_blacklist() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "templates/foo/config.json");
        touch(td.path(), ".staging/bar/config.json");
        touch(td.path(), "blobs/baz/config.json");
        touch(td.path(), ".env-overrides/quux/config.json");
        let r = walk_cell_directories(td.path());
        assert!(r.is_empty(), "blacklist must skip all four");
    }

    #[test]
    fn walk_descends_into_subtrees() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "main/config.json");
        touch(td.path(), "main/a/config.json");
        touch(td.path(), "main/a/b/config.json");
        let r = walk_cell_directories(td.path());
        assert_eq!(r.len(), 3);
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(dir: &std::path::Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"{}").unwrap();
    }

    #[test]
    fn root_dir_maps_to_slash() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "main/config.json");
        let root_dir = td.path().join("main");
        let p = fs_to_meclaw_path(&root_dir, &root_dir).unwrap();
        assert_eq!(p.as_str(), "/");
    }

    #[test]
    fn subpath_strips_root_dir_name() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "main/a/b/config.json");
        let root_dir = td.path().join("main");
        let sub = root_dir.join("a").join("b");
        let p = fs_to_meclaw_path(&root_dir, &sub).unwrap();
        assert_eq!(p.as_str(), "/a/b");
    }

    #[test]
    fn assert_single_root_returns_dir() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "main/config.json");
        let r = assert_single_root_dir(td.path()).unwrap();
        assert_eq!(r.file_name().unwrap().to_str().unwrap(), "main");
    }

    #[test]
    fn assert_single_root_errors_on_multiple() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "main/config.json");
        touch(td.path(), "other/config.json");
        let err = assert_single_root_dir(td.path()).unwrap_err();
        assert!(err.contains("found 2"));
    }

    #[test]
    fn assert_single_root_errors_on_none() {
        let td = TempDir::new().unwrap();
        let err = assert_single_root_dir(td.path()).unwrap_err();
        assert!(err.contains("no root cell"));
        // New: error must include the actual root path for debugging.
        let path_str = td.path().to_string_lossy().to_string();
        assert!(
            err.contains(path_str.as_str()),
            "error should include actual root path {path_str}, got: {err}"
        );
    }
}

/// A hive scope discovered during planning.
#[derive(Debug, Clone)]
pub struct PlannedHive {
    /// Meclaw path for this hive scope.
    pub path: McPath,
}

/// A non-hive cell discovered during planning. `params` is the raw JSON
/// block from `config.json` (already validated by the factory's
/// `validate_params`).
#[derive(Debug, Clone)]
pub struct PlannedCell {
    /// Meclaw path for this cell.
    pub path: McPath,
    /// Absolute filesystem directory of this cell — populated by `plan_bootstrap`
    /// from the walked FS path. Reached through to `CellFactory::spawn_cell` as
    /// `cell_dir` at apply-phase.
    pub fs_path: std::path::PathBuf,
    /// Registered cell type identifier.
    pub cell_type: String,
    /// Raw validated params block.
    pub params: JsonValue,
    /// Taken from `CellHeader::restart_limit`; `None` means "use default (5)"
    /// which is applied in `RegistryEntry` construction.
    pub restart_limit: Option<u32>,
    /// UUID v7, assigned once in plan_bootstrap. Stable across reboots
    /// (persisted in registry table, UPSERT does not overwrite cell_id).
    pub cell_id: Uuid,
    /// Contract extracted from `config.json::contract`; passed to `CellFactory::spawn_cell`
    /// at apply-phase. Defaults to all-false if the `contract` block is absent.
    pub contract_view: ContractView,
    /// Phase-13: `cell.timeout` from `CellHeader`. For the 0/>0/-1 semantics see
    /// the spec (`docs/config.md` l.42). Wired into `cell_task_stateful` in
    /// 13-K/13-L — today it is propagated behaviour-neutrally.
    pub cell_timeout: i64,
    /// Phase-13: an optional per-cell override for the idle duration. The fallback
    /// in `bootstrap_apply` is `DEFAULT_IDLE_TIMEOUT_MS` (phase-13 limitation:
    /// volles colony.json-Parsing deferred). Spec: docs/config.md Z.42-43.
    pub idle_timeout_ms: Option<u64>,
    /// P3-B-plumb-2: optional per-cell `cell.message_timeout` override (B-backstop)
    /// from `config.json`. `None` → colony `message_timeout_default_ms`; resolved at
    /// the spawn call-site via `resolve_message_timeout`. Semantics: `>0` → backstop
    /// of that many ms, `0`/`-1` → no backstop. Same shape as `idle_timeout_ms`.
    pub message_timeout: Option<i64>,
    /// Activity flag: `true` for genuinely-new FS nodes (no overlay entry); for
    /// rehydrated nodes, `true` iff the overlay's persisted `status == "active"`.
    /// Threaded into `RegistryEntry.active` at apply-phase.
    pub active: bool,
    /// Failure flag (Paket-6 C): `true` iff the overlay's persisted
    /// `status == "failed"`. Threaded into `RegistryEntry.failed` at apply-phase.
    /// The persisted `failed` state wins over edge-derived activity (overview
    /// Z.1426 "persistierter Stand gewinnt") — a failed cell stays
    /// `active=false, failed=true` across reboot even when fully wired.
    pub failed: bool,
    /// Per-cell bounded-mailbox capacity from `cell.mailbox_size`. `None` means
    /// "fall back to colony-wide default". Validated at plan-phase (0 is rejected
    /// as `InvalidCellField`). Wired at apply-phase (Paket 1).
    pub mailbox_size: Option<usize>,
    /// Hardening Slice 1: 14-B header projection of this cell's `contract`
    /// block (emits.hop keys + required consumes keys). Threaded into the
    /// colony's `node_contracts` map via `ColonyMsg::SetNodeContract` at
    /// apply-phase.
    pub header_view: crate::mutation::validate::HeaderNodeView,
}

/// An edge discovered during planning, with scope-relative `from`/`to`
/// already resolved to absolute meclaw paths.
#[derive(Debug, Clone)]
pub struct PlannedEdge {
    /// Unique edge identifier.
    pub id: Uuid,
    /// Absolute source path.
    pub from: McPath,
    /// Absolute destination path.
    pub to: McPath,
    /// Pre-compiled CEL condition (Phase 13.5-A1). `None` = always take.
    pub condition: Option<crate::cel_eval::CompiledCondition>,
    /// Pre-compiled CEL modifier (Phase 13.5-A1). `None` = identity headers.
    pub modifier: Option<crate::cel_eval::CompiledModifier>,
}

/// A fully validated bootstrap plan. `apply_bootstrap_plan` (Task 15b) takes
/// this and performs spawn/register/AddEdge/AddHiveScope — guaranteed not to
/// fail because all validation happened in `plan_bootstrap`.
#[derive(Debug, Default, Clone)]
pub struct BootstrapPlan {
    /// All hive scopes found in the filesystem tree.
    pub hives: Vec<PlannedHive>,
    /// All non-hive cells found in the filesystem tree.
    pub cells: Vec<PlannedCell>,
    /// All validated edges declared in hive params blocks.
    pub edges: Vec<PlannedEdge>,
    /// A5b (Phase-16 W1b, Ruling 2026-06-12): on a **Reboot**, a cell directory
    /// whose path is absent from the persisted registry overlay (an unknown
    /// `config.json` node — manually placed, never instantiated/mutated) is
    /// **reported here, never adopted**: registration happens exclusively via
    /// instantiation/mutation, never via boot discovery. The consumer warns per
    /// path (live boot) / lists them (`--validate`); the node is NOT planned as
    /// a cell (no `cell_id`, no spawn, no registry entry). On a `FirstBoot` the
    /// walk IS the source of truth, so this stays empty there.
    pub unregistered_nodes: Vec<McPath>,
}

/// Probe for cell.db integrity. Returns `Ok(())` for absent or healthy DBs and
/// `Err(reason)` on quick_check ≠ "ok" or a schema_version mismatch.
fn probe_cell_db(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(()); // absent = first-boot, OK
    }
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("open: {e}"))?;
    let qc: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|e| format!("quick_check: {e}"))?;
    if qc != "ok" {
        return Err(format!("quick_check: {qc}"));
    }
    let v =
        crate::persist::read_schema_version(&conn).map_err(|e| format!("schema_version: {e}"))?;
    if v != 1 {
        return Err(format!("schema_version: {v} (expected 1)"));
    }
    Ok(())
}

/// Build a `ContractView` from a parsed `contract` block, compiling its
/// `emits` schemas (P13/D-010a). `Err(reason)` if a schema is malformed — the
/// caller turns this into a `BootstrapError`/`MutationError` so the boot /
/// mutation path fails loudly (config.md Z.37 Boot-Strict-Kultur), never a
/// silent "validation off". `validate_emits` is left `false` here; the spawn
/// path resolves the effective flag (B5) since `strict_validation` lives in
/// colony.json. Shared between the boot path (B3) and the mutation path (B4).
pub(crate) fn compile_contract_view(
    block: &crate::config::ContractBlock,
) -> Result<ContractView, String> {
    let emits = if block.emits.body.is_empty() && block.emits.hop.is_empty() {
        None
    } else {
        Some(std::sync::Arc::new(meclaw_core::CompiledEmits::compile(
            &block.emits,
        )?))
    };
    // Slice 2: compile the required-`consumes` views (infallible projection).
    // Vacuous (no required keys) → `None`, so the delivery-boundary check
    // (Task 2.4) short-circuits without touching the message. An absent
    // `consumes` block (Slice 4: presence-detectable Option) compiles like
    // an empty one — absent ⇒ empty ⇒ vacuous, semantics unchanged.
    let default_consumes = meclaw_core::ConsumesBlock::default();
    let cc = meclaw_core::CompiledConsumes::compile(
        block.consumes.as_ref().unwrap_or(&default_consumes),
    );
    let consumes = if cc.is_vacuous() {
        None
    } else {
        Some(std::sync::Arc::new(cc))
    };
    Ok(ContractView {
        multi_send_capable: block.multi_send_capable,
        emits,
        validate_emits: false,
        consumes,
    })
}

/// Walk the filesystem, parse every `config.json`, validate cells and
/// edges, return a fully validated `BootstrapPlan`. Atomic-strict: returns
/// `Err(BootstrapErrors)` if ANY validation failed, with ALL errors
/// collected (does not abort at first error).
///
/// **No spawn**: this function does not touch Colony, does not spawn any
/// cell task, does not register anything. The plan is pure data.
pub fn plan_bootstrap(
    root: &std::path::Path,
    factories: &CellFactoryRegistry,
    overlay: &crate::persist::colony_db::RegistryOverlay,
) -> Result<BootstrapPlan, BootstrapErrors> {
    // A5b: the no-env wrapper defaults to FirstBoot semantics (walk = source).
    // Reboot-report behaviour is opt-in via `plan_bootstrap_with_env` with an
    // explicit `BootState::Reboot` — keeps every existing call site unchanged.
    plan_bootstrap_with_env(root, factories, overlay, BootState::FirstBoot, None)
}

/// `plan_bootstrap` with an explicit `.env` location (U7, `--env` CLI flag).
/// `None` keeps the `{root}/.env` default; `Some(path)` reads that file
/// instead. The substitution model itself is unchanged (Befund 4 / 8c73186).
pub fn plan_bootstrap_with_env(
    root: &std::path::Path,
    factories: &CellFactoryRegistry,
    overlay: &crate::persist::colony_db::RegistryOverlay,
    boot_state: BootState,
    env_path: Option<&std::path::Path>,
) -> Result<BootstrapPlan, BootstrapErrors> {
    let mut errors = BootstrapErrors::new();
    let mut plan = BootstrapPlan::default();

    // Deep-Audit F2 (b): a half-applied mutation (mid-rename strict-fail panic)
    // leaves an `in_flight` `mutation_log` row that no production code transitions
    // (recovery is not wired into the boot path). That row is the signal that
    // overlay-miss cell dirs may be orphans of an interrupted mutation. On a
    // Reboot the unregistered-node seam already reports them; here we also report
    // on a FirstBoot-classified tree (empty registry) carrying such a row, so a
    // first-mutation panic's orphans are surfaced, never silently adopted. A clean
    // FirstBoot (no colony.db / no in_flight row) keeps the walk-as-source path.
    let has_pending_mutation = has_in_flight_mutation(root);

    // Slice 6 (Phase-14-B as build-time error): collect per-node contract views
    // (keyed by absolute meclaw path) and per-edge modifier key-sets, then run
    // the pure `validate_header_contract_locality` once the walk is done. Empty
    // `consumes` ⇒ vacuously true ⇒ existing topologies never break.
    let mut header_nodes: std::collections::BTreeMap<
        String,
        crate::mutation::validate::HeaderNodeView,
    > = std::collections::BTreeMap::new();
    let mut header_edges: Vec<crate::mutation::validate::HeaderEdgeView> = Vec::new();
    // F1 fix: absolute hive paths for the locality check — an edge with a hive
    // `from` is a transit pass-through and contributes the fan-in intersection
    // of the hive's inbound edges (same key walk the runtime performs).
    let mut header_hives: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let root_dir = match assert_single_root_dir(root) {
        Ok(d) => d,
        Err(e) => {
            if e.contains("no root cell") {
                errors.push(BootstrapError::NoRootDir);
            } else if e.contains("found ") {
                let n = e
                    .split_whitespace()
                    .find_map(|w| w.parse::<usize>().ok())
                    .unwrap_or(0);
                errors.push(BootstrapError::MultipleRootDirs { count: n });
            } else {
                errors.push(BootstrapError::InvalidPath { reason: e });
            }
            return Err(errors);
        }
    };

    let mut dirs = vec![root_dir.clone()];
    dirs.extend(walk_cell_directories(&root_dir));

    // Befund 4: boot-time `${ENV_VAR}` substitution shares the mutation path's
    // model (spec § Variable substitution: substituted "when reading
    // config.json"; § Behavior on errors l.1366: missing plain `${VAR}` at the
    // initial bootstrap ⇒ daemon failed-to-start). `{root}/.env` absent ⇒ empty
    // map (only configs that actually use `${VAR}` then fail); malformed ⇒
    // loud plan error.
    let env_file = env_path
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| root.join(".env"));
    let env = match crate::env_file::load_env(&env_file) {
        Ok(m) => m,
        Err(e) => {
            errors.push(BootstrapError::InvalidPath {
                reason: format!(".env: {e}"),
            });
            return Err(errors);
        }
    };

    for fs_path in &dirs {
        let mc_path = match fs_to_meclaw_path(&root_dir, fs_path) {
            Ok(p) => p,
            Err(e) => {
                errors.push(BootstrapError::InvalidPath { reason: e });
                continue;
            }
        };
        let raw = match std::fs::read_to_string(fs_path.join("config.json")) {
            Ok(s) => s,
            Err(e) => {
                errors.push(BootstrapError::InvalidJson {
                    path: fs_path.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let raw_parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                errors.push(BootstrapError::InvalidJson {
                    path: fs_path.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        // Befund 4: substitute IN-MEMORY over the whole parsed value (the
        // mutation path substitutes the whole config too) — the on-disk
        // config.json is NEVER rewritten at boot (Authority-Modell:
        // `config.json`-Writes only at instantiation). Env-only token set:
        // `${ctx.*}` / `${uuid7:*}` are mutation-side substitutions and have no
        // filesystem-side producer (spec § Variable substitution).
        let substituted = match crate::mutation::substitute::substitute_env_only(&raw_parsed, &env)
        {
            Ok(v) => v,
            Err(e) => {
                let reason = match &e {
                    crate::mutation::MutationError::EnvVarMissing(var) => format!(
                        "env_var_missing: ${{{var}}} has no value in {{root}}/.env and no default"
                    ),
                    crate::mutation::MutationError::UnsupportedSubstitution(form) => {
                        format!("unsupported_substitution: ${{{form}}}")
                    }
                    other => format!("{other:?}"),
                };
                errors.push(BootstrapError::EnvSubstitution {
                    path: fs_path.clone(),
                    reason,
                });
                continue;
            }
        };
        let cfg: ParsedConfig = match serde_json::from_value(substituted.clone()) {
            Ok(c) => c,
            Err(e) => {
                errors.push(BootstrapError::InvalidJson {
                    path: fs_path.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };

        if cfg.cell.cell_type == "hive" {
            plan.hives.push(PlannedHive {
                path: mc_path.clone(),
            });
            header_hives.insert(mc_path.as_str().to_string());
            // A hive without a `params` block is valid (no edges declared).
            let params_value = if cfg.params.is_null() {
                serde_json::json!({})
            } else {
                cfg.params.clone()
            };
            let hp: HiveParams = match serde_json::from_value(params_value) {
                Ok(h) => h,
                Err(e) => {
                    errors.push(BootstrapError::InvalidJson {
                        path: fs_path.clone(),
                        reason: format!("params: {e}"),
                    });
                    continue;
                }
            };
            for spec in &hp.graph.edges {
                // Phase 13.5-A1 T5: parse-validate CEL condition + modifier
                // instead of strict-failing on their presence.
                let compiled_condition = match &spec.condition {
                    None => None,
                    Some(src) => match crate::cel_eval::parse_condition(src) {
                        Ok(c) => Some(c),
                        Err(reason) => {
                            errors.push(BootstrapError::EdgeConditionParse {
                                scope: mc_path.clone(),
                                from: spec.from.clone(),
                                to: spec.to.clone(),
                                reason,
                            });
                            continue;
                        }
                    },
                };
                let compiled_modifier = match &spec.modifier {
                    None => None,
                    Some(m) => match crate::cel_eval::parse_modifier(m) {
                        Ok(cm) => Some(cm),
                        Err((key, reason)) => {
                            errors.push(BootstrapError::EdgeModifierParse {
                                scope: mc_path.clone(),
                                from: spec.from.clone(),
                                to: spec.to.clone(),
                                key,
                                reason,
                            });
                            continue;
                        }
                    },
                };
                let from_abs = McPath::resolve(&mc_path, &spec.from);
                let to_abs = McPath::resolve(&mc_path, &spec.to);
                // Slice 6: project this edge's `ModifierSpec` key-sets into a
                // `HeaderEdgeView` (from/to as absolute meclaw paths — same
                // namespace as the per-node contract keys collected below).
                let mut edge_view = crate::mutation::validate::HeaderEdgeView {
                    from: from_abs.as_str().to_string(),
                    to: to_abs.as_str().to_string(),
                    ..Default::default()
                };
                if let Some(m) = &spec.modifier {
                    edge_view.set_context = m.set_context.keys().cloned().collect();
                    edge_view.delete_context = m.delete_context.iter().cloned().collect();
                    edge_view.set_hop = m.set_hop.keys().cloned().collect();
                    edge_view.delete_hop = m.delete_hop.iter().cloned().collect();
                }
                header_edges.push(edge_view);
                plan.edges.push(PlannedEdge {
                    id: Uuid::now_v7(),
                    from: from_abs,
                    to: to_abs,
                    condition: compiled_condition,
                    modifier: compiled_modifier,
                });
            }
        } else {
            // A5b (Phase-16 W1b, Ruling 2026-06-12): on a Reboot, a cell whose
            // path is absent from the persisted registry overlay is an unknown
            // node (manually placed, never instantiated/mutated). Registration
            // happens EXCLUSIVELY via instantiation/mutation — the reboot walk
            // REPORTS such a node (consistency view), it never adopts it. It is
            // not planned (no cell_id, no spawn, no contract enforcement); the
            // consumer warns / lists it. The root `/` is always a known node on
            // any legitimate reboot and is never diverted. On FirstBoot the walk
            // IS the source, so this branch is skipped (every node is planned).
            if (matches!(boot_state, BootState::Reboot) || has_pending_mutation)
                && mc_path.as_str() != "/"
                && overlay.get(&mc_path).is_none()
            {
                plan.unregistered_nodes.push(mc_path.clone());
                continue;
            }
            // Strict unknown-field coverage for `cell.*`: anything not on the curated
            // allow-list is rejected here. The allow-list grows
            // pro Phase (z.B. `cell.timeout` ab Phase 13-B-0, `cell.id` ab
            // Phase 13.5 Slice 4 — swap_nodes writes the preserved cell_id into
            // config.json so that it survives a reboot).
            const ALLOWED_CELL_FIELDS: &[&str] = &[
                "type",
                "restart_limit",
                "timeout",
                "idle_timeout_ms",
                "id",
                "message_timeout",
                "mailbox_size",
            ];
            // Befund 4: reuse the already-parsed, env-substituted value (keys
            // are untouched by substitution; this check only inspects keys).
            let raw_value = &substituted;
            if let Some(cell_obj) = raw_value.get("cell").and_then(|v| v.as_object()) {
                let mut unknown_found = false;
                for key in cell_obj.keys() {
                    if !ALLOWED_CELL_FIELDS.contains(&key.as_str()) {
                        errors.push(BootstrapError::UnknownCellField {
                            path: fs_path.clone(),
                            field: format!("cell.{key}"),
                        });
                        unknown_found = true;
                    }
                }
                if unknown_found {
                    continue;
                }
            }

            // T4: mailbox_size:0 has no capacity semantics and must be
            // rejected hard. Negative values are already impossible via
            // `usize` deserialisation — only 0 needs an explicit check.
            if cfg.cell.mailbox_size == Some(0) {
                errors.push(BootstrapError::InvalidCellField {
                    path: fs_path.clone(),
                    field: "cell.mailbox_size".into(),
                    reason: "mailbox_size must be >= 1 (0 has no capacity semantics)".into(),
                });
                continue;
            }

            let Some(factory) = factories.get(&cfg.cell.cell_type) else {
                errors.push(BootstrapError::UnknownCellType {
                    path: fs_path.clone(),
                    cell_type: cfg.cell.cell_type.clone(),
                });
                continue;
            };
            if let Err(reason) = factory.validate_params(&cfg.params) {
                errors.push(BootstrapError::InvalidParams {
                    path: fs_path.clone(),
                    reason,
                });
                continue;
            }

            let cell_db_path = fs_path.join("cell.db");
            if let Err(reason) = probe_cell_db(&cell_db_path) {
                errors.push(BootstrapError::CorruptCellDb {
                    path: cell_db_path,
                    reason,
                });
                continue;
            }

            // Phase-13.5 Lifecycle-3a: identity overlay. A known path (present
            // in `colony.db`'s registry from a prior boot) reuses its persisted
            // cell_id — RAM identity stays stable across reboots (G5). A
            // genuinely-new path (absent from the overlay) gets a fresh cell_id
            // exactly as before. Reconciliation: FS node ∉ overlay = new; an
            // overlay entry ∉ FS (no directory) is an orphan that the FS walk
            // never reaches — it keeps its persisted cell_id+status in the DB
            // and is simply not re-spawned (no-delete, no re-mint, no crash;
            // see lifecycle_3a_demo orphan test). Status is READ only (3a) —
            // never written to 'inactive' here (that is 3b).
            // Phase-13.5 Lifecycle-3b: map the persisted `status` into `active`.
            // A genuinely-new FS node (overlay miss) defaults to active. A
            // rehydrated node maps `status == "active"` to `true`, anything else
            // (e.g. `"inactive"`) to `false`. The 3a overlay already delivers the
            // status string; this is the read-side consumption point.
            let overlay_entry = overlay.get(&mc_path);
            let cell_id = overlay_entry
                .map(|(id, _status)| *id)
                .unwrap_or_else(Uuid::now_v7);
            let active = match overlay_entry {
                Some((_id, status)) => status == "active",
                None => true,
            };
            // Paket-6 C: distinguish a persisted `failed` status from a plain
            // `inactive` one. A failed cell rehydrates `failed=true` (and
            // `active=false`, already derived above) and is never re-spawned;
            // the persisted state wins over edge-derived activity.
            let failed = matches!(overlay_entry, Some((_id, status)) if status == "failed");
            // Hardening Slice 4 (Task 4.2): the builder-mandatory contract
            // presence keys (`version`/`settings`/`consumes`) are
            // substrate-enforced at config load (config.md § contract,
            // Enforcement-Stufen). Hive markers never reach this branch
            // (exempt — their contract block is not evaluated).
            if let Err(reason) = crate::config::validate_contract_presence(&cfg.contract) {
                errors.push(BootstrapError::ContractIncomplete {
                    path: fs_path.clone(),
                    reason,
                });
                continue;
            }
            // Paket-7 B3: compile `contract.emits` here so a malformed schema is
            // a LOUD boot error (analog UnknownCellField — config.md Z.37
            // Boot-Strict-Kultur), never a silent "validation off".
            let contract_view = match compile_contract_view(&cfg.contract) {
                Ok(cv) => cv,
                Err(reason) => {
                    errors.push(BootstrapError::InvalidEmitsSchema {
                        path: fs_path.clone(),
                        reason,
                    });
                    continue;
                }
            };
            // Slice 6: project this cell's `consumes`/`emits` into a
            // `HeaderNodeView` keyed by its absolute meclaw path (same namespace
            // as the edge from/to above). Only `required` consume keys carry a
            // build-time obligation; non-required keys are omitted.
            // Hardening Slice 1: the same view also rides on the `PlannedCell`
            // so apply-phase can register it via `ColonyMsg::SetNodeContract`.
            let header_view = crate::mutation::validate::header_view_from_contract(&cfg.contract);
            header_nodes.insert(mc_path.as_str().to_string(), header_view.clone());
            // P3-B-plumb-2: cell.message_timeout is now propagated into
            // `PlannedCell.message_timeout` and resolved at the spawn call-site
            // (`bootstrap_apply.rs`) into the active B-backstop.
            plan.cells.push(PlannedCell {
                path: mc_path,
                fs_path: fs_path.clone(),
                cell_type: cfg.cell.cell_type,
                params: cfg.params,
                restart_limit: cfg.cell.restart_limit,
                cell_id,
                contract_view,
                cell_timeout: cfg.cell.timeout,
                idle_timeout_ms: cfg.cell.idle_timeout_ms,
                message_timeout: cfg.cell.message_timeout,
                active,
                failed,
                mailbox_size: cfg.cell.mailbox_size,
                header_view,
            });
        }
    }

    // A7 (Phase-16 W1a, Ruling 2026-06-12 — Option B, recompute-mirror): the
    // FIRST bootstrap derives activity with the SAME rule as the mutation
    // recompute — "a node's activity is the result of the last connectivity
    // computation that REACHED it; a node never reached keeps its
    // instantiation-activity". The boot computation is seeded by the
    // `params.graph` edge endpoints, exactly like a mutation's `involved` set,
    // and only the nodes inside the resulting `affected_scope` are recomputed.
    // A node NOT reached (an edge-less single cell, or an edge-less cell inside
    // an otherwise-disconnected sub-hive) keeps the Instanziierungs-Grace
    // (stays active) — symmetric to a single-cell `add_nodes` without an edge.
    // True ISLANDS (sub-hives whose internal edges seed the recompute over
    // their own scope) derive INACTIVE (the disconnected hive gates the
    // subtree). The root `/` is always active. Rehydrated nodes (overlay-hit)
    // keep their persisted status (Reboot).
    {
        let mut edges_view = crate::edge_table::EdgeTable::new();
        for e in &plan.edges {
            edges_view.insert(crate::edge_table::Edge {
                id: e.id,
                from: e.from.clone(),
                to: e.to.clone(),
                condition: None,
                modifier: None,
            });
        }
        let mut hive_view = crate::hive_scope::HiveScopeTable::new();
        for h in &plan.hives {
            hive_view.register(crate::hive_scope::HiveScope {
                path: h.path.clone(),
            });
        }
        // The recompute seeds: every edge endpoint (mirrors a mutation's
        // `involved`). `known_paths` is the cell universe (subtree expansion),
        // `hive_paths` the registered hives (crossing-edge hive flips).
        let involved: Vec<McPath> = plan
            .edges
            .iter()
            .flat_map(|e| [e.from.clone(), e.to.clone()])
            .collect();
        let known_paths: Vec<McPath> = plan.cells.iter().map(|c| c.path.clone()).collect();
        let hive_paths: Vec<McPath> = plan.hives.iter().map(|h| h.path.clone()).collect();
        let scope = crate::connectivity::affected_scope(&involved, &known_paths, &hive_paths);
        for cell in &mut plan.cells {
            if overlay.get(&cell.path).is_none()
                && cell.path.as_str() != "/"
                && scope.contains(&cell.path)
            {
                cell.active =
                    crate::connectivity::compute_active(&cell.path, &edges_view, &hive_view);
            }
        }
    }

    // Slice 6 + A5 (Phase-16 W1a, Ruling 2026-06-12): run the pure
    // header-contract locality check over the collected post_state node
    // contracts + edge modifier key-sets — but ONLY over ACTIVE nodes. A
    // disconnected/inactive node (persisted inactive at reboot, or derived
    // inactive at first boot per A7) is pure bookkeeping at boot: its contract
    // obligations are not enforced here (the full check lives at the mutation
    // point that wires it). This mirrors the mutation path's participation rule
    // (header_views.rs) and makes A5 uniform across both boot kinds. Empty
    // `consumes` ⇒ vacuously true ⇒ no existing topology breaks. A violation on
    // an ACTIVE node is a LOUD boot error (Phase-14-B leak class).
    let active_paths: std::collections::HashSet<&str> = plan
        .cells
        .iter()
        .filter(|c| c.active)
        .map(|c| c.path.as_str())
        .collect();
    let active_header_nodes: std::collections::BTreeMap<
        String,
        crate::mutation::validate::HeaderNodeView,
    > = header_nodes
        .iter()
        .filter(|(path, _)| active_paths.contains(path.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Err(crate::mutation::MutationError::EdgeSchema(reason)) =
        crate::mutation::validate::validate_header_contract_locality(
            &active_header_nodes,
            &header_edges,
            &header_hives,
        )
    {
        errors.push(BootstrapError::HeaderContractViolation { reason });
    }

    errors.into_result(plan)
}

/// Boot-state classification for colony.db (T20, E9).
///
/// - `FirstBoot`: all persistence tables empty (or the file absent).
/// - `Reboot`: all persistence tables non-empty — a re-boot, hydrate instead of InitialApply.
/// - `Inconsistent`: mixed state — STRICT-FAIL, externe Korruption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootState {
    /// All tables empty or the file absent.
    FirstBoot,
    /// All tables non-empty.
    Reboot,
    /// Mischzustand — externe Korruption.
    Inconsistent {
        /// Diagnose-String.
        reason: String,
    },
}

/// Inspects the three persistence tables (registry/edges/hive_scopes) and classifies.
///
/// A read-only connection probe; performs no write.
///
/// **Bootstrap-Recovery (Run-5/5b-Befund)**: a durable `bootstrap_in_flight`
/// marker in the `meta` table means the last FIRST apply was interrupted
/// mid-way (crash between the per-cell registry upserts and the atomic
/// `InitialApply` bundle that clears the marker in the same transaction). That
/// state classifies as `FirstBoot`: the apply path is idempotent (registry
/// upserts are cell_id-stable via the identity overlay, `InitialApply` is
/// INSERT OR IGNORE), so the boot simply resumes the rebuild from the
/// filesystem — the FS is the source. A mixed table state WITHOUT the marker
/// stays `Inconsistent` (external corruption, strict-fail).
pub fn probe_boot_state(db_path: &std::path::Path) -> Result<BootState, BootstrapError> {
    if !db_path.exists() {
        return Ok(BootState::FirstBoot);
    }
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| BootstrapError::InconsistentColonyDb {
                reason: format!("open: {e}"),
            })?;
    let marker: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key='bootstrap_in_flight'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if marker > 0 {
        return Ok(BootState::FirstBoot);
    }
    let mut counts: Vec<i64> = Vec::with_capacity(3);
    for table in &["registry", "edges", "hive_scopes"] {
        let q = format!("SELECT COUNT(*) FROM {table}");
        let c: i64 = conn.query_row(&q, [], |r| r.get(0)).unwrap_or(0);
        counts.push(c);
    }
    let all_empty = counts.iter().all(|&c| c == 0);
    let all_full = counts.iter().all(|&c| c > 0);
    if all_empty {
        Ok(BootState::FirstBoot)
    } else if all_full {
        Ok(BootState::Reboot)
    } else {
        Ok(BootState::Inconsistent {
            reason: format!(
                "table counts mixed (registry={}, edges={}, hive_scopes={})",
                counts[0], counts[1], counts[2]
            ),
        })
    }
}

/// Deep-Audit F2 (b): true iff `colony.db` carries at least one `in_flight`
/// `mutation_log` row — the durable signature of a mutation that started but never
/// committed (mid-rename strict-fail panic; production does not transition such a
/// row). Absent db / missing table / read error → `false`, so a clean FirstBoot
/// keeps its walk-as-source behaviour. Read-only; runs before the colony writer
/// thread opens the db (no contention).
fn has_in_flight_mutation(root: &std::path::Path) -> bool {
    let db_path = root.join("colony.db");
    if !db_path.exists() {
        return false;
    }
    let Ok(conn) = rusqlite::Connection::open(&db_path) else {
        return false;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM mutation_log WHERE status='in_flight'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Errors collected during bootstrap planning. Atomic-strict: a single
/// error from any of these aborts the whole bootstrap.
#[derive(Debug, Clone)]
pub enum BootstrapError {
    /// JSON parse error in a `config.json`.
    InvalidJson { path: PathBuf, reason: String },
    /// `cell.type` value has no registered factory.
    UnknownCellType { path: PathBuf, cell_type: String },
    /// Factory's `validate_params` rejected the params block.
    InvalidParams { path: PathBuf, reason: String },
    /// CEL `condition` failed to parse (Phase 13.5-A1, replaces the Phase-4
    /// `EdgeCondition` strict-fail variant).
    EdgeConditionParse {
        /// Hive scope where the edge was declared.
        scope: McPath,
        /// Scope-relative `from` path.
        from: String,
        /// Scope-relative `to` path.
        to: String,
        /// CEL parse-error reason.
        reason: String,
    },
    /// CEL `modifier.set_context.*`/`set_hop.*` failed to parse (Phase 13.5).
    EdgeModifierParse {
        /// Hive scope where the edge was declared.
        scope: McPath,
        /// Scope-relative `from` path.
        from: String,
        /// Scope-relative `to` path.
        to: String,
        /// `modifier.set_context`/`set_hop` key (prefixed) whose expression
        /// failed to parse.
        key: String,
        /// CEL parse-error reason.
        reason: String,
    },
    /// More than one top-level cell directory under {root} (after blacklist).
    MultipleRootDirs { count: usize },
    /// No top-level cell directory under {root}.
    NoRootDir,
    /// Filesystem path could not be mapped to a meclaw path.
    InvalidPath { reason: String },
    /// cell.db exists but is corrupt (quick_check failed or a schema_version
    /// mismatch). Probed in plan_bootstrap (T19); apply never fails at this point.
    CorruptCellDb {
        /// Path to the broken cell.db.
        path: PathBuf,
        /// Diagnostic string (the quick_check result or the schema_version value).
        reason: String,
    },
    /// colony.db is in a mixed state across the persistence tables
    /// (registry/edges/hive_scopes). Detected via probe_boot_state (T20). A STRICT
    /// FAIL against external corruption.
    InconsistentColonyDb {
        /// Diagnostic string (which tables are empty vs. non-empty).
        reason: String,
    },
    /// config.json contains a `cell.*` field not supported in phase 5
    /// (e.g. cell.timeout — that only arrives in phase 10/13).
    UnknownCellField {
        /// Path to the config.json.
        path: PathBuf,
        /// Feld-Name (z.B. "cell.timeout").
        field: String,
    },
    /// A cell's `contract.emits` schema failed to compile (P13/D-010a).
    /// Boot fails loudly — never a silent "validation off" (config.md Z.37).
    InvalidEmitsSchema {
        /// Filesystem path of the offending cell directory.
        path: PathBuf,
        /// Compile-error reason (names the failing section, e.g. `emits.body`).
        reason: String,
    },
    /// Hardening Slice 4 (Task 4.2): a NON-hive cell's `contract` block does
    /// not declare the builder-mandatory presence keys `version` / `settings` /
    /// `consumes`, or a key has the wrong JSON type (config.md § contract,
    /// Enforcement-Stufen). Boot fails loudly; hive markers are exempt (their
    /// contract block is not evaluated).
    ContractIncomplete {
        /// Filesystem path of the offending cell directory.
        path: PathBuf,
        /// Presence-check reason (names the first missing/invalid key).
        reason: String,
    },
    /// Slice 6 (Phase-14-B as build-time error): the post_state header graph
    /// violates hop-locality (fan-in intersection) or context-presence
    /// reachability. Carries the `EdgeSchema` reason from
    /// [`crate::mutation::validate::validate_header_contract_locality`].
    HeaderContractViolation {
        /// Human-readable reason (names the node + key + which rule failed).
        reason: String,
    },
    /// A known `cell.*` field has an invalid value (e.g. `mailbox_size: 0`
    /// which has no capacity semantics). The field name is in `cell.*` form.
    InvalidCellField {
        /// Absolute filesystem path of the offending `config.json` directory.
        path: PathBuf,
        /// Field name in `cell.*` form (e.g. `"cell.mailbox_size"`).
        field: String,
        /// Human-readable reason why the value was rejected.
        reason: String,
    },
    /// Substrat-Fix Befund 4 — `${ENV_VAR}` substitution over a boot
    /// `config.json` failed: a plain `${VAR}` without default has no value in
    /// `{root}/.env` (spec § Behavior on errors l.1366: daemon failed-to-start,
    /// Exit≠0), or the token uses an unsupported operator form. Boot and
    /// mutation share one substitution model (overview Z.1366/1367); `reason`
    /// carries the mutation-path token verbatim (`env_var_missing: ...` /
    /// `unsupported_substitution: ...`) so `--validate` stderr matches it.
    EnvSubstitution {
        /// Filesystem path of the offending `config.json` directory.
        path: PathBuf,
        /// Substitution-error reason, prefixed with the spec error token.
        reason: String,
    },
    /// A8 (Phase-16 W1a, Ruling 2026-06-12): a `params.graph` edge endpoint
    /// resolves to nothing the colony knows — not a plan cell/hive, not a live
    /// registry path, not a `/colony/*` virtual endpoint. A typo / dead edge is
    /// a LOUD boot fail (the `--validate` pre-check downgrades this to a warning
    /// since a static run cannot see runtime-spawned cells; `--strict` promotes
    /// it back to an error).
    DanglingEndpoint {
        /// The offending edge's id.
        edge_id: Uuid,
        /// The endpoint path that does not resolve.
        endpoint: McPath,
    },
}

/// Error during re-hydration of persisted edges from `colony.db` (Reboot).
#[derive(Debug, thiserror::Error)]
pub enum EdgeHydrationError {
    /// rusqlite error while reading the edges table.
    #[error("sqlite error during edge hydration: {0}")]
    Sql(rusqlite::Error),
    /// `edges.id` is not a valid UUID.
    #[error("invalid uuid in edges.id={edge_id}: {error}")]
    InvalidUuid { edge_id: String, error: String },
    /// Persisted condition source no longer parses as CEL.
    #[error(
        "CEL condition parse failed for edge {edge_id}: source={condition_source:?}; {parse_error}"
    )]
    ConditionParseFailed {
        edge_id: String,
        condition_source: String,
        parse_error: String,
    },
    /// Persisted modifier is not valid ModifierSpec JSON.
    #[error("modifier JSON invalid for edge {edge_id}: source={modifier_source:?}; {error}")]
    ModifierJsonInvalid {
        edge_id: String,
        modifier_source: String,
        error: String,
    },
    /// ModifierSpec parses but a CEL expression within it does not.
    #[error(
        "CEL modifier parse failed for edge {edge_id}: source={modifier_source:?}; {parse_error}"
    )]
    ModifierParseFailed {
        edge_id: String,
        modifier_source: String,
        parse_error: String,
    },
}

/// Collector for bootstrap errors. Implements `into_result(plan)` for
/// atomic-strict bootstrap: if any errors were pushed, return `Err`.
#[derive(Debug, Default)]
pub struct BootstrapErrors {
    items: Vec<BootstrapError>,
}

impl BootstrapErrors {
    /// Creates a new empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a bootstrap error to the collector.
    pub fn push(&mut self, e: BootstrapError) {
        self.items.push(e);
    }

    /// Returns `true` if no errors have been collected.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns the number of collected errors.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns a read-only slice of all collected errors.
    pub fn items(&self) -> &[BootstrapError] {
        &self.items
    }

    /// Atomic-strict merge: returns `Ok(ok)` if no errors were collected,
    /// otherwise returns `Err(self)` so the caller gets the full error list.
    pub fn into_result<T>(self, ok: T) -> Result<T, BootstrapErrors> {
        if self.is_empty() { Ok(ok) } else { Err(self) }
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::ContractView;
    use crate::SpawnedCellKind;
    use crate::factory::CellFactory;
    use std::sync::Arc;
    use tempfile::TempDir;

    struct OkFactory;
    impl CellFactory for OkFactory {
        fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
            Ok(())
        }
        fn spawn_cell(
            self: Arc<Self>,
            _: McPath,
            _: JsonValue,
            _: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
            _cell_dir: std::path::PathBuf,
            _contract: ContractView,
            _colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
            _idle_timeout: Option<std::time::Duration>,
            _cell_timeout: i64,
            _message_timeout: Option<std::time::Duration>,
            _blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
            _mailbox_capacity: usize,
        ) -> Result<SpawnedCellKind, String> {
            unimplemented!("not used in plan tests")
        }
    }

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn factories_with_echo() -> CellFactoryRegistry {
        let mut r: CellFactoryRegistry = std::collections::HashMap::new();
        r.insert("echo".into(), Arc::new(OkFactory));
        r
    }

    /// Empty identity overlay: these plan-phase tests predate Lifecycle-3a and
    /// assert on plan structure, not cell identity — an empty overlay means
    /// every cell gets a fresh cell_id, the pre-3a behaviour.
    fn empty_overlay() -> crate::persist::colony_db::RegistryOverlay {
        crate::persist::colony_db::RegistryOverlay::new()
    }

    #[test]
    fn plan_returns_plan_for_clean_tree() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        assert_eq!(plan.hives.len(), 1);
        assert_eq!(plan.cells.len(), 2);
        assert_eq!(plan.edges.len(), 1);
        assert_eq!(plan.edges[0].from.as_str(), "/a");
        assert_eq!(plan.edges[0].to.as_str(), "/b");
    }

    /// F1 fix (K-H1 reject #2 shape): full bootstrap wiring, honest contract.
    /// `cellA` behind the `/sub` hive transit declares `consumes.hop.hmark
    /// required:true`; the key is set on the edge INTO the hive
    /// (`entry → /sub`, `set_hop.hmark`). The runtime delivers it spec-exactly
    /// (hop survives the transit — K-H1 assert A1), so the plan must boot.
    #[test]
    fn plan_accepts_required_hop_delivered_across_hive_transit() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
                {"from":"./entry","to":"./sub","modifier":{"set_hop":{"hmark":"'HM-R2'"}}},
                {"from":"./sub","to":"./sub/cellA"}
            ]}}}"#,
        );
        write(
            td.path(),
            "main/entry/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/sub/config.json",
            r#"{"cell":{"type":"hive"}}"#,
        );
        write(
            td.path(),
            "main/sub/cellA/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},
                "consumes":{"hop":{"hmark":{"type":"string","required":true}}}}}"#,
        );

        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("honest transit-delivered required hop key must boot (F1 / K-H1)");
        assert_eq!(plan.hives.len(), 2);
        assert_eq!(plan.edges.len(), 2);
    }

    /// F1 fix, mandatory point 2 (loop shape): a 14a-style tool loop — the
    /// worker loops back INTO its own hive — with an honest required key
    /// keeps booting. The transit walk must terminate (proven by returning)
    /// and must not falsely empty the intersection at the loop edge: the
    /// worker re-emits the key (`emits.hop.k`), so BOTH inbound edges of
    /// `/loop` (entry `set_hop` + worker emits) provide it.
    #[test]
    fn plan_accepts_tool_loop_with_required_hop_across_transit() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
                {"from":"./entry","to":"./loop","modifier":{"set_hop":{"k":"'v'"}}},
                {"from":"./loop","to":"./loop/worker"},
                {"from":"./loop/worker","to":"./loop"}
            ]}}}"#,
        );
        write(
            td.path(),
            "main/entry/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/loop/config.json",
            r#"{"cell":{"type":"hive"}}"#,
        );
        write(
            td.path(),
            "main/loop/worker/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},
                "emits":{"hop":{"k":{"type":"string"}}},
                "consumes":{"hop":{"k":{"type":"string","required":true}}}}}"#,
        );

        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("tool-loop shape with honest required hop key must keep booting (F1)");
        assert_eq!(plan.edges.len(), 3);
    }

    #[test]
    fn plan_returns_no_root_dir_error() {
        let td = TempDir::new().unwrap();
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(matches!(err.items()[0], BootstrapError::NoRootDir));
    }

    #[test]
    fn plan_returns_multiple_root_dirs_error() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "other/config.json",
            r#"{"cell":{"type":"hive"}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(matches!(
            err.items()[0],
            BootstrapError::MultipleRootDirs { .. }
        ));
    }

    /// Phase 13.5-A1 T5: malformed CEL `condition` and unknown cell_type are
    /// both collected in the same plan-pass. (Pre-A1 this test asserted
    /// strict-fail on `condition` presence; A1 swaps that for CEL parse.)
    /// Truly unparseable CEL source: `"=="`.
    #[test]
    fn plan_collects_unknown_cell_type_plus_edge_condition_parse() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b","condition":"=="}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"unknown"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(
            err.items()
                .iter()
                .any(|e| matches!(e, BootstrapError::UnknownCellType { .. }))
        );
        assert!(
            err.items()
                .iter()
                .any(|e| matches!(e, BootstrapError::EdgeConditionParse { .. }))
        );
    }

    /// Phase 13.5-A1 T5: valid CEL condition is parsed and stored on PlannedEdge.
    #[test]
    fn plan_accepts_edge_with_valid_cel_condition() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b","condition":"headers.x == 'y'"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("plan accepts valid cel");
        assert_eq!(plan.edges.len(), 1, "one edge with valid condition");
        assert!(
            plan.edges[0].condition.is_some(),
            "condition is parsed and stored"
        );
    }

    /// Phase 13.5-A1 T5: malformed CEL condition is rejected with cel in error.
    #[test]
    fn plan_rejects_edge_with_malformed_cel_condition() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b","condition":"=="}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        let s = format!("{err:?}");
        assert!(s.to_lowercase().contains("cel"), "error mentions cel: {s}");
        assert!(
            err.items()
                .iter()
                .any(|e| matches!(e, BootstrapError::EdgeConditionParse { .. }))
        );
    }

    /// Phase 13.5-A1 T5 (Slice 3): malformed CEL modifier.set_hop is rejected.
    #[test]
    fn plan_rejects_edge_with_malformed_cel_modifier_set() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b","modifier":{"set_hop":{"tier":"=="}}}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(err.items().iter().any(
            |e| matches!(e, BootstrapError::EdgeModifierParse { key, .. } if key == "set_hop.tier")
        ));
    }

    #[test]
    fn plan_threads_restart_limit_through_to_planned_cell() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","restart_limit":3},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert_eq!(cell.restart_limit, Some(3));
    }

    /// Paket-6 C1: the overlay's persisted `status` maps into BOTH `active` and
    /// `failed` on the `PlannedCell`. `"failed"` → `active=false, failed=true`;
    /// `"inactive"` → `active=false, failed=false`; `"active"` →
    /// `active=true, failed=false`. The persisted state wins over edge-derived
    /// activity (overview Z.1426).
    #[test]
    fn plan_threads_failed_status_into_planned_cell() {
        use meclaw_core::Path;

        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // status == "failed" → active=false, failed=true.
        let mut overlay = empty_overlay();
        overlay.insert(Path::new("/a"), (Uuid::now_v7(), "failed".to_string()));
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &overlay).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert!(!cell.active, "failed cell must rehydrate active=false");
        assert!(cell.failed, "failed cell must rehydrate failed=true");

        // status == "inactive" → active=false, failed=false.
        let mut overlay = empty_overlay();
        overlay.insert(Path::new("/a"), (Uuid::now_v7(), "inactive".to_string()));
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &overlay).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert!(!cell.active, "inactive cell must rehydrate active=false");
        assert!(
            !cell.failed,
            "merely inactive cell must NOT be marked failed"
        );

        // status == "active" → active=true, failed=false.
        let mut overlay = empty_overlay();
        overlay.insert(Path::new("/a"), (Uuid::now_v7(), "active".to_string()));
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &overlay).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert!(cell.active, "active cell must rehydrate active=true");
        assert!(!cell.failed, "active cell must not be marked failed");
    }

    /// A5b (Phase-16 W1b, test a): on a REBOOT a cell directory whose path is
    /// absent from the persisted overlay is an unknown node — REPORTED in
    /// `unregistered_nodes`, never planned. Registration is
    /// instantiation/mutation-only; the reboot walk never adopts. The known
    /// (overlay-hit) node is planned as usual and the boot still succeeds.
    /// Red-demo: neutralising the Reboot-divert branch plans `/foreign` as a
    /// cell and leaves `unregistered_nodes` empty.
    #[test]
    fn plan_reboot_reports_unknown_cell_dir_without_adopting_it() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/known/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/foreign/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // Reboot overlay: `/known` is registered (persisted), `/foreign` is not.
        let mut overlay = empty_overlay();
        overlay.insert(
            McPath::new("/known"),
            (Uuid::now_v7(), "active".to_string()),
        );

        let plan = plan_bootstrap_with_env(
            td.path(),
            &factories_with_echo(),
            &overlay,
            BootState::Reboot,
            None,
        )
        .expect("reboot with an unknown cell dir still boots successfully");

        assert!(
            plan.unregistered_nodes
                .iter()
                .any(|p| p.as_str() == "/foreign"),
            "unknown cell dir must be REPORTED in unregistered_nodes; got {:?}",
            plan.unregistered_nodes
        );
        assert!(
            !plan.cells.iter().any(|c| c.path.as_str() == "/foreign"),
            "unknown cell dir must NOT be planned (no adoption) on reboot"
        );
        assert!(
            plan.cells.iter().any(|c| c.path.as_str() == "/known"),
            "the registered (overlay-hit) node must still be planned"
        );
    }

    /// A5b (Phase-16 W1b, test e): FirstBoot pin — the walk IS the source of
    /// truth on a first boot. Every cell dir is planned as a new entry, nothing
    /// is diverted to `unregistered_nodes` (symmetric counter to the reboot
    /// report). Guards against the divert leaking into the FirstBoot path.
    #[test]
    fn plan_first_boot_walks_all_cell_dirs_as_source() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let plan = plan_bootstrap_with_env(
            td.path(),
            &factories_with_echo(),
            &empty_overlay(),
            BootState::FirstBoot,
            None,
        )
        .unwrap();

        assert!(
            plan.unregistered_nodes.is_empty(),
            "FirstBoot reports nothing — the walk is the source of truth; got {:?}",
            plan.unregistered_nodes
        );
        assert!(plan.cells.iter().any(|c| c.path.as_str() == "/a"));
        assert!(plan.cells.iter().any(|c| c.path.as_str() == "/b"));
    }

    /// Deep-Audit F2 (b): a FirstBoot-classified tree (empty registry) that
    /// nonetheless carries an `in_flight` `mutation_log` row — the signature of a
    /// mutation that started but never committed (e.g. a mid-rename strict-fail
    /// panic on an otherwise-empty colony) — must REPORT overlay-miss cell dirs as
    /// unregistered orphans, NOT silently plan/adopt them. Closes the FirstBoot
    /// silent-adoption gap on the existing unregistered-node seam.
    #[test]
    fn plan_first_boot_with_in_flight_mutation_reports_orphans_not_adopts() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/orphan/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        // Seed colony.db with an in_flight mutation_log row (the half-applied
        // mutation signal) and NO registry rows (FirstBoot classification).
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO mutation_log (id, scope, payload_json, status, created_at) \
             VALUES ('MID', '/', '{}', 'in_flight', 100)",
            [],
        )
        .unwrap();
        drop(conn);

        let plan = plan_bootstrap_with_env(
            td.path(),
            &factories_with_echo(),
            &empty_overlay(),
            BootState::FirstBoot,
            None,
        )
        .expect("FirstBoot with an in_flight mutation still boots");

        assert!(
            plan.unregistered_nodes
                .iter()
                .any(|p| p.as_str() == "/orphan"),
            "an overlay-miss dir under a half-applied (in_flight) mutation must be \
             REPORTED, not silently adopted; got {:?}",
            plan.unregistered_nodes
        );
        assert!(
            !plan.cells.iter().any(|c| c.path.as_str() == "/orphan"),
            "the orphan must NOT be planned/adopted on a FirstBoot with an in_flight mutation"
        );
    }

    /// A7 (Phase-16 W1a, Ruling 2026-06-12 — Option B, recompute-mirror): the
    /// FIRST bootstrap recomputes activity for the nodes the boot connectivity
    /// computation REACHES (seeded by the `params.graph` edges). An island
    /// sub-hive's INTERNAL edge seeds the recompute over its own scope, so the
    /// island subtree is reached → derives INACTIVE (the disconnected hive gates
    /// it); wired top-level cells (reached via their edge) stay active. The root
    /// `/` is always active.
    #[test]
    fn plan_first_boot_derives_island_subtree_inactive() {
        let td = TempDir::new().unwrap();
        // Root hive wires two top-level cells `/a → /b` (both connected/active)
        // and contains an island sub-hive `/iso` with only INTERNAL wiring.
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/iso/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./x","to":"./y"}]}}}"#,
        );
        write(
            td.path(),
            "main/iso/x/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/iso/y/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let active = |p: &str| {
            plan.cells
                .iter()
                .find(|c| c.path.as_str() == p)
                .unwrap()
                .active
        };
        assert!(active("/a"), "/a is wired → active");
        assert!(active("/b"), "/b is wired → active");
        assert!(
            !active("/iso/x"),
            "/iso/x is in an island subtree → inactive at first boot (A7, no grace)"
        );
        assert!(
            !active("/iso/y"),
            "/iso/y is in an island subtree → inactive at first boot (A7, no grace)"
        );
    }

    /// A7 (Phase-16 W1a — Option B, grace preservation): an EDGE-LESS single
    /// cell is NOT reached by the boot connectivity recompute (no edge seeds its
    /// scope), so it keeps its instantiation-activity (boots ACTIVE) — exactly
    /// like a single-cell `add_nodes` without an edge at mutation time. This
    /// pins the boot↔mutation symmetry: the recompute-mirror does NOT mark a
    /// lone edge-less cell inactive.
    #[test]
    fn plan_first_boot_keeps_grace_for_edge_less_single_cell() {
        let td = TempDir::new().unwrap();
        // Root hive with NO edges + one edge-less child cell.
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/lonely/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan
            .cells
            .iter()
            .find(|c| c.path.as_str() == "/lonely")
            .unwrap();
        assert!(
            cell.active,
            "/lonely is edge-less → not reached by the recompute → keeps grace (active)"
        );
    }

    /// A5 (a): a PARKED cell — persisted `inactive` at reboot, disconnected
    /// (no incoming edge), with an HONEST `required` hop contract — must BOOT.
    /// At reboot it is pure bookkeeping (Registry-Rehydration); its contract
    /// obligation is not enforced at boot (the full check lives at the mutation
    /// that wires it). Pre-A5 the boot locality check rejected it (required hop
    /// with no incoming edge).
    #[test]
    fn plan_reboot_parked_cell_with_honest_required_contract_boots() {
        use meclaw_core::Path;
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/parked/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"route":{"type":"string","required":true}}}}}"#,
        );
        // Reboot: /parked persisted inactive (disconnected bookkeeping).
        let mut overlay = empty_overlay();
        overlay.insert(
            Path::new("/parked"),
            (Uuid::now_v7(), "inactive".to_string()),
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &overlay)
            .expect("parked inactive cell with honest required contract must boot (A5)");
        let cell = plan
            .cells
            .iter()
            .find(|c| c.path.as_str() == "/parked")
            .unwrap();
        assert!(!cell.active, "/parked rehydrates inactive");
    }

    /// A5 (b): an ISLAND cell (first boot, derived inactive per A7) with an
    /// honest `required` hop that its incoming edge does NOT provide must still
    /// BOOT — inactive ⇒ no boot-time contract enforcement. Uniform with (a)
    /// across both boot kinds.
    #[test]
    fn plan_first_boot_island_with_honest_required_contract_boots() {
        let td = TempDir::new().unwrap();
        // /iso is an island sub-hive (no external edge); internal a → c.
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/iso/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./c"}]}}}"#,
        );
        write(
            td.path(),
            "main/iso/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        // /iso/c requires hop.k, which a (empty emits.hop) does not provide.
        write(
            td.path(),
            "main/iso/c/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"k":{"type":"string","required":true}}}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("island cell with unprovided required hop must boot (A5: inactive ⇒ no check)");
        let cell = plan
            .cells
            .iter()
            .find(|c| c.path.as_str() == "/iso/c")
            .unwrap();
        assert!(!cell.active, "/iso/c is an island → inactive");
    }

    /// A5 (f) NON-VACUITY: an ACTIVE, wired cell whose `required` hop is NOT in
    /// the fan-in intersection of its incoming edges must STILL be rejected at
    /// boot — the locality checker stays sharp exactly where it applies. Guards
    /// against the A5 filter degenerating into a vacuous always-pass.
    #[test]
    fn plan_first_boot_active_wired_unfulfillable_required_is_rejected() {
        let td = TempDir::new().unwrap();
        // /a → /b, both wired/active. /b requires hop.k; /a's emits.hop is empty.
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./a","to":"./b"}]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{"hop":{"k":{"type":"string","required":true}}}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).expect_err(
            "active /b with unfulfillable required hop must be rejected (A5 non-vacuity)",
        );
        assert!(
            format!("{err:?}").contains("HeaderContractViolation"),
            "expected HeaderContractViolation, got {err:?}"
        );
    }

    #[test]
    fn plan_resolves_nested_hive_edges_against_subscope() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/pool/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./w1","to":"./w2"}]}}}"#,
        );
        write(
            td.path(),
            "main/pool/w1/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/pool/w2/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let edge = &plan.edges[0];
        assert_eq!(edge.from.as_str(), "/pool/w1");
        assert_eq!(edge.to.as_str(), "/pool/w2");
    }

    /// R12 symmetry pin: the bootstrap `params.graph` path NEVER had the
    /// level-1 restriction the mutation validator carried — a hive may declare
    /// DEPTH edges (`./pool/w1`) into its sub-scopes; `McPath::resolve`
    /// normalises them unrestricted (K-H1 boots nested shapes). Pinned so the
    /// boot and mutation paths stay in the same beat.
    #[test]
    fn plan_resolves_depth_edges_from_parent_hive_graph() {
        let td = TempDir::new().unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./pool/w1","to":"./sink"}]}}}"#,
        );
        write(
            td.path(),
            "main/pool/config.json",
            r#"{"cell":{"type":"hive"}}"#,
        );
        write(
            td.path(),
            "main/pool/w1/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/sink/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );

        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let edge = &plan.edges[0];
        assert_eq!(edge.from.as_str(), "/pool/w1");
        assert_eq!(edge.to.as_str(), "/sink");
    }

    #[test]
    fn plan_bootstrap_accepts_cell_timeout_as_i64() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","timeout":-1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert_eq!(cell.cell_timeout, -1);
    }

    #[test]
    fn planned_cell_propagates_idle_timeout_ms() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","idle_timeout_ms":250},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert_eq!(cell.idle_timeout_ms, Some(250));
    }

    #[test]
    fn planned_cell_idle_timeout_ms_defaults_to_none() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert!(cell.idle_timeout_ms.is_none());
    }

    #[test]
    fn planned_cell_propagates_message_timeout() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","message_timeout":5000},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert_eq!(cell.message_timeout, Some(5000));
    }

    #[test]
    fn planned_cell_message_timeout_defaults_to_none() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert!(cell.message_timeout.is_none());
    }

    #[test]
    fn plan_bootstrap_cell_timeout_defaults_to_zero_when_absent() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        assert_eq!(cell.cell_timeout, 0);
    }

    /// `cell.message_timeout` set → plan succeeds (boot remains green). Since
    /// P3-B-plumb-2 the value is also propagated into `PlannedCell.message_timeout`
    /// (covered by `planned_cell_propagates_message_timeout`); here we only assert
    /// the happy-path plan still builds.
    #[test]
    fn plan_bootstrap_with_message_timeout_boot_stays_green() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","message_timeout":5000},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        // Must succeed — message_timeout is deferred, not a hard error.
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("cell with message_timeout must not be rejected at boot");
        assert_eq!(plan.cells.len(), 1, "one cell planned");
    }

    /// T1 (Red → Green): `cell.message_timeout` must pass the ALLOWED_CELL_FIELDS check.
    #[test]
    fn plan_bootstrap_accepts_cell_message_timeout() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","message_timeout":5000},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("message_timeout must not trigger UnknownCellField");
        assert_eq!(plan.cells.len(), 1);
    }

    /// T3 (Red → Green): `cell.mailbox_size` must pass the ALLOWED_CELL_FIELDS check.
    #[test]
    fn plan_bootstrap_accepts_cell_mailbox_size() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","mailbox_size":16},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect("mailbox_size must not trigger UnknownCellField");
        assert_eq!(plan.cells.len(), 1);
    }

    /// T5: `PlannedCell.mailbox_size` propagates from `CellHeader::mailbox_size`.
    /// With field present: `Some(value)`. Without field: `None`.
    #[test]
    fn planned_cell_propagates_mailbox_size() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","mailbox_size":16},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let a = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        let b = plan.cells.iter().find(|c| c.path.as_str() == "/b").unwrap();
        assert_eq!(a.mailbox_size, Some(16), "mailbox_size:16 must propagate");
        assert_eq!(b.mailbox_size, None, "absent mailbox_size must be None");
    }

    /// T4: `cell.mailbox_size:0` is a hard boot error — zero capacity has no
    /// valid semantics. The error variant is `InvalidCellField` with
    /// `field == "cell.mailbox_size"`.
    #[test]
    fn plan_bootstrap_rejects_mailbox_size_zero() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","mailbox_size":0},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay())
            .expect_err("mailbox_size:0 must be rejected");
        assert!(
            err.items().iter().any(|e| matches!(
                e,
                BootstrapError::InvalidCellField { field, .. }
                    if field == "cell.mailbox_size"
            )),
            "expected InvalidCellField for cell.mailbox_size, got: {:?}",
            err.items()
        );
    }

    #[test]
    fn plan_bootstrap_still_rejects_other_unknown_cell_field() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo","totally_unknown_field":42},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(err.items().iter().any(|e| matches!(e,
            BootstrapError::UnknownCellField { field, .. } if field == "cell.totally_unknown_field")));
    }

    #[test]
    fn plan_bootstrap_rejects_malformed_emits_schema() {
        // Verifies that the boot loop surfaces InvalidEmitsSchema loudly
        // (analog UnknownCellField — Boot-Strict-Kultur, config.md Z.37).
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{},
              "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"body":{"x":{"type":"stringg"}},"hop":{}}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(
            err.items()
                .iter()
                .any(|e| matches!(e, BootstrapError::InvalidEmitsSchema { .. }))
        );
    }

    #[test]
    fn plan_bootstrap_rejects_missing_contract_presence() {
        // Hardening Slice 4 (Task 4.2): a NON-hive config.json that does not
        // declare the builder-mandatory presence keys (`contract.version` /
        // `settings` / `consumes`) is a LOUD boot error (config.md § contract,
        // Enforcement-Stufen). The hive marker stays exempt (no contract block).
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{},
              "contract":{"settings":{},"consumes":{}}}"#,
        );
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(
            err.items().iter().any(|e| matches!(
                e,
                BootstrapError::ContractIncomplete { reason, .. } if reason.contains("version")
            )),
            "expected ContractIncomplete naming version, got: {:?}",
            err.items()
        );
    }

    #[test]
    fn plan_bootstrap_rejects_corrupt_cell_db() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        // A deliberately broken cell.db
        std::fs::write(td.path().join("main/a/cell.db"), b"not a sqlite file").unwrap();
        let err = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        assert!(
            err.items()
                .iter()
                .any(|e| matches!(e, BootstrapError::CorruptCellDb { .. }))
        );
    }

    #[test]
    fn plan_bootstrap_accepts_absent_cell_db() {
        // Absent cell.db = first-boot, OK
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        assert_eq!(plan.cells.len(), 1);
    }

    #[test]
    fn planned_cell_carries_contract_view_from_config_json() {
        let td = TempDir::new().unwrap();
        std::fs::create_dir_all(td.path().join("root_cell")).unwrap();
        std::fs::write(
            td.path().join("root_cell/config.json"),
            r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{},"multi_send_capable":true}}"#,
        )
        .unwrap();
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan
            .cells
            .iter()
            .find(|c| c.fs_path.ends_with("root_cell"))
            .unwrap();
        assert!(cell.contract_view.multi_send_capable);
    }

    #[test]
    fn malformed_emits_is_a_boot_error_not_silent() {
        // The plan phase must report a broken emits schema as a BootstrapError
        // (analog UnknownCellField — Boot-Strict-Kultur, config.md Z.37).
        let cfg_json = r#"{"cell":{"type":"code"},"params":{"runner":"python3"},
          "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"body":{"x":{"type":"stringg"}},"hop":{}}}}"#;
        let block: crate::config::ContractBlock =
            meclaw_core::serde_json::from_str::<crate::config::ParsedConfig>(cfg_json)
                .unwrap()
                .contract;
        let err = compile_contract_view(&block).unwrap_err();
        assert!(err.contains("emits.body"), "loud schema error: {err}");
    }

    #[test]
    fn well_formed_emits_compiles_into_contract_view() {
        let cfg_json = r#"{"cell":{"type":"code"},"params":{},
          "contract":{"version":"0.1.0","settings":{},"consumes":{},"emits":{"body":{"messages":{"type":"array"}},"hop":{}}}}"#;
        let block: crate::config::ContractBlock =
            meclaw_core::serde_json::from_str::<crate::config::ParsedConfig>(cfg_json)
                .unwrap()
                .contract;
        let cv = compile_contract_view(&block).unwrap();
        assert!(cv.emits.is_some());
    }

    #[test]
    fn plan_bootstrap_assigns_cell_id_uuid_v7() {
        let td = TempDir::new().unwrap();
        write(td.path(), "main/config.json", r#"{"cell":{"type":"hive"}}"#);
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
        let _ = cell.cell_id; // accessible field of type Uuid
        // v7 has timestamp in first 48 bits; sanity-check that two cells get distinct ids.
        write(
            td.path(),
            "main/b/config.json",
            r#"{"cell":{"type":"echo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan2 = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        let ids: std::collections::HashSet<_> = plan2.cells.iter().map(|c| c.cell_id).collect();
        assert_eq!(ids.len(), plan2.cells.len(), "cell_ids unique across plan");
    }

    /// Substrat-Fix Befund 4 — boot-time `${ENV_VAR}` substitution. Spec
    /// § Variable substitution: substituted "when reading config.json"; the
    /// mutation path and the boot path share the same substitution model
    /// (overview Z.1366/1367, same error table). The plan reads `{root}/.env`
    /// and substitutes every config.json value, so the cell (and
    /// `validate_params`) sees only the substituted value.
    #[test]
    fn plan_substitutes_env_var_in_params_from_root_env() {
        let td = TempDir::new().unwrap();
        std::fs::write(td.path().join(".env"), "FOO=hello-env\n").unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"greeting":"${FOO}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        assert_eq!(plan.cells.len(), 1);
        assert_eq!(
            plan.cells[0].params["greeting"], "hello-env",
            "boot must substitute ${{FOO}} from {{root}}/.env"
        );
    }

    /// Befund 4 — missing plain `${VAR}` without default at boot → plan error
    /// (spec § Behavior on errors l.1366: daemon failed-to-start; the error text
    /// carries the `env_var_missing` token so `--validate` stderr matches).
    #[test]
    fn plan_missing_env_var_without_default_fails_to_start() {
        let td = TempDir::new().unwrap();
        // No .env at all — ${UNSET_XYZ} has no value and no default.
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"greeting":"${UNSET_XYZ}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let errs = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap_err();
        let rendered = format!("{errs:?}");
        assert!(
            rendered.contains("env_var_missing"),
            "plan error must carry the env_var_missing token: {rendered}"
        );
        assert!(
            rendered.contains("UNSET_XYZ"),
            "plan error must name the missing variable: {rendered}"
        );
    }

    /// U7 — `--env <path>` override: the `.env` lives OUTSIDE `{root}`; the
    /// root carries none. `plan_bootstrap_with_env` must read the explicit
    /// path; the 3-arg `plan_bootstrap` keeps the `{root}/.env` default
    /// (existing Befund-4 tests pin that).
    #[test]
    fn plan_with_env_override_reads_env_from_explicit_path() {
        let td = TempDir::new().unwrap();
        let env_dir = TempDir::new().unwrap();
        let env_file = env_dir.path().join("custom.env");
        std::fs::write(&env_file, "FOO=from-override\n").unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"greeting":"${FOO}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap_with_env(
            td.path(),
            &factories_with_echo(),
            &empty_overlay(),
            BootState::FirstBoot,
            Some(&env_file),
        )
        .unwrap();
        assert_eq!(
            plan.cells[0].params["greeting"], "from-override",
            "boot must substitute ${{FOO}} from the --env override path"
        );
    }

    /// Befund 4 — POSIX default + escape forms behave exactly like the mutation
    /// path: `${UNSET:-fb}` resolves to the fallback, `$${FOO}` stays a literal
    /// `${FOO}` (substitution runs exclusively at instantiation).
    #[test]
    fn plan_env_default_and_escape_forms_match_mutation_semantics() {
        let td = TempDir::new().unwrap();
        std::fs::write(td.path().join(".env"), "FOO=real\n").unwrap();
        write(
            td.path(),
            "main/config.json",
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write(
            td.path(),
            "main/a/config.json",
            r#"{"cell":{"type":"echo"},"params":{"with_default":"${UNSET:-fb}","escaped":"$${FOO}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
        let plan = plan_bootstrap(td.path(), &factories_with_echo(), &empty_overlay()).unwrap();
        assert_eq!(plan.cells[0].params["with_default"], "fb");
        assert_eq!(plan.cells[0].params["escaped"], "${FOO}");
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn collector_accumulates_multiple_errors() {
        let mut errs = BootstrapErrors::new();
        errs.push(BootstrapError::NoRootDir);
        errs.push(BootstrapError::InvalidPath { reason: "x".into() });
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn into_result_returns_ok_when_empty() {
        let errs = BootstrapErrors::new();
        let r: Result<i32, _> = errs.into_result(42);
        assert_eq!(r.unwrap(), 42);
    }

    #[test]
    fn into_result_returns_err_when_nonempty() {
        let mut errs = BootstrapErrors::new();
        errs.push(BootstrapError::NoRootDir);
        let r: Result<i32, _> = errs.into_result(42);
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().len(), 1);
    }

    #[test]
    fn bootstrap_error_has_corrupt_cell_db_variant() {
        let e = BootstrapError::CorruptCellDb {
            path: std::path::PathBuf::from("/tmp/x"),
            reason: "quick_check failed".into(),
        };
        assert!(matches!(e, BootstrapError::CorruptCellDb { .. }));
    }

    #[test]
    fn bootstrap_error_has_inconsistent_colony_db_variant() {
        let e = BootstrapError::InconsistentColonyDb {
            reason: "mixed state: edges empty, registry non-empty".into(),
        };
        assert!(matches!(e, BootstrapError::InconsistentColonyDb { .. }));
    }

    #[test]
    fn bootstrap_error_has_unknown_cell_field_variant() {
        let e = BootstrapError::UnknownCellField {
            path: std::path::PathBuf::from("/tmp/y/config.json"),
            field: "cell.timeout".into(),
        };
        assert!(matches!(e, BootstrapError::UnknownCellField { .. }));
    }
}

#[cfg(test)]
mod boot_state_tests {
    use super::*;

    #[test]
    fn boot_state_absent_db_returns_first_boot() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        // The file does NOT exist
        let bs = probe_boot_state(&db_path).unwrap();
        assert!(matches!(bs, BootState::FirstBoot));
    }

    #[test]
    fn boot_state_empty_initialized_db_returns_first_boot() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        // Initialize DB schema but insert nothing.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        drop(conn);
        let bs = probe_boot_state(&db_path).unwrap();
        assert!(matches!(bs, BootState::FirstBoot));
    }

    #[test]
    fn boot_state_all_tables_non_empty_returns_reboot() {
        use meclaw_core::Uuid;
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at)
             VALUES ('/a', ?, 'echo', 'active', 0, 0)",
            rusqlite::params![Uuid::now_v7().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (id, from_path, to_path, created_at) VALUES (?, '/a', '/b', 0)",
            rusqlite::params![Uuid::now_v7().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO hive_scopes (path, created_at) VALUES ('/h', 0)",
            [],
        )
        .unwrap();
        drop(conn);
        let bs = probe_boot_state(&db_path).unwrap();
        assert!(matches!(bs, BootState::Reboot));
    }

    #[test]
    fn boot_state_mixed_returns_inconsistent() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        // Only edges has data — a mixed state.
        conn.execute(
            "INSERT INTO edges (id, from_path, to_path, created_at) VALUES ('id1', '/a', '/b', 0)",
            [],
        )
        .unwrap();
        drop(conn);
        let bs = probe_boot_state(&db_path).unwrap();
        assert!(
            matches!(bs, BootState::Inconsistent { .. }),
            "mixed edges-non-empty + registry-empty → Inconsistent"
        );
    }

    /// Bootstrap-Recovery (Run-5/5b-Befund): a mixed state WITH the durable
    /// `bootstrap_in_flight` meta marker is an interrupted FIRST apply, not
    /// external corruption — it classifies as `FirstBoot` (idempotent resume,
    /// deterministic rebuild from the filesystem). The marker-less mixed state
    /// stays `Inconsistent` (pin above).
    #[test]
    fn boot_state_mixed_with_bootstrap_marker_returns_first_boot() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        // Run-5b shape: registry rows committed, edges/hive_scopes never written.
        conn.execute(
            "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at)
             VALUES ('/a', ?, 'echo', 'active', 0, 0)",
            rusqlite::params![Uuid::now_v7().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('bootstrap_in_flight', '0')",
            [],
        )
        .unwrap();
        drop(conn);
        let bs = probe_boot_state(&db_path).unwrap();
        assert_eq!(
            bs,
            BootState::FirstBoot,
            "interrupted first apply (marker present) must resume as FirstBoot"
        );
    }

    /// The marker also wins over an all-full table state: a crash between the
    /// last batch commit and nothing (defensive — InitialApply clears the
    /// marker in the SAME transaction, so this state is constructionally
    /// unreachable, but the classification must stay resume-safe).
    #[test]
    fn boot_state_full_with_bootstrap_marker_returns_first_boot() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::persist::setup_colony_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO registry (path, cell_id, cell_type, status, created_at, updated_at)
             VALUES ('/a', ?, 'echo', 'active', 0, 0)",
            rusqlite::params![Uuid::now_v7().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (id, from_path, to_path, created_at) VALUES ('id1', '/a', '/b', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO hive_scopes (path, created_at) VALUES ('/h', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('bootstrap_in_flight', '0')",
            [],
        )
        .unwrap();
        drop(conn);
        let bs = probe_boot_state(&db_path).unwrap();
        assert_eq!(
            bs,
            BootState::FirstBoot,
            "marker present → resume as FirstBoot regardless of table counts \
             (the apply path is idempotent)"
        );
    }
}
