//! GH #169 — `move_nodes`: the same cell at a different address.
//!
//! A path is a cell's identity, which is why there was no operation that
//! changed one. Tidying a capability into the hive it belongs to therefore
//! meant `add_nodes` at the new path, `add_edges` for every edge the old node
//! had, `remove_nodes` on the old one, plus an operator wipe outside the
//! mutation flow — and, since an edge cannot address a node the same diff
//! creates unless it is spelled as a path (#166), in practice two mutations.
//! That costs the cell's `cell_id`, its `instantiated_at` and its `cell.db`,
//! makes every condition and modifier a re-typed string at the new address,
//! and opens a window in which the lane is either wired twice (the call fans
//! out and runs twice) or not at all (it dead-letters).
//!
//! ```json
//! {"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}
//! ```
//!
//! Deliberately NOT `swap_nodes`, which is the closest existing operation and
//! the opposite intent: a swap swings the external edges of one implementation
//! onto a DIFFERENT one, with its own identity and its own `cell.db`. A move
//! keeps the cell and changes where it lives. The edge mechanics are literally
//! shared — [`crate::mutation::swap::plan_edge_swing`] re-points every edge
//! naming a path onto another, and a move needs exactly that — but the two
//! operations mean different things and are named accordingly.
//!
//! # What a move carries, and what it deliberately does not
//!
//! A path is keyed on in more places than the registry. This module owns:
//!
//! * the **directory**, moved by `rename(2)` — `config.json` (with its
//!   `cell.id`), `cell.db` and everything else inside travel as one inode;
//! * the **registry row**, re-addressed by an UPDATE
//!   ([`crate::persist::writer::ColonyWriteOp::MoveRegistryPath`]), so
//!   `cell_id`, `created_at` and the provenance columns survive;
//! * every **edge** naming the old path at either end, condition and modifier
//!   included.
//!
//! It deliberately does not own:
//!
//! * the parent hive's `params.graph`. Since GH #168 the persisted edge table
//!   IS the boot topology on a `Reboot`; the `config.json` blocks are still
//!   parsed (a malformed CEL condition stays a loud boot error) but no longer
//!   decide anything. Rewriting a neighbouring node's file to keep a hint in
//!   sync would put the colony back in the business of treating the file as
//!   state, which is the defect #168 fixed.
//! * the canvas store's node positions. Those live in another cell's own
//!   `cell.db`, and § Database isolation forbids the colony writing there —
//!   even for its own convenience. The canvas re-places a node it does not
//!   recognise; that is a cosmetic cost, and the alternative is a hole in the
//!   isolation rule.
//! * **hives.** Moving a hive means moving every child's registry row, every
//!   subtree-internal edge and the hive scope itself, and a half-moved hive
//!   leaves its children addressed under a path that no longer exists — the
//!   boot failure #168 is about. [`validate_move_nodes`] refuses it by name
//!   instead of doing part of it.

use super::MutationError;
use crate::mutation::validate::name_is_taken;
use meclaw_core::Path as McPath;
use meclaw_core::serde_json::Value as JsonValue;
use std::path::PathBuf;

/// One resolved `move_nodes` entry: which cell, and where it is going.
///
/// Both the logical addresses and their on-disk directories, because a
/// relocation has to change both and they are resolved by different rules
/// (`resolve_scoped_path` for the logical one, `path_truth::resolve_cell_dir`
/// for the filesystem one, which anchors under the single root cell directory).
#[derive(Debug, Clone)]
pub(crate) struct PlannedMove {
    /// The logical address the cell is leaving.
    pub(crate) from: McPath,
    /// The logical address it is taking.
    pub(crate) to: McPath,
    /// The cell's directory today.
    pub(crate) from_dir: PathBuf,
    /// Where that directory is going.
    pub(crate) to_dir: PathBuf,
}

