//! Pure connectivity- and activity-recompute logic (Phase 13.5-Lifecycle-3b).
//!
//! The activity of every graph node (cell or hive) is **fully derived from the
//! edge table** — there is no explicit activate/deactivate mutation op (spec:
//! `docs/meclaw-overview.md` § Connectivity and activity). This module holds the
//! pure functions that recompute that state; wiring into `handle_mutation` is a
//! separate step (Task 4). Every function here takes references and returns
//! values — no side effects, no `.await`.
//!
//! Rules (spec § Connectivity and activity, `docs/cell-types.md` § Connectivity
//! of the hive):
//! - **connected (cell)**: a node participates in ≥1 edge (as `from` **or**
//!   `to`) — [`is_connected`]. A cell has no descendants, so naming it is the
//!   whole of it.
//! - **connected (hive)**: at least one edge is EXTERNAL to the **unit** —
//!   [`has_external_edge`]. The unit is the hive path **together with its whole
//!   subtree**, and that is the load-bearing part (GH #265): the hive boundary
//!   MANDATES that a hive serves its own children with edges whose `from` is the
//!   hive itself (`{"from": ".", "to": "./cell"}`), so the hive path is an
//!   endpoint of its own inside. Reading such an edge as a connection let a unit
//!   with nothing left but its own inside count as connected — a swapped-out
//!   generation stayed awake, timer and all. Both external forms fall out of the
//!   one predicate: a parent-level edge naming the hive path, and a depth-port
//!   edge naming a descendant.
//! - **active** (recursive): a node is active iff it is connected **and** its
//!   parent-hive is active. The root (`/`) is always active. A disconnected hive
//!   therefore deactivates its entire subtree regardless of internal wiring.

use crate::edge_table::EdgeTable;
use crate::hive_scope::HiveScopeTable;
use meclaw_core::Path;
use std::collections::HashSet;

/// Returns true if `path` participates in at least one edge — as `from`
/// **or** as `to`.
///
/// This is the spec's connectivity predicate for a **cell**
/// (`docs/meclaw-overview.md` § Connectivity and activity): a single in- or
/// out-edge suffices, and a cell has no descendants, so naming it is the whole
/// of it.
///
/// **Not the predicate for a hive** (GH #265). A hive's connectivity is
/// [`has_external_edge`]. This function used to serve both, on the assumption
/// — written down right here, which is why the defect was invisible to a reader
/// — that internal wiring only ever uses descendant paths as endpoints. It does
/// not: the hive boundary mandates `<hive> → <hive>/<cell>`, whose `from` IS the
/// hive path. A unit with nothing but its own inside left therefore read as
/// connected.
pub fn is_connected(path: &Path, edges: &EdgeTable) -> bool {
    !edges.edges_from(path).is_empty() || !edges.edges_to(path).is_empty()
}

/// **The connectivity predicate of a hive**: returns true if at least one edge
/// is EXTERNAL to the unit rooted at `path` — exactly one endpoint lies in the
/// unit (the path itself **or** anything under it), the other outside it.
///
/// The unit is `path` **and** its subtree, not the subtree alone (GH #265).
/// That single detail decides the whole question, because the hive boundary
/// *mandates* the wiring in which a hive serves its own children — `{"from":
/// ".", "to": "./cell"}`, i.e. `<hive> → <hive>/<cell>` (`docs/cell-types.md`
/// § Die Hive-Grenze) — and the way back out, `<hive>/<cell> → <hive>`. Both
/// name the hive path. Counting the hive path as *outside* its own unit made
/// them look like boundary crossings, and a unit whose external edges had all
/// been swung away stayed connected by its own inside.
///
/// One predicate, both external forms (spec § Connectivity and activity,
/// hive sharpening):
/// - a parent-level edge naming the hive path (`/top → /h`), and
/// - a depth-port edge naming a descendant (`/anchor → /h/dispatch`, R12) —
///   which wires the unit to the world without naming the hive path at all.
///
/// Purely internal wiring (both endpoints in the unit) never counts.
///
/// Callers gate this on registered hive paths ([`compute_active`]); for the
/// root (`/`) every path is in the unit, so no edge is ever external to it —
/// the root stays governed by its always-active rule.
pub fn has_external_edge(path: &Path, edges: &EdgeTable) -> bool {
    if path.as_str() == "/" {
        return false;
    }
    edges
        .iter()
        .any(|e| is_self_or_descendant(&e.from, path) != is_self_or_descendant(&e.to, path))
}

