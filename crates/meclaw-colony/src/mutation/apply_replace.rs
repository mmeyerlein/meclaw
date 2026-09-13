//! GH #682 — `replace_nodes`: the apply half of lifting a standing node in
//! place to another version of its template.
//!
//! [`crate::mutation::stage_replace`] builds the plan under `.staging` and
//! refuses what it can see from there. This module is what the apply arm of
//! `handle_mutation` (apply sequence step 9d) does with a [`StagedReplace`],
//! in the order Spec § 2.4 fixes — everything still pre-destructive first,
//! then the moves:
//!
//! 1. **Front checks**, before the first rename: the registry half of the
//!    `<child>~<from_version>` collision ([`refuse_taken_rows`] — staging has
//!    no registry handle and could only see the disk), and the new inner
//!    edges compiled ([`compile_inner_edges`]) so nothing about them can fail
//!    once directories have moved. The hive-contract guard of Spec § 2.6 (a
//!    lane an outer edge uses cannot be dropped) runs even earlier, in the
//!    validate stage beside the other contract checks; the hive↔leaf class
//!    guard (ruling T4-C4) in staging, where both classes are first known.
//! 2. **Files** ([`apply_lift_files`]): every changed child's OLD directory is
//!    renamed beside itself (`<child>~<from_version>`, whole subtree), the
//!    added and changed children are renamed in from staging, and the hive's
//!    renewed declaration replaces its `config.json` — the old bytes kept in
//!    memory for the way back.
//! 3. **Registry rows** ([`move_rows_aside`]): the old child's entry, its
//!    `node_contracts` mirror and — should the child be a hive — every row
//!    and scope below it move to the suffixed path, in RAM at once and in
//!    `colony.db` through the mutation's write buffer, so a refusal further
//!    down discards the durable half by discarding the buffer.
//! 4. **Inner edges**: handled by the caller with the buffers it owns —
//!    INSERT-BEFORE-REMOVE like the swap's swing: the new template's edges
//!    go in (a content-equal old edge is kept, not doubled), then every old
//!    edge with both ends under the hive that no new edge equals comes out
//!    ([`old_inner_edges`], [`retained_by`]). One exception to the order:
//!    an old edge a new one equals on the five terms but not on the lane
//!    comes out FIRST ([`displaced_by`]) — the dedup would otherwise take
//!    the new edge for the old one and the lane would never land.
//! 5. **One recompute**: every node of the subtree is seeded into `involved`;
//!    step 10b then stops the renamed-old and the left children (edge-less)
//!    and spawns the new ones. There is no explicit stop before it.
//! 6. **Rollback** ([`undo_lift_files`], [`move_rows_back`]): every refusal
//!    after the moves — the stop-wiring guard among them — renames the old
//!    children back, takes the fresh directories out, restores the
//!    declaration and moves the rows back; the swap's rollback list is the
//!    model, the registry rows this mutation registered come out through
//!    the same `rollback_registered_nodes` every other refusal uses.
//!
//! The registration of the added and changed-fresh cells is not here: they
//! ride the same loop the subtree merge's rename-roots ride (apply step
//! 9c(1)), because a cell born by a lift needs exactly what a cell born by a
//! merge needs.
//!
//! **A parked entry keeps the task it was born with.** The old child's
//! registry entry moves to `<child>~<from_version>`, but its running task,
//! its watcher and its respawn closure are still addressed at the BIRTH
//! path — the path the new child now holds. Two consequences, both closed:
//!
//! * A parked row is never wired again: the validate stage refuses every
//!   edge that resolves to a path with a `~` in any segment
//!   (`validate::is_parked_path` — the parked node or a row under a parked
//!   hive), and no operator-given name may carry the character
//!   (`validate::PARKED_MARKER`), so no recompute ever calls the old respawn
//!   closure and spawns a task under the successor's name.
//! * **A non-peaceful death of the old task during the stop wait is the
//!   parked entry's** (GH #688). Step 10b peace-stops the old task and waits
//!   for its death-ack. A peace exit is silent and comes back as `Stopped`,
//!   whose owner test `handle_stopped` has (GH #682: the entry whose mailbox
//!   the returned receiver closed is parked, wherever the lift moved it). A
//!   task that dies between the stop signal and its ack by PANIC or by the
//!   `message_timeout` BACKSTOP — the two deaths `handle_cell_died`
//!   restarts — makes its watcher send `CellDied` under the birth path,
//!   which the loop processes after the mutation committed. The corridor is
//!   keyed by path and byte-frozen, so the `CellDied` arm decides the owner
//!   BEFORE it (`colony::claim_parked_death`), by the same test: when the
//!   entry at the path is alive and a parked entry still names the path with
//!   a closed mailbox, the parked entry is parked the way a peaceful stop
//!   would have parked it, the new child is not touched — no restart, no
//!   second task, no later removal of its row — and the remainder the old
//!   task's `MailboxGuard` handed over as `MailboxRescued` under the birth
//!   path is dead-lettered as `cell_inactive` instead of reaching the new
//!   child. Every other `CellDied` is the corridor's exactly as before.
//!   Re-keying the parked task the way `move_nodes` rebuilds a relocated cell
//!   (stop, rename, respawn at the new path through the step-9 spawn loop)
//!   would spawn the old cell once more only to park it, and was not taken.

