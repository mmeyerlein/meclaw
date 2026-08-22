//! Post-state header-view builder for the 14-B header-contract locality check
//! (Pre-Integration-Hardening Slice 1, Task 1.3).
//!
//! Builds the hypothetical POST-state `(node_views, edge_views)` pair that
//! [`super::validate::validate_header_contract_locality`] consumes, from the
//! LIVE colony state (`node_contracts` + `EdgeTable`) plus a substituted
//! mutation diff. PURE except for template-config reads (sync
//! `std::fs::read_to_string` on template directories — the established
//! `template_to_cell_type` pattern in `handle_mutation`).
//!
//! # Verified apply-arm order (the arms are the contract)
//!
//! Mirrors `colony::handle_mutation`, verified against `colony.rs` 2026-08-19.
//!
//! There is no order-free formulation of this projection. `remove_edges` is a
//! PATTERN, not a set subtraction: what it takes out depends on what the table
//! holds when it runs. So "the post-state" is not definable without saying at
//! which point the pattern is read — and the only authority for that is the
//! apply arm. Hence a sequence mirror, step for step, and hence the rule that
//! a change to the arms is a change here.
//!
//! 1. **Step 8** — `remove_nodes`: COLLECT resolved paths only (no edge
//!    mutation yet).
//! 2. **Step 9** — `add_nodes` single-cell staging/spawn (node
//!    registration).
//! 3. **Step 9b** — `swap_nodes`: edge swing over the table as it is at
//!    that point (live edges only — subtree/`add_edges` inserts come later);
//!    subtree-internal edges are not swung and resulting self-loops are
//!    dropped (`plan_edge_swing` semantics).
//! 4. **Step 9c** — subtree registration + internal-edge insert (with
//!    `contains_equal` dedup).
//! 5. **Step 10, first block** — `remove_nodes` DISCONNECT: ALL edges
//!    incident to each removed path are removed. NOTE: this runs BEFORE
//!    `add_edges`/`remove_edges` ("remove-before-add ordering" — a same-diff
//!    `add_edges` edge to a removed-and-rewired node survives).
//! 6. **Step 10** — `remove_edges`: filter with
//!    [`super::validate::remove_edges_pattern_hits`] against the table at that
//!    point — live edges plus the swing inserts and the subtree internal
//!    edges, and NOT this diff's `add_edges`.
//! 7. **Step 10** — `add_edges` insert (with dedup).
//!
//! Steps 6 and 7 are in that order because of GH #158: replacing an edge —
//! drop the old one, lay a new one with one more key promoted — belongs in ONE
//! mutation so the lane is never missing in between, and a `{from, to}` pattern
//! matches every edge between the pair. Read the other way round, the removal
//! takes away the replacement the same diff just laid. The apply arm was put
//! right in #158; this mirror kept the old order until #257, where it refused
//! such a replacement for a key the removed lane used to promote — a false
//! refusal, naming an edge that was not the problem.
//!
//! # Participation rule
//!
//! Only nodes with ≥1 incident POST-state edge carry obligations. This is an
//! intentional asymmetry to the stricter bootstrap check: it keeps
//! `remove_nodes` disconnects and incremental builder flows legal (a
//! disconnected node's view stays in the map until the final filter drops it).

use super::MutationError;
use super::validate::{
    EdgeMatchView, HeaderEdgeView, HeaderNodeView, header_view_from_contract,
    remove_edges_pattern_hits,
};
use crate::config::ModifierSpec;
use meclaw_core::serde_json;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Internal post-state edge representation. Carries the SOURCE-level data
/// (condition source string + `ModifierSpec`) needed for both the
/// [`HeaderEdgeView`] projection and the `remove_edges` match predicate
/// (`EdgeMatchView` equivalence with a live `Edge`).
#[derive(Debug, Clone)]
struct PostStateEdge {
    /// Absolute `from` path string.
    from: String,
    /// Absolute `to` path string.
    to: String,
    /// CEL condition source (`edge.condition.source` for live edges).
    condition_source: Option<String>,
    /// Modifier spec (`edge.modifier.source` for live edges).
    modifier: Option<ModifierSpec>,
    /// serde-JSON of `modifier`, cached at construction — the dedup identity
    /// component and `EdgeMatchView::modifier_source`. Stays valid across the
    /// swap arm (which only rewrites endpoints, never the modifier).
    modifier_json: Option<serde_json::Value>,
}

impl PostStateEdge {
    /// Construct with the modifier-JSON cache computed once (avoids
    /// re-serializing the spec on every dedup comparison).
    fn new(
        from: String,
        to: String,
        condition_source: Option<String>,
        modifier: Option<ModifierSpec>,
    ) -> Self {
        let modifier_json = modifier_json(modifier.as_ref());
        Self {
            from,
            to,
            condition_source,
            modifier,
            modifier_json,
        }
    }

    /// Build the F6 match-view for [`remove_edges_pattern_hits`] — identical
    /// to `EdgeMatchView::from(&Edge)` for live-derived entries
    /// (`modifier_source` = serde-JSON of the `ModifierSpec` source).
    fn match_view(&self) -> EdgeMatchView {
        EdgeMatchView {
            from: self.from.clone(),
            to: self.to.clone(),
            condition_source: self.condition_source.clone(),
            modifier_source: self.modifier_json.clone(),
        }
    }
}

