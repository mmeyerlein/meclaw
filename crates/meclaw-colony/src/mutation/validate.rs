//! Mutation validation (phase 6 + phase 11). Single-stage colony validation
//! against the post_state graph; all checks pure, without FS or DB.

use crate::CellFactoryRegistry;
use crate::mutation::MutationError;
use meclaw_core::JsonValue;

/// T10 — schema check + template existence.
///
/// T11 extends this with match patterns and naming uniqueness; T11b adds
/// cycle and edge schema (see phase-6 plan, decision 7).
pub fn validate_post_state(
    diff_substituted: &JsonValue,
    factories: &CellFactoryRegistry,
) -> Result<(), MutationError> {
    let obj = diff_substituted
        .as_object()
        .ok_or_else(|| MutationError::Schema("diff is not an object".into()))?;
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            let template = n
                .get("template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_nodes[].template missing".into()))?;
            if !factories.contains_key(template) {
                return Err(MutationError::TemplateMissing(template.into()));
            }
        }
    }
    Ok(())
}

/// T11 — extends T10's checks with Match-Pattern hit and Naming-Eindeutigkeit.
///
/// `registry_names`: current cell-name-set in the scope (last path segment).
/// Phase-6-MVP: post_state = (registry_names \ remove_matches) ∪ add_names.
/// T11b adds cycle + edge_schema on top.
pub fn validate_post_state_full(
    diff: &JsonValue,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
) -> Result<(), MutationError> {
    validate_post_state(diff, factories)?;
    let obj = diff
        .as_object()
        .expect("validate_post_state covered schema");
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        // Befund 7: in-diff name uniqueness, mirroring `validate_naming_and_match`.
        let mut seen_in_diff: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for n in adds {
            let name = n
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_nodes[].name missing".into()))?;
            if registry_names.iter().any(|r| r == name) || !seen_in_diff.insert(name) {
                return Err(MutationError::NamingCollision(name.into()));
            }
        }
    }
    if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            let pat_name = r
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("remove_nodes[].match.name missing".into()))?;
            if !registry_names.iter().any(|r| r == pat_name) {
                return Err(MutationError::MatchNoHit(pat_name.into()));
            }
        }
    }
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            let pat_name = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("swap_nodes[].match.name missing".into()))?;
            if !registry_names.iter().any(|r| r == pat_name) {
                return Err(MutationError::MatchNoHit(pat_name.into()));
            }
        }
    }
    Ok(())
}

/// T11b — extends T11's checks with edge_schema (endpoints exist in post_state)
/// and cycle-freeness via hand-rolled DFS.
///
/// `existing_edges` is the current (from, to) edge-list, as Phase-6-MVP passes
/// the in-memory `EdgeTable` snapshot when calling from the Mutation arm (T13).
///
/// post_state-node-set: (registry_names \ remove_matches) ∪ add_names ∪ hive_endpoint_names.
/// post_state-edges: existing_edges ++ add_edges.
///
/// `hive_endpoint_names`: hive short-names (last path segment) that are also
/// valid edge endpoints — Cell ∪ Hive symmetry (Phase 13.5 step-6).
pub fn validate_post_state_with_edges(
    diff: &JsonValue,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
) -> Result<(), MutationError> {
    // Thin wrapper: single-cell mutations contribute no subtree nodes/edges, so
    // delegate with empty subtree slices — byte-behavior-unchanged.
    validate_post_state_with_edges_and_subtree(
        diff,
        factories,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        &[],
        &[],
    )
}

/// T7 (Phase 13.5 a5-subtree) — extends [`validate_post_state_with_edges`] so a
/// SUBTREE instantiation can be validated in a single pass.
///
/// In addition to the single-cell inputs, the caller contributes the subtree's
/// own nodes and internal edges (empty for single-cell mutations):
///
/// - `subtree_node_endpoints`: every subtree cell + hive path, expressed in the
///   validator's endpoint representation. They are added to the valid-endpoint
///   node-set so internal edges pointing at a nested node validate.
/// - `subtree_internal_edges`: the subtree's resolved internal `(from, to)` edges
///   (from each hive's `params.graph`), in the same representation. They join the
///   post_state edge-set BEFORE the endpoint-existence check and the cycle check,
///   so an internal edge that forms a cycle (alone or combined with existing/diff
///   edges) is rejected.
///
/// # Representation contract
/// The validator is representation-agnostic: it only does string-set membership.
/// The caller MUST express `subtree_node_endpoints` and `subtree_internal_edges`
/// in the SAME representation as each other (the subtree resolver uses absolute
/// logical paths, e.g. `/main/m1/inner_a`). The existing single-cell inputs use
/// scope-relative short-names; the two namespaces do not collide, which is
/// intended — a subtree-internal edge endpoint only matches a subtree node.
#[allow(clippy::too_many_arguments)]
pub fn validate_post_state_with_edges_and_subtree(
    diff: &JsonValue,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
) -> Result<(), MutationError> {
    validate_post_state_full(diff, factories, registry_names)?;
    let obj = diff.as_object().expect("validated above");
    // Phase 13.5-A1 T4: edge-schema + cycle + CEL-parse-validate share one
    // implementation (`validate_edges_and_cycle`). Scope-agnostic helper:
    // depth endpoints resolve against root, pre-state depth paths are not
    // known here (R12 callers use `validate_post_state_with_templates_scoped`).
    validate_edges_and_cycle(
        obj,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        "/",
        &[],
    )
}

/// Inner helper: naming-collision + match-no-hit checks, shared between
/// `validate_post_state_full` and `validate_post_state_with_templates`.
/// Does NOT check `add_nodes[].template` against factories (that is caller's job).
///
/// `hive_match_names`: SCOPE-FILTERED hive short-names (parent path == guard_scope),
/// mirroring `registry_names`. For `swap_nodes`, a match.name that refers to a HIVE
/// (which has no registry entry but IS in the pre-state via `hive_scopes`) must also
/// pass the match check — hives are valid swap sources (they carry external edges
/// just like cells). Per spec Z.265 ("Names are unique per scope") this set MUST
/// be scope-filtered so a hive in a FOREIGN scope cannot satisfy a short-name match
/// (apply-side `resolve_scoped_path` is scope-correct, so a global set here would be a
/// validate-side false-positive — Paket-5 companion finding Paket-2-b'). Pass an empty
/// slice for callers that do not have this information (e.g. `validate_post_state_full`,
/// which does not know hive names; existing swap-of-cell tests are unaffected).
fn validate_naming_and_match(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    hive_match_names: &[String],
) -> Result<(), MutationError> {
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        // Befund 7: track names seen WITHIN this diff so an in-diff duplicate is
        // caught here (naming_collision) — before staging, where the second
        // rename(2) failed as a `schema` IO error and stranded the first node.
        let mut seen_in_diff: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for n in adds {
            let name = n
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_nodes[].name missing".into()))?;
            if registry_names.iter().any(|r| r == name) || !seen_in_diff.insert(name) {
                return Err(MutationError::NamingCollision(name.into()));
            }
        }
    }
    if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            let pat_name = r
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("remove_nodes[].match.name missing".into()))?;
            if !registry_names.iter().any(|r| r == pat_name) {
                return Err(MutationError::MatchNoHit(pat_name.into()));
            }
        }
    }
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            let pat_name = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("swap_nodes[].match.name missing".into()))?;
            // PRE-STATE check: cell registry OR hive scope (hive is a valid swap source
            // because it carries external edges). Both sets are scope-filtered by the
            // caller — a hive in a foreign scope must NOT satisfy this short-name match.
            let in_registry = registry_names.iter().any(|r| r == pat_name);
            let in_hives = hive_match_names.iter().any(|h| h == pat_name);
            if !in_registry && !in_hives {
                return Err(MutationError::MatchNoHit(pat_name.into()));
            }
        }
    }
    Ok(())
}

/// Inner helper: edge-schema + cycle checks over the post-state graph.
/// Extracted to be shared between `validate_post_state_with_edges` and
/// `validate_post_state_with_templates`.
///
/// R12 depth resolution: `scope` is the mutation's guard scope and
/// `deep_endpoint_paths` the absolute paths of ALL pre-state nodes (registry ∪
/// hive scopes, any depth). A multi-segment `add_edges` endpoint (contains `/`
/// after the `./`-strip) resolves against `scope` — the SAME normalisation the
/// apply side uses (`resolve_scoped_path`) — and is membership-tested in the
/// ABSOLUTE-path namespace (`deep_endpoint_paths` ∪ `subtree_node_endpoints`).
/// Single-segment endpoints keep the unchanged short-name test. Containment is
/// NOT relaxed: `validate_scope_containment` ran before this and already
/// rejected `..`/absolute endpoints (`scope_out_of_bounds`), so a resolved
/// depth path is within scope by construction.
#[allow(clippy::too_many_arguments)]
fn validate_edges_and_cycle(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
    scope: &str,
    deep_endpoint_paths: &[String],
) -> Result<(), MutationError> {
    use std::collections::HashSet;
    let mut nodes: HashSet<String> = registry_names.iter().cloned().collect();
    // Phase 13.5 step-6: hives are valid edge endpoints too (Cell ∪ Hive). Add
    // their short-names before the add_edges endpoint check below.
    nodes.extend(hive_endpoint_names.iter().cloned());
    // Phase 13.5 a5-subtree T7: a subtree instantiation contributes its own
    // nested cell + hive paths as valid edge endpoints (same representation as
    // its internal edges below). Empty for single-cell mutations.
    nodes.extend(subtree_node_endpoints.iter().cloned());
    if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            if let Some(name) = r
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                nodes.remove(name);
            }
        }
    }
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            if let Some(name) = n.get("name").and_then(|v| v.as_str()) {
                nodes.insert(name.into());
            }
        }
    }
    // Existing edges are part of the post_state graph, but Befund 2 removed the
    // topological cycle gate — they no longer need to be accumulated here. The
    // endpoint-existence check below is purely a node-set membership test.
    let _ = existing_edges;
    // Phase 13.5 a5-subtree T7: subtree-internal edges' endpoints must each be a
    // known post_state node (subtree node, registry, hive, or an add_node from
    // this diff). Endpoint-existence only — no cycle accumulation (Befund 2).
    for (from, to) in subtree_internal_edges {
        if !nodes.contains(from.as_str()) {
            return Err(MutationError::EdgeSchema(format!(
                "subtree internal edge from='{from}' unknown"
            )));
        }
        if !nodes.contains(to.as_str()) {
            return Err(MutationError::EdgeSchema(format!(
                "subtree internal edge to='{to}' unknown"
            )));
        }
    }
    if let Some(adds) = obj.get("add_edges").and_then(|v| v.as_array()) {
        for e in adds {
            let from = e
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_edges[].from missing".into()))?;
            let to = e
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_edges[].to missing".into()))?;
            // Befund 6: endpoints are scope-relative; the canonical mutation
            // form is `./name` (overview § Variable substitution example). Both
            // `./name` and the bare `name` denote the same scope-local node, so
            // strip the `./` before the short-name membership test — otherwise a
            // `./`-prefixed endpoint never matched an `add_nodes`/registry
            // short-name and rejected as `edge_schema` ("from='./a' unknown").
            // (Apply mirrors this: `resolve_scoped_path` now normalises `./`.)
            //
            // R12: a multi-segment endpoint (`./unit/dispatch`) addresses a node
            // DEEPER than level 1 within the scope (spec Z.227 has no depth
            // restriction). Resolve it against `scope` and test in the
            // absolute-path namespace: `deep_endpoint_paths` (pre-state at any
            // depth) ∪ `nodes` (which carries the absolute
            // `subtree_node_endpoints` — diff-new nodes, Befund-6 semantics at
            // depth). Befund-6 short-name semantics for single segments are
            // byte-unchanged.
            let known = |endpoint: &str| -> bool {
                let stripped = endpoint.strip_prefix("./").unwrap_or(endpoint);
                if stripped.contains('/') {
                    let abs = crate::mutation::resolve_scoped_path(scope, endpoint);
                    deep_endpoint_paths.iter().any(|p| p == abs.as_str())
                        || nodes.contains(abs.as_str())
                } else {
                    nodes.contains(stripped)
                }
            };
            if !known(from) {
                return Err(MutationError::EdgeSchema(format!("from='{from}' unknown")));
            }
            if !known(to) {
                return Err(MutationError::EdgeSchema(format!("to='{to}' unknown")));
            }
            // Phase 13.5-A1 T4 (Slice 3): CEL parse-validate for condition +
            // modifier.set_context.* / set_hop.*. Parse-fail → MutationError::
            // EdgeSchema (error_code "edge_schema", per spec § Mutation-Format
            // Z.263).
            if let Some(cond_str) = e.get("condition").and_then(|v| v.as_str())
                && let Err(p) = crate::cel_eval::parse_condition(cond_str)
            {
                return Err(MutationError::EdgeSchema(format!(
                    "add_edges[].condition invalid cel: {p}"
                )));
            }
            if let Some(modif) = e.get("modifier") {
                // Befund-6-Folge: the modifier must match the
                // `{set_context, delete_context, set_hop, delete_hop}` schema
                // (spec § Validation l.277). Reject any unknown key — the old
                // flat `{"headers.X": ...}` map form was previously ignored at
                // apply and committed silently (builder foot-gun).
                if let Some(modif_obj) = modif.as_object() {
                    for k in modif_obj.keys() {
                        if !matches!(
                            k.as_str(),
                            "set_context"
                                | "delete_context"
                                | "set_hop"
                                | "delete_hop"
                                | "restore_ttl"
                        ) {
                            return Err(MutationError::EdgeSchema(format!(
                                "add_edges[].modifier unknown key '{k}' (valid: set_context, \
                                 delete_context, set_hop, delete_hop, restore_ttl)"
                            )));
                        }
                    }
                } else {
                    return Err(MutationError::EdgeSchema(
                        "add_edges[].modifier must be an object".into(),
                    ));
                }
                for set_key in ["set_context", "set_hop"] {
                    if let Some(set_obj) = modif.get(set_key).and_then(|v| v.as_object()) {
                        for (k, v) in set_obj {
                            let expr_str = v.as_str().ok_or_else(|| {
                                MutationError::EdgeSchema(format!(
                                    "add_edges[].modifier.{set_key}.{k} must be string"
                                ))
                            })?;
                            if let Err(p) = crate::cel_eval::parse_condition(expr_str) {
                                return Err(MutationError::EdgeSchema(format!(
                                    "add_edges[].modifier.{set_key}.{k} invalid cel: {p}"
                                )));
                            }
                        }
                    }
                }
                for del_key in ["delete_context", "delete_hop"] {
                    if let Some(del) = modif.get(del_key)
                        && del.as_array().is_none()
                    {
                        return Err(MutationError::EdgeSchema(format!(
                            "add_edges[].modifier.{del_key} must be array"
                        )));
                    }
                }
                // GH #82 (ruling 2026-08-13): `restore_ttl` is a boolean
                // declaration, not an expression — and a restoring edge opts its
                // cycle out of the TTL loop guard, so it must carry a bound of its
                // own. The mutation path enforces the same minimum as config load
                // (`BootstrapError::EdgeTtlRestoreUnconditional`): a restoring edge
                // without a `condition` is rejected.
                if let Some(rt) = modif.get("restore_ttl") {
                    let Some(rt) = rt.as_bool() else {
                        return Err(MutationError::EdgeSchema(
                            "add_edges[].modifier.restore_ttl must be boolean".into(),
                        ));
                    };
                    if rt && e.get("condition").and_then(|v| v.as_str()).is_none() {
                        return Err(MutationError::EdgeSchema(format!(
                            "add_edges[] {from}->{to}: modifier.restore_ttl needs a condition — a \
                             ttl-restoring edge is exempt from the TTL loop guard, so it must be \
                             bounded by its own iteration condition (e.g. \
                             \"int(context.iter) < 12\")"
                        )));
                    }
                }
            }
        }
    }
    // Finding 2 — NO general cycle reject. Spec overview § Validation:
    // "cycle-freedom … insofar as the application forbids cycles; meclaw-core
    // does not reject cycles in general". Tool/reply loops are legitimately
    // instantiable per mutation (and boot fine from the filesystem); the
    // runtime TTL loop-guard bounds any traversal cycle. Edge endpoints were
    // verified above (node-set membership); the topological cycle gate is gone.
    Ok(())
}

