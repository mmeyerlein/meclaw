//! Pure edge-swing helper for the graph-swap (Paket-2 T3).
//!
//! Given a source node `t2` and a target node `t3`, [`plan_edge_swing`]
//! computes how to "swing" all **external** edges of `t2` onto `t3`.
//!
//! *External* = edges where `e.from == t2` OR `e.to == t2` (exact path
//! match). Subtree-internal edges (`t2/child → x`) are **not** touched
//! because their paths do not equal `t2` exactly.
//!
//! The function is **pure**: no I/O, no Uuid generation, no mutation of the
//! table. It returns a [`SwingPlan`] that the apply step (T4) feeds into the
//! existing edge-insert/edge-remove buffers inside `handle_mutation`.

use crate::{
    cel_eval::{CompiledCondition, CompiledModifier},
    edge_table::EdgeTable,
};
use meclaw_core::{Path, Uuid};

// ── Public-crate types ────────────────────────────────────────────────────────

/// A single edge to be inserted as part of a swing operation.
///
/// Carries the new `(from, to)` endpoints plus the cloned CEL condition and
/// modifier from the original edge.  **No new UUID is assigned here** — T4
/// calls `Uuid::now_v7()` at apply-time so this helper stays deterministic
/// and unit-testable without time-based IDs.
///
/// The convenience fields `cond_src` and `mod_src` hold the serialised source
/// strings that `ColonyWriteOp::InsertEdge` needs, pre-computed so T4 does
/// not have to re-derive them from the compiled forms.
#[derive(Debug, Clone)]
pub(crate) struct SwungEdge {
    /// Source path for the new edge.
    pub(crate) from: Path,
    /// Target path for the new edge.
    pub(crate) to: Path,
    /// Cloned CEL condition (may be `None` for unconditional edges).
    pub(crate) condition: Option<CompiledCondition>,
    /// Cloned CEL modifier (may be `None` for identity-header edges).
    pub(crate) modifier: Option<CompiledModifier>,
    /// Pre-serialised condition source string for `InsertEdge` WriteOp.
    /// Equals `condition.as_ref().map(|c| c.source.clone())`.
    pub(crate) cond_src: Option<String>,
    /// Pre-serialised modifier JSON string for `InsertEdge` WriteOp.
    /// Equals `serde_json::to_string(&modifier.source)` when `Some`.
    pub(crate) mod_src: Option<String>,
}

/// The plan returned by [`plan_edge_swing`].
///
/// `inserts` lists every edge to be added (already with swung endpoints).
/// `remove_ids` lists the old-edge UUIDs that must be deleted.
///
/// Self-loop cases (both endpoints swing to `t3`) produce a `remove_ids`
/// entry but **no** corresponding `inserts` entry (the edge is dropped).
#[derive(Debug, Default)]
pub(crate) struct SwingPlan {
    /// Edges to insert (new endpoints, cloned condition/modifier, no UUID yet).
    pub(crate) inserts: Vec<SwungEdge>,
    /// IDs of old edges that must be removed from the edge table.
    pub(crate) remove_ids: Vec<Uuid>,
}

// ── Core function ─────────────────────────────────────────────────────────────