/// serde-JSON value of a `ModifierSpec` — the stored representation both
/// `EdgeTable::contains_equal` and `EdgeMatchView` compare against.
fn modifier_json(m: Option<&ModifierSpec>) -> Option<serde_json::Value> {
    m.and_then(|spec| serde_json::to_value(spec).ok())
}

/// Append `edge` unless a content-equal edge is already present — mirrors the
/// `EdgeTable::contains_equal` dedup the apply arms run before every insert
/// (edge identity = from + to + condition source + modifier source, spec
/// Z.265).
fn push_dedup(list: &mut Vec<PostStateEdge>, edge: PostStateEdge) {
    let duplicate = list.iter().any(|e| {
        e.from == edge.from
            && e.to == edge.to
            && e.condition_source == edge.condition_source
            && e.modifier_json == edge.modifier_json
    });
    if !duplicate {
        list.push(edge);
    }
}

/// Project an optional [`ModifierSpec`] into the [`HeaderEdgeView`] key-sets
/// the 14-B locality check consumes (`set_context.keys()`, `delete_context`,
/// `set_hop.keys()`, `delete_hop` — same projection as the bootstrap walk).
/// `None` yields empty key-sets (identity-header edge).
pub fn edge_view_from_modifier_spec(
    from: &str,
    to: &str,
    m: Option<&ModifierSpec>,
) -> HeaderEdgeView {
    let mut view = HeaderEdgeView {
        from: from.to_string(),
        to: to.to_string(),
        ..Default::default()
    };
    if let Some(spec) = m {
        view.set_context = spec.set_context.keys().cloned().collect();
        view.delete_context = spec.delete_context.iter().cloned().collect();
        view.set_hop = spec.set_hop.keys().cloned().collect();
        view.delete_hop = spec.delete_hop.iter().cloned().collect();
    }
    view
}

/// Build the hypothetical post-state header views for the 14-B locality
/// check from the LIVE colony state (`node_contracts` + edge table) plus the
/// substituted mutation diff. Mirrors the apply-arm order of
/// `handle_mutation` (see the module doc for the VERIFIED order — the arms
/// are the contract). Participation rule: only nodes with ≥1 incident
/// post-state edge carry obligations (intentional asymmetry to the stricter
/// bootstrap check: keeps `remove_nodes` disconnects and incremental builder
/// flows legal). Returns the `(node_views, edge_views, hive_paths)` triple
/// for [`super::validate::validate_header_contract_locality`] — `hive_paths`
/// is the POST-state hive set (live `hive_scopes` ∪ hives added by this
/// diff's subtree templates), which the locality check needs to treat
/// hive-`from` edges as transit pass-throughs (F1 fix). Hive-ness is
/// monotone under the apply arms (`remove_nodes` disconnects but never
/// un-marks a scope), so live ∪ added is exact.
///
/// # Errors
///
/// - [`MutationError::TemplateMissing`] if an `add_nodes` template reference
///   does not resolve (already validated upstream — defensive).
/// - [`MutationError::Schema`] if a template `config.json` cannot be read or
///   parsed, a subtree contract block or internal-edge modifier does not
///   deserialize, or a subtree edge escapes its root.
#[allow(clippy::type_complexity)]
pub fn build_post_state_header_views(
    node_contracts: &HashMap<meclaw_core::Path, crate::NodeContract>,
    edges: &crate::edge_table::EdgeTable,
    diff_subst: &meclaw_core::JsonValue,
    scope: &str,
    templates: &crate::templates::TemplatesRegistry,
    hive_scopes: &crate::hive_scope::HiveScopeTable,
) -> Result<
    (
        BTreeMap<String, HeaderNodeView>,
        Vec<HeaderEdgeView>,
        std::collections::BTreeSet<String>,
    ),
    MutationError,