/// Phase-11 T14 — additive validation for template-based mutations.
///
/// Ebene 0: schema, naming-collision, match-no-hit (sans factory-check for add_nodes).
/// Ebene 1: every `add_nodes[].template` must resolve in `templates`.
/// Ebene 2: the resolved template's `cell.type` (via `template_to_cell_type`) must be in `factories`.
/// Edge-Schema + Cycle: delegated to the inner helper.
///
/// `hive_endpoint_names`: hive short-names accepted as edge endpoints
/// (Cell ∪ Hive symmetry, Phase 13.5 step-6). COLONY-GLOBAL by design — `add_edges`
/// endpoints are scope-relative and a hive defining a transit scope may legitimately
/// be referenced from anywhere.
///
/// `hive_match_names` (Paket-5 T4, P10b companion): SCOPE-FILTERED hive short-names
/// (parent path == guard_scope), used ONLY for the `swap_nodes` `match.name` existence
/// check. Distinct from `hive_endpoint_names`: a `match.name` is scope-bound (spec
/// Z.265), so a hive in a foreign scope must not satisfy it (validate-side mirror of
/// the scope-correct apply-side `resolve_scoped_path`).
///
/// `subtree_node_endpoints` / `subtree_internal_edges` (Phase 13.5 a5-subtree
/// T8b-2): a SUBTREE `add_nodes` contributes its nested cell + hive paths as valid
/// edge endpoints AND its resolved internal edges, so the subtree's internal graph
/// participates in the endpoint-existence + cycle check up front (before staging).
/// Empty for single-cell mutations → byte-behavior-unchanged. Same absolute-path
/// representation as [`validate_post_state_with_edges_and_subtree`].
#[allow(clippy::too_many_arguments)]
pub fn validate_post_state_with_templates(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    template_to_cell_type: &[(String, String)],
    hive_endpoint_names: &[String],
    hive_match_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
) -> Result<(), MutationError> {
    // Thin wrapper (same pattern as `validate_post_state_with_edges`):
    // root scope + no pre-state depth paths — byte-behavior-unchanged for
    // callers without R12 depth-endpoint needs.
    validate_post_state_with_templates_scoped(
        diff,
        templates,
        factories,
        registry_names,
        existing_edges,
        template_to_cell_type,
        hive_endpoint_names,
        hive_match_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        "/",
        &[],
    )
}

/// R12 — full form of [`validate_post_state_with_templates`] with depth-endpoint
/// resolution: `scope` is the mutation's guard scope, `deep_endpoint_paths` the
/// absolute paths of ALL pre-state nodes (registry ∪ hive scopes, any depth).
/// A multi-segment `add_edges` endpoint (`./unit/dispatch`) resolves against
/// `scope` and is membership-tested in the absolute-path namespace
/// (`deep_endpoint_paths` ∪ `subtree_node_endpoints`) — spec Z.227 declares
/// edge paths scope-relative WITHOUT a depth restriction. Containment is not
/// relaxed: `validate_scope_containment` runs before this (caller order) and
/// rejects `..`/absolute endpoints as `scope_out_of_bounds`.
#[allow(clippy::too_many_arguments)]
pub fn validate_post_state_with_templates_scoped(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    template_to_cell_type: &[(String, String)],
    hive_endpoint_names: &[String],
    hive_match_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
    scope: &str,
    deep_endpoint_paths: &[String],
) -> Result<(), MutationError> {
    // Ebene 0a: diff must be an object.
    let obj = diff
        .as_object()
        .ok_or_else(|| MutationError::Schema("diff is not an object".into()))?;

    // Ebene 0b: naming-collision + match-no-hit (no factory-check here).
    // Pass the SCOPE-FILTERED `hive_match_names` so a swap_nodes match.name can resolve
    // against a hive in pre-state ONLY within the mutation scope (spec Z.265) — a hive
    // in a foreign scope must not produce a short-name false-positive (Paket-5 T4).
    validate_naming_and_match(obj, registry_names, hive_match_names)?;

    // Build lookup map for template → cell_type.
    let ct_map: std::collections::HashMap<&str, &str> = template_to_cell_type
        .iter()
        .map(|(t, c)| (t.as_str(), c.as_str()))
        .collect();

    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            // A5b 2b (Phase-16 W1b, Ruling 2026-06-12): an `adopt` entry
            // instantiates from an EXISTING on-disk node, not a template. Grammar
            // (pure, here): `adopt` is an object declaring the expected identity
            // with a mandatory `type`; `template` is mutually exclusive; a bare
            // `adopt: true` / an `adopt` without `type` is a `schema` reject (NO
            // blind adoption — ruling 2026-06-12). The FS/registry-dependent
            // checks (path exists, unregistered, on-disk type/version match) live
            // in `colony::handle_mutation` Step 1a. Skip the template-existence /
            // factory checks below for an adopt entry.
            if let Some(adopt) = n.get("adopt") {
                if n.get("template").is_some() {
                    return Err(MutationError::Schema(
                        "add_nodes[].adopt and .template are mutually exclusive".into(),
                    ));
                }
                let adopt_obj = adopt.as_object().ok_or_else(|| {
                    MutationError::Schema(
                        "add_nodes[].adopt must be an object declaring the expected `type` \
                         (no blind adoption)"
                            .into(),
                    )
                })?;
                if adopt_obj
                    .get("type")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .is_none()
                {
                    return Err(MutationError::Schema(
                        "add_nodes[].adopt.type (non-empty string) is required — no blind \
                         adoption"
                            .into(),
                    ));
                }
                continue;
            }
            let template = n
                .get("template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_nodes[].template missing".into()))?;

            // Ebene 1: template must exist in templates registry.
            let entry = templates
                .resolve(template)
                .map_err(|_| MutationError::TemplateMissing(template.into()))?;

            // F2-Ruling (R10, 2026-06-11): `override_params` has no addressing
            // semantics for the sub-cells of a SUBTREE template — it used to
            // COMMIT as a silent no-op (`stage_subtree_merge` never reads it;
            // K-H2 E3 measured every addressing form as committed-without-
            // effect). Reject loudly and pre-destructively; sub-cell
            // parametrization goes through ctx substitution baked into the
            // template (spec overview § Mutation-Format).
            if n.get("override_params").is_some()
                && crate::mutation::subtree::parse_subtree(&entry.filesystem_path)?
                    .cells
                    .len()
                    > 1
            {
                return Err(MutationError::Schema(format!(
                    "override_params unsupported on subtree templates — use ctx \
                     substitution (template '{template}', R10 ruling)"
                )));
            }

            // Ebene 2: cell.type for resolved template must be in factories.
            // Use entry.name (resolved name, e.g. "echo") not raw template string
            // (e.g. "echo@1.0.0") as ct_map key — fixes R3 versioned-ref mismatch.
            let cell_type = ct_map
                .get(entry.name.as_str())
                .ok_or_else(|| MutationError::TemplateMissing(template.into()))?;
            // Phase-13.5 a5-subtree T8b-1: a SUBTREE template's ROOT cell.type is
            // `hive` — a scope marker, never an actor, so it has NO factory by
            // design (CONTRIBUTING.md: "a hive is not an actor"). Skip the level-2
            // factory check for a hive root; the spawnable nested cells are
            // staged + registered by `stage_subtree` (their own cell-types are
            // validated by bootstrap-side factory presence at spawn time).
            if *cell_type != "hive" && !factories.contains_key(*cell_type) {
                return Err(MutationError::UnknownCellType((*cell_type).into()));
            }
        }
    }

    // swap_nodes: validate `with` per new re-dedicated shape (paket-2 T1).
    // T5 Part 1: collect add_names (scope-bound, from add_nodes in this same diff)
    // so that `with.name` (existing form) can forward-reference a node being added
    // in the same composite diff. The post-state set = registry_names ∪ add_names.
    let add_names: Vec<String> = obj
        .get("add_nodes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| {
                    n.get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            let match_name = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("swap_nodes[].match.name missing".into()))?;
            let with_val = s
                .get("with")
                .ok_or_else(|| MutationError::Schema("swap_nodes[].with missing".into()))?;
            validate_swap_with_entry_full(
                with_val,
                match_name,
                registry_names,
                &add_names,
                templates,
            )?;
        }
    }

    // Edge-Schema + Cycle. Subtree contributions (T8b-2) flow through so a
    // subtree's internal graph participates in the endpoint + cycle check;
    // scope + pre-state depth paths enable R12 depth-endpoint resolution.
    validate_edges_and_cycle(
        obj,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        scope,
        deep_endpoint_paths,
    )?;
    Ok(())
}