/// Recursively computes whether the node at `path` is **active**.
///
/// Spec rule (`docs/meclaw-overview.md` § Connectivity and activity): a node is
/// active iff it is itself [`is_connected`] **and** its parent-hive is active;
/// the root (`/`) is always active. This is the path-segment parent chain walked
/// upward — no root-to-leaf traversal — so the cost is O(edges) local plus
/// O(depth) for the chain.
///
/// Activity is fully edge-derived, so the registry is not consulted in the
/// recursion: hives carry no registry entry (their activity is computed, never
/// stored). `hive_scopes` IS consulted, and for two reasons — to know which
/// ancestor paths gate a subtree, and to know **which predicate applies to
/// `path` itself**: a registered hive is connected by an edge external to its
/// unit ([`has_external_edge`]), a cell by any edge naming it
/// ([`is_connected`]). A non-hive ancestor (or the root) is treated as the
/// always-active top-level scope.
pub fn compute_active(path: &Path, edges: &EdgeTable, hive_scopes: &HiveScopeTable) -> bool {
    // GH #265 — the two predicates are alternatives, not a disjunction. A hive
    // is connected ONLY by an edge external to its unit; `is_connected` would
    // additionally count the mandated `<hive> → <hive>/<cell>` inward wiring,
    // which is the unit's own inside and connects it to nothing.
    let connected = if hive_scopes.get(path).is_some() {
        has_external_edge(path, edges)
    } else {
        is_connected(path, edges)
    };
    connected && parent_hive_active(path, edges, hive_scopes)
}

/// Returns whether the parent-hive of `path` is active.
///
/// The root (`/`) is the implicit top-level scope and is always active, so any
/// node whose parent is the root is gated only by its own connectivity. A parent
/// that is a registered hive scope gates its subtree: it is active iff it has an
/// [`has_external_edge`] **and** its own parent-hive is active — applied
/// recursively up to the root. A parent that is not a registered hive scope is
/// treated as a top-level (always-active) boundary.
fn parent_hive_active(path: &Path, edges: &EdgeTable, hive_scopes: &HiveScopeTable) -> bool {
    let parent = path.parent();
    if parent.as_str() == "/" || parent.as_str() == path.as_str() {
        // Parent is the always-active root scope, or `path` had no proper parent.
        return true;
    }
    if hive_scopes.get(&parent).is_none() {
        // The parent is not a registered hive — no hive gates this node above its
        // immediate scope, so treat the boundary as always-active.
        return true;
    }
    compute_active(&parent, edges, hive_scopes)
}

/// Computes the set of node paths whose activity must be recomputed after a
/// mutation, given the paths directly involved in the mutation (`add_edges` /
/// `remove_edges` `from`/`to` endpoints and `remove_nodes` paths).
///
/// Spec F1 (`docs/meclaw-overview.md` § Connectivity and activity): the affected
/// scope is the **local edge participation plus the parent chain** — no
/// root-to-leaf walk. Because removing the last edge of a hive deactivates its
/// **entire subtree** (internal wiring notwithstanding), each involved path also
/// contributes its subtree: every `known_paths` entry that is the path itself or
/// a descendant (`<involved>/...`). Finally each involved path contributes its
/// parent chain up to (and including) the root, so a reconnect can re-activate
/// ancestors.
///
/// `known_paths` is the universe of registered node paths (Task 4 passes the
/// registry keys) — needed because subtree members are arbitrary and cannot be
/// derived from a path string alone.
///
/// R12: `hive_paths` are the registered hive-scope paths. A depth-port edge
/// can flip the activity of a HIVE ANCESTOR of an involved endpoint (the
/// crossing connects the hive, see [`has_external_edge`]), and a hive
/// flip gates its whole subtree — so a parent-chain member that is a
/// registered hive AND whose boundary the mutation actually CROSSES (≥1
/// involved path strictly inside, ≥1 outside) contributes its subtree too.
/// Locality is preserved: an all-internal mutation pulls no hive subtree, and
/// for the root every path is "inside" (nothing crosses it), so no
/// root-to-leaf walk is introduced.
pub fn affected_scope(
    involved: &[Path],
    known_paths: &[Path],
    hive_paths: &[Path],
) -> HashSet<Path> {
    let mut scope: HashSet<Path> = HashSet::new();
    for path in involved {
        // The involved path itself.
        scope.insert(path.clone());
        // Subtree members: known paths equal to or under `path`.
        for known in known_paths {
            if is_self_or_descendant(known, path) {
                scope.insert(known.clone());
            }
        }
        // Parent chain up to the root. R12: a registered-hive parent whose
        // boundary the mutation crosses contributes its subtree (its activity
        // may flip via the crossing edge).
        let mut cur = path.clone();
        loop {
            let parent = cur.parent();
            if parent.as_str() == cur.as_str() {
                break; // reached root (`/`) or a fixed point
            }
            scope.insert(parent.clone());
            let is_hive = hive_paths.iter().any(|h| h == &parent);
            let crossed = is_hive && involved.iter().any(|p| !is_self_or_descendant(p, &parent));
            if crossed {
                for known in known_paths {
                    if is_self_or_descendant(known, &parent) {
                        scope.insert(known.clone());
                    }
                }
            }
            if parent.as_str() == "/" {
                break;
            }
            cur = parent;
        }
    }
    scope
}