use super::MutationError;
use super::PlannedMove;
use super::stage_replace::StagedReplace;
use crate::edge_table::{Edge, EdgeTable};
use crate::hive_scope::{HiveScope, HiveScopeTable};
use crate::persist::writer::ColonyWriteOp;
use crate::{NodeContract, RegistryEntry};
use meclaw_core::{Path, Uuid};
use std::collections::HashMap;
use std::path::PathBuf;

/// What one lift did to the live tree and the registry — the record its
/// rollback reads. Built step by step by [`apply_lift_files`] and
/// [`move_rows_aside`], so a failure half-way leaves an accurate list.
#[derive(Debug, Default)]
pub struct AppliedLift {
    /// The renames done, in order: old child → `<child>~<from_version>`.
    pub asides: Vec<PlannedMove>,
    /// The directories renamed in from staging (added and changed-fresh
    /// roots) — fresh by construction, removable on rollback.
    pub fresh_dirs: Vec<PathBuf>,
    /// The hive's `config.json` this lift overwrote, and the bytes that
    /// stood there before.
    pub declaration: Option<(PathBuf, Vec<u8>)>,
    /// Every registry row moved aside, `(from, to)`, prefix rows included.
    pub moved_rows: Vec<(Path, Path)>,
    /// Every hive scope moved aside, `(from, to)`.
    pub moved_scopes: Vec<(Path, Path)>,
}

/// The registry half of the aside name: staging chose `<child>~<from_version>`
/// (or `~<n>`) as the first name FREE ON DISK, having no registry handle; a
/// row standing at that name — or below it — without a directory (rare under
/// No-Delete: a row whose directory was taken away by hand) is refused here,
/// before the first rename.
///
/// # Errors
/// [`MutationError::NamingCollision`] naming the taken row.
pub fn refuse_taken_rows(
    lift: &StagedReplace,
    registry: &HashMap<Path, RegistryEntry>,
) -> Result<(), MutationError> {
    for change in &lift.changed {
        let to = &change.aside.to;
        let taken = registry
            .keys()
            .find(|k| crate::connectivity::is_self_or_descendant(k, to));
        if let Some(row) = taken {
            let name = lift
                .absolute_path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default();
            return Err(MutationError::NamingCollision(format!(
                "replace_nodes[] '{name}': the old '{}' would be renamed beside itself as '{}', \
                 but the registry already holds a row at {} without a directory there (a lift \
                 from {} left it, and the directory is gone). Nothing was written. Remove that \
                 row's claim first.",
                change
                    .aside
                    .from
                    .as_str()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
                to.as_str().rsplit('/').next().unwrap_or_default(),
                row.as_str(),
                change.from_version
            )));
        }
    }
    Ok(())
}