/// Paket-2 T1 — Validate a `swap_nodes[].with` object.
///
/// Discriminator: presence of the `template` key.
///
/// **Instantiate form (new, with `name`):** `{"template": "<tmpl>", "name": "<t3>", "params": {...}}`
///   Unknown keys → Schema. `name` required → Schema.
///   t3 already in registry → NamingCollision.
///   Unknown `template` → TemplateMissing. Subtree template → Schema.
///
/// **Instantiate form without `name`:** `{"template": "<tmpl>"}` → Schema error.
///   The graph-swap instantiate form REQUIRES `name` (t3 needs an own name to
///   swing edges onto). The legacy no-name form was retired with the old
///   transplant apply-arm (paket-2 T4).
///
/// **Existing-node form:** `{"name": "<t3>"}` — only `name`, NO `template`.
///   Unknown keys → Schema (typo guard). t3 NOT in registry → MatchNoHit.
///
/// `registry_names` MUST be scope-filtered by the caller (colony.rs passes
/// names from paths within `guard_scope` only — A2 scope-binding).
///
/// T5 Part 1 — Full form for validating `swap_nodes[].with`; also accepts a
/// `with.name` (existing-node form) that refers to a node being ADDED in the same
/// diff via `add_nodes` (forward reference / post-state resolution).
///
/// `add_names`: short-names of nodes introduced by `add_nodes` in the same diff
/// (scope-bound). The effective post-state set for the existing-form check is:
/// `registry_names ∪ add_names`.
///
/// # Invariant
///
/// `match.name` (t2 in the swap source) is still checked against PRE-STATE only
/// (via `validate_naming_and_match` / `validate_post_state_full`). Only the
/// `with.name` TARGET gains forward-reference resolution here.
fn validate_swap_with_entry_full(
    with_val: &JsonValue,
    match_name: &str,
    registry_names: &[String],
    add_names: &[String],
    templates: &crate::templates::TemplatesRegistry,
) -> Result<(), MutationError> {
    let with_obj = with_val
        .as_object()
        .ok_or_else(|| MutationError::Schema("swap_nodes[].with must be an object".into()))?;

    let has_template = with_obj.contains_key("template");
    let has_name = with_obj.contains_key("name");

    if has_template && has_name {
        // ── New instantiate form: allowed keys = {template, name, params} ─
        for key in with_obj.keys() {
            if !matches!(key.as_str(), "template" | "name" | "params") {
                return Err(MutationError::Schema(format!(
                    "swap_nodes[].with unknown key '{key}' in instantiate form"
                )));
            }
        }

        // `name` is guaranteed present (has_name is true); `.as_str()` fails only
        // for a non-string value such as `{"name": 42}` — that is a real schema
        // reject, not dead code.
        let name = with_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.name missing".into()))?;

        // Rule 4 (Strict-Regel b): target name collision.
        if registry_names.iter().any(|r| r == name) {
            return Err(MutationError::NamingCollision(name.into()));
        }

        let template = with_obj
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.template missing".into()))?;

        // Rule 5a: template must exist.
        let entry = templates
            .resolve(template)
            .map_err(|_| MutationError::TemplateMissing(template.into()))?;

        // Rule 5b (A6): subtree templates not allowed.
        crate::mutation::stage::reject_if_subtree_template(&entry.filesystem_path, template)?;
    } else if has_template {
        // ── `template` without `name` ────────────────────────────────────
        // The instantiate form REQUIRES `name` (paket-2 graph-swap: t3 needs an
        // own name to swing edges onto). A `with` carrying `template` but no
        // `name` is a schema error (the legacy no-name transplant form was
        // retired with the old apply-arm in paket-2 T4).
        return Err(MutationError::Schema(
            "swap_nodes[].with: instantiate form requires 'name'".into(),
        ));
    } else {
        // ── Existing-node form: allowed keys = {name} ────────────────────
        for key in with_obj.keys() {
            if key.as_str() != "name" {
                return Err(MutationError::Schema(format!(
                    "swap_nodes[].with unknown key '{key}' in existing-node form"
                )));
            }
        }

        let name = with_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.name missing".into()))?;

        // T13: identity-swap guard — with.name == match.name (same node, same scope)
        // would drop every external edge of t2 (all swung edges become t3→t3 self-loops
        // and are silently dropped). Loud reject instead of silent edge-loss.
        if name == match_name {
            return Err(MutationError::Schema(
                "swap_nodes[].with.name must differ from match.name".into(),
            ));
        }

        // Rule 3 (Strict-Regel a): existing target must resolve within POST-STATE scope.
        // T5 Part 1: post-state = registry_names (pre-state) ∪ add_names (nodes being
        // added in the same diff). A `with.name` that forward-references a node from
        // `add_nodes` in the same composite diff is VALID — the node will exist at
        // apply time. Cross-scope rejection still applies (a name absent from BOTH
        // sets is rejected as MatchNoHit, preserving A2 scope-binding).
        let in_pre_state = registry_names.iter().any(|r| r == name);
        let in_add_names = add_names.iter().any(|a| a == name);
        if !in_pre_state && !in_add_names {
            return Err(MutationError::MatchNoHit(name.into()));
        }
    }

    Ok(())
}

/// A pre-state edge reduced to the fields needed for `remove_edges` match
/// equality (Paket-5 T1/T2 / D-031).
///
/// The endpoints are ABSOLUTE logical paths (as stored on the live
/// [`crate::edge_table::Edge`]). `condition_source` mirrors
/// `edge.condition.source` (the original CEL string) and `modifier_source` is
/// the serde-JSON value of `edge.modifier.source` — exactly the two stored
/// representations the apply-time F6 match compares against
/// (`colony.rs` remove_edges arm). Keeping this view here lets validate and
/// apply share ONE equality definition (`remove_edges_pattern_hits`).
#[derive(Debug, Clone)]
pub struct EdgeMatchView {
    /// Absolute `from` path of the edge.
    pub from: String,
    /// Absolute `to` path of the edge.
    pub to: String,
    /// `edge.condition.source` (the raw CEL string), or `None` for an
    /// unconditional edge.
    pub condition_source: Option<String>,
    /// serde-JSON value of `edge.modifier.source`, or `None` for an edge
    /// without a modifier.
    pub modifier_source: Option<JsonValue>,
}

/// Builds an [`EdgeMatchView`] from a live [`crate::edge_table::Edge`] — the
/// SINGLE mapping of a stored edge into the F6 match-view, shared by both the
/// validate-time pre-check and the apply-time remove_edges arm in `colony.rs`.
/// Keeping this conversion here (next to the predicate it feeds) removes the
/// last drift risk between validate and apply: both call sites map identically.
impl From<&crate::edge_table::Edge> for EdgeMatchView {
    fn from(e: &crate::edge_table::Edge) -> Self {
        EdgeMatchView {
            from: e.from.as_str().to_string(),
            to: e.to.as_str().to_string(),
            condition_source: e.condition.as_ref().map(|c| c.source.clone()),
            modifier_source: e
                .modifier
                .as_ref()
                .and_then(|m| meclaw_core::serde_json::to_value(&m.source).ok()),
        }
    }
}

/// Shared `remove_edges` match predicate — the SINGLE source of truth for the
/// F6 equality, used by both validate-time (Paket-5 T1/T2) and apply-time
/// (`colony.rs` remove_edges arm).
///
/// An edge matches a `remove_edges[].match` pattern iff:
/// - `edge.from == from_path` AND `edge.to == to_path` (mandatory, absolute
///   paths — the caller resolves the scope-relative pattern names via
///   [`crate::mutation::resolve_scoped_path`] before calling), AND
/// - if `pat_condition` is `Some`, the edge's `condition_source` equals it
///   byte-for-byte (string equality); if `None`, condition is unconstrained,
///   AND
/// - if `pat_modifier` is `Some`, the edge's `modifier_source` JSON equals it;
///   if `None`, modifier is unconstrained.
pub fn remove_edges_pattern_hits(
    edge: &EdgeMatchView,
    from_path: &str,
    to_path: &str,
    pat_condition: Option<&str>,
    pat_modifier: Option<&JsonValue>,
) -> bool {
    if edge.from != from_path || edge.to != to_path {
        return false;
    }
    // F6: condition-source string-equality (not semantic). Pattern absent =>
    // any edge passes; pattern present => stored source must equal it exactly.
    if let Some(pc) = pat_condition
        && edge.condition_source.as_deref() != Some(pc)
    {
        return false;
    }
    // F6: modifier serde-JSON equality on the stored `ModifierSpec` source.
    if let Some(pm) = pat_modifier
        && edge.modifier_source.as_ref() != Some(pm)
    {
        return false;
    }
    true
}

/// Paket-5 T1/T2 (P10a / D-031) — validate-time reject for malformed or no-hit
/// `remove_edges` match patterns, reaching parity with `remove_nodes` /
/// `swap_nodes` (spec Z.272: every `remove_*` pattern must hit ≥1 element of the
/// pre_state; Z.279: the whole diff is rejected, no partial commit).
///
/// Before this check, a `remove_edges` entry was validated ONLY at apply-time,
/// where a missing `match.from`/`match.to` was silently skipped and a no-hit
/// pattern was a silent no-op. This makes both loud and PRE-destructive.
///
/// - missing `match.from` → `Schema("remove_edges[].match.from missing")`
/// - missing `match.to`   → `Schema("remove_edges[].match.to missing")`
/// - pattern that matches zero edges → `MatchNoHit(...)`
///
/// `scope` is the mutation scope; the pattern's `from`/`to` are scope-relative
/// names resolved with [`crate::mutation::resolve_scoped_path`] before being
/// compared against the absolute endpoints in `existing_edges` — mirroring
/// apply EXACTLY so validate and apply agree. The optional `condition` /
/// `modifier` keys follow the same F6 semantics via [`remove_edges_pattern_hits`].
pub fn validate_remove_edges(
    diff: &JsonValue,
    scope: &str,
    existing_edges: &[EdgeMatchView],
) -> Result<(), MutationError> {
    let Some(obj) = diff.as_object() else {
        return Err(MutationError::Schema("diff is not an object".into()));
    };
    let Some(rems) = obj.get("remove_edges").and_then(|v| v.as_array()) else {
        return Ok(());
    };
    for r in rems {
        let m = r.get("match");
        let from_name = m
            .and_then(|v| v.get("from"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("remove_edges[].match.from missing".into()))?;
        let to_name = m
            .and_then(|v| v.get("to"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("remove_edges[].match.to missing".into()))?;
        let pat_condition = m.and_then(|v| v.get("condition")).and_then(|v| v.as_str());
        let pat_modifier = m.and_then(|v| v.get("modifier"));
        let from_path = crate::mutation::resolve_scoped_path(scope, from_name);
        let to_path = crate::mutation::resolve_scoped_path(scope, to_name);
        let hit = existing_edges.iter().any(|e| {
            remove_edges_pattern_hits(
                e,
                from_path.as_str(),
                to_path.as_str(),
                pat_condition,
                pat_modifier,
            )
        });
        if !hit {
            return Err(MutationError::MatchNoHit(format!("{from_name}->{to_name}")));
        }
    }
    Ok(())
}

/// Befund 22 — scope-containment guard. Every scoped name in a mutation diff
/// resolves (apply-side [`crate::mutation::resolve_scoped_path`], a pure
/// string-join WITHOUT `..`-normalisation) to an absolute path that MUST stay
/// within the mutation's declared `guard_scope` prefix. A name that walks out of
/// its scope (e.g. via `..` segments) would otherwise let a *scoped* mutation
/// touch the registry / filesystem OUTSIDE its hive scope — a confinement breach.
///
/// Two layers (Slice-0 security hotfix added the first):
///
/// 1. **Raw traversal reject**: a name containing a `..` segment or starting
///    with `/` is rejected outright. The apply-side FS joins
///    (`resolve_cell_dir`, `staging_root.join(name)`) do NOT root-clamp `..`
///    and honour absolute names as join-replacement, so such a name diverges
///    from the clamped logical normalisation below — at root scope the clamp
///    made `/../escape` look contained while the filesystem realisation
///    walked OUTSIDE `{root}`.
/// 2. **Prefix containment**: we normalise the resolved path (root-clamped
///    `..` pop) and reject if the result escapes the (normalised) scope prefix.
///
/// Both reject with [`MutationError::ScopeOutOfBounds`]. Runs BEFORE any
/// FS/registry mutation, parallel to the `naming_collision` / `match_no_hit`
/// validate-time checks.
///
/// Only TOP-LEVEL diff names are checked (`add_nodes[].name`,
/// `remove_nodes[].match.name`, `swap_nodes[].match.name`/`.with.name`,
/// `add_edges[].from`/`.to`, `remove_edges[].match.from`/`.to`). Subtree-internal
/// `params.graph` edges (which legitimately use `./` and `../` relative
/// addressing, resolved by the subtree resolver) are NOT in scope here.
pub fn validate_scope_containment(
    diff: &JsonValue,
    guard_scope: &str,
) -> Result<(), MutationError> {
    let Some(obj) = diff.as_object() else {
        // Shape errors are surfaced by schema validation; nothing to contain.
        return Ok(());
    };
    let root = meclaw_core::Path::new("/");
    let scope_norm = meclaw_core::Path::resolve(&root, guard_scope);
    let check = |name: &str| -> Result<(), MutationError> {
        let resolved = crate::mutation::resolve_scoped_path(guard_scope, name);
        // FS-realisation guard (Slice-0 security hotfix): the apply-side joins
        // (`resolve_cell_dir`, `staging_root.join(name)`) do NOT root-clamp
        // `..`, and `PathBuf::join` REPLACES the base for an absolute name —
        // both diverge from the clamped logical normalisation below (worst
        // case: a directory OUTSIDE `{root}`). A raw `..` segment or an
        // absolute name has no legitimate top-level-diff producer (the
        // legitimate `./`/`../` relative addressing lives in subtree-internal
        // `params.graph`, which is NOT checked here) → reject outright,
        // reporting the UN-normalised resolved path (the normalised one would
        // look in-bounds at root scope and hide the escape).
        if name.starts_with('/') || name.split('/').any(|seg| seg == "..") {
            return Err(MutationError::ScopeOutOfBounds { path: resolved });
        }
        let normalized = meclaw_core::Path::resolve(&root, resolved.as_str());
        if path_within(&scope_norm, &normalized) {
            Ok(())
        } else {
            Err(MutationError::ScopeOutOfBounds { path: normalized })
        }
    };
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            if let Some(name) = n.get("name").and_then(|v| v.as_str()) {
                check(name)?;
            }
        }
    }
    if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            if let Some(name) = r
                .get("match")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
        }
    }
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            if let Some(name) = s
                .get("match")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
            if let Some(name) = s
                .get("with")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
        }
    }
    if let Some(es) = obj.get("add_edges").and_then(|v| v.as_array()) {
        for e in es {
            if let Some(f) = e.get("from").and_then(|v| v.as_str()) {
                check(f)?;
            }
            if let Some(t) = e.get("to").and_then(|v| v.as_str()) {
                check(t)?;
            }
        }
    }
    if let Some(es) = obj.get("remove_edges").and_then(|v| v.as_array()) {
        for e in es {
            let m = e.get("match");
            if let Some(f) = m.and_then(|v| v.get("from")).and_then(|v| v.as_str()) {
                check(f)?;
            }
            if let Some(t) = m.and_then(|v| v.get("to")).and_then(|v| v.as_str()) {
                check(t)?;
            }
        }
    }
    Ok(())
}