/// Resolve every `move_nodes` entry of a diff.
///
/// Schema only — whether the source exists and the target is free is
/// [`validate_move_nodes`]'s question. A diff with no `move_nodes` key yields
/// an empty vector, which is what makes this callable unconditionally.
pub(crate) fn plan_moves(
    scope: &str,
    diff: &JsonValue,
    root: &std::path::Path,
) -> Result<Vec<PlannedMove>, MutationError> {
    let Some(entries) = diff.get("move_nodes") else {
        return Ok(Vec::new());
    };
    let entries = entries
        .as_array()
        .ok_or_else(|| MutationError::Schema("move_nodes must be an array".into()))?;
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let from_name = e
            .get("match")
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("move_nodes[].match.name missing".into()))?;
        let to_name = e
            .get("to")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                MutationError::Schema("move_nodes[].to missing (non-empty string)".into())
            })?;
        let from = super::resolve_scoped_path(scope, from_name);
        let to = super::resolve_scoped_path(scope, to_name);
        if from == to {
            return Err(MutationError::Schema(format!(
                "move_nodes: '{from_name}' and '{to_name}' are the same path ({}) — a move to \
                 where the cell already is has nothing to do",
                from.as_str()
            )));
        }
        out.push(PlannedMove {
            from_dir: cell_dir_of(root, &from),
            to_dir: cell_dir_of(root, &to),
            from,
            to,
        });
    }
    Ok(out)
}

/// The on-disk directory of an ALREADY-RESOLVED logical path.
///
/// Goes through `path_truth` like every other logical→fs mapping, but with the
/// scope already folded in: the diff name has been resolved against the scope
/// by the caller, so passing it on as a scope-relative name a second time would
/// resolve it twice.
fn cell_dir_of(root: &std::path::Path, path: &McPath) -> PathBuf {
    crate::path_truth::resolve_cell_dir(root, "/", path.as_str().trim_start_matches('/'))
}

/// Pre-destructive validation of every `move_nodes` entry: the source must be a
/// cell that exists, the target must be free, and neither may be a hive.
///
/// The name-vs-path namespace decision is [`name_is_taken`]'s (GH #179) — the
/// same one `add_nodes`, `remove_nodes` and `swap_nodes` ask, so a multi-segment
/// `to` such as `talky/fetch` is compared against the paths that exist rather
/// than against a set of short names it could never equal. Scope containment is
/// NOT re-checked here: `validate_scope_containment` runs before this and has
/// already refused `..` segments and absolute names, which is what makes the
/// resolved paths below scope-contained by construction.
///
/// `registry_names`/`deep_registry_paths` and `hive_names`/`deep_hive_paths` are
/// the two spellings of the same pre-state, exactly as
/// `validate_naming_and_match` takes them. `add_names` are the `add_nodes` names
/// of the SAME diff — a move and an instantiation aiming at one free path are
/// two claims on it, and the second one to be applied would land on a directory
/// that now exists.
///
/// GH #195: in-diff claims are decided in ONE place now, and it is not this one.
/// `collect_duplicate_claims` (`validate.rs`) spans `add_nodes`,
/// `swap_nodes[].with` and `move_nodes[].to` and runs before this function, so a
/// duplicate claim is already refused — with a message that names both entries —
/// by the time a move gets here. The `add_names` check below stays because this
/// is a pure function with its own tests and must not start depending on a
/// caller's ordering to hold its contract; what it must NOT become is a third
/// spelling of the rule. A claimant that is missing here belongs in the claim
/// set over there.
///
/// `ground` answers the two questions the registry cannot. A directory with no
/// registry row is still occupied ground, and renaming onto it would bury
/// whatever is inside. And the target's parent has to exist already: the move is
/// a bare `rename(2)`, and creating the missing levels would leave directories
/// behind that are neither a cell nor a hive — nodes the next boot walks into
/// and cannot classify.
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_move_nodes(
    moves: &[PlannedMove],
    scope: &str,
    registry_names: &[String],
    deep_registry_paths: &[String],
    hive_names: &[String],
    deep_hive_paths: &[String],
    add_names: &[String],
    ground: &dyn Fn(&PlannedMove) -> TargetGround,
) -> Result<(), MutationError> {
    // Targets claimed earlier in this same diff, resolved — two moves onto one
    // address are one address, whatever the two spellings look like.
    let mut claimed: Vec<String> = add_names
        .iter()
        .map(|n| super::resolve_scoped_path(scope, n).as_str().to_string())
        .collect();
    for mv in moves {
        let from = mv.from.as_str();
        let to = mv.to.as_str();
        // A hive first, because "is a hive" is the more useful answer than "is
        // not a registered cell": a hive has no registry row at all, so the
        // existence check below would report it as a typo.
        if is_hive(scope, from, hive_names, deep_hive_paths) {
            return Err(MutationError::Schema(format!(
                "move_nodes: '{from}' is a hive. Moving a hive means moving every child's \
                 registry row, every subtree-internal edge and the hive scope itself; this \
                 version does not, and a half-moved hive leaves its children addressed under a \
                 path that no longer exists. Move the cells inside it instead (GH #169)"
            )));
        }
        if !name_is_taken(scope, from, registry_names, deep_registry_paths) {
            return Err(MutationError::MatchNoHit(from.into()));
        }
        // A cell with registered descendants is a hive in all but its
        // `cell.type` — the same subtree the refusal above is about.
        let prefix = format!("{from}/");
        if deep_registry_paths.iter().any(|p| p.starts_with(&prefix))
            || deep_hive_paths.iter().any(|p| p.starts_with(&prefix))
        {
            return Err(MutationError::Schema(format!(
                "move_nodes: '{from}' has nodes beneath it, which a move would leave addressed \
                 under a path that no longer exists (GH #169)"
            )));
        }
        // The target must be free in every namespace that can hold ground:
        // registry, hive scopes, this diff's own claims, and the filesystem.
        let ground = ground(mv);
        if name_is_taken(scope, to, registry_names, deep_registry_paths)
            || is_hive(scope, to, hive_names, deep_hive_paths)
            || claimed.iter().any(|c| c == to)
            || ground.occupied
        {
            return Err(MutationError::NamingCollision(to.into()));
        }
        if !ground.parent_exists {
            return Err(MutationError::Schema(format!(
                "move_nodes: nothing holds '{to}' — its parent directory does not exist. A move \
                 is a rename, not an instantiation: it lands a cell inside a hive that is \
                 already there (GH #169)"
            )));
        }
        claimed.push(to.to_string());
    }
    Ok(())
}