/// Returns true if `candidate` is `ancestor` itself or a path strictly nested
/// under it (segment-aware: `/a/b` is under `/a`, but `/ab` is not).
///
/// `pub(crate)` so the subtree-staging containment check
/// ([`crate::mutation::subtree::stage_subtree`]) can reuse the same
/// segment-aware predicate instead of duplicating it.
pub(crate) fn is_self_or_descendant(candidate: &Path, ancestor: &Path) -> bool {
    let (c, a) = (candidate.as_str(), ancestor.as_str());
    if c == a {
        return true;
    }
    if a == "/" {
        // Everything absolute is under the root.
        return c.starts_with('/');
    }
    // `a` followed by a `/` boundary guards against `/ab` matching `/a`.
    c.starts_with(a) && c.as_bytes().get(a.len()) == Some(&b'/')
}

/// Builds the **post-state** edge view a mutation's connectivity-recompute
/// (apply step 10b) will see, computed at apply step 9 (the eager-spawn
/// loop) — BEFORE the diff's edges have been applied to the live `edges` table.
///
/// Paket-3 P3-C1 (Reviewer-Auflage A3): the C1 activity gate must derive a
/// would-be-inactive cell's activity against the edges as they WILL be after
/// the diff applies, NOT the current committed `edges` (a fresh cell would
/// always look inactive there). The view is
/// `current ∪ adds − removes`, mirroring exactly what step 10b recomputes
/// against: step 10b runs after `add_edges` (insert), `remove_edges` /
/// `remove_nodes` (remove) have mutated `edges` in place, so this pure helper
/// reconstructs the same set on a clone.
///
/// `adds` are `(from, to)` endpoint pairs (scope-resolved `add_edges`);
/// `remove_pairs` are `(from, to)` pairs (scope-resolved `remove_edges`);
/// `remove_node_paths` are node paths (scope-resolved `remove_nodes`) whose
/// EVERY edge (`from == p` OR `to == p`) is dropped — the disconnect semantics
/// of apply step 10. Only endpoints matter to [`compute_active`] (it
/// consults `edges_from`/`edges_to`), so synthesized edges carry no condition /
/// modifier and a throwaway UUID.
///
/// NOTE (minimal cut): `swap_nodes`-swing and subtree-internal edges are NOT
/// folded in here. They run in apply steps 9b/9c (after the single-cell
/// spawn loop) and cannot change the activity of a single-cell `add_nodes`
/// staged cell — `swap` produces no `staged` single cell whose activity hinges
/// on the swing, and subtree cells live in `staged_subtrees`, not `staged`.
/// The P8 case is specifically a single-cell `add_nodes` whose
/// `add_edges` / `remove_*` derive it inactive, which this view covers exactly.
pub fn post_state_edges(
    current: &EdgeTable,
    adds: &[(Path, Path)],
    remove_pairs: &[(Path, Path)],
    remove_node_paths: &[Path],
) -> EdgeTable {
    // Rebuild the view from the current edges (EdgeTable is not Clone — Edge is),
    // dropping every removed edge as we go. `remove_edges`: exact (from, to)
    // match. `remove_nodes`: every edge touching the node path (from OR to).
    // Mirrors step 10's remove-before-add ordering for the recompute view.
    let mut view = EdgeTable::new();
    for e in current.iter() {
        let removed = remove_pairs.iter().any(|(f, t)| &e.from == f && &e.to == t)
            || remove_node_paths.iter().any(|p| &e.from == p || &e.to == p);
        if !removed {
            view.insert(e.clone());
        }
    }
    // Then apply adds (endpoint-only synthetic edges).
    for (from, to) in adds {
        view.insert(crate::edge_table::Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: from.clone(),
            to: to.clone(),
            condition: None,
            modifier: None,
            is_default: false,
            lane: None,
        });
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_table::Edge;
    use meclaw_core::Uuid;

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            id: Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: None,
            modifier: None,
            is_default: false,
            lane: None,
        }
    }

    #[test]
    fn is_connected_false_when_no_edge_touches_path() {
        let mut edges = EdgeTable::new();
        edges.insert(edge("/a", "/b"));
        assert!(!is_connected(&Path::new("/lonely"), &edges));
    }

    #[test]
    fn is_connected_true_with_only_outbound_edge() {
        let mut edges = EdgeTable::new();
        edges.insert(edge("/src", "/dst"));
        assert!(is_connected(&Path::new("/src"), &edges));
    }

    #[test]
    fn is_connected_true_with_only_inbound_edge() {
        let mut edges = EdgeTable::new();
        edges.insert(edge("/src", "/dst"));
        assert!(is_connected(&Path::new("/dst"), &edges));
    }

    use crate::hive_scope::{HiveScope, HiveScopeTable};

    fn hive_scopes(paths: &[&str]) -> HiveScopeTable {
        let mut t = HiveScopeTable::new();
        for p in paths {
            t.register(HiveScope { path: Path::new(p) });
        }
        t
    }

    /// Phase 13.5-Lifecycle-3b T2.5: table-driven activity over a synthetic
    /// topology — no Colony spawn.
    ///
    /// Topology: root `/`; top-level cell `/top`; hive `/h`; cells `/h/c1`,
    /// `/h/c2` inside the hive.
    #[test]
    fn compute_active_recursive_rules() {
        let hs = hive_scopes(&["/h"]);

        // (a) root child connected → active.
        {
            let mut edges = EdgeTable::new();
            edges.insert(edge("/top", "/sink"));
            assert!(
                compute_active(&Path::new("/top"), &edges, &hs),
                "(a) connected node directly under root is active"
            );
        }

        // (b) connected node under an inactive parent-hive → inactive.
        // `/h/c1`→`/h/c2` is INTERNAL wiring: both cells are connected, but the
        // hive `/h` has no parent-level edge, so the whole subtree is inactive.
        {
            let mut edges = EdgeTable::new();
            edges.insert(edge("/h/c1", "/h/c2"));
            assert!(
                !compute_active(&Path::new("/h/c1"), &edges, &hs),
                "(b) connected node under disconnected hive is inactive"
            );
            assert!(
                !compute_active(&Path::new("/h"), &edges, &hs),
                "(c) hive with only internal wiring is inactive — internal edges do not count"
            );
        }

        // (c) hive active iff ≥1 parent-level edge points at its path; then a
        // connected node inside it is active.
        {
            let mut edges = EdgeTable::new();
            edges.insert(edge("/top", "/h")); // parent-level edge → hive connected
            edges.insert(edge("/h/c1", "/h/c2")); // internal wiring
            assert!(
                compute_active(&Path::new("/h"), &edges, &hs),
                "(c) hive with a parent-level edge is active"
            );
            assert!(
                compute_active(&Path::new("/h/c1"), &edges, &hs),
                "(c) connected node under an active hive is active"
            );
        }

        // (d) subtree under a disconnected hive → all inactive despite internal
        // edges. Remove the parent-level edge again: only internal wiring remains.
        {
            let mut edges = EdgeTable::new();
            edges.insert(edge("/h/c1", "/h/c2"));
            for n in ["/h", "/h/c1", "/h/c2"] {
                assert!(
                    !compute_active(&Path::new(n), &edges, &hs),
                    "(d) {n} under disconnected hive must be inactive"
                );
            }
        }
    }

    /// Phase 13.5-Lifecycle-3b T2.5: a disconnected node (no edge at all) is
    /// inactive even directly under the root.
    #[test]
    fn compute_active_disconnected_node_is_inactive() {
        let hs = hive_scopes(&[]);
        let edges = EdgeTable::new();
        assert!(!compute_active(&Path::new("/lonely"), &edges, &hs));
    }

    /// Phase 13.5-Lifecycle-3b T2.5: nested hives — a node is inactive if ANY
    /// ancestor hive in its chain is disconnected.
    #[test]
    fn compute_active_nested_hive_chain() {
        let hs = hive_scopes(&["/outer", "/outer/inner"]);
        let mut edges = EdgeTable::new();
        // `/outer` connected to root, `/outer/inner` connected to `/outer`,
        // `/outer/inner/leaf` connected internally.
        edges.insert(edge("/top", "/outer"));
        edges.insert(edge("/outer/x", "/outer/inner"));
        edges.insert(edge("/outer/inner/leaf", "/outer/inner/other"));
        assert!(compute_active(&Path::new("/outer/inner/leaf"), &edges, &hs));

        // Now disconnect the INNER hive (drop the edge pointing at it).
        let mut edges2 = EdgeTable::new();
        edges2.insert(edge("/top", "/outer"));
        edges2.insert(edge("/outer/inner/leaf", "/outer/inner/other"));
        assert!(
            !compute_active(&Path::new("/outer/inner/leaf"), &edges2, &hs),
            "leaf under disconnected inner hive is inactive even though outer is active"
        );
    }

    fn paths(ps: &[&str]) -> Vec<Path> {
        ps.iter().map(|p| Path::new(p)).collect()
    }

    /// Phase 13.5-Lifecycle-3b T2.7: removing the last edge of a hive puts the
    /// whole subtree (plus the parent chain) into the affected scope.
    #[test]
    fn affected_scope_includes_full_subtree_and_parent_chain() {
        // Registry universe.
        let known = paths(&["/top", "/h", "/h/c1", "/h/c2", "/h/sub/deep", "/unrelated"]);
        // The removed edge referenced the hive `/h`.
        let involved = paths(&["/h"]);
        let scope = affected_scope(&involved, &known, &[]);

        // Whole subtree of `/h`.
        for n in ["/h", "/h/c1", "/h/c2", "/h/sub/deep"] {
            assert!(scope.contains(&Path::new(n)), "{n} must be in scope");
        }
        // Parent chain up to root.
        assert!(scope.contains(&Path::new("/")), "root must be in scope");
        // Unrelated node not pulled in.
        assert!(!scope.contains(&Path::new("/unrelated")));
        // `/top` is a sibling, not an ancestor or descendant of `/h`.
        assert!(!scope.contains(&Path::new("/top")));
    }

    /// Phase 13.5-Lifecycle-3b T2.7: a leaf edge endpoint pulls in only itself
    /// plus its parent chain (no spurious siblings) and respects segment
    /// boundaries (`/h` must not match `/house`).
    #[test]
    fn affected_scope_leaf_endpoint_and_segment_boundary() {
        let known = paths(&["/h", "/h/c1", "/house", "/house/x"]);
        let involved = paths(&["/h/c1"]);
        let scope = affected_scope(&involved, &known, &[]);

        assert!(scope.contains(&Path::new("/h/c1")));
        assert!(scope.contains(&Path::new("/h"))); // parent
        assert!(scope.contains(&Path::new("/"))); // root
        // Segment boundary: `/house*` must NOT be dragged in by `/h`.
        assert!(!scope.contains(&Path::new("/house")));
        assert!(!scope.contains(&Path::new("/house/x")));
    }

    /// Phase 13.5-Lifecycle-3b T2.7: multiple involved paths union their scopes.
    #[test]
    fn affected_scope_unions_multiple_involved_paths() {
        let known = paths(&["/a", "/a/x", "/b", "/b/y"]);
        let involved = paths(&["/a", "/b/y"]);
        let scope = affected_scope(&involved, &known, &[]);
        for n in ["/a", "/a/x", "/b/y", "/b", "/"] {
            assert!(scope.contains(&Path::new(n)), "{n} expected in union scope");
        }
    }

    fn pair(from: &str, to: &str) -> (Path, Path) {
        (Path::new(from), Path::new(to))
    }

    /// Paket-3 P3-C1 (A3 pin): a deriving edge present ONLY in the diff buffer
    /// (an `add`), not yet in the committed table, IS visible in the post-state
    /// view → `compute_active` against the view sees it. The fresh cell `/h/lr`
    /// is connected (lr→sink) but its parent hive `/h` is inactive → inactive.
    #[test]
    fn post_state_view_includes_buffer_only_add_edge_and_derives_inactive() {
        let hs = hive_scopes(&["/h"]);
        // Committed edges are EMPTY — the deriving edge exists only in the diff.
        let current = EdgeTable::new();
        let adds = vec![pair("/h/lr", "/h/sink")];
        let view = post_state_edges(&current, &adds, &[], &[]);
        // The buffer-only edge is visible in the view.
        assert!(
            is_connected(&Path::new("/h/lr"), &view),
            "buffer edge visible"
        );
        // Parent hive `/h` has no incoming edge → inactive → the cell is inactive.
        assert!(
            !compute_active(&Path::new("/h/lr"), &view, &hs),
            "cell connected only via buffer edge under inactive hive → inactive"
        );
    }

    /// Paket-3 P3-C1 / C2 grace counter-case: WITHOUT the deriving edge, the
    /// fresh cell under root is edge-less → `is_connected == false` → the C1 gate
    /// (which keys on `is_connected && !compute_active`) does NOT fire → spawn.
    #[test]
    fn post_state_view_edge_less_cell_stays_disconnected_grace() {
        let current = EdgeTable::new();
        let view = post_state_edges(&current, &[], &[], &[]);
        assert!(
            !is_connected(&Path::new("/lr"), &view),
            "edge-less fresh cell is not connected → Grace path (gate does not fire)"
        );
    }

    /// Paket-3 P3-C1: a `remove_nodes` path drops EVERY edge touching it (from OR
    /// to) in the post-state view — so removing the last gating edge of a parent
    /// hive deactivates a previously-active sibling correctly.
    #[test]
    fn post_state_view_remove_node_drops_all_touching_edges() {
        let mut current = EdgeTable::new();
        current.insert(edge("/top", "/h")); // gating edge for hive /h
        current.insert(edge("/h/c1", "/h/c2"));
        // remove_nodes /top → every edge touching /top is dropped (the gate edge).
        let view = post_state_edges(&current, &[], &[], &[Path::new("/top")]);
        assert!(
            !is_connected(&Path::new("/h"), &view),
            "removing /top drops /top→/h → hive /h disconnected in the view"
        );
    }

    /// Paket-3 P3-C1: a `remove_edges` (from, to) pair drops the exact matching
    /// edge in the post-state view (mirrors step 10's exact-pair match).
    #[test]
    fn post_state_view_remove_edge_drops_exact_pair() {
        let mut current = EdgeTable::new();
        current.insert(edge("/top", "/h"));
        let view = post_state_edges(&current, &[], &[pair("/top", "/h")], &[]);
        assert!(
            !is_connected(&Path::new("/h"), &view),
            "remove_edges /top→/h drops the gate edge → hive /h disconnected"
        );
    }

    // ── R12: boundary-crossing edges connect the hive (depth-port semantics) ──

    /// R12: a depth edge crossing INTO the hive subtree (`/anchor → /h/c1`)
    /// connects the hive itself — the unit is wired to the world, no island
    /// status. Internal wiring under the now-active hive activates too.
    #[test]
    fn hive_connected_by_inbound_boundary_crossing_edge() {
        let hs = hive_scopes(&["/h"]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/anchor", "/h/c1"));
        edges.insert(edge("/h/c1", "/h/c2"));
        assert!(
            compute_active(&Path::new("/h"), &edges, &hs),
            "hive crossed by an inbound depth edge must be active"
        );
        assert!(compute_active(&Path::new("/h/c1"), &edges, &hs));
        assert!(
            compute_active(&Path::new("/h/c2"), &edges, &hs),
            "internally wired cell under the now-active hive must be active"
        );
    }

    /// R12: an OUTBOUND crossing (`/h/c1 → /sink`) connects the hive too —
    /// a single in- OR out-port suffices (spec connectivity predicate).
    #[test]
    fn hive_connected_by_outbound_boundary_crossing_edge() {
        let hs = hive_scopes(&["/h"]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/h/c1", "/sink"));
        assert!(
            compute_active(&Path::new("/h"), &edges, &hs),
            "hive crossed by an outbound depth edge must be active"
        );
        assert!(compute_active(&Path::new("/h/c1"), &edges, &hs));
    }

    /// R12 guard: internal wiring alone still does NOT connect the hive —
    /// both endpoints lie inside the subtree, nothing crosses the boundary
    /// (pre-R12 pin `post_state_view_includes_buffer_only_add_edge_and_
    /// derives_inactive` semantics preserved).
    #[test]
    fn internal_wiring_alone_still_leaves_hive_disconnected() {
        let hs = hive_scopes(&["/h"]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/h/c1", "/h/c2"));
        assert!(!compute_active(&Path::new("/h"), &edges, &hs));
        assert!(!compute_active(&Path::new("/h/c1"), &edges, &hs));
    }

    /// R12: nested hives — a depth edge into `/a/b/leaf` crosses BOTH `/a`
    /// and `/a/b`; the whole chain activates.
    #[test]
    fn nested_hives_connected_by_deep_crossing_edge() {
        let hs = hive_scopes(&["/a", "/a/b"]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/anchor", "/a/b/leaf"));
        assert!(compute_active(&Path::new("/a"), &edges, &hs));
        assert!(compute_active(&Path::new("/a/b"), &edges, &hs));
        assert!(compute_active(&Path::new("/a/b/leaf"), &edges, &hs));
    }

    /// R12 guard: crossing connectivity is gated on REGISTERED hives — a
    /// non-hive path gains nothing from edges under a same-prefix path.
    #[test]
    fn crossing_connectivity_only_applies_to_registered_hives() {
        let hs = hive_scopes(&[]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/anchor", "/h/c1"));
        assert!(
            !compute_active(&Path::new("/h"), &edges, &hs),
            "/h is not a registered hive scope → no crossing connectivity"
        );
    }

    /// R12: `affected_scope` pulls in the SUBTREE of a registered-hive
    /// ANCESTOR whose boundary the mutation CROSSES (the depth edge
    /// `/anchor → /h/c1` puts `/anchor` outside and `/h/c1` inside) — the
    /// crossing can flip the hive's activity, which gates its whole subtree
    /// (`/h/c2` must be recomputed although only `/h/c1` is an endpoint).
    #[test]
    fn affected_scope_includes_subtree_of_crossed_hive_ancestors() {
        let known = paths(&["/anchor", "/h/c1", "/h/c2", "/unrelated"]);
        let involved = paths(&["/anchor", "/h/c1"]);
        let hives = paths(&["/h"]);
        let scope = affected_scope(&involved, &known, &hives);
        assert!(
            scope.contains(&Path::new("/h/c2")),
            "sibling under the crossed hive ancestor must be recomputed"
        );
        assert!(!scope.contains(&Path::new("/unrelated")));
    }

    // ── GH #265: the unit is the hive path TOGETHER WITH its subtree ──────
    //
    // The hive boundary MANDATES that a hive serves its own children with
    // edges whose `from` is the hive itself (`{"from": ".", "to": "./cell"}`,
    // `cell-types.md` § Die Hive-Grenze). That wiring names the hive path, and
    // it also has exactly one endpoint strictly BELOW the hive path — so both
    // predicates used to read it as a connection. A unit with nothing left but
    // its own inside therefore counted as connected and stayed awake.

    /// GH #265: the two forms in which a hive wires its own children —
    /// `<hive> → <hive>/<cell>` and `<hive>/<cell> → <hive>` — are INTERNAL.
    /// A unit that has nothing but its own inside left is disconnected, and
    /// its whole subtree sleeps with it.
    #[test]
    fn a_hives_own_inward_wiring_does_not_connect_it() {
        let hs = hive_scopes(&["/gen2"]);
        let mut edges = EdgeTable::new();
        // The mandated hive-boundary form, both directions.
        edges.insert(edge("/gen2", "/gen2/worker"));
        edges.insert(edge("/gen2/worker", "/gen2"));
        // Plus ordinary sibling wiring inside.
        edges.insert(edge("/gen2/worker", "/gen2/sink"));
        assert!(
            !compute_active(&Path::new("/gen2"), &edges, &hs),
            "a unit whose only edges are its own inside must be disconnected"
        );
        assert!(
            !compute_active(&Path::new("/gen2/worker"), &edges, &hs),
            "the subtree of a disconnected unit sleeps with it"
        );
    }

    /// GH #265 counter-pin — the important half: an activity rule that sleeps
    /// too much takes a running installation off the net. A unit reached ONLY
    /// through a depth port (nothing names its own path) stays awake, even
    /// though it carries the full mandated inward wiring.
    #[test]
    fn a_unit_reached_only_through_a_depth_port_stays_awake() {
        let hs = hive_scopes(&["/unit"]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/anchor", "/unit/dispatch")); // the only external edge
        edges.insert(edge("/unit", "/unit/dispatch"));
        edges.insert(edge("/unit/dispatch", "/unit"));
        assert!(
            compute_active(&Path::new("/unit"), &edges, &hs),
            "a crossing depth edge connects the unit — nothing names its path"
        );
        assert!(compute_active(&Path::new("/unit/dispatch"), &edges, &hs));
    }

    /// GH #265 counter-pin: a self-contained unit that only DRAINS outward —
    /// one edge from inside to the world, nothing pointing in — stays awake.
    /// This is the shape a unit that runs on its own clock needs, and the rule
    /// stands on its own: the citation that used to sit here named
    /// `examples/meclaw-os/grow-argus.json` as an instance of it, and that
    /// declaration draws no edge at all any more (GH #391). The shape is built
    /// below rather than pointed at, so nothing outside this file can retire it
    /// silently again.
    #[test]
    fn a_unit_that_only_drains_outward_stays_awake() {
        let hs = hive_scopes(&["/unit"]);
        let mut edges = EdgeTable::new();
        edges.insert(edge("/unit", "/unit/tick"));
        edges.insert(edge("/unit/tick", "/unit"));
        edges.insert(edge("/unit", "/sink")); // the one external edge
        assert!(compute_active(&Path::new("/unit"), &edges, &hs));
        assert!(compute_active(&Path::new("/unit/tick"), &edges, &hs));
    }

    /// GH #265: a parent-level edge naming the hive path still connects it,
    /// mandated inward wiring present or not — and removing that ONE edge is
    /// what puts the unit to sleep.
    #[test]
    fn a_parent_level_edge_connects_a_unit_that_also_wires_itself() {
        let hs = hive_scopes(&["/unit"]);
        let mut wired = EdgeTable::new();
        wired.insert(edge("/top", "/unit"));
        wired.insert(edge("/unit", "/unit/worker"));
        assert!(compute_active(&Path::new("/unit"), &wired, &hs));
        assert!(compute_active(&Path::new("/unit/worker"), &wired, &hs));

        let mut unwired = EdgeTable::new();
        unwired.insert(edge("/unit", "/unit/worker"));
        assert!(
            !compute_active(&Path::new("/unit"), &unwired, &hs),
            "dropping the last external edge puts the unit to sleep"
        );
    }

    /// R12 locality guards: (a) an all-INTERNAL mutation (both endpoints
    /// inside `/h`) pulls no hive subtree beyond the involved paths; (b) a
    /// root-level mutation never pulls the root's subtree (everything is
    /// "inside" the root, nothing crosses it) — test-relevant: a registered
    /// root hive must not turn every mutation into a colony-wide recompute.
    #[test]
    fn affected_scope_internal_mutation_stays_local() {
        let known = paths(&["/h/c1", "/h/c2", "/h/c3", "/t", "/c", "/lonely"]);
        // (a) internal edge /h/c1 → /h/c2: /h/c3 is NOT pulled in.
        let scope = affected_scope(&paths(&["/h/c1", "/h/c2"]), &known, &paths(&["/h"]));
        assert!(!scope.contains(&Path::new("/h/c3")));
        // (b) root-level edge /t → /c with the ROOT registered as hive:
        // /lonely must NOT be recomputed (no colony-wide walk).
        let scope = affected_scope(&paths(&["/t", "/c"]), &known, &paths(&["/"]));
        assert!(!scope.contains(&Path::new("/lonely")));
    }
}