/// Segment-aware containment: `inner` is within `scope` iff it equals `scope` or
/// is a strict path-descendant (`scope`/...). Root scope (`/`) contains every
/// absolute path. Both inputs are expected pre-normalised.
fn path_within(scope: &meclaw_core::Path, inner: &meclaw_core::Path) -> bool {
    let s = scope.as_str();
    if s == "/" {
        return true;
    }
    let i = inner.as_str();
    i == s || i.starts_with(&format!("{s}/"))
}

/// Standard-Header-Konvention (spec § Standard-Header-Konvention): the context
/// keys that exist by virtue of ingress-at-birth. A node that requires one of
/// these is reachable as soon as a path leads to it from the graph entry without
/// an intervening `delete_context` for that key — no explicit `set_context`
/// setter edge is needed (the value is born at ingress).
pub const INGRESS_CONTEXT_KEYS: &[&str] =
    &["turn_id", "session_id", "user_id", "chat_id", "locale"];

/// Per-node contract projection used by [`validate_header_contract_locality`].
/// Carries only the key-sets the two header rules need — the caller projects a
/// `ContractBlock` (or in-memory `ConsumesBlock`/`EmitsBlock`) into this shape.
#[derive(Debug, Clone, Default)]
pub struct HeaderNodeView {
    /// `emits.hop` keys this node writes (the hop keys it provides downstream).
    pub emits_hop: std::collections::BTreeSet<String>,
    /// `consumes.context` keys this node declares `required: true`.
    pub required_context: std::collections::BTreeSet<String>,
    /// `consumes.hop` keys this node declares `required: true`.
    pub required_hop: std::collections::BTreeSet<String>,
}

/// Project a parsed `contract` block into the [`HeaderNodeView`] the
/// 14-B locality check consumes. Only `required: true` consume keys carry a
/// build-time obligation; non-required keys are omitted. Shared by the
/// bootstrap walk, the mutation validate step and the staging path.
pub fn header_view_from_contract(block: &crate::config::ContractBlock) -> HeaderNodeView {
    let required_keys = |m: &std::collections::BTreeMap<String, meclaw_core::ConsumeSpec>| {
        m.iter()
            .filter(|(_, s)| s.required)
            .map(|(k, _)| k.clone())
            .collect::<std::collections::BTreeSet<String>>()
    };
    // Absent `consumes` (Slice 4: presence-detectable Option) projects like
    // an empty block — absent ⇒ empty ⇒ vacuous, semantics unchanged.
    let default_consumes = meclaw_core::ConsumesBlock::default();
    let consumes = block.consumes.as_ref().unwrap_or(&default_consumes);
    HeaderNodeView {
        emits_hop: block.emits.hop.keys().cloned().collect(),
        required_context: required_keys(&consumes.context),
        required_hop: required_keys(&consumes.hop),
    }
}

/// Per-edge modifier projection used by [`validate_header_contract_locality`].
/// `from`/`to` are node names in the SAME namespace as the `node_contracts`
/// keys (the caller MUST use one consistent representation, e.g. absolute
/// meclaw paths). The four key-sets mirror `ModifierSpec`'s four fields.
#[derive(Debug, Clone, Default)]
pub struct HeaderEdgeView {
    /// Source node name.
    pub from: String,
    /// Destination node name.
    pub to: String,
    /// `modifier.set_context` keys promoted on this edge.
    pub set_context: std::collections::BTreeSet<String>,
    /// `modifier.delete_context` keys removed on this edge.
    pub delete_context: std::collections::BTreeSet<String>,
    /// `modifier.set_hop` keys written on this edge.
    pub set_hop: std::collections::BTreeSet<String>,
    /// `modifier.delete_hop` keys removed on this edge.
    pub delete_hop: std::collections::BTreeSet<String>,
}

/// Build-time header-contract check (Phase-14-B locality). PURE — the caller
/// supplies the already-loaded post_state node contracts + the edge modifier
/// key-sets; this function does NO FS/DB access. Two rules:
///
/// - **hop locality (strict, fan-in intersection):** for every node `N` with a
///   required `consumes.hop` key `k`, `k` MUST lie in the INTERSECTION over ALL
///   incoming edges `e: from → N` of the keys `e` contributes. A CELL `from`
///   contributes `(emits.hop(from) ∪ set_hop(e)) − delete_hop(e)`. A HIVE
///   `from` (`hives` member) is a transit pass-through (F1 fix, K-H1): the
///   hive forwards the hop compartment unchanged (spec § Hive-Pfade als
///   Target — hop decays only at a CELL emission), so `e` contributes
///   `(set_hop(e) ∪ INTERSECTION over the contributions of ALL inbound edges
///   of the hive) − delete_hop(e)` — recursively across multi-level transits,
///   mirroring the runtime key walk. Cycles are legal (tool-/refine-loops): an
///   edge already on the walk stack imposes no additional obligation (every
///   runtime delivery is rooted at a finite cell-emission path — greatest-
///   fixpoint reading), and a hive with NO inbound edge can never deliver a
///   message via `e`, so it is vacuously providing. A node with a required hop
///   key but NO incoming edge is rejected. A key not delivered by EVERY
///   incoming edge is rejected (`EdgeSchema`, message names node + key +
///   "14-B locality / fan-in intersection"). This is the collector safeguard.
/// - **context REACHABILITY (presence, not freshness):** for every required
///   `consumes.context` key `k` at `N`, a path must exist backwards over
///   incoming edges to a context SETTER with NO intervening `delete_context: k`
///   on that path. Two setter roots count: an edge carrying `set_context: k`
///   (source-cell promotion), OR — for a key in [`INGRESS_CONTEXT_KEYS`] — the
///   ingress-at-birth (a path from the graph entry, i.e. a node with no
///   incoming edge, to `N` without an intervening `delete_context: k`). No such
///   path → `EdgeSchema` ("context presence not reachable"). This is honestly
///   PRESENCE/reachability, NOT freshness — a key proven reachable may still be
///   stale.
///
/// Empty `consumes` (no required keys) makes the check vacuously true, so
/// existing topologies that declare no `consumes` never break.
pub fn validate_header_contract_locality(
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    edges: &[HeaderEdgeView],
    hives: &std::collections::BTreeSet<String>,
) -> Result<(), MutationError> {
    use std::collections::HashMap;

    // Index incoming edges per node (by `to`), as indices into `edges` — the
    // transit walk needs a cycle guard, and the index is the edge identity.
    let mut incoming: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        incoming.entry(e.to.as_str()).or_default().push(i);
    }

    for (node, view) in node_contracts {
        // ── Rule 1: hop locality (fan-in intersection) ──────────────────────
        if !view.required_hop.is_empty() {
            let in_edges = incoming.get(node.as_str());
            // A node with required hop keys but no incoming edge can never have
            // those keys delivered → reject.
            let Some(in_edges) = in_edges else {
                let key = view.required_hop.iter().next().cloned().unwrap_or_default();
                return Err(MutationError::EdgeSchema(format!(
                    "node '{node}' requires consumes.hop '{key}' but has no incoming edge \
                     (14-B locality / fan-in intersection)"
                )));
            };
            for key in &view.required_hop {
                // The key must be provided by EVERY incoming edge (transit
                // edges recurse into the hive's inbound fan-in — F1 fix).
                let provided_by_all = in_edges.iter().all(|&ei| {
                    let mut walk = std::collections::HashSet::new();
                    edge_provides_hop_key(
                        ei,
                        key,
                        edges,
                        node_contracts,
                        hives,
                        &incoming,
                        &mut walk,
                    )
                });
                if !provided_by_all {
                    return Err(MutationError::EdgeSchema(format!(
                        "node '{node}' requires consumes.hop '{key}' not in the fan-in \
                         intersection of all incoming edges \
                         (14-B locality / fan-in intersection)"
                    )));
                }
            }
        }

        // ── Rule 2: context reachability (presence, not freshness) ──────────
        for key in &view.required_context {
            let is_ingress = INGRESS_CONTEXT_KEYS.contains(&key.as_str());
            if !context_key_reachable(node, key, is_ingress, edges, &incoming) {
                return Err(MutationError::EdgeSchema(format!(
                    "node '{node}' requires consumes.context '{key}' but context presence \
                     not reachable from any setter{}",
                    if is_ingress {
                        " or ingress-at-birth"
                    } else {
                        ""
                    }
                )));
            }
        }
    }
    Ok(())
}

/// Contribution test for one incoming edge in Rule 1 of
/// [`validate_header_contract_locality`]: does the edge at `edge_idx`
/// guarantee hop key `key` on every message it delivers? PURE.
///
/// - `delete_hop: key` on the edge severs the key (delete wins over set).
/// - `set_hop: key` on the edge provides it.
/// - CELL `from`: the key must be in the cell's `emits.hop` view.
/// - HIVE `from` (F1 fix, K-H1): the hive is a transit pass-through — the
///   hop compartment survives (it decays only at a CELL emission), so the
///   edge contributes the INTERSECTION over the contributions of ALL inbound
///   edges of the hive, recursively across multi-level transits (the same
///   key walk the runtime performs). An edge already on the `walk` stack is
///   a loop back-edge and imposes no additional obligation: every runtime
///   delivery is rooted at a finite cell-emission path, so the looping
///   continuation is covered by the acyclic prefixes (greatest-fixpoint
///   reading — returning `false` here would falsely empty the intersection
///   at legal tool-/refine-loops). A hive with NO inbound edge can never
///   deliver a message via this edge → vacuously providing (zero runtime
///   paths impose zero obligations; the wiring mutation that later connects
///   the hive re-validates the full post-state).
#[allow(clippy::too_many_arguments)]
fn edge_provides_hop_key(
    edge_idx: usize,
    key: &str,
    edges: &[HeaderEdgeView],
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    hives: &std::collections::BTreeSet<String>,
    incoming: &std::collections::HashMap<&str, Vec<usize>>,
    walk: &mut std::collections::HashSet<usize>,
) -> bool {
    let e = &edges[edge_idx];
    if e.delete_hop.contains(key) {
        return false;
    }
    if e.set_hop.contains(key) {
        return true;
    }
    if !hives.contains(e.from.as_str()) {
        // Cell (or unknown) source: only its emits view counts.
        return node_contracts
            .get(e.from.as_str())
            .map(|fv| fv.emits_hop.contains(key))
            .unwrap_or(false);
    }
    // Hive source: recurse into the hive's inbound fan-in (cycle-guarded).
    if !walk.insert(edge_idx) {
        return true;
    }
    let provided = incoming.get(e.from.as_str()).is_none_or(|ins| {
        ins.iter()
            .all(|&g| edge_provides_hop_key(g, key, edges, node_contracts, hives, incoming, walk))
    });
    walk.remove(&edge_idx);
    provided
}