/// What the filesystem says about a move's target: whether something is already
/// there, and whether the place it is going to exists at all.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetGround {
    /// A directory already sits at the target path.
    pub(crate) occupied: bool,
    /// The target's parent directory exists.
    pub(crate) parent_exists: bool,
}

/// Whether `name` addresses a hive, tested in whichever namespace the name
/// itself selects (GH #179). Hives carry no registry row, so this is a separate
/// question from "is a registered cell", not a refinement of it.
fn is_hive(scope: &str, name: &str, hive_names: &[String], deep_hive_paths: &[String]) -> bool {
    name_is_taken(scope, name, hive_names, deep_hive_paths)
}

/// Everything the spawn loop needs in order to build a relocated cell at its new
/// address, read out of the cell's own `config.json`.
///
/// A relocation names no template — the node's configuration is whatever it
/// already carried, which is also why nothing here is written back. The read
/// mirrors the boot's (`plan_bootstrap_with_env`): `${VAR}` env substitution
/// first, then parse, then compile the contract, so a relocated cell is built
/// from exactly the same values a reboot would build it from.
pub(crate) struct RelocatedNode {
    /// `cell.type` — the registry's `cell_type` and the factory key.
    pub(crate) cell_type: String,
    /// The `params` block, env-substituted.
    pub(crate) params: JsonValue,
    /// Compiled `contract`, as `spawn_cell` takes it.
    pub(crate) contract_view: crate::factory::ContractView,
    /// `cell.timeout` (0 = idle model, >0 one-shot, -1 persistent).
    pub(crate) cell_timeout: i64,
    /// Optional `cell.idle_timeout_ms` override.
    pub(crate) idle_timeout_ms: Option<u64>,
    /// Optional `cell.message_timeout` (B-backstop) override.
    pub(crate) message_timeout: Option<i64>,
    /// Optional `cell.mailbox_size` override.
    pub(crate) mailbox_size: Option<usize>,
    /// 14-B header projection of the same `contract` block, for the colony's
    /// `node_contracts` map at the new address.
    pub(crate) header_view: crate::mutation::validate::HeaderNodeView,
}