/// The new template's inner edges as the edge table takes them: condition
/// and modifier compiled, fresh ids, phase and lane verbatim. Done at the
/// front so a template whose edge does not compile is refused before a
/// directory moves — the colony's apply path panics on nothing it can refuse.
///
/// # Errors
/// [`MutationError::Schema`] naming the edge and what did not compile.
pub fn compile_inner_edges(lift: &StagedReplace) -> Result<Vec<Edge>, MutationError> {
    let mut out = Vec::with_capacity(lift.internal_edges.len());
    for edge in &lift.internal_edges {
        let condition = match &edge.condition {
            Some(src) => Some(crate::cel_eval::parse_condition(src).map_err(|e| {
                MutationError::Schema(format!(
                    "replace_nodes: inner edge {} -> {} of the new template has a condition \
                     that does not compile: {e}",
                    edge.from.as_str(),
                    edge.to.as_str()
                ))
            })?),
            None => None,
        };
        let modifier = match &edge.modifier {
            Some(m) => {
                let spec: crate::config::ModifierSpec =
                    meclaw_core::serde_json::from_value(m.clone()).map_err(|e| {
                        MutationError::Schema(format!(
                            "replace_nodes: inner edge {} -> {} of the new template has a modifier \
                         that is not a modifier spec: {e}",
                            edge.from.as_str(),
                            edge.to.as_str()
                        ))
                    })?;
                Some(crate::cel_eval::parse_modifier(&spec).map_err(|(k, e)| {
                    MutationError::Schema(format!(
                        "replace_nodes: inner edge {} -> {} of the new template has a modifier \
                         that does not compile ({k}): {e}",
                        edge.from.as_str(),
                        edge.to.as_str()
                    ))
                })?)
            }
            None => None,
        };
        out.push(Edge {
            id: Uuid::now_v7(),
            from: edge.from.clone(),
            to: edge.to.clone(),
            condition,
            modifier,
            is_default: edge.is_default,
            lane: edge.lane.clone(),
        });
    }
    Ok(out)
}

/// Steps 1–3 on disk, in the only order that works: the old children go
/// aside FIRST (a `rename(2)` onto a full directory is `ENOTEMPTY`), then the
/// staged ones come in, then the declaration is swapped — its old bytes read
/// into the record before the swap, so the way back needs no file. What
/// moved is appended to `done` step by step, so a failure half-way leaves an
/// accurate record for [`undo_lift_files`].
///
/// # Errors
/// The first `rename(2)`/read that fails. Every path was checked free or
/// present before this ran, so a failure here is an environment failure
/// after the live tree started moving — the caller treats it as the
/// mid-rename strict-fail class.
pub fn apply_lift_files(lift: &StagedReplace, done: &mut AppliedLift) -> Result<(), String> {
    for change in &lift.changed {
        let mv = &change.aside;
        std::fs::rename(&mv.from_dir, &mv.to_dir)
            .map_err(|e| format!("rename {:?} -> {:?}: {e}", mv.from_dir, mv.to_dir))?;
        done.asides.push(mv.clone());
    }
    let fresh_roots = lift
        .added
        .iter()
        .chain(lift.changed.iter().map(|c| &c.fresh));
    for root in fresh_roots {
        std::fs::rename(&root.root_staging_path, &root.root_final_path).map_err(|e| {
            format!(
                "rename {:?} -> {:?}: {e}",
                root.root_staging_path, root.root_final_path
            )
        })?;
        done.fresh_dirs.push(root.root_final_path.clone());
    }
    if let Some(decl) = &lift.declaration {
        let previous = std::fs::read(&decl.final_path)
            .map_err(|e| format!("read the standing declaration {:?}: {e}", decl.final_path))?;
        std::fs::rename(&decl.staging_path, &decl.final_path).map_err(|e| {
            format!(
                "rename {:?} -> {:?}: {e}",
                decl.staging_path, decl.final_path
            )
        })?;
        done.declaration = Some((decl.final_path.clone(), previous));
    }
    Ok(())
}