/// Backwards reachability for one required `consumes.context` key (Rule 2 of
/// [`validate_header_contract_locality`]). PURE. Walks incoming edges from
/// `node` looking for a setter root, pruning any path that crosses a
/// `delete_context: key`. Returns `true` iff such a path exists.
///
/// Setter roots: an edge with `set_context: key` (promotion), OR — when
/// `is_ingress` — a graph-entry node (a node with no incoming edge), which
/// represents ingress-at-birth for the standard header keys.
fn context_key_reachable(
    node: &str,
    key: &str,
    is_ingress: bool,
    edges: &[HeaderEdgeView],
    incoming: &std::collections::HashMap<&str, Vec<usize>>,
) -> bool {
    use std::collections::HashSet;
    // BFS backwards over nodes; the frontier holds nodes whose incoming edges we
    // still need to inspect. We start at `node` itself.
    let mut visited: HashSet<&str> = HashSet::new();
    let mut frontier: Vec<&str> = vec![node];
    while let Some(current) = frontier.pop() {
        if !visited.insert(current) {
            continue;
        }
        match incoming.get(current) {
            None => {
                // Graph entry: ingress-at-birth satisfies the standard keys.
                if is_ingress {
                    return true;
                }
            }
            Some(in_edges) => {
                for &ei in in_edges {
                    let e = &edges[ei];
                    // A delete on this edge severs the key on THIS path; do not
                    // traverse it.
                    if e.delete_context.contains(key) {
                        continue;
                    }
                    // A promotion on this edge is a setter root → reachable.
                    if e.set_context.contains(key) {
                        return true;
                    }
                    frontier.push(e.from.as_str());
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use std::sync::Arc;

    fn factories_with(types: &[&str]) -> CellFactoryRegistry {
        struct Noop;
        impl crate::CellFactory for Noop {
            fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
                Ok(())
            }
            fn spawn_cell(
                self: Arc<Self>,
                _: meclaw_core::Path,
                _: JsonValue,
                _: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
                _cell_dir: std::path::PathBuf,
                _contract: crate::ContractView,
                _colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
                _idle_timeout: Option<std::time::Duration>,
                _cell_timeout: i64,
                _message_timeout: Option<std::time::Duration>,
                _blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
                _mailbox_capacity: usize,
            ) -> Result<crate::SpawnedCellKind, String> {
                unimplemented!()
            }
        }
        let mut m = CellFactoryRegistry::new();
        for t in types {
            m.insert((*t).into(), Arc::new(Noop) as Arc<dyn crate::CellFactory>);
        }
        m
    }

    #[test]
    fn schema_check_rejects_non_object_diff() {
        let factories = factories_with(&["echo"]);
        let err = validate_post_state(&json!([]), &factories).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    #[test]
    fn template_missing_when_add_node_uses_unknown_type() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"name": "foo", "template": "unknown"}]});
        let err = validate_post_state(&diff, &factories).unwrap_err();
        assert_eq!(err.error_code(), "template_missing");
    }

    #[test]
    fn valid_minimal_diff_passes() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"name": "foo", "template": "echo"}]});
        assert!(validate_post_state(&diff, &factories).is_ok());
    }

    #[test]
    fn match_no_hit_when_remove_pattern_misses_registry() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"remove_nodes": [{"match": {"name": "absent"}}]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    #[test]
    fn naming_collision_when_add_node_duplicates_existing_name() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"name": "x", "template": "echo"}]});
        let registry_names: Vec<String> = vec!["x".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "naming_collision");
    }

    /// Substrate-fix finding 7 — two `add_nodes` with the SAME name in one diff
    /// form a post_state duplicate (spec § Naming collisions: "a node name that
    /// occurs twice within the same scope in the post_state"). This is
    /// `naming_collision` and must be caught in VALIDATE — before staging — so
    /// it never reaches the rename step, where the second `rename(2)` failed
    /// with a `schema` (rename-IO) token AND left the first node's directory
    /// stranded in `{root}` (no spurless reject).
    #[test]
    fn in_diff_duplicate_add_node_name_is_naming_collision() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [
            {"name": "dup", "template": "echo"},
            {"name": "dup", "template": "echo"}
        ]});
        let registry_names: Vec<String> = vec![];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "naming_collision");
    }

    /// Substrate fix, finding 2 — spec overview § Validation: "cycle-freedom …
    /// insofar as the application forbids cycles; meclaw-core does not reject
    /// cycles in general". A self-edge `a → a` (a tool/reply loop's degenerate
    /// form) is instantiable per mutation; meclaw-core does NOT reject it. The runtime
    /// TTL loop-guard (`hive_cycle_terminates_with_ttl_expired`) bounds it.
    #[test]
    fn self_edge_cycle_is_tolerated() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "a", "to": "a"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "meclaw-Core does not reject cycles (Befund 2)"
        );
    }

    /// Befund 2 — a 2-node reply loop (`a ⇄ b`, the bot-basic reply-leg shape)
    /// must validate; the identical shape boots fine from the filesystem, so the
    /// mutation path matches that tolerance.
    #[test]
    fn two_node_loop_cycle_is_tolerated() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![("a".into(), "b".into())];
        let diff = json!({"add_edges": [{"from": "b", "to": "a"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "meclaw-Core does not reject cycles (Befund 2)"
        );
    }

    #[test]
    fn dag_passes_cycle_check() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let existing_edges: Vec<(String, String)> = vec![("a".into(), "b".into())];
        let diff = json!({"add_edges": [{"from": "b", "to": "c"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok()
        );
    }

    /// Substrat-Fix Befund 6 — an `add_edges` edge may reference a node added in
    /// the SAME diff (spec § Mutation-Format: post_state validation). The
    /// canonical mutation endpoint form is `./name` (overview § Variablen-
    /// Substitution example). Both `./a`/`./b` and the bare `a`/`b` must resolve
    /// against the diff's `add_nodes` short-names — `./`-prefixed endpoints were
    /// rejected as `edge_schema` ("from='./a' unknown") because the membership
    /// check compared the raw string against bare short-names.
    #[test]
    fn add_edges_dot_slash_endpoints_resolve_against_same_diff_add_nodes() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec![];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_nodes": [
                {"name": "a", "template": "echo"},
                {"name": "b", "template": "echo"}
            ],
            "add_edges": [{"from": "./a", "to": "./b"}]
        });
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "`./a`/`./b` must resolve against same-diff add_nodes (Befund 6)"
        );
    }

    /// Befund 6 control: a `./ghost` endpoint with no matching node (neither in
    /// registry nor add_nodes) still rejects — the fix normalises the form, it
    /// does not weaken the existence check.
    #[test]
    fn add_edges_dot_slash_endpoint_to_unknown_node_still_rejects() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "./a", "to": "./ghost"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    #[test]
    fn edge_schema_rejected_when_endpoint_unknown() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "a", "to": "ghost"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// Phase 13.5-A1 T4: malformed CEL `condition` → EdgeSchema(... cel ...).
    #[test]
    fn validate_with_edges_rejects_malformed_cel_condition() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["x".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{"from": "x", "to": "x", "condition": "hop.foo ==="}]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("cel"), "msg should mention cel: {msg}");
                assert!(
                    msg.contains("condition"),
                    "msg should mention condition: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Phase 13.5-A1 T4: valid CEL condition is not rejected on cel-grounds.
    /// (self-edge `x → x` still trips the cycle check, but NOT EdgeSchema(cel).)
    #[test]
    fn validate_with_edges_accepts_valid_cel_condition() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["x".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{"from": "x", "to": "x", "condition": "hop.foo == 'bar'"}]
        });
        let result = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        );
        if let Err(MutationError::EdgeSchema(msg)) = &result {
            assert!(
                !msg.contains("cel"),
                "valid CEL must not be rejected: {msg}"
            );
        }
    }

    /// Phase 13.5-A1 T4 (Slice 3): malformed CEL `modifier.set_hop.*` → EdgeSchema.
    #[test]
    fn validate_with_edges_rejects_malformed_cel_modifier_set() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"set_hop": {"tier": "hop.priority ==="}}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("cel"), "msg should mention cel: {msg}");
                assert!(
                    msg.contains("modifier.set_hop"),
                    "msg should mention modifier.set_hop: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Phase 13.5-A1 T4 (Slice 3): `modifier.delete_hop` must be an array if present.
    #[test]
    fn validate_with_edges_rejects_non_array_modifier_delete() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"delete_hop": "not_an_array"}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("delete"), "msg should mention delete: {msg}")
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// GH #82 (ruling 2026-08-13): the mutation path enforces the same minimum
    /// as config load — a `modifier.restore_ttl` edge is exempt from the TTL
    /// loop guard, so it must carry a bound of its own. No `condition` → reject.
    #[test]
    fn validate_with_edges_rejects_ttl_restoring_edge_without_condition() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"restore_ttl": true}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(
                    msg.contains("restore_ttl") && msg.contains("condition"),
                    "msg must name the field and the missing bound: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// The counterpart: a restoring edge bounded by an iteration condition is a
    /// legal `add_edges` entry — `restore_ttl` is a known modifier key, not an
    /// unknown one.
    #[test]
    fn validate_with_edges_accepts_bounded_ttl_restoring_edge() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "condition": "int(context.iter) < 12",
                "modifier": {
                    "set_context": {"iter": "int(context.iter) + 1"},
                    "restore_ttl": true
                }
            }]
        });
        validate_post_state_with_edges(&diff, &factories, &registry_names, &existing_edges, &[])
            .expect("a restoring edge bounded by an iteration condition must validate");
    }

    /// `restore_ttl` is a declaration, not an expression: a non-boolean value is
    /// a schema error rather than a silently ignored key.
    #[test]
    fn validate_with_edges_rejects_non_boolean_restore_ttl() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "condition": "int(context.iter) < 12",
                "modifier": {"restore_ttl": "yes"}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(
                    msg.contains("restore_ttl") && msg.contains("boolean"),
                    "msg must say restore_ttl is boolean: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Substrate fix, modifier schema (post-finding-6): a `modifier` key outside
    /// the `{set_context, delete_context, set_hop, delete_hop}` schema (spec
    /// overview § Validation l.277: "modifier (if set) matches the
    /// {set?, delete?} schema") must reject as `edge_schema`. The old flat
    /// `{"headers.X": ...}` map form used to commit silently (the unknown key
    /// was ignored at apply) — a builder foot-gun: silent no-op instead of
    /// schema feedback.
    #[test]
    fn validate_with_edges_rejects_unknown_modifier_key() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"headers.msg_type": "'old_form'"}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(
                    msg.contains("headers.msg_type"),
                    "msg should name the unknown key: {msg}"
                );
                assert!(
                    msg.contains("modifier"),
                    "msg should mention modifier: {msg}"
                );
            }
            other => panic!("expected EdgeSchema for unknown modifier key, got {other:?}"),
        }
    }

    /// modifier-Schema control: a modifier that uses ONLY the four valid keys is
    /// not rejected on schema grounds.
    #[test]
    fn validate_with_edges_accepts_all_four_valid_modifier_keys() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {
                    "set_context": {"k": "hop.k"},
                    "delete_context": ["old"],
                    "set_hop": {"h": "hop.h"},
                    "delete_hop": ["gone"]
                }
            }]
        });
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "the four canonical modifier keys must pass the schema check"
        );
    }

    /// Slice 4: malformed CEL `modifier.set_context.<k>` → EdgeSchema, message
    /// names `set_context.iter` (schema-compat coverage for the context path).
    #[test]
    fn validate_with_edges_rejects_malformed_cel_modifier_set_context() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"set_context": {"iter": "=="}}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("cel"), "msg should mention cel: {msg}");
                assert!(
                    msg.contains("set_context.iter"),
                    "msg should mention set_context.iter: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Slice 4: a well-formed four-field modifier (all of `set_context`,
    /// `delete_context`, `set_hop`, `delete_hop` valid) is not rejected on
    /// modifier-schema grounds (no `EdgeSchema` from the modifier-parse path).
    #[test]
    fn validate_with_edges_accepts_well_formed_four_field_modifier() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {
                    "set_context": {"tag": "'gold'"},
                    "delete_context": ["stale"],
                    "set_hop": {"tier": "hop.priority"},
                    "delete_hop": ["transient"]
                }
            }]
        });
        let result = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        );
        assert!(
            result.is_ok(),
            "well-formed four-field modifier must be accepted: {result:?}"
        );
    }

    /// Phase 13.5 step-6: an `add_edges` endpoint that names a hive (by its
    /// short-name) is accepted when `hive_endpoint_names` carries it — symmetric
    /// to the cell case.
    #[test]
    fn edge_schema_accepts_hive_endpoint_via_hive_names() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let hive_names: Vec<String> = vec!["pool".into()];
        // `from=a` (cell) → `to=pool` (hive short-name): no EdgeSchema error.
        let diff = json!({"add_edges": [{"from": "a", "to": "pool"}]});
        let result = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &hive_names,
        );
        assert!(result.is_ok(), "hive endpoint must be accepted: {result:?}");
    }

    /// Phase 13.5 step-6: a truly unknown endpoint still trips EdgeSchema, even
    /// with `hive_endpoint_names` populated (the unknown name is in neither set).
    #[test]
    fn edge_schema_still_rejects_unknown_with_hive_names_present() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let hive_names: Vec<String> = vec!["pool".into()];
        let diff = json!({"add_edges": [{"from": "a", "to": "ghost"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &hive_names,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    #[test]
    fn edge_schema_accepts_endpoint_added_in_same_diff() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        // "newcomer" is born via add_nodes in the same diff — edge endpoint ok.
        let diff = json!({
            "add_nodes": [{"name": "newcomer", "template": "echo"}],
            "add_edges": [{"from": "a", "to": "newcomer"}]
        });
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok()
        );
    }

    // ── T14: validate_post_state_with_templates ──────────────────────────────

    struct StubFactory;
    impl crate::CellFactory for StubFactory {
        fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
            Ok(())
        }
        fn spawn_cell(
            self: Arc<Self>,
            _: meclaw_core::Path,
            _: JsonValue,
            _: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
            _cell_dir: std::path::PathBuf,
            _contract: crate::ContractView,
            _colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
            _idle_timeout: Option<std::time::Duration>,
            _cell_timeout: i64,
            _message_timeout: Option<std::time::Duration>,
            _blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
            _mailbox_capacity: usize,
        ) -> Result<crate::SpawnedCellKind, String> {
            unimplemented!()
        }
    }

    #[test]
    fn validate_rejects_unknown_template() {
        let diff = json!({"add_nodes": [{"name":"x","template":"unknown"}]});
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, MutationError::TemplateMissing(_)));
    }

    #[test]
    fn swap_nodes_rejects_unknown_with_template() {
        // match.name hits "existing" in the registry — rejection must be about
        // the unknown with.template (template_missing), not a match_no_hit.
        // with.name is required (paket-2 T1); target "newnode" is fresh (no collision).
        let diff = json!({
            "swap_nodes": [{"match": {"name": "existing"}, "with": {"template": "unknown", "name": "newnode"}}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let registry_names: Vec<String> = vec!["existing".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "template_missing");
    }

    #[test]
    fn validate_rejects_unknown_cell_type_even_if_template_exists() {
        use std::path::PathBuf;
        let diff = json!({"add_nodes": [{"name":"x","template":"echo"}]});
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: PathBuf::from("/tmp/echo"),
            },
        ]);
        let factories = CellFactoryRegistry::new(); // empty → cell.type lookup fails
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            // simulate: the template directory carries a config.json with cell.type = "ghost"
            &[("echo".into(), "ghost".into())],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, MutationError::UnknownCellType(_)));
    }

    #[test]
    fn validate_passes_when_template_and_cell_type_both_known() {
        use std::path::PathBuf;
        let diff = json!({"add_nodes": [{"name":"x","template":"echo"}]});
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: PathBuf::from("/tmp/echo"),
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(result.is_ok());
    }

    /// T17.fix — R3 conformance: versioned template refs must be resolved via entry.name.
    ///
    /// Bug: the ct_map lookup used the raw `template` string ("echo@1.0.0") instead of
    /// `entry.name` ("echo").
    /// Fix: resolve() returns a TemplateEntry; the ct_map lookup uses entry.name.
    #[test]
    fn validate_resolves_versioned_template_ref() {
        use std::path::PathBuf;
        let diff = json!({"add_nodes": [{"name":"x","template":"echo@1.0.0"}]});
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: Some("1.0.0".into()),
                filesystem_path: PathBuf::from("/tmp/echo"),
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "versioned template ref must resolve via entry.name"
        );
    }

    // ── Phase 13.5 Slice 4 T1: swap_nodes validate arm ──────────────────────

    /// T1 (RED): swap_nodes with a match.name that hits no registry cell must be
    /// rejected with error_code == "match_no_hit".
    #[test]
    fn swap_nodes_match_no_hit_is_rejected() {
        let factories = factories_with(&["echo"]);
        let diff =
            json!({"swap_nodes": [{"match": {"name": "absent"}, "with": {"template": "echo"}}]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    // ── T7: validate_post_state_with_edges_and_subtree ──────────────────────

    /// T7 (RED): a subtree-internal edge whose target is a subtree-nested node
    /// validates — the nested node counts as a valid edge endpoint.
    #[test]
    fn subtree_internal_edge_to_nested_node_validates() {
        let factories = factories_with(&["echo"]);
        // No diff add_edges; the subtree contributes its own nodes + internal edge.
        let diff = json!({});
        let subtree_nodes: Vec<String> = vec![
            "/main/m1".into(),
            "/main/m1/inner_a".into(),
            "/main/m1/inner_b".into(),
        ];
        let subtree_edges: Vec<(String, String)> =
            vec![("/main/m1/inner_a".into(), "/main/m1/inner_b".into())];
        let result = validate_post_state_with_edges_and_subtree(
            &diff,
            &factories,
            &[],
            &[],
            &[],
            &subtree_nodes,
            &subtree_edges,
        );
        assert!(
            result.is_ok(),
            "nested subtree edge must validate: {result:?}"
        );
    }

    /// Substrat-Fix Befund 2: a subtree-internal edge-set that forms a cycle
    /// (a→b, b→a) is TOLERATED — meclaw-Core does not reject cycles generally
    /// (spec overview § Validation). Endpoint-existence still applies (both
    /// endpoints are in `subtree_nodes`), so this validates clean.
    #[test]
    fn subtree_internal_edge_cycle_is_tolerated() {
        let factories = factories_with(&["echo"]);
        let diff = json!({});
        let subtree_nodes: Vec<String> = vec!["/main/m1/a".into(), "/main/m1/b".into()];
        let subtree_edges: Vec<(String, String)> = vec![
            ("/main/m1/a".into(), "/main/m1/b".into()),
            ("/main/m1/b".into(), "/main/m1/a".into()),
        ];
        assert!(
            validate_post_state_with_edges_and_subtree(
                &diff,
                &factories,
                &[],
                &[],
                &[],
                &subtree_nodes,
                &subtree_edges,
            )
            .is_ok(),
            "cyclic subtree-internal edges are tolerated (Befund 2)"
        );
    }

    /// T7 (RED): a subtree-internal edge whose endpoint is absent from the
    /// node-set is rejected with the edge-schema error.
    #[test]
    fn subtree_internal_edge_endpoint_not_in_nodeset_is_rejected() {
        let factories = factories_with(&["echo"]);
        let diff = json!({});
        let subtree_nodes: Vec<String> = vec!["/main/m1/a".into()];
        // edge points at /main/m1/ghost which is in neither node-set.
        let subtree_edges: Vec<(String, String)> =
            vec![("/main/m1/a".into(), "/main/m1/ghost".into())];
        let err = validate_post_state_with_edges_and_subtree(
            &diff,
            &factories,
            &[],
            &[],
            &[],
            &subtree_nodes,
            &subtree_edges,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// T7 (RED): the OLD wrapper on a single-cell diff is byte-behavior-unchanged
    /// — a valid single-cell diff still returns `Ok(())` (guards the delegation).
    #[test]
    fn existing_validate_post_state_with_edges_unchanged_for_single_cell() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let existing_edges: Vec<(String, String)> = vec![("a".into(), "b".into())];
        let diff = json!({"add_edges": [{"from": "b", "to": "c"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok()
        );
    }

    /// Pin: `swap_nodes` is **name-keyed** → a single swap entry resolves to
    /// EXACTLY ONE target (`resolve_scoped_path(scope, match.name)`). A match
    /// WITHOUT `name` (e.g. a `template`-only match that could otherwise hit >1
    /// cell) is rejected at validation with `error_code == "schema"` — it is NEVER
    /// silently applied to multiple cells. This nails the single-target invariant
    /// behind the deferred "Multi-Target-swap atomicity" item (only a swap_nodes
    /// ARRAY with multiple entries is multi-target, and that is applied per-entry
    /// after the whole diff has validated).
    #[test]
    fn swap_nodes_match_without_name_is_rejected_schema() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"swap_nodes": [
            {"match": {"template": "echo"}, "with": {"template": "echo"}}
        ]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    // ── paket-2 T1: swap_nodes with-schema + strict rejects + scope-binding ──

    /// Rule 1 (Strict-Regel c): `with` missing entirely → schema error.
    #[test]
    fn swap_nodes_with_missing_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [{"match": {"name": "t2"}}]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1 (Strict-Regel c): instantiate form with unknown key → schema error.
    /// Typo `"tempalte"` must NOT be silently treated as existing-node form.
    #[test]
    fn swap_nodes_instantiate_unknown_key_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // "tempalte" is a typo — has no "template" key, has unknown key → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"tempalte": "echo", "name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1 (Strict-Regel c): existing-node form with unknown key → schema error.
    #[test]
    fn swap_nodes_existing_unknown_key_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // No "template" key, but "extra" key is unknown → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t2", "extra": "bad"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1: new instantiate form (`template` + `name` + unknown extra key) → schema error.
    /// Legacy form `{template}` (no `name`) is backward-compatible and NOT rejected here.
    #[test]
    fn swap_nodes_new_instantiate_extra_key_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // Has both template AND name → new form. Extra key "foo" → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": "t3", "foo": "bad"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1: `with.name` missing in existing-node form (empty `with`) → schema error.
    #[test]
    fn swap_nodes_existing_name_missing_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // Empty `with` object — no name → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 3 (Strict-Regel a): existing-node form where t3 is not in registry →
    /// MatchNoHit (scope-bound rejection, loud).
    #[test]
    fn swap_nodes_existing_target_not_found_is_match_no_hit() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t3_ghost"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()]; // t3_ghost not present
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// Rule 3: existing-node form where t3 IS in registry → passes validation.
    #[test]
    fn swap_nodes_existing_target_found_passes() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "t3".into()];
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "existing-form with known t3 must pass: {result:?}"
        );
    }

    /// Rule 4 (Strict-Regel b): instantiate form where t3 is already in registry →
    /// NamingCollision.
    #[test]
    fn swap_nodes_instantiate_target_collision_is_naming_collision() {
        use std::path::PathBuf;
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: PathBuf::from("/tmp/echo_single"),
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        // t3 = "already_there" is already in registry → NamingCollision.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": "already_there"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "already_there".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "naming_collision");
    }

    /// Rule 4: instantiate form where t3 is NOT in registry → no collision, passes.
    /// Uses a real temp dir so `reject_if_subtree_template` can scan it.
    #[test]
    fn swap_nodes_instantiate_fresh_target_passes() {
        // Build a minimal single-cell template dir (no nested config.json → not a subtree).
        let tmp = tempfile::tempdir().unwrap();
        let tpl_dir = tmp.path().to_path_buf();
        std::fs::write(tpl_dir.join("template.json"), r#"{"name":"echo"}"#).unwrap();
        std::fs::write(
            tpl_dir.join("config.json"),
            r#"{"cell":{"type":"echo_type"}}"#,
        )
        .unwrap();

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: tpl_dir,
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": "t3_fresh"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "instantiate-form with fresh t3 must pass: {result:?}"
        );
    }

    /// Rule 5 (A6 subtree-guard): instantiate form whose template resolves to a
    /// SUBTREE template → schema error.
    ///
    /// Uses a real temp dir with a nested `config.json` to trigger the subtree
    /// guard in `reject_if_subtree_template`.
    #[test]
    fn swap_nodes_instantiate_subtree_template_is_schema() {
        use std::path::PathBuf;
        // Build a temp dir that looks like a subtree template (has nested dir with config.json).
        let tmp = tempfile::tempdir().unwrap();
        let tpl_dir = tmp.path();
        std::fs::write(tpl_dir.join("template.json"), r#"{"name":"subtree_tpl"}"#).unwrap();
        std::fs::write(tpl_dir.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
        let nested = tpl_dir.join("inner_cell");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "st1".into(),
                name: "subtree_tpl".into(),
                version: None,
                filesystem_path: PathBuf::from(tpl_dir),
            },
        ]);
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "subtree_tpl", "name": "t3_fresh"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "schema",
            "subtree template must reject as schema, got {err:?}"
        );
    }

    /// Instantiate form where `name` is a non-string (e.g. `{"name": 42}`) →
    /// the `.as_str()` conversion fails and the `ok_or_else(Schema)` fires.
    /// This is an intentional schema reject, not dead code.
    #[test]
    fn swap_nodes_instantiate_name_non_string_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // Both template + name present (instantiate form), but name is an integer.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": 42}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "schema",
            "non-string name must be a schema error, got {err:?}"
        );
    }

    // ── T5 Part 1: with.name post-state resolution (forward reference) ─────────

    /// T5-P1 direct unit test: validate_swap_with_entry must accept a `with.name`
    /// that is in `add_names` (post-state) but NOT in `registry_names` (pre-state).
    /// And it must still reject a `with.name` that is in neither.
    #[test]
    fn validate_swap_with_entry_accepts_add_names_forward_reference() {
        let templates = crate::templates::TemplatesRegistry::default();

        // Existing-form `with: {name: "t3"}`. t3 NOT in pre-state registry_names,
        // but IS in add_names (post-state contribution from add_nodes in same diff).
        let with_val = json!({"name": "t3"});
        let registry_names: Vec<String> = vec!["t2".into()];
        let add_names: Vec<String> = vec!["t3".into()]; // t3 added by add_nodes in same diff

        // Should PASS: t3 is reachable via post-state (registry ∪ add_names).
        // match_name = "t2" (the source being swapped); with.name = "t3" ≠ "t2".
        let result =
            validate_swap_with_entry_full(&with_val, "t2", &registry_names, &add_names, &templates);
        assert!(
            result.is_ok(),
            "with.name forward ref from add_nodes must pass: {result:?}"
        );

        // Ghost: in neither pre-state nor add_names → still match_no_hit.
        let with_ghost = json!({"name": "ghost"});
        let err = validate_swap_with_entry_full(
            &with_ghost,
            "t2",
            &registry_names,
            &add_names,
            &templates,
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "match_no_hit",
            "ghost name must still reject as match_no_hit: {err:?}"
        );
    }

    /// T13 (identity-swap reject): existing-node form where with.name == match.name
    /// (same node, same scope) must be rejected with error_code == "schema".
    /// Swapping a node onto itself drops all edges (every swung edge becomes a
    /// self-loop and is dropped) — that is a loud spec violation, not a silent no-op.
    #[test]
    fn swap_nodes_existing_identity_swap_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // with.name == match.name → identity swap → must be rejected.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t2"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "schema",
            "identity swap must be schema error, got {err:?}"
        );
    }

    /// T13 guard: existing-node form with.name != match.name must still pass.
    #[test]
    fn swap_nodes_existing_non_identity_swap_still_passes() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "t3".into()];
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "normal existing-form swap must pass: {result:?}"
        );
    }

    /// Rule 6 (A2 scope-binding): `with.name` (existing form) naming a node that
    /// lives in a DIFFERENT scope is rejected as MatchNoHit because the caller
    /// (colony.rs) filters `registry_names` to scope-local names only.
    /// Here we model the filtered set: the cross-scope name is absent.
    #[test]
    fn swap_nodes_existing_target_cross_scope_is_match_no_hit() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // The diff names t3 = "other_scope_node".
        // registry_names is filtered to current scope — "other_scope_node" absent.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "other_scope_node"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()]; // scope-filtered: no "other_scope_node"
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// Paket-5 T4 (P10b companion, finding Paket-2-b'): a `swap_nodes` `match.name`
    /// naming a HIVE that lives in a FOREIGN scope must be rejected as MatchNoHit.
    /// The caller scope-filters `hive_match_names` (parent path == guard_scope), so a
    /// hive in another scope never contributes its short-name here — the global
    /// `hive_endpoint_names` (8th→7th arg) MUST NOT short-circuit this. Modelled by
    /// passing the foreign hive ONLY in the global endpoint set, NOT in the scoped
    /// match set. Before the split this was a false-positive pass.
    #[test]
    fn swap_nodes_match_name_foreign_scope_hive_is_match_no_hit() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "pool"}, "with": {"name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        // Global endpoint set carries the foreign hive (it lives elsewhere in the
        // colony); the SCOPED match set does NOT — so match.name must miss.
        let hive_endpoint_names: Vec<String> = vec!["pool".into()];
        let hive_match_names: Vec<String> = vec![];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &hive_endpoint_names,
            &hive_match_names,
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// Paket-5 T4 (P10b companion): a `swap_nodes` `match.name` naming a HIVE in the
    /// CORRECT scope still passes — the scoped `hive_match_names` carries it, so no
    /// false-negative. Mirror of `edge_schema_accepts_hive_endpoint_via_hive_names`
    /// but for the match.name existence check.
    #[test]
    fn swap_nodes_match_name_in_scope_hive_passes() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // match.name = "pool" (the in-scope hive). `with` is the existing-node form
        // naming a real registry target so the swap's `with` check also passes — that
        // isolates the assertion on the match.name step succeeding for the hive.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "pool"}, "with": {"name": "target_cell"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "target_cell".into()];
        let hive_endpoint_names: Vec<String> = vec!["pool".into()];
        let hive_match_names: Vec<String> = vec!["pool".into()]; // scope-local hive
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &hive_endpoint_names,
            &hive_match_names,
            &[],
            &[],
        );
        // match.name "pool" is satisfied by the in-scope hive → no match_no_hit.
        // (The `with` here is an existing-node form with a fresh, non-colliding name,
        // which is accepted, so the overall result is Ok.)
        assert!(
            result.is_ok(),
            "in-scope hive match.name must pass, got: {result:?}"
        );
    }

    // ── Paket-5 T1/T2: remove_edges validate-time reject (P10a / D-031) ──────

    fn edge_view(from: &str, to: &str) -> EdgeMatchView {
        EdgeMatchView {
            from: from.into(),
            to: to.into(),
            condition_source: None,
            modifier_source: None,
        }
    }

    /// T1 (RED): a `remove_edges` entry with a missing `match.from` →
    /// Schema("remove_edges[].match.from missing").
    #[test]
    fn remove_edges_missing_from_is_schema() {
        let diff = json!({"remove_edges": [{"match": {"to": "b"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        let err = validate_remove_edges(&diff, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "schema");
        match err {
            MutationError::Schema(msg) => assert!(
                msg.contains("match.from"),
                "msg should mention match.from: {msg}"
            ),
            other => panic!("expected Schema, got {other:?}"),
        }
    }

    /// T1 (RED): a `remove_edges` entry with a missing `match.to` →
    /// Schema("remove_edges[].match.to missing").
    #[test]
    fn remove_edges_missing_to_is_schema() {
        let diff = json!({"remove_edges": [{"match": {"from": "a"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        let err = validate_remove_edges(&diff, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "schema");
        match err {
            MutationError::Schema(msg) => {
                assert!(
                    msg.contains("match.to"),
                    "msg should mention match.to: {msg}"
                )
            }
            other => panic!("expected Schema, got {other:?}"),
        }
    }

    /// T2 (RED): a `remove_edges` pattern matching ZERO existing edges →
    /// MatchNoHit (loud reject, NOT a silent no-op).
    #[test]
    fn remove_edges_no_hit_is_match_no_hit() {
        let diff = json!({"remove_edges": [{"match": {"from": "a", "to": "ghost"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        let err = validate_remove_edges(&diff, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// T2 (positive): a `remove_edges` pattern hitting an existing edge (from/to
    /// only, scope-resolved) → Ok.
    #[test]
    fn remove_edges_hit_passes() {
        let diff = json!({"remove_edges": [{"match": {"from": "a", "to": "b"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        assert!(validate_remove_edges(&diff, "/main", &edges).is_ok());
    }

    /// T2 (positive, F6 condition): pattern with a `condition` matching the
    /// edge's stored condition source → Ok; a non-matching condition → no-hit.
    #[test]
    fn remove_edges_condition_constraint_matches_and_misses() {
        let edges = vec![EdgeMatchView {
            from: "/main/a".into(),
            to: "/main/b".into(),
            condition_source: Some("hop.x == 'y'".into()),
            modifier_source: None,
        }];
        // Matching condition → Ok.
        let hit = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "condition": "hop.x == 'y'"}}
        ]});
        assert!(validate_remove_edges(&hit, "/main", &edges).is_ok());
        // Non-matching condition → MatchNoHit (constraint narrows the match).
        let miss = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "condition": "hop.x == 'z'"}}
        ]});
        let err = validate_remove_edges(&miss, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// T2 (positive, F6 modifier): pattern with a `modifier` matching the edge's
    /// stored modifier JSON → Ok.
    #[test]
    fn remove_edges_modifier_constraint_matches() {
        let modifier = json!({"set_hop": {"tier": "hop.priority"}});
        let edges = vec![EdgeMatchView {
            from: "/main/a".into(),
            to: "/main/b".into(),
            condition_source: None,
            modifier_source: Some(modifier.clone()),
        }];
        let diff = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "modifier": modifier}}
        ]});
        assert!(validate_remove_edges(&diff, "/main", &edges).is_ok());
    }

    /// T2 (F6 modifier-miss): pattern with a `modifier` that does NOT equal the
    /// edge's stored modifier JSON → MatchNoHit (mirrors the condition-miss
    /// case; the modifier constraint narrows the match away).
    #[test]
    fn remove_edges_modifier_constraint_misses() {
        let edges = vec![EdgeMatchView {
            from: "/main/a".into(),
            to: "/main/b".into(),
            condition_source: None,
            modifier_source: Some(json!({"set_hop": {"tier": "hop.priority"}})),
        }];
        let miss = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "modifier": {"set_hop": {"tier": "hop.other"}}}}
        ]});
        let err = validate_remove_edges(&miss, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// No `remove_edges` key at all → Ok (nothing to check).
    #[test]
    fn remove_edges_absent_passes() {
        let diff = json!({"add_nodes": []});
        assert!(validate_remove_edges(&diff, "/main", &[]).is_ok());
    }

    // ── Paket-5 T3: regression-lock pin-tests for already-loud diff arms ─────
    // These pin the CURRENT loud behavior (malformed / no-hit) of the other
    // diff arms so future refactors cannot silently regress it. They are
    // expected to PASS immediately; a failure here is a real gap, not a fix.

    /// Pin: `remove_nodes` with a `match` that omits `name` →
    /// Schema("remove_nodes[].match.name missing"). The single-target invariant
    /// requires a `name`; a nameless match is never silently applied.
    #[test]
    fn remove_nodes_match_without_name_is_schema() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"remove_nodes": [{"match": {}}]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Pin: `add_edges` entry missing `from` →
    /// Schema("add_edges[].from missing"). A malformed edge is rejected loudly,
    /// never silently dropped.
    #[test]
    fn add_edges_missing_from_is_schema() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"to": "b"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Pin: `add_edges` entry missing `to` →
    /// Schema("add_edges[].to missing").
    #[test]
    fn add_edges_missing_to_is_schema() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "a"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Pin: `add_nodes` entry missing `name` →
    /// Schema("add_nodes[].name missing"). Checked by `validate_post_state_full`
    /// (the naming-collision pass needs a name to compare against the registry).
    #[test]
    fn add_nodes_missing_name_is_schema() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"template": "echo"}]});
        let registry_names: Vec<String> = vec![];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    // ── Slice 6: header-contract locality (Phase-14-B as build-time error) ───

    use std::collections::{BTreeMap, BTreeSet};

    fn keys(ks: &[&str]) -> BTreeSet<String> {
        ks.iter().map(|s| s.to_string()).collect()
    }

    /// Build a [`HeaderNodeView`] from emits.hop / required-context / required-hop.
    fn node(emits_hop: &[&str], req_ctx: &[&str], req_hop: &[&str]) -> HeaderNodeView {
        HeaderNodeView {
            emits_hop: keys(emits_hop),
            required_context: keys(req_ctx),
            required_hop: keys(req_hop),
        }
    }

    /// Build a bare [`HeaderEdgeView`] from `from`/`to` with no modifier keys.
    fn edge(from: &str, to: &str) -> HeaderEdgeView {
        HeaderEdgeView {
            from: from.into(),
            to: to.into(),
            ..Default::default()
        }
    }

    /// (a) Collector requires `consumes.hop.operation`; its single predecessor
    /// neither emits `operation` (emits.hop) nor sets it on the edge → reject.
    #[test]
    fn rejects_required_hop_consume_not_set_by_immediate_predecessor() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), node(&["other"], &[], &[]));
        contracts.insert("collector".into(), node(&[], &[], &["operation"]));
        let edges = vec![edge("src", "collector")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("operation"), "names key: {msg}");
                assert!(msg.contains("hop"), "names hop: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (b) Two incoming edges; one delivers the required hop key, the other does
    /// not → reject (fan-in intersection, Korrektur #2).
    #[test]
    fn rejects_required_hop_consume_missing_in_one_fan_in_edge() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("a".into(), node(&["operation"], &[], &[]));
        contracts.insert("b".into(), node(&[], &[], &[])); // does NOT emit operation
        contracts.insert("collector".into(), node(&[], &[], &["operation"]));
        let edges = vec![edge("a", "collector"), edge("b", "collector")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
        if let MutationError::EdgeSchema(msg) = err {
            assert!(msg.contains("operation"), "names key: {msg}");
            assert!(
                msg.contains("intersection"),
                "names fan-in intersection: {msg}"
            );
        }
    }

    /// (c) Node requires a NON-ingress `consumes.context` key with no
    /// `set_context` setter anywhere upstream → reject.
    #[test]
    fn rejects_required_context_consume_with_no_setter_path() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), node(&[], &[], &[]));
        contracts.insert("sink".into(), node(&[], &["custom_key"], &[]));
        let edges = vec![edge("src", "sink")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("custom_key"), "names key: {msg}");
                assert!(msg.contains("context"), "names context: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (d) Node requires an INGRESS context key (`turn_id`), reachable from the
    /// graph entry (no intervening delete) → accepted.
    #[test]
    fn accepts_required_context_consume_for_ingress_key() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), node(&[], &[], &[]));
        contracts.insert("sink".into(), node(&[], &["turn_id"], &[]));
        let edges = vec![edge("src", "sink")];
        assert!(validate_header_contract_locality(&contracts, &edges, &keys(&[])).is_ok());
    }

    /// (e) Two incoming edges, BOTH deliver the required hop key → accepted.
    #[test]
    fn accepts_hop_consume_in_fan_in_intersection() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("a".into(), node(&["operation"], &[], &[]));
        contracts.insert("b".into(), node(&["operation"], &[], &[]));
        contracts.insert("collector".into(), node(&[], &[], &["operation"]));
        let edges = vec![edge("a", "collector"), edge("b", "collector")];
        assert!(validate_header_contract_locality(&contracts, &edges, &keys(&[])).is_ok());
    }

    /// (f) Regression: the backwards context-reachability BFS must TERMINATE on a
    /// CYCLIC edge graph (meclaw graphs may legitimately contain tool-loops). The
    /// topology `a → b`, `b → a`, `a → sink` cycles between `a` and `b`; `sink`
    /// requires a NON-ingress `consumes.context` key (`custom_key`) with no
    /// `set_context` setter anywhere → the key is unreachable. The test proves
    /// termination purely by RETURNING (rather than hanging) with the expected
    /// unreachable `Err(EdgeSchema)`; the `visited`-set guards the cycle.
    #[test]
    fn context_reachability_terminates_on_cyclic_graph() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("a".into(), node(&[], &[], &[]));
        contracts.insert("b".into(), node(&[], &[], &[]));
        contracts.insert("sink".into(), node(&[], &["custom_key"], &[]));
        // Cycle: a ⇄ b, plus a → sink (the consuming node).
        let edges = vec![edge("a", "b"), edge("b", "a"), edge("a", "sink")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("custom_key"), "names key: {msg}");
                assert!(msg.contains("context"), "names context: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (g) Regression: a SELF-LOOP `n → n` must also terminate the BFS. `n`
    /// requires a NON-ingress `consumes.context` key with no setter → unreachable,
    /// returns `Err(EdgeSchema)` instead of looping on its own incoming edge.
    #[test]
    fn context_reachability_terminates_on_self_loop() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("n".into(), node(&[], &["custom_key"], &[]));
        let edges = vec![edge("n", "n")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("custom_key"), "names key: {msg}");
                assert!(msg.contains("context"), "names context: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    // ── F1 fix (K-H1): hive-transit participation in the fan-in intersection ─

    /// (h) F1 / K-H1 shape: a hop key set on the edge INTO a hive must credit
    /// the consumer behind the transit — the hive passes the hop compartment
    /// through unchanged (spec § Hive-Pfade als Target — Transit-Auswertung;
    /// hop decays only at a CELL emission). Honest contract
    /// (`consumes.hop.hmark required:true` behind the transit) must validate.
    #[test]
    fn accepts_required_hop_set_on_edge_into_hive_transit() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/entry".into(), node(&[], &[], &[]));
        contracts.insert("/sub/cellA".into(), node(&[], &[], &["hmark"]));
        let mut into_hive = edge("/entry", "/sub");
        into_hive.set_hop = keys(&["hmark"]);
        let edges = vec![into_hive, edge("/sub", "/sub/cellA")];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/sub"])).is_ok(),
            "set_hop on the edge into the hive must satisfy the consumer behind the transit"
        );
    }

    /// (i) Multi-level transit chain (K-H4 needs depth 3): the source cell's
    /// `emits.hop` key walks across THREE chained hives unchanged; consumers
    /// behind depth 2 AND depth 3 both validate.
    #[test]
    fn accepts_required_hop_across_multi_level_transit_chain() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/src".into(), node(&["k"], &[], &[]));
        contracts.insert("/mid".into(), node(&[], &[], &["k"])); // behind depth 2
        contracts.insert("/sink".into(), node(&[], &[], &["k"])); // behind depth 3
        let edges = vec![
            edge("/src", "/h1"),
            edge("/h1", "/h2"),
            edge("/h2", "/mid"),
            edge("/h2", "/h3"),
            edge("/h3", "/sink"),
        ];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h1", "/h2", "/h3"]))
                .is_ok(),
            "emits.hop key must walk across chained transits (depth 2 and 3)"
        );
    }

    /// (j) NEGATIVE (non-vacuity guard, mandatory point TDD b): a key NOT
    /// guaranteed on EVERY path into the hive must still reject — a second
    /// inbound edge without the key empties the transit contribution.
    #[test]
    fn rejects_required_hop_missing_on_one_path_into_hive_transit() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/a".into(), node(&[], &[], &[]));
        contracts.insert("/b".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut a_in = edge("/a", "/h");
        a_in.set_hop = keys(&["k"]);
        let edges = vec![a_in, edge("/b", "/h"), edge("/h", "/sink")];
        let err =
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h"])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("'k'"), "names key: {msg}");
                assert!(msg.contains("intersection"), "names intersection: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (k) `delete_hop` on the transit edge severs the key even when the
    /// hive's inbound fan-in provides it (delete wins over the pass-through).
    #[test]
    fn rejects_required_hop_deleted_on_transit_edge() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/a".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut a_in = edge("/a", "/h");
        a_in.set_hop = keys(&["k"]);
        let mut transit = edge("/h", "/sink");
        transit.delete_hop = keys(&["k"]);
        let edges = vec![a_in, transit];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h"])).is_err(),
            "delete_hop on the transit edge must sever the key"
        );
    }

    /// (l) Loops are legal (tool-/refine-loops, mandatory point 2): a hive⇄hive
    /// cycle on the walk must TERMINATE (proven by returning) and must NOT
    /// falsely empty the intersection — the loop back-edge imposes no
    /// additional obligation (greatest-fixpoint reading).
    #[test]
    fn transit_walk_terminates_and_provides_across_hive_cycle() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/src".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut src_in = edge("/src", "/h1");
        src_in.set_hop = keys(&["k"]);
        let edges = vec![
            src_in,
            edge("/h1", "/h2"),
            edge("/h2", "/h1"), // cycle back-edge
            edge("/h1", "/sink"),
        ];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h1", "/h2"])).is_ok(),
            "hive cycle must not falsely empty the fan-in intersection"
        );
    }

    /// (m) The cycle guard must NOT make loops vacuously green: a
    /// `delete_hop` on the loop edge severs the key on every looped path —
    /// the intersection at the cycling hive empties → reject.
    #[test]
    fn rejects_required_hop_deleted_on_loop_edge_of_hive_cycle() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/src".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut src_in = edge("/src", "/h1");
        src_in.set_hop = keys(&["k"]);
        let mut loop_out = edge("/h1", "/h2");
        loop_out.delete_hop = keys(&["k"]);
        let edges = vec![
            src_in,
            loop_out,
            edge("/h2", "/h1"), // looped path arrives at /h1 without k
            edge("/h1", "/sink"),
        ];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h1", "/h2"])).is_err(),
            "a looped path that lost the key must empty the intersection"
        );
    }

    /// (n) A hive with NO inbound edge can never deliver via its out-edge:
    /// zero runtime paths impose zero obligations (vacuous), and the wiring
    /// mutation that later connects the hive re-validates the full
    /// post-state.
    #[test]
    fn accepts_required_hop_behind_unconnected_hive_vacuously() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let edges = vec![edge("/h", "/sink")];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h"])).is_ok(),
            "unconnected hive's out-edge can never fire — vacuously providing"
        );
    }

    /// Hardening Slice 1: the pure `ContractBlock` → [`HeaderNodeView`]
    /// projection keeps only `required: true` consume keys and all `emits.hop`
    /// keys; `required` defaults to true (config.md Z.107).
    #[test]
    fn header_view_from_contract_projects_required_keys_only() {
        let block: crate::config::ContractBlock =
            meclaw_core::serde_json::from_value(meclaw_core::serde_json::json!({
                "emits": {"hop": {"k1": {"type": "string"}}},
                "consumes": {
                    "context": {"c1": {"type": "string", "required": true},
                                 "c2": {"type": "string", "required": false}},
                    "hop": {"h1": {"type": "string"}}
                }
            }))
            .unwrap();
        let view = header_view_from_contract(&block);
        assert_eq!(view.emits_hop.iter().collect::<Vec<_>>(), vec!["k1"]);
        assert_eq!(view.required_context.iter().collect::<Vec<_>>(), vec!["c1"]);
        // required defaultet auf true (config.md Z.107)
        assert_eq!(view.required_hop.iter().collect::<Vec<_>>(), vec!["h1"]);
    }

    /// Substrat-Fix Slice 0 (security hotfix) — FS-realisation traversal guard.
    /// The apply-side joins (`resolve_cell_dir`, `staging_root.join(name)`) do
    /// NOT root-clamp `..`, while the logical normalisation here does: at root
    /// scope `/../escape` clamps to `/escape` ("within /") and the filesystem
    /// realisation walks OUTSIDE `{root}`. A raw `..` segment in any top-level
    /// diff name must therefore reject as `scope_out_of_bounds` regardless of
    /// where the clamped path lands (spec § Mutation-Format: paths that would
    /// land outside the scope are rejected at validation).
    #[test]
    fn scope_containment_rejects_dotdot_segment_in_every_name_bearing_op_at_root() {
        let diffs = [
            (
                "add_nodes[].name",
                json!({"add_nodes": [{"name": "../escape", "template": "echo"}]}),
            ),
            (
                "remove_nodes[].match.name",
                json!({"remove_nodes": [{"match": {"name": "../escape"}}]}),
            ),
            (
                "swap_nodes[].match.name",
                json!({"swap_nodes": [{"match": {"name": "../escape"}, "with": {"name": "ok"}}]}),
            ),
            (
                "swap_nodes[].with.name",
                json!({"swap_nodes": [{"match": {"name": "ok"}, "with": {"name": "../escape"}}]}),
            ),
            (
                "add_edges[].from",
                json!({"add_edges": [{"from": "../escape", "to": "ok"}]}),
            ),
            (
                "add_edges[].to",
                json!({"add_edges": [{"from": "ok", "to": "../escape"}]}),
            ),
            (
                "remove_edges[].match.from",
                json!({"remove_edges": [{"match": {"from": "../escape", "to": "ok"}}]}),
            ),
            (
                "remove_edges[].match.to",
                json!({"remove_edges": [{"match": {"from": "ok", "to": "../escape"}}]}),
            ),
        ];
        for (field, diff) in diffs {
            let err = validate_scope_containment(&diff, "/")
                .expect_err(&format!("{field}: raw `..` must reject at root scope"));
            assert_eq!(err.error_code(), "scope_out_of_bounds", "{field}");
        }
    }

    /// Absolute names: `PathBuf::join` REPLACES the base when joined with an
    /// absolute path, so an absolute `add_nodes[].name` realises a directory at
    /// an arbitrary host location. No top-level diff op has a legitimate
    /// absolute-name producer (`resolve_scoped_path` string-joins them blindly,
    /// so they were never honoured as absolute logically) → reject.
    #[test]
    fn scope_containment_rejects_absolute_name() {
        let diff = json!({"add_nodes": [{"name": "/abs/escape", "template": "echo"}]});
        let err = validate_scope_containment(&diff, "/").unwrap_err();
        assert_eq!(err.error_code(), "scope_out_of_bounds");
    }

    /// Embedded separators hiding a traversal (`a/../../escape`): the clamped
    /// logical path lands within scope, the FS realisation does not → reject.
    #[test]
    fn scope_containment_rejects_embedded_dotdot_traversal() {
        let diff = json!({"add_nodes": [{"name": "a/../../escape", "template": "echo"}]});
        let err = validate_scope_containment(&diff, "/").unwrap_err();
        assert_eq!(err.error_code(), "scope_out_of_bounds");
    }

    /// Positive control: legitimate name forms (bare name, `./`-prefixed edge
    /// endpoint) stay accepted at root scope — the guard rejects traversal,
    /// not shape (`./a` is an established mutation-diff form, see
    /// workshop/fixtures/negative/edge_schema).
    #[test]
    fn scope_containment_accepts_bare_and_dot_slash_names() {
        let diff = json!({
            "add_nodes": [{"name": "worker", "template": "echo"}],
            "add_edges": [{"from": "./worker", "to": "worker"}]
        });
        assert!(validate_scope_containment(&diff, "/").is_ok());
    }

    // ── R12: add_edges depth-endpoint resolution into sub-scopes ────────────
    //
    // Spec Z.227: edge from/to are "paths relative to the hive scope" — WITHOUT a
    // depth restriction. A `./unit/dispatch` endpoint must resolve against the
    // mutation scope (post_state membership), while containment stays sharp
    // (Form C of the llm-unit RECEIPT matrix is covered by
    // `validate_scope_containment`, unchanged).

    /// R12 Form B (validate level): a depth endpoint matches a node that
    /// already EXISTS in the registry at depth ≥ 2 within the scope, passed
    /// via `deep_endpoint_paths` (absolute representation).
    #[test]
    fn depth_endpoint_resolves_against_deep_registry_paths() {
        let diff = json!({
            "add_edges": [{"from": "./unit/dispatch", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let deep: Vec<String> = vec!["/unit/dispatch".into()];
        let result = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/",
            &deep,
        );
        assert!(result.is_ok(), "depth endpoint must validate: {result:?}");
    }

    /// R12 Form A (validate level): a depth endpoint matches a DIFF-NEW node
    /// contributed by a subtree `add_nodes` in the SAME mutation
    /// (`subtree_node_endpoints`, absolute representation — Befund-6 semantics
    /// hold at depth: the post_state includes diff-new nodes).
    #[test]
    fn depth_endpoint_resolves_against_subtree_nodes_in_same_diff() {
        let diff = json!({
            "add_edges": [{"from": "./unit/dispatch", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let subtree_nodes: Vec<String> = vec!["/unit".into(), "/unit/dispatch".into()];
        let result = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &subtree_nodes,
            &[],
            "/",
            &[],
        );
        assert!(
            result.is_ok(),
            "depth endpoint into same-diff subtree must validate: {result:?}"
        );
    }

    /// R12 negative: a depth path to a NON-existent node rejects as
    /// `edge_schema` (post_state membership miss — not scope_out_of_bounds,
    /// the path IS contained).
    #[test]
    fn depth_endpoint_unknown_rejects_edge_schema() {
        let diff = json!({
            "add_edges": [{"from": "./unit/nonexistent", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let err = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/",
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// R12 scope-relativity: the depth endpoint resolves against the MUTATION
    /// scope (not root). `./unit/dispatch` at scope `/main` is
    /// `/main/unit/dispatch`; the same node under a FOREIGN scope must not
    /// satisfy the membership test.
    #[test]
    fn depth_endpoint_resolves_relative_to_mutation_scope() {
        let diff = json!({
            "add_edges": [{"from": "./unit/dispatch", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let deep_in_scope: Vec<String> = vec!["/main/unit/dispatch".into()];
        let result = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/main",
            &deep_in_scope,
        );
        assert!(result.is_ok(), "in-scope depth node must match: {result:?}");

        let deep_foreign: Vec<String> = vec!["/other/unit/dispatch".into()];
        let err = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/main",
            &deep_foreign,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }
}