/// Compute the swing plan for re-dedicating all external edges of `t2` onto
/// `t3`.
///
/// # Semantics
///
/// For every edge `e` in `edges` where `e.from == t2` OR `e.to == t2`
/// (exact path match — subtree paths like `t2/child` are excluded):
///
/// 1. Replace the `t2` endpoint(s) with `t3`.
/// 2. Clone `condition` and `modifier` verbatim.
/// 3. If the resulting edge would have `from == to` (both endpoints became
///    `t3`) — i.e. the original edge directly connected `t2 ↔ t3` or was a
///    self-loop on `t2` — **drop** the insert but **still** remove the old
///    edge.
///
/// No deduplication against existing `t3` edges is performed (YAGNI).
pub(crate) fn plan_edge_swing(t2: &Path, t3: &Path, edges: &EdgeTable) -> SwingPlan {
    let mut plan = SwingPlan::default();

    // Collect all edges that involve t2 (exact match on either endpoint).
    // Use the indexed edges_from for the outgoing direction and the scan-based
    // edges_to for the incoming direction, then deduplicate by edge id so a
    // t2→t2 self-loop (which appears in both iterators) is only processed once.

    // Outgoing: from == t2
    let outgoing = edges.edges_from(t2).iter();

    // Incoming: to == t2 (linear scan; excludes outgoing-only edges already
    // handled above, but may overlap for self-loops).
    let incoming: Vec<&crate::edge_table::Edge> = edges.edges_to(t2);

    // Merge and deduplicate by id.
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut all: Vec<&crate::edge_table::Edge> = Vec::new();
    for e in outgoing.into_iter().chain(incoming) {
        if seen.insert(e.id) {
            all.push(e);
        }
    }

    for e in all {
        // Swing: replace t2 endpoint(s) with t3.
        let new_from = if &e.from == t2 {
            t3.clone()
        } else {
            e.from.clone()
        };
        let new_to = if &e.to == t2 {
            t3.clone()
        } else {
            e.to.clone()
        };

        // Always remove the old edge.
        plan.remove_ids.push(e.id);

        // Self-loop drop: both endpoints became t3 → no insert.
        if new_from == new_to {
            continue;
        }

        // Serialise source strings for the durable WriteOp (T4 needs them).
        let cond_src = e.condition.as_ref().map(|c| c.source.clone());
        let mod_src = e
            .modifier
            .as_ref()
            .and_then(|m| meclaw_core::serde_json::to_string(&m.source).ok());

        plan.inserts.push(SwungEdge {
            from: new_from,
            to: new_to,
            condition: e.condition.clone(),
            modifier: e.modifier.clone(),
            cond_src,
            mod_src,
        });
    }

    plan
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_table::{Edge, EdgeTable};
    use meclaw_core::Path;

    fn make_edge(from: &str, to: &str) -> Edge {
        Edge {
            id: Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: None,
            modifier: None,
        }
    }

    fn table_with(edges: Vec<Edge>) -> EdgeTable {
        let mut t = EdgeTable::new();
        for e in edges {
            t.insert(e);
        }
        t
    }

    // ── T1: single outgoing edge (from == t2) ──────────────────────────────

    #[test]
    fn single_outgoing_edge_swings_from_endpoint() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");
        let other = Path::new("/other");

        let e = make_edge("/t2", "/other");
        let old_id = e.id;
        let tbl = table_with(vec![e]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert_eq!(plan.remove_ids, vec![old_id]);
        assert_eq!(plan.inserts.len(), 1);
        assert_eq!(plan.inserts[0].from, t3);
        assert_eq!(plan.inserts[0].to, other);
    }

    // ── T2: single incoming edge (to == t2) ───────────────────────────────

    #[test]
    fn single_incoming_edge_swings_to_endpoint() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");
        let other = Path::new("/other");

        let e = make_edge("/other", "/t2");
        let old_id = e.id;
        let tbl = table_with(vec![e]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert_eq!(plan.remove_ids, vec![old_id]);
        assert_eq!(plan.inserts.len(), 1);
        assert_eq!(plan.inserts[0].from, other);
        assert_eq!(plan.inserts[0].to, t3);
    }

    // ── T3: condition + modifier carried verbatim ─────────────────────────

    #[test]
    fn condition_and_modifier_are_cloned_verbatim() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");

        let cond = crate::cel_eval::parse_condition("hop.tier == 'gold'").unwrap();
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop.insert("x".into(), "'v'".into());
        let modif = crate::cel_eval::parse_modifier(&spec).unwrap();

        let e = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/t2"),
            to: Path::new("/sink"),
            condition: Some(cond.clone()),
            modifier: Some(modif.clone()),
        };
        let tbl = table_with(vec![e]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert_eq!(plan.inserts.len(), 1);
        let sw = &plan.inserts[0];

        // Condition source must be byte-identical.
        assert_eq!(
            sw.condition.as_ref().map(|c| c.source.as_str()),
            Some("hop.tier == 'gold'")
        );
        // cond_src convenience field must equal the original condition source.
        assert_eq!(sw.cond_src.as_deref(), Some("hop.tier == 'gold'"));

        // Modifier set value must survive verbatim (checked via source.set, not compiled form).
        assert_eq!(
            sw.modifier
                .as_ref()
                .and_then(|m| m.source.set_hop.get("x"))
                .map(String::as_str),
            Some("'v'"),
            "modifier set value 'x' must be carried verbatim"
        );
        // mod_src must equal the serialised original spec (verbatim round-trip).
        let expected_mod_src = meclaw_core::serde_json::to_string(&spec).unwrap();
        assert_eq!(
            sw.mod_src.as_deref(),
            Some(expected_mod_src.as_str()),
            "mod_src must be the serialised original ModifierSpec"
        );
    }

    // ── T4: multiple incoming + outgoing edges ────────────────────────────

    #[test]
    fn multiple_in_and_out_edges_all_swung() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");

        let e1 = make_edge("/t2", "/a");
        let e2 = make_edge("/t2", "/b");
        let e3 = make_edge("/x", "/t2");
        let e4 = make_edge("/y", "/t2");
        let ids = vec![e1.id, e2.id, e3.id, e4.id];

        let tbl = table_with(vec![e1, e2, e3, e4]);
        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert_eq!(plan.remove_ids.len(), 4);
        assert_eq!(plan.inserts.len(), 4);

        // All removes are the originals.
        let mut sorted_removes: Vec<Uuid> = plan.remove_ids.clone();
        sorted_removes.sort();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        assert_eq!(sorted_removes, sorted_ids);

        // All inserts have t3 in the swung slot.
        for sw in &plan.inserts {
            assert!(sw.from == t3 || sw.to == t3, "expected t3 in swung edge");
        }
    }

    // ── T5: self-loop t2 → t2 ─────────────────────────────────────────────

    #[test]
    fn self_loop_on_t2_drops_insert_but_still_removes() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");

        let e = make_edge("/t2", "/t2");
        let old_id = e.id;
        let tbl = table_with(vec![e]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert_eq!(plan.remove_ids, vec![old_id]);
        assert!(plan.inserts.is_empty(), "self-loop must be dropped");
    }

    // ── T6: edge directly connecting t2 ↔ t3 ────────────────────────────

    #[test]
    fn edge_between_t2_and_t3_drops_insert_but_still_removes() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");

        // t2 → t3
        let e1 = make_edge("/t2", "/t3");
        let id1 = e1.id;
        // t3 → t2
        let e2 = make_edge("/t3", "/t2");
        let id2 = e2.id;

        let tbl = table_with(vec![e1, e2]);
        let plan = plan_edge_swing(&t2, &t3, &tbl);

        let mut removes = plan.remove_ids.clone();
        removes.sort();
        let mut expected = vec![id1, id2];
        expected.sort();
        assert_eq!(removes, expected);

        // Both would become t3→t3 after swinging → no inserts.
        assert!(plan.inserts.is_empty(), "t2↔t3 edges must be dropped");
    }

    // ── T7: subtree internal edge t2/child → x is NOT touched ────────────

    #[test]
    fn subtree_internal_edge_is_not_touched() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");

        // This edge has from = /t2/child, NOT /t2 exactly → must be ignored.
        let e = make_edge("/t2/child", "/other");
        let tbl = table_with(vec![e]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert!(plan.remove_ids.is_empty());
        assert!(plan.inserts.is_empty());
    }

    // ── T8: no edges at all → empty plan ──────────────────────────────────

    #[test]
    fn empty_table_produces_empty_plan() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");
        let tbl = EdgeTable::new();

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert!(plan.remove_ids.is_empty());
        assert!(plan.inserts.is_empty());
    }

    // ── T9: edge not involving t2 is not touched ──────────────────────────

    #[test]
    fn unrelated_edges_are_not_touched() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");

        let e = make_edge("/a", "/b");
        let tbl = table_with(vec![e]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert!(plan.remove_ids.is_empty());
        assert!(plan.inserts.is_empty());
    }

    // ── T10: bidirectional edge between t2 and third party ───────────────

    #[test]
    fn both_from_and_to_t2_on_different_edges_both_swung() {
        let t2 = Path::new("/t2");
        let t3 = Path::new("/t3");
        let c = Path::new("/c");

        let out = make_edge("/t2", "/c");
        let inc = make_edge("/c", "/t2");
        let tbl = table_with(vec![out.clone(), inc.clone()]);

        let plan = plan_edge_swing(&t2, &t3, &tbl);

        assert_eq!(plan.inserts.len(), 2);
        // Outgoing: t3 → /c
        let swung_out = plan
            .inserts
            .iter()
            .find(|sw| sw.to == c)
            .expect("swung outgoing edge");
        assert_eq!(swung_out.from, t3);
        // Incoming: /c → t3
        let swung_in = plan
            .inserts
            .iter()
            .find(|sw| sw.from == c)
            .expect("swung incoming edge");
        assert_eq!(swung_in.to, t3);
    }
}