/// The way back on disk, in reverse: the fresh directories out first (they
/// hold the paths the old children return to), then the old children back,
/// then the declaration's old bytes. A step that fails is logged, never
/// swallowed — the same discipline as the reject sweep.
pub fn undo_lift_files(done: &AppliedLift, mutation_id: &str) {
    for dir in done.fresh_dirs.iter().rev() {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::error!(
                mutation_id = %mutation_id,
                path = %dir.display(),
                error = %e,
                "lift rollback could not remove a directory it renamed in"
            ),
        }
    }
    for mv in done.asides.iter().rev() {
        if let Err(e) = std::fs::rename(&mv.to_dir, &mv.from_dir) {
            tracing::error!(
                mutation_id = %mutation_id,
                from = %mv.to_dir.display(),
                to = %mv.from_dir.display(),
                error = %e,
                "lift rollback could not rename an old child back"
            );
        }
    }
    if let Some((path, previous)) = &done.declaration
        && let Err(e) = std::fs::write(path, previous)
    {
        tracing::error!(
            mutation_id = %mutation_id,
            path = %path.display(),
            error = %e,
            "lift rollback could not restore the hive's declaration"
        );
    }
}

/// Step 3: move the old child's registry row(s) and scope(s) to the suffixed
/// path — the entry itself, its `node_contracts` mirror, and every row and
/// hive scope below it when the child is a hive (the prefix rows Task 4
/// named). RAM at once; `colony.db` through `write_buffer`, which is the
/// mutation's own and is discarded by any refusal, so the durable half needs
/// no undo of its own. The `MoveRegistryPath` op moves the row by UPDATE
/// (identity, `created_at` and provenance stay — the difference between
/// moving a cell and replacing it), and rides ahead of the new cell's
/// `UpsertRegistry` in the same FIFO buffer.
///
/// Appends what it moved to `done` for [`move_rows_back`].
pub fn move_rows_aside(
    aside: &PlannedMove,
    registry: &mut HashMap<Path, RegistryEntry>,
    node_contracts: &mut HashMap<Path, NodeContract>,
    hive_scopes: &mut HiveScopeTable,
    write_buffer: &mut Vec<ColonyWriteOp>,
    now: i64,
    done: &mut AppliedLift,
) {
    let rebase = |p: &Path| -> Path {
        let rest = &p.as_str()[aside.from.as_str().len()..];
        Path::new(&format!("{}{rest}", aside.to.as_str()))
    };
    let mut rows: Vec<Path> = registry
        .keys()
        .filter(|k| crate::connectivity::is_self_or_descendant(k, &aside.from))
        .cloned()
        .collect();
    rows.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for from in rows {
        let to = rebase(&from);
        if let Some(entry) = registry.remove(&from) {
            registry.insert(to.clone(), entry);
        }
        if let Some(contract) = node_contracts.remove(&from) {
            node_contracts.insert(to.clone(), contract);
        }
        write_buffer.push(ColonyWriteOp::MoveRegistryPath {
            from: from.clone(),
            to: to.clone(),
            updated_at: now,
        });
        done.moved_rows.push((from, to));
    }
    let mut scopes: Vec<Path> = hive_scopes
        .paths()
        .filter(|p| crate::connectivity::is_self_or_descendant(p, &aside.from))
        .cloned()
        .collect();
    scopes.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for from in scopes {
        let to = rebase(&from);
        hive_scopes.remove(&from);
        hive_scopes.register(HiveScope { path: to.clone() });
        // The old row stays: the new child at `from` is a hive too (the
        // template names it as one) and re-registers that path itself.
        write_buffer.push(ColonyWriteOp::InsertHiveScope {
            path: to.clone(),
            created_at: now,
        });
        done.moved_scopes.push((from, to));
    }
}