> {
    // ── Live extraction ──────────────────────────────────────────────────────
    let mut post_hives: std::collections::BTreeSet<String> = hive_scopes
        .paths()
        .map(|p| p.as_str().to_string())
        .collect();
    let mut node_views: BTreeMap<String, HeaderNodeView> = node_contracts
        .iter()
        .map(|(p, nc)| (p.as_str().to_string(), nc.header_view.clone()))
        .collect();
    let mut post_edges: Vec<PostStateEdge> = edges
        .iter()
        .map(|e| {
            PostStateEdge::new(
                e.from.as_str().to_string(),
                e.to.as_str().to_string(),
                e.condition.as_ref().map(|c| c.source.clone()),
                e.modifier.as_ref().map(|cm| cm.source.clone()),
            )
        })
        .collect();

    // ── Step 8 mirror: remove_nodes — collect resolved paths only ───────────
    let mut removed_node_paths: Vec<String> = Vec::new();
    if let Some(rems) = diff_subst.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            let Some(name) = r
                .get("match")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            else {
                continue; // schema-validate (upstream) reports the missing name.
            };
            removed_node_paths.push(super::resolve_scoped_path(scope, name).as_str().to_string());
        }
    }

    // ── Step 9 + 9c node views: add_nodes (single-cell + subtree cells) ─────
    // Subtree internal edges are collected here but inserted AFTER the swap
    // arm, mirroring the 9b-before-9c order in `handle_mutation`.
    let mut subtree_edges: Vec<PostStateEdge> = Vec::new();
    if let Some(adds) = diff_subst.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            let (Some(name), Some(tpl_ref)) = (
                n.get("name").and_then(|v| v.as_str()),
                n.get("template").and_then(|v| v.as_str()),
            ) else {
                continue; // schema-validate (upstream) reports the missing field.
            };
            let tpl = templates
                .resolve(tpl_ref)
                .map_err(|_| MutationError::TemplateMissing(tpl_ref.to_string()))?;
            // Same subtree discriminator as `handle_mutation` (parse error →
            // not a subtree → the single-cell path surfaces it as Schema).
            let parsed_subtree = super::subtree::parse_subtree(&tpl.filesystem_path, templates)
                .ok()
                .filter(|t| t.cells.len() > 1);
            if let Some(template) = parsed_subtree {
                let subtree_root = super::resolve_scoped_path(scope, name);
                let hive_set: HashSet<&str> = template.hives.iter().map(|s| s.as_str()).collect();
                // Per NON-hive cell: project its `contract` block (absence →
                // default ⇒ vacuous view). Keys use the SAME absolute
                // resolution `resolve_subtree` applies (root for `""`, else
                // rel_path joined onto the subtree root).
                for cell in &template.cells {
                    let abs = if cell.rel_path.is_empty() {
                        subtree_root.clone()
                    } else {
                        super::resolve_scoped_path(subtree_root.as_str(), &cell.rel_path)
                    };
                    if hive_set.contains(cell.rel_path.as_str()) {
                        // Hive marker: no node view, but it joins the
                        // post-state hive set (transit participation, F1).
                        post_hives.insert(abs.as_str().to_string());
                        continue;
                    }
                    let block: crate::config::ContractBlock = match cell.config.get("contract") {
                        None => Default::default(),
                        Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                            MutationError::Schema(format!(
                                "subtree cell {} contract: {e}",
                                abs.as_str()
                            ))
                        })?,
                    };
                    // Resume semantics (overview Z.170-180): a cell node at an
                    // EXISTING path is a Reconnect/Resume — it keeps its
                    // on-disk config + contract, so the LIVE view wins over
                    // the template projection.
                    node_views
                        .entry(abs.as_str().to_string())
                        .or_insert_with(|| header_view_from_contract(&block));
                }
                // Internal edges via the SHARED resolver (one resolution
                // truth) — full form with condition + modifier JSON.
                let resolved =
                    super::subtree::resolve_subtree(&tpl.filesystem_path, scope, name, templates)?;
                for re in resolved.internal_edges_resolved {
                    let spec: Option<ModifierSpec> = match re.modifier {
                        None => None,
                        Some(m) => Some(serde_json::from_value(m).map_err(|e| {
                            MutationError::Schema(format!(
                                "subtree edge {}->{} modifier: {e}",
                                re.from.as_str(),
                                re.to.as_str()
                            ))
                        })?),
                    };
                    subtree_edges.push(PostStateEdge::new(
                        re.from.as_str().to_string(),
                        re.to.as_str().to_string(),
                        re.condition,
                        spec,
                    ));
                }
            } else {
                // Single-cell: template config.json → ParsedConfig →
                // header_view_from_contract (template existence was already
                // validated upstream; parse failure is a Schema error).
                let cfg_path = tpl.filesystem_path.join("config.json");
                let raw = std::fs::read_to_string(&cfg_path).map_err(|e| {
                    MutationError::Schema(format!("read {}: {e}", cfg_path.display()))
                })?;
                let cfg: crate::config::ParsedConfig = serde_json::from_str(&raw).map_err(|e| {
                    MutationError::Schema(format!("parse {}: {e}", cfg_path.display()))
                })?;
                let key = super::resolve_scoped_path(scope, name).as_str().to_string();
                // Resume semantics (overview Z.170-180): add_nodes at an
                // EXISTING path is a Reconnect/Resume — the cell keeps its
                // on-disk config + contract, so the LIVE view wins over the
                // template projection.
                node_views
                    .entry(key)
                    .or_insert_with(|| header_view_from_contract(&cfg.contract));
            }
        }
    }

    // ── Step 9b mirror: swap_nodes — edge swing, self-loops dropped ─────────
    // Runs on the CURRENT list (live edges only at this point — subtree and
    // add_edges inserts come later), mirroring `plan_edge_swing`'s input.
    if let Some(swaps) = diff_subst.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            let (Some(t2_name), Some(t3_name)) = (
                s.get("match")
                    .and_then(|m| m.get("name"))
                    .and_then(|v| v.as_str()),
                s.get("with")
                    .and_then(|w| w.get("name"))
                    .and_then(|v| v.as_str()),
            ) else {
                continue; // schema-validate (upstream) reports the missing name.
            };
            let t2 = super::resolve_scoped_path(scope, t2_name)
                .as_str()
                .to_string();
            let t3 = super::resolve_scoped_path(scope, t3_name)
                .as_str()
                .to_string();
            post_edges = post_edges
                .into_iter()
                .filter_map(|mut e| {
                    let touches_t2 = e.from == t2 || e.to == t2;
                    // GH #256: a subtree-internal edge is not swung at all —
                    // `t2 → t2/child` and `t2/child → t2` wire the replaced
                    // unit's own inside and stay with it. Same rule as
                    // `plan_edge_swing`, so the projection keeps mirroring the
                    // arm.
                    if touches_t2 {
                        let other = if e.from == t2 { &e.to } else { &e.from };
                        if super::swap::is_inside_subtree(other, &t2) {
                            return Some(e);
                        }
                    }
                    if e.from == t2 {
                        e.from = t3.clone();
                    }
                    if e.to == t2 {
                        e.to = t3.clone();
                    }
                    // Self-loop drop (`plan_edge_swing` semantics): a swung
                    // edge whose endpoints both became t3 is removed without a
                    // replacement. Untouched edges pass through unchanged.
                    if touches_t2 && e.from == e.to {
                        None
                    } else {
                        Some(e)
                    }
                })
                .collect();
        }
    }

    // ── Step 9b' mirror: move_nodes — the node and its edges change address ──
    //
    // The header-contract check is about a node's SURROUNDINGS: which keys reach
    // it along the edges that lead to it. A move changes exactly that, and it is
    // the one operation that changes it without changing the graph's shape — so
    // a post-state built without this mirror would judge the moved cell where it
    // no longer is, and find nothing where it now is. The direction that matters
    // is the second: moving a cell out of the hive whose ingress promoted a key
    // it requires is precisely the mistake a locality check should catch.
    //
    // The node view travels under the new key; the edges are re-pointed the same
    // way the swap arm above re-points them (`plan_edge_swing` semantics),
    // except that a move can produce no self-loop — the target was validated
    // free, so no edge can already name it.
    if let Some(ms) = diff_subst.get("move_nodes").and_then(|v| v.as_array()) {
        for m in ms {
            let (Some(from_name), Some(to_name)) = (
                m.get("match")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str()),
                m.get("to").and_then(|v| v.as_str()),
            ) else {
                continue; // schema-validate (upstream) reports the missing field.
            };
            let from = super::resolve_scoped_path(scope, from_name)
                .as_str()
                .to_string();
            let to = super::resolve_scoped_path(scope, to_name)
                .as_str()
                .to_string();
            if let Some(view) = node_views.remove(&from) {
                node_views.insert(to.clone(), view);
            }
            for e in post_edges.iter_mut() {
                if e.from == from {
                    e.from = to.clone();
                }
                if e.to == from {
                    e.to = to.clone();
                }
            }
        }
    }

    // ── Step 9c mirror: subtree internal edges (dedup) ──────────────────────
    for e in subtree_edges {
        push_dedup(&mut post_edges, e);
    }

    // ── Step 10 mirror, first block: remove_nodes DISCONNECT ────────────────
    // ALL incident edges removed. Runs BEFORE add_edges (remove-vor-add); the
    // node view stays in the map — the participation filter drops it if no
    // later edge rewires the node.
    for p in &removed_node_paths {
        post_edges.retain(|e| e.from != *p && e.to != *p);
    }

    // ── Step 10 mirror: remove_edges (exact apply predicate) ────────────────
    // GH #158/#257 — remove-BEFORE-add. Filters the CURRENT list, which at this
    // point holds the live edges plus the swap swing and the subtree internal
    // edges, and NOT this diff's `add_edges`. A `{from, to}` pattern therefore
    // takes the edge that was there before the diff and spares the replacement
    // laid in the same breath — which is what the apply arm does.
    if let Some(rems) = diff_subst.get("remove_edges").and_then(|v| v.as_array()) {
        for r in rems {
            let (Some(from_name), Some(to_name)) = (
                r.get("match")
                    .and_then(|m| m.get("from"))
                    .and_then(|v| v.as_str()),
                r.get("match")
                    .and_then(|m| m.get("to"))
                    .and_then(|v| v.as_str()),
            ) else {
                continue; // defensive skip, mirroring the apply arm.
            };
            let pat_condition = r
                .get("match")
                .and_then(|m| m.get("condition"))
                .and_then(|v| v.as_str());
            let pat_modifier = r.get("match").and_then(|m| m.get("modifier"));
            let from_path = super::resolve_scoped_path(scope, from_name);
            let to_path = super::resolve_scoped_path(scope, to_name);
            post_edges.retain(|e| {
                !remove_edges_pattern_hits(
                    &e.match_view(),
                    from_path.as_str(),
                    to_path.as_str(),
                    pat_condition,
                    pat_modifier,
                )
            });
        }
    }

    // ── Step 10 mirror: add_edges (dedup), AFTER remove_edges ───────────────
    if let Some(adds) = diff_subst.get("add_edges").and_then(|v| v.as_array()) {
        for e in adds {
            let (Some(from_name), Some(to_name)) = (
                e.get("from").and_then(|v| v.as_str()),
                e.get("to").and_then(|v| v.as_str()),
            ) else {
                continue; // schema-validate (upstream) reports the missing field.
            };
            let from = super::resolve_scoped_path(scope, from_name)
                .as_str()
                .to_string();
            let to = super::resolve_scoped_path(scope, to_name)
                .as_str()
                .to_string();
            let condition_source = e
                .get("condition")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let modifier: Option<ModifierSpec> = match e.get("modifier") {
                None => None,
                Some(v) => Some(serde_json::from_value(v.clone()).map_err(|err| {
                    MutationError::Schema(format!("add_edges {from}->{to} modifier: {err}"))
                })?),
            };
            push_dedup(
                &mut post_edges,
                PostStateEdge::new(from, to, condition_source, modifier),
            );
        }
    }

    // ── Projection + participation filter (LAST) ────────────────────────────
    let edge_views: Vec<HeaderEdgeView> = post_edges
        .iter()
        .map(|e| edge_view_from_modifier_spec(&e.from, &e.to, e.modifier.as_ref()))
        .collect();
    node_views.retain(|name, _| edge_views.iter().any(|e| e.from == *name || e.to == *name));
    Ok((node_views, edge_views, post_hives))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeContract;
    use crate::edge_table::{Edge, EdgeTable};
    use crate::mutation::validate::validate_header_contract_locality;
    use crate::templates::{TemplateEntry, TemplatesRegistry};
    use meclaw_core::serde_json::json;
    use meclaw_core::{Path, Uuid};
    use std::collections::HashMap;

    fn node_view(emits_hop: &[&str], req_ctx: &[&str], req_hop: &[&str]) -> HeaderNodeView {
        HeaderNodeView {
            emits_hop: emits_hop.iter().map(|s| s.to_string()).collect(),
            required_context: req_ctx.iter().map(|s| s.to_string()).collect(),
            required_hop: req_hop.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn contracts(entries: &[(&str, HeaderNodeView)]) -> HashMap<Path, NodeContract> {
        entries
            .iter()
            .map(|(p, v)| {
                (
                    Path::new(p),
                    NodeContract {
                        header_view: v.clone(),
                        emits: None,
                        validate_emits: false,
                    },
                )
            })
            .collect()
    }

    fn spec(
        set_ctx: &[&str],
        del_ctx: &[&str],
        set_hop: &[&str],
        del_hop: &[&str],
    ) -> ModifierSpec {
        ModifierSpec {
            set_context: set_ctx
                .iter()
                .map(|k| (k.to_string(), "'v'".to_string()))
                .collect(),
            delete_context: del_ctx.iter().map(|s| s.to_string()).collect(),
            set_hop: set_hop
                .iter()
                .map(|k| (k.to_string(), "'v'".to_string()))
                .collect(),
            delete_hop: del_hop.iter().map(|s| s.to_string()).collect(),
            restore_ttl: false,
        }
    }

    fn edge(from: &str, to: &str, condition: Option<&str>, modifier: Option<ModifierSpec>) -> Edge {
        Edge {
            id: Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: condition.map(|c| crate::cel_eval::parse_condition(c).unwrap()),
            modifier: modifier.map(|m| crate::cel_eval::parse_modifier(&m).unwrap()),
        }
    }

    fn table(list: Vec<Edge>) -> EdgeTable {
        let mut t = EdgeTable::new();
        for e in list {
            t.insert(e);
        }
        t
    }

    fn keys(s: &std::collections::BTreeSet<String>) -> Vec<&str> {
        s.iter().map(|x| x.as_str()).collect()
    }

    /// Empty live hive table — most tests run hive-free graphs.
    fn no_hives() -> crate::hive_scope::HiveScopeTable {
        crate::hive_scope::HiveScopeTable::new()
    }

    // ── edge_view_from_modifier_spec ─────────────────────────────────────────

    #[test]
    fn edge_view_from_modifier_spec_projects_key_sets() {
        let m = spec(&["sc"], &["dc"], &["sh"], &["dh"]);
        let v = edge_view_from_modifier_spec("/a", "/b", Some(&m));
        assert_eq!(v.from, "/a");
        assert_eq!(v.to, "/b");
        assert_eq!(keys(&v.set_context), vec!["sc"]);
        assert_eq!(keys(&v.delete_context), vec!["dc"]);
        assert_eq!(keys(&v.set_hop), vec!["sh"]);
        assert_eq!(keys(&v.delete_hop), vec!["dh"]);
    }

    #[test]
    fn edge_view_from_modifier_spec_without_modifier_is_empty() {
        let v = edge_view_from_modifier_spec("/a", "/b", None);
        assert!(v.set_context.is_empty());
        assert!(v.delete_context.is_empty());
        assert!(v.set_hop.is_empty());
        assert!(v.delete_hop.is_empty());
    }

    // ── push_dedup ───────────────────────────────────────────────────────────

    #[test]
    fn push_dedup_suppresses_content_equal_edge() {
        // Dedup identity = from + to + condition source + modifier JSON
        // (spec Z.265, `EdgeTable::contains_equal` mirror).
        let mk = |m: Option<ModifierSpec>| {
            PostStateEdge::new("/a".into(), "/b".into(), Some("true".into()), m)
        };
        let mut list: Vec<PostStateEdge> = Vec::new();
        push_dedup(&mut list, mk(Some(spec(&["k"], &[], &[], &[]))));
        push_dedup(&mut list, mk(Some(spec(&["k"], &[], &[], &[]))));
        assert_eq!(list.len(), 1, "content-equal edge must be suppressed");
        push_dedup(&mut list, mk(Some(spec(&["other"], &[], &[], &[]))));
        assert_eq!(list.len(), 2, "content-differing modifier is a new edge");
    }

    // ── Live extraction ──────────────────────────────────────────────────────

    #[test]
    fn live_views_project_edge_modifier_keys() {
        let nc = contracts(&[
            ("/a", node_view(&["h"], &[], &[])),
            ("/b", node_view(&[], &[], &["h"])),
        ]);
        let edges = table(vec![edge(
            "/a",
            "/b",
            None,
            Some(spec(&["sc"], &["dc"], &["sh"], &["dh"])),
        )]);
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &json!({}),
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 1);
        let v = &edge_views[0];
        assert_eq!(v.from, "/a");
        assert_eq!(v.to, "/b");
        assert_eq!(keys(&v.set_context), vec!["sc"]);
        assert_eq!(keys(&v.delete_context), vec!["dc"]);
        assert_eq!(keys(&v.set_hop), vec!["sh"]);
        assert_eq!(keys(&v.delete_hop), vec!["dh"]);
        assert_eq!(nodes.len(), 2);
        assert_eq!(keys(&nodes["/a"].emits_hop), vec!["h"]);
        assert_eq!(keys(&nodes["/b"].required_hop), vec!["h"]);
    }

    // ── add_edges ────────────────────────────────────────────────────────────

    #[test]
    fn add_edges_with_set_context_appears_in_views() {
        let nc = contracts(&[
            ("/a", node_view(&[], &[], &[])),
            ("/b", node_view(&[], &["k"], &[])),
        ]);
        let diff = json!({
            "add_edges": [
                {"from": "a", "to": "b", "modifier": {"set_context": {"k": "'v'"}}}
            ]
        });
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &table(vec![]),
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 1);
        assert_eq!(edge_views[0].from, "/a");
        assert_eq!(edge_views[0].to, "/b");
        assert_eq!(keys(&edge_views[0].set_context), vec!["k"]);
        assert_eq!(nodes.len(), 2, "both endpoints participate");
    }

    // ── add_nodes (single-cell) ──────────────────────────────────────────────

    #[test]
    fn add_nodes_single_cell_contributes_node_view_from_template_config() {
        let td = tempfile::TempDir::new().unwrap();
        let tpl = td.path().join("templates/echo");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"echo"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},
                "contract":{"emits":{"hop":{"h":{"type":"string"}}},
                            "consumes":{"context":{"k":{"type":"string","required":true}}}}}"#,
        )
        .unwrap();
        let templates = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: "echo".into(),
            version: None,
            filesystem_path: tpl,
        }]);
        let nc = contracts(&[("/src", node_view(&[], &[], &[]))]);
        let diff = json!({
            "add_nodes": [{"name": "w", "template": "echo"}],
            "add_edges": [{"from": "src", "to": "w"}]
        });
        let (nodes, edge_views, _) =
            build_post_state_header_views(&nc, &table(vec![]), &diff, "/", &templates, &no_hives())
                .unwrap();
        assert_eq!(edge_views.len(), 1);
        assert_eq!(nodes.len(), 2);
        assert_eq!(keys(&nodes["/w"].emits_hop), vec!["h"]);
        assert_eq!(keys(&nodes["/w"].required_context), vec!["k"]);
    }

    #[test]
    fn add_nodes_at_existing_path_keeps_live_view_resume_semantics() {
        // add_nodes at an EXISTING path is a Resume/Reconnect (overview
        // Z.170-180): the cell keeps its on-disk config + contract — it is
        // NOT re-instantiated from the template. The LIVE view must win
        // over the template projection.
        let td = tempfile::TempDir::new().unwrap();
        let tpl = td.path().join("templates/echo");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"echo"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{},
                "contract":{"consumes":{"hop":{"tpl_key":{"type":"string","required":true}}}}}"#,
        )
        .unwrap();
        let templates = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: "echo".into(),
            version: None,
            filesystem_path: tpl,
        }]);
        let nc = contracts(&[
            ("/src", node_view(&[], &[], &[])),
            ("/w", node_view(&[], &[], &["live_key"])),
        ]);
        let edges = table(vec![edge("/src", "/w", None, None)]);
        let diff = json!({"add_nodes": [{"name": "w", "template": "echo"}]});
        let (nodes, edge_views, _) =
            build_post_state_header_views(&nc, &edges, &diff, "/", &templates, &no_hives())
                .unwrap();
        assert_eq!(edge_views.len(), 1, "live edge keeps /w participating");
        assert_eq!(
            keys(&nodes["/w"].required_hop),
            vec!["live_key"],
            "resume keeps the live contract view, not the template projection"
        );
    }

    // ── remove_edges ─────────────────────────────────────────────────────────

    #[test]
    fn remove_edges_drops_matched_edge_from_views() {
        let nc = contracts(&[
            ("/a", node_view(&[], &[], &[])),
            ("/b", node_view(&[], &[], &[])),
        ]);
        let edges = table(vec![edge("/a", "/b", None, None)]);
        let diff = json!({"remove_edges": [{"match": {"from": "a", "to": "b"}}]});
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert!(edge_views.is_empty(), "matched edge must be dropped");
        assert!(nodes.is_empty(), "edge-less nodes drop out (participation)");
    }

    /// GH #158/#257: the apply arm filters the live EdgeTable BEFORE
    /// `add_edges` runs, so a condition-less pattern takes the LIVE edge and
    /// spares the replacement the same diff lays. Mirror that exactly — read
    /// the other way round, the mirror deletes its own new edge and the
    /// locality check refuses a replacement that is fine.
    #[test]
    fn remove_edges_spares_the_edge_the_same_diff_adds() {
        let nc = contracts(&[
            ("/a", node_view(&[], &[], &[])),
            ("/b", node_view(&[], &[], &[])),
        ]);
        let edges = table(vec![edge("/a", "/b", None, None)]);
        let diff = json!({
            "add_edges": [{"from": "a", "to": "b", "condition": "true"}],
            "remove_edges": [{"match": {"from": "a", "to": "b"}}]
        });
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(
            edge_views.len(),
            1,
            "the replacement survives, the live edge does not: {edge_views:?}"
        );
        assert_eq!(edge_views[0].from, "/a");
        assert_eq!(edge_views[0].to, "/b");
        assert_eq!(
            nodes.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["/a", "/b"],
            "both endpoints keep taking part through the new edge"
        );
    }

    /// The counter-direction of the same order: a diff that removes one lane
    /// and opens a DIFFERENT one must still do both. Remove-before-add is not
    /// "remove nothing".
    #[test]
    fn remove_edges_still_takes_the_edge_that_was_there_before() {
        let nc = contracts(&[
            ("/a", node_view(&[], &[], &[])),
            ("/b", node_view(&[], &[], &[])),
        ]);
        let edges = table(vec![edge("/a", "/b", None, None)]);
        let diff = json!({
            "add_edges": [{"from": "b", "to": "a"}],
            "remove_edges": [{"match": {"from": "a", "to": "b"}}]
        });
        let (_nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 1, "one lane out, one lane in");
        assert_eq!(edge_views[0].from, "/b");
        assert_eq!(edge_views[0].to, "/a");
    }

    // ── remove_nodes ─────────────────────────────────────────────────────────

    #[test]
    fn remove_nodes_drops_incident_edges_and_node_leaves_views() {
        let nc = contracts(&[
            ("/a", node_view(&[], &[], &[])),
            ("/b", node_view(&[], &[], &[])),
            ("/c", node_view(&[], &[], &[])),
            ("/d", node_view(&[], &[], &[])),
        ]);
        let edges = table(vec![
            edge("/a", "/b", None, None),
            edge("/c", "/d", None, None),
        ]);
        let diff = json!({"remove_nodes": [{"match": {"name": "b"}}]});
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 1, "only the unrelated edge survives");
        assert_eq!(edge_views[0].from, "/c");
        assert!(!nodes.contains_key("/b"), "disconnected node drops out");
        assert!(!nodes.contains_key("/a"), "now edge-less node drops out");
        assert!(nodes.contains_key("/c") && nodes.contains_key("/d"));
    }

    #[test]
    fn remove_nodes_disconnect_runs_before_add_edges() {
        // Verified arm order: the remove_nodes DISCONNECT (step 10, first
        // block) runs BEFORE add_edges — a same-diff edge rewiring the removed
        // node survives ("remove-before-add": remove+add at the same path).
        let nc = contracts(&[
            ("/a", node_view(&[], &[], &[])),
            ("/b", node_view(&[], &[], &[])),
            ("/c", node_view(&[], &[], &[])),
        ]);
        let edges = table(vec![edge("/a", "/b", None, None)]);
        let diff = json!({
            "remove_nodes": [{"match": {"name": "b"}}],
            "add_edges": [{"from": "b", "to": "c"}]
        });
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 1);
        assert_eq!(edge_views[0].from, "/b");
        assert_eq!(edge_views[0].to, "/c");
        assert!(nodes.contains_key("/b"), "rewired node participates again");
        assert!(!nodes.contains_key("/a"));
    }

    // ── swap_nodes ───────────────────────────────────────────────────────────

    #[test]
    fn swap_repoints_edges_and_drops_self_loops() {
        let nc = contracts(&[
            ("/x", node_view(&[], &[], &[])),
            ("/y", node_view(&[], &[], &[])),
            ("/t2", node_view(&[], &[], &[])),
            ("/t3", node_view(&[], &[], &[])),
        ]);
        let edges = table(vec![
            edge("/x", "/t2", None, Some(spec(&[], &[], &["sh"], &[]))),
            edge("/t2", "/y", None, None),
            edge("/t2", "/t3", None, None), // becomes t3→t3 → dropped
        ]);
        let diff = json!({"swap_nodes": [{"match": {"name": "t2"}, "with": {"name": "t3"}}]});
        let (nodes, edge_views, _) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 2, "self-loop must be dropped");
        let xt3 = edge_views
            .iter()
            .find(|e| e.from == "/x" && e.to == "/t3")
            .expect("x→t3 swung edge");
        assert_eq!(keys(&xt3.set_hop), vec!["sh"], "modifier carried verbatim");
        assert!(
            edge_views.iter().any(|e| e.from == "/t3" && e.to == "/y"),
            "t2→y swings to t3→y"
        );
        assert!(!nodes.contains_key("/t2"), "t2 is edge-less → excluded");
        assert!(nodes.contains_key("/t3"));
    }

    // ── subtree add_nodes ────────────────────────────────────────────────────

    #[test]
    fn subtree_add_contributes_cells_and_internal_edges() {
        let td = tempfile::TempDir::new().unwrap();
        let tpl = td.path().join("templates/sub");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"sub"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},
                "params":{"graph":{"edges":[
                    {"from":"./inner_a","to":"./inner_b",
                     "modifier":{"set_hop":{"h":"'1'"}}}
                ]}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(tpl.join("inner_a")).unwrap();
        std::fs::write(
            tpl.join("inner_a/config.json"),
            r#"{"cell":{"type":"echo"},
                "contract":{"emits":{"hop":{"h":{"type":"string"}}}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(tpl.join("inner_b")).unwrap();
        std::fs::write(
            tpl.join("inner_b/config.json"),
            r#"{"cell":{"type":"echo"},
                "contract":{"consumes":{"hop":{"h":{"type":"string","required":true}}}}}"#,
        )
        .unwrap();
        let templates = TemplatesRegistry::from_entries(vec![TemplateEntry {
            template_id: "t1".into(),
            name: "sub".into(),
            version: None,
            filesystem_path: tpl,
        }]);
        let diff = json!({"add_nodes": [{"name": "m1", "template": "sub"}]});
        let (nodes, edge_views, hives) = build_post_state_header_views(
            &HashMap::new(),
            &table(vec![]),
            &diff,
            "/main",
            &templates,
            &no_hives(),
        )
        .unwrap();
        assert_eq!(edge_views.len(), 1);
        assert_eq!(edge_views[0].from, "/main/m1/inner_a");
        assert_eq!(edge_views[0].to, "/main/m1/inner_b");
        assert_eq!(keys(&edge_views[0].set_hop), vec!["h"]);
        assert_eq!(nodes.len(), 2, "hive root carries no node view");
        assert_eq!(keys(&nodes["/main/m1/inner_a"].emits_hop), vec!["h"]);
        assert_eq!(keys(&nodes["/main/m1/inner_b"].required_hop), vec!["h"]);
        assert!(
            hives.contains("/main/m1"),
            "subtree root hive joins the post-state hive set, got {hives:?}"
        );
        // Positive locality proof: emits.hop h satisfies required hop h.
        assert!(validate_header_contract_locality(&nodes, &edge_views, &hives).is_ok());
    }

    // ── F1: hive-transit participation (mutation-path twin) ─────────────────

    /// Live K-H1 shape: `/entry → /sub` (`set_hop.hmark`), `/sub → /sub/cellA`
    /// (hive `from` = transit), `cellA` requires `hop.hmark`. The post-state
    /// views of an UNRELATED `add_edges` diff must validate — the locality
    /// walk crosses the live hive via the returned hive set.
    #[test]
    fn live_hive_transit_credits_required_hop_in_post_state() {
        let nc = contracts(&[
            ("/entry", node_view(&[], &[], &[])),
            ("/x", node_view(&[], &[], &[])),
            ("/sub/cellA", node_view(&[], &[], &["hmark"])),
        ]);
        let edges = table(vec![
            edge(
                "/entry",
                "/sub",
                None,
                Some(spec(&[], &[], &["hmark"], &[])),
            ),
            edge("/sub", "/sub/cellA", None, None),
        ]);
        let mut live_hives = crate::hive_scope::HiveScopeTable::new();
        live_hives.register(crate::hive_scope::HiveScope {
            path: Path::new("/sub"),
        });
        let diff = json!({"add_edges": [{"from": "x", "to": "entry"}]});
        let (nodes, edge_views, hives) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &live_hives,
        )
        .unwrap();
        assert!(
            hives.contains("/sub"),
            "live hive rides into the post-state"
        );
        assert!(
            validate_header_contract_locality(&nodes, &edge_views, &hives).is_ok(),
            "transit-delivered required hop key must validate on the mutation path"
        );
    }

    /// Negative twin: an `add_edges` diff wiring a key-less source INTO the
    /// hive empties the transit intersection — the post-state views must
    /// reject (the mutation-path check must not go vacuous).
    #[test]
    fn add_edges_breaking_hive_transit_intersection_is_rejected() {
        let nc = contracts(&[
            ("/entry", node_view(&[], &[], &[])),
            ("/x", node_view(&[], &[], &[])),
            ("/sub/cellA", node_view(&[], &[], &["hmark"])),
        ]);
        let edges = table(vec![
            edge(
                "/entry",
                "/sub",
                None,
                Some(spec(&[], &[], &["hmark"], &[])),
            ),
            edge("/sub", "/sub/cellA", None, None),
        ]);
        let mut live_hives = crate::hive_scope::HiveScopeTable::new();
        live_hives.register(crate::hive_scope::HiveScope {
            path: Path::new("/sub"),
        });
        let diff = json!({"add_edges": [{"from": "x", "to": "sub"}]});
        let (nodes, edge_views, hives) = build_post_state_header_views(
            &nc,
            &edges,
            &diff,
            "/",
            &TemplatesRegistry::default(),
            &live_hives,
        )
        .unwrap();
        assert!(
            validate_header_contract_locality(&nodes, &edge_views, &hives).is_err(),
            "a key-less inbound edge must empty the transit intersection"
        );
    }

    // ── Participation rule ───────────────────────────────────────────────────

    #[test]
    fn node_without_any_edge_is_excluded() {
        // A hop-consumer with NO incident post-state edge carries no
        // obligation (intentional asymmetry to the bootstrap check): the
        // views come back empty and the locality check passes vacuously.
        let nc = contracts(&[("/lonely", node_view(&[], &[], &["needs_h"]))]);
        let (nodes, edge_views, hives) = build_post_state_header_views(
            &nc,
            &table(vec![]),
            &json!({}),
            "/",
            &TemplatesRegistry::default(),
            &no_hives(),
        )
        .unwrap();
        assert!(nodes.is_empty());
        assert!(edge_views.is_empty());
        assert!(validate_header_contract_locality(&nodes, &edge_views, &hives).is_ok());
    }
}