/// Read a node's own `config.json` into the shape the spawn loop consumes.
///
/// Called BEFORE the directory is renamed, so a node whose `config.json` cannot
/// be read, substituted, parsed or compiled is a clean pre-destructive reject —
/// nothing has moved at that point.
pub(crate) fn read_relocated_node(
    cell_dir: &std::path::Path,
    env: &std::collections::HashMap<String, String>,
) -> Result<RelocatedNode, MutationError> {
    let cfg_path = cell_dir.join("config.json");
    let raw = std::fs::read_to_string(&cfg_path).map_err(|e| {
        MutationError::Schema(format!("move_nodes: read {}: {e}", cfg_path.display()))
    })?;
    let parsed: JsonValue = meclaw_core::serde_json::from_str(&raw).map_err(|e| {
        MutationError::Schema(format!("move_nodes: parse {}: {e}", cfg_path.display()))
    })?;
    // Env-only, exactly as the boot does: `${ctx.*}` and `${uuid7:*}` are
    // mutation-side tokens that were resolved once at instantiation and have no
    // filesystem-side producer.
    let substituted = super::substitute::substitute_env_only(&parsed, env)?;
    let cfg: crate::config::ParsedConfig = meclaw_core::serde_json::from_value(substituted)
        .map_err(|e| {
            MutationError::Schema(format!(
                "move_nodes: {} is not a cell config: {e}",
                cfg_path.display()
            ))
        })?;
    let contract_view =
        crate::bootstrap::compile_spawn_view(&cfg.contract, &cfg.params).map_err(|e| {
            MutationError::Schema(format!(
                "move_nodes: {} does not compile into a spawn view: {e}",
                cfg_path.display()
            ))
        })?;
    let header_view = crate::mutation::validate::header_view_from_contract(&cfg.contract);
    Ok(RelocatedNode {
        cell_type: cfg.cell.cell_type,
        params: cfg.params,
        contract_view,
        cell_timeout: cfg.cell.timeout,
        idle_timeout_ms: cfg.cell.idle_timeout_ms,
        message_timeout: cfg.cell.message_timeout,
        mailbox_size: cfg.cell.mailbox_size,
        header_view,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn moves(scope: &str, diff: &JsonValue) -> Result<Vec<PlannedMove>, MutationError> {
        plan_moves(scope, diff, std::path::Path::new("/tmp/does-not-matter"))
    }

    #[test]
    fn a_diff_without_move_nodes_plans_nothing() {
        let plan = moves("/", &json!({"add_nodes": []})).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn names_resolve_against_the_scope() {
        let plan = moves(
            "/main",
            &json!({"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}),
        )
        .unwrap();
        assert_eq!(plan[0].from.as_str(), "/main/fetch");
        assert_eq!(plan[0].to.as_str(), "/main/talky/fetch");
    }

    /// Befund 6: `./a` and `a` denote the same node, so the two spellings must
    /// not look like a move from a path to itself — nor like two different ones.
    #[test]
    fn the_canonical_dot_slash_prefix_resolves_away() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "./fetch"}, "to": "./talky/fetch"}]}),
        )
        .unwrap();
        assert_eq!(plan[0].from.as_str(), "/fetch");
        assert_eq!(plan[0].to.as_str(), "/talky/fetch");
    }

    #[test]
    fn a_move_to_where_the_cell_already_is_is_a_schema_error() {
        let err = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "fetch"}, "to": "./fetch"}]}),
        )
        .unwrap_err();
        assert!(matches!(err, MutationError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn a_missing_target_is_a_schema_error() {
        let err = moves("/", &json!({"move_nodes": [{"match": {"name": "fetch"}}]})).unwrap_err();
        assert!(matches!(err, MutationError::Schema(_)), "got {err:?}");
    }

    /// The filesystem a validation test wants unless it says otherwise: the
    /// target is free and the hive it goes into is there.
    fn free_ground(_: &PlannedMove) -> TargetGround {
        TargetGround {
            occupied: false,
            parent_exists: true,
        }
    }

    #[test]
    fn a_source_that_names_nothing_is_match_no_hit() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "ghost"}, "to": "talky/ghost"}]}),
        )
        .unwrap();
        let err =
            validate_move_nodes(&plan, "/", &[], &[], &[], &[], &[], &free_ground).unwrap_err();
        assert!(matches!(err, MutationError::MatchNoHit(_)), "got {err:?}");
    }

    #[test]
    fn a_hive_source_is_refused_by_name() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "talky"}, "to": "moved"}]}),
        )
        .unwrap();
        let err = validate_move_nodes(
            &plan,
            "/",
            &[],
            &[],
            &["talky".to_string()],
            &["/talky".to_string()],
            &[],
            &free_ground,
        )
        .unwrap_err();
        match err {
            MutationError::Schema(msg) => assert!(msg.contains("hive"), "got {msg}"),
            other => panic!("expected a Schema reject naming the hive, got {other:?}"),
        }
    }

    #[test]
    fn a_source_with_registered_descendants_is_refused() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "unit"}, "to": "talky/unit"}]}),
        )
        .unwrap();
        let err = validate_move_nodes(
            &plan,
            "/",
            &["unit".to_string()],
            &["/unit".to_string(), "/unit/child".to_string()],
            &[],
            &[],
            &[],
            &free_ground,
        )
        .unwrap_err();
        assert!(matches!(err, MutationError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn an_occupied_target_is_a_naming_collision() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "fetch"}, "to": "anchor"}]}),
        )
        .unwrap();
        let err = validate_move_nodes(
            &plan,
            "/",
            &["fetch".to_string(), "anchor".to_string()],
            &["/fetch".to_string(), "/anchor".to_string()],
            &[],
            &[],
            &[],
            &free_ground,
        )
        .unwrap_err();
        assert!(
            matches!(err, MutationError::NamingCollision(_)),
            "got {err:?}"
        );
    }

    /// A directory with no registry row is still occupied ground — renaming
    /// onto it would bury whatever `cell.db` is inside.
    #[test]
    fn an_unregistered_directory_at_the_target_is_a_naming_collision() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}),
        )
        .unwrap();
        let err = validate_move_nodes(
            &plan,
            "/",
            &["fetch".to_string()],
            &["/fetch".to_string()],
            &[],
            &[],
            &[],
            &|_| TargetGround {
                occupied: true,
                parent_exists: true,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, MutationError::NamingCollision(_)),
            "got {err:?}"
        );
    }

    /// Two moves onto one address are one address; the second must not commit
    /// on the strength of the first not having been applied yet.
    #[test]
    fn two_moves_onto_one_target_collide() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [
                {"match": {"name": "a"}, "to": "talky/x"},
                {"match": {"name": "b"}, "to": "./talky/x"}
            ]}),
        )
        .unwrap();
        let err = validate_move_nodes(
            &plan,
            "/",
            &["a".to_string(), "b".to_string()],
            &["/a".to_string(), "/b".to_string()],
            &[],
            &[],
            &[],
            &free_ground,
        )
        .unwrap_err();
        assert!(
            matches!(err, MutationError::NamingCollision(_)),
            "got {err:?}"
        );
    }

    /// An `add_nodes` in the same diff claims its path too — the move would be
    /// applied onto a directory that by then exists.
    #[test]
    fn a_target_an_add_nodes_already_claims_collides() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}),
        )
        .unwrap();
        let err = validate_move_nodes(
            &plan,
            "/",
            &["fetch".to_string()],
            &["/fetch".to_string()],
            &[],
            &[],
            &["talky/fetch".to_string()],
            &free_ground,
        )
        .unwrap_err();
        assert!(
            matches!(err, MutationError::NamingCollision(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn a_free_target_and_a_real_source_pass() {
        let plan = moves(
            "/",
            &json!({"move_nodes": [{"match": {"name": "fetch"}, "to": "talky/fetch"}]}),
        )
        .unwrap();
        validate_move_nodes(
            &plan,
            "/",
            &["fetch".to_string()],
            &["/fetch".to_string()],
            &["talky".to_string()],
            &["/talky".to_string()],
            &[],
            &free_ground,
        )
        .expect("a real source and a free target is the whole happy path");
    }
}