/// The RAM half of the way back for [`move_rows_aside`], in reverse. The
/// caller has already taken out the rows this mutation registered at the
/// old paths (`rollback_registered_nodes`), so the moves land on free keys.
pub fn move_rows_back(
    done: &AppliedLift,
    registry: &mut HashMap<Path, RegistryEntry>,
    node_contracts: &mut HashMap<Path, NodeContract>,
    hive_scopes: &mut HiveScopeTable,
) {
    for (from, to) in done.moved_rows.iter().rev() {
        if let Some(entry) = registry.remove(to) {
            registry.insert(from.clone(), entry);
        }
        if let Some(contract) = node_contracts.remove(to) {
            node_contracts.insert(from.clone(), contract);
        }
    }
    for (from, to) in done.moved_scopes.iter().rev() {
        hive_scopes.remove(to);
        hive_scopes.register(HiveScope { path: from.clone() });
    }
}

/// Every edge with BOTH ends under `hive` (the hive path itself included) —
/// the old inner graph. An outer edge has exactly one end outside and is
/// never in here: the hive keeps its path, so it keeps its outer edges.
pub fn old_inner_edges(edges: &EdgeTable, hive: &Path) -> Vec<Edge> {
    edges
        .iter()
        .filter(|e| {
            crate::connectivity::is_self_or_descendant(&e.from, hive)
                && crate::connectivity::is_self_or_descendant(&e.to, hive)
        })
        .cloned()
        .collect()
}

/// True iff one of `new` is the same edge as `old` on the five identity terms
/// (from, to, condition, modifier, phase — the dedup's identity) AND on the
/// lane. Such an old edge is retained by the lift rather than removed and
/// re-laid: the edge stays under the same id, and a kept child stays wired
/// without a gap.
pub fn retained_by(old: &Edge, new: &[Edge]) -> bool {
    new.iter()
        .any(|n| same_identity(old, n) && old.lane == n.lane)
}

/// True iff one of `new` is the same edge as `old` on the five identity terms
/// but NOT on the lane. The lane is no identity term, so the dedup would take
/// the new edge for the old one and skip it — the declaration would never
/// land. Such an old edge is displaced: it comes out BEFORE the new edges go
/// in, and the new one is inserted like any other.
pub fn displaced_by(old: &Edge, new: &[Edge]) -> bool {
    new.iter()
        .any(|n| same_identity(old, n) && old.lane != n.lane)
}

/// The five-term identity the dedup compares on, as one predicate.
fn same_identity(a: &Edge, b: &Edge) -> bool {
    crate::mutation::validate::edge_identity_equal_views(
        &crate::mutation::validate::EdgeMatchView::from(a),
        &crate::mutation::validate::EdgeMatchView::from(b),
    )
}

/// The hive contracts as they stand once this diff's lifts have swapped their
/// declarations: every lifted hive's entry replaced by the contract its
/// `config.json` now carries (read through the one reader,
/// [`crate::mutation::hive_contract::contract_from_cell_dir`]), or dropped
/// when it carries none. The post-state lane-doors check is measured against
/// this list — a hive judged by the contract it just gave up would refuse
/// the way back.
pub fn renewed_contracts(
    standing: Vec<crate::mutation::hive_contract::HiveContract>,
    lifts: &[StagedReplace],
) -> Vec<crate::mutation::hive_contract::HiveContract> {
    let mut out: Vec<_> = standing
        .into_iter()
        .filter(|c| {
            !lifts
                .iter()
                .any(|l| l.absolute_path.as_str() == c.hive_path)
        })
        .collect();
    for lift in lifts {
        if lift.declaration.is_none() {
            continue;
        }
        if let Some(c) = crate::mutation::hive_contract::contract_from_cell_dir(
            &lift.final_path,
            lift.absolute_path.as_str(),
        ) {
            out.push(c);
        }
    }
    out
}

/// Every logical path a lift touches, for the one recompute: the hive, the
/// renamed-old children, the added and changed-fresh cells and hive markers,
/// the kept cells and hives, the left nodes, and both ends of every new
/// inner edge.
pub fn involved_paths(lift: &StagedReplace, done: &AppliedLift) -> Vec<Path> {
    let mut out = vec![lift.absolute_path.clone()];
    out.extend(done.moved_rows.iter().map(|(_, to)| to.clone()));
    out.extend(done.moved_scopes.iter().map(|(_, to)| to.clone()));
    for root in lift
        .added
        .iter()
        .chain(lift.changed.iter().map(|c| &c.fresh))
    {
        out.extend(root.cells.iter().map(|c| c.absolute_path.clone()));
        out.extend(root.hive_scopes.iter().cloned());
    }
    out.extend(lift.kept.iter().map(|k| k.absolute_path.clone()));
    out.extend(lift.kept_hives.iter().map(|k| k.absolute_path.clone()));
    out.extend(lift.left.iter().map(|l| Path::new(&l.abs_path)));
    for e in &lift.internal_edges {
        out.push(e.from.clone());
        out.push(e.to.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, cond: Option<&str>) -> Edge {
        Edge {
            id: Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: cond.map(|c| crate::cel_eval::parse_condition(c).unwrap()),
            modifier: None,
            is_default: false,
            lane: None,
        }
    }

    /// The inner graph is what has both ends under the hive; the hive's own
    /// path counts as inside, and a lookalike sibling (`/h2`) does not.
    #[test]
    fn old_inner_edges_take_both_ends_under_the_hive_and_nothing_else() {
        let mut t = EdgeTable::new();
        t.insert(edge("/s", "/h", None));
        t.insert(edge("/h", "/h/a", Some("has(hop.route)")));
        t.insert(edge("/h/a", "/h", None));
        t.insert(edge("/h", "/c", None));
        t.insert(edge("/h2", "/h2/x", None));
        let mut inner: Vec<(String, String)> = old_inner_edges(&t, &Path::new("/h"))
            .iter()
            .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
            .collect();
        inner.sort();
        assert_eq!(
            inner,
            vec![
                ("/h".to_string(), "/h/a".to_string()),
                ("/h/a".to_string(), "/h".to_string())
            ]
        );
    }

    /// Retention is the five-term identity: the same edge with the same
    /// condition is retained, a different condition is not.
    #[test]
    fn an_old_edge_is_retained_only_by_an_identical_new_one() {
        let old = edge("/h", "/h/a", Some("hop.route == 'x'"));
        assert!(retained_by(
            &old,
            &[edge("/h", "/h/a", Some("hop.route == 'x'"))]
        ));
        assert!(!retained_by(
            &old,
            &[edge("/h", "/h/a", Some("hop.route == 'y'"))]
        ));
        assert!(!retained_by(
            &old,
            &[edge("/h", "/h/b", Some("hop.route == 'x'"))]
        ));
    }

    /// GH #682 (final review): the lane is not one of the five identity
    /// terms, so a new edge that names the old one on the five terms under
    /// another lane — or under none — is the SAME edge to the dedup and would
    /// be skipped. Such an old edge is not retained; it is displaced, and it
    /// has to be out before the new one goes in.
    #[test]
    fn a_lane_disagreement_displaces_the_old_edge_instead_of_retaining_it() {
        let laned = |lane: Option<&str>| Edge {
            lane: lane.map(str::to_string),
            ..edge("/h", "/h/a", Some("hop.route == 'x'"))
        };
        let old = laned(None);
        assert!(retained_by(&old, &[laned(None)]));
        assert!(!displaced_by(&old, &[laned(None)]));

        assert!(!retained_by(&old, &[laned(Some("view"))]));
        assert!(displaced_by(&old, &[laned(Some("view"))]));

        let old = laned(Some("view"));
        assert!(retained_by(&old, &[laned(Some("view"))]));
        assert!(!retained_by(&old, &[laned(Some("notice"))]));
        assert!(displaced_by(&old, &[laned(Some("notice"))]));
        assert!(!retained_by(&old, &[laned(None)]));
        assert!(displaced_by(&old, &[laned(None)]));

        // Another edge altogether is neither.
        let other = edge("/h", "/h/b", Some("hop.route == 'x'"));
        assert!(!retained_by(&old, std::slice::from_ref(&other)));
        assert!(!displaced_by(&old, std::slice::from_ref(&other)));
    }
}
