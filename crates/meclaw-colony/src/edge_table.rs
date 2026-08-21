use crate::cel_eval::{CompiledCondition, CompiledModifier};
use meclaw_core::{Headers, Path, Uuid};
use std::collections::HashMap;

/// A directed edge between two paths in the graph.
#[derive(Debug, Clone)]
pub struct Edge {
    /// Unique identifier for this edge.
    pub id: Uuid,
    /// Source path of the edge.
    pub from: Path,
    /// Target path of the edge.
    pub to: Path,
    /// Pre-compiled CEL condition (Phase 13.5-A1). `None` = always take
    /// (Identity).
    pub condition: Option<CompiledCondition>,
    /// Pre-compiled CEL modifier (four-field: set_context/delete_context/
    /// set_hop/delete_hop). `None` = identity headers.
    pub modifier: Option<CompiledModifier>,
}

/// Table of edges indexed by their source path for O(1) fan-out lookups.
#[derive(Debug, Default)]
pub struct EdgeTable {
    by_from: HashMap<Path, Vec<Edge>>,
}

impl EdgeTable {
    /// Creates a new empty `EdgeTable`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts an edge into the table, indexing it by its `from` path.
    pub fn insert(&mut self, edge: Edge) {
        self.by_from
            .entry(edge.from.clone())
            .or_default()
            .push(edge);
    }

    /// Removes the edge with the given `id` from the table, if present.
    ///
    /// Phase-6 T21: the `remove_edges` handler in `handle_mutation` calls this per
    /// edge selected by the `match{from,to}` pattern.
    pub fn remove(&mut self, id: &Uuid) {
        for v in self.by_from.values_mut() {
            v.retain(|e| &e.id != id);
        }
    }

    /// Returns `true` iff an edge with the SAME identity already lives in the
    /// table. Edge identity per spec Z.265 is `from` + `to` + `condition` +
    /// `modifier` (NOT the UUID): two edges are equal iff their `from`/`to`
    /// paths match AND their `condition.source` strings match AND their
    /// `modifier.source` serde-JSON values match. This mirrors the F6
    /// equality used by `remove_edges_pattern_hits` / `EdgeMatchView`
    /// (string-eq on condition source, serde-JSON-eq on modifier source).
    ///
    /// Paket-5 T8 (A1): the mutation path calls this before every edge
    /// insert to make re-applied diffs idempotent — a duplicate insert would
    /// otherwise mean DOUBLE delivery on the routing cascade.
    pub fn contains_equal(&self, candidate: &Edge) -> bool {
        let cand_cond = candidate.condition.as_ref().map(|c| c.source.as_str());
        let cand_mod = candidate
            .modifier
            .as_ref()
            .and_then(|m| meclaw_core::serde_json::to_value(&m.source).ok());
        self.edges_from(&candidate.from).iter().any(|e| {
            e.to == candidate.to
                && e.condition.as_ref().map(|c| c.source.as_str()) == cand_cond
                && e.modifier
                    .as_ref()
                    .and_then(|m| meclaw_core::serde_json::to_value(&m.source).ok())
                    == cand_mod
        })
    }

    /// Returns all edges originating from `from`, or an empty slice if none exist.
    pub fn edges_from(&self, from: &Path) -> &[Edge] {
        self.by_from.get(from).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Returns all edges whose `to` equals `path`, in arbitrary order.
    ///
    /// Phase 13.5-Lifecycle-3b T2 (connectivity-recompute): there is no reverse
    /// index — connectivity-recompute is not a hot path (YAGNI), so this scans
    /// every edge via [`Self::iter`]. A scan cannot return a borrowed slice, so
    /// it collects into a `Vec<&Edge>` (empty if no edge targets `path`).
    pub fn edges_to(&self, path: &Path) -> Vec<&Edge> {
        self.iter().filter(|e| &e.to == path).collect()
    }

    /// Iterate over all edges in the table (used by Mutation validate to snapshot
    /// the current (from, to) edge-list for cycle/edge-schema checks).
    pub fn iter(&self) -> impl Iterator<Item = &Edge> {
        self.by_from.values().flatten()
    }
}

/// Decision produced by `evaluate_edge` when an edge takes a message: the
/// new target path and the (potentially modifier-transformed) headers.
#[derive(Debug, Clone)]
pub struct EdgeDecision {
    /// The target path this edge routes to.
    pub target: Path,
    /// The two-compartment headers after applying any modifier.
    pub headers_out: Headers,
    /// GH #82: this edge's modifier declared `restore_ttl`. `ttl` is envelope,
    /// so the edge only REPORTS the declaration here; the colony (the sole
    /// envelope setter) applies it when it builds the follow-up message.
    pub restore_ttl: bool,
}

/// Evaluate a single edge against the input headers.
///
/// Phase 13.5-A1 T3: CEL `condition` is evaluated; `Ok(false)` skips the
/// edge (`None` return). `Err(_)` is also a skip per spec F3 (CEL-Standard:
/// undefined-header / eval-error → skip).
///
/// GH #80: the skip is unchanged, the LEVEL follows the error class. A missing
/// key ([`crate::cel_eval::CondErrorKind::MissingKey`]) is the normal steady
/// state of a fan-out — one per non-matching lane per message — and logs at
/// `debug`. A genuine eval error stays at `warn`, which is the condition the
/// warning exists to catch. Topologies should still guard optional keys with
/// `has(hop.k) && hop.k == '...'`; that form produces no line at all.
///
/// Phase 13.5-A1 T7+T8: modifier `set` is applied to the headers between
/// condition-check and `EdgeDecision`-build, per spec Z.832 (set before
/// delete). `set`-eval-errors skip the whole edge with a warn-log.
/// Modifier `delete` runs after `set` (Z.832) and is idempotent for
/// non-existent keys (A1-F7).
pub fn evaluate_edge(edge: &Edge, headers: &Headers) -> Option<EdgeDecision> {
    if let Some(cond) = &edge.condition {
        // CEL surface is context.*/hop.*: both compartments are bound.
        match crate::cel_eval::evaluate_condition(cond, &headers.context, &headers.hop) {
            Ok(true) => {}
            Ok(false) => return None,
            Err(e) => {
                match e.kind {
                    crate::cel_eval::CondErrorKind::MissingKey => tracing::debug!(
                        edge_id = %edge.id,
                        from = %edge.from.as_str(),
                        to = %edge.to.as_str(),
                        condition = %cond.source,
                        error = %e,
                        "edge condition reads a key this message does not carry — skipping edge \
                         (spec F3; guard optional keys with has())"
                    ),
                    crate::cel_eval::CondErrorKind::Eval => tracing::warn!(
                        edge_id = %edge.id,
                        from = %edge.from.as_str(),
                        to = %edge.to.as_str(),
                        condition = %cond.source,
                        error = %e,
                        "edge condition eval failed — skipping edge (spec F3: CEL-standard semantics)"
                    ),
                }
                return None;
            }
        }
    }
    // Four-field modifier (set_context/delete_context/set_hop/delete_hop): all
    // set-expressions read the incoming context.*/hop.* as a fixed namespace;
    // per compartment set runs before delete (spec Z.832). An eval error skips
    // the whole edge with a warn-log (spec F3).
    let headers_out = match &edge.modifier {
        None => headers.clone(),
        Some(modif) => match crate::cel_eval::apply_modifier(modif, headers) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(
                    edge_id = %edge.id,
                    from = %edge.from.as_str(),
                    to = %edge.to.as_str(),
                    error = %e,
                    "edge modifier eval failed — skipping edge"
                );
                return None;
            }
        },
    };
    Some(EdgeDecision {
        target: edge.to.clone(),
        headers_out,
        restore_ttl: edge.modifier.as_ref().is_some_and(|m| m.restore_ttl),
    })
}

/// Apply all edges matching `from` to `headers`, returning every edge's
/// decision (used by the outputs-arm for fan-out).
pub fn apply_edges(table: &EdgeTable, from: &Path, headers: &Headers) -> Vec<EdgeDecision> {
    table
        .edges_from(from)
        .iter()
        .filter_map(|e| evaluate_edge(e, headers))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_lookup_by_from_path() {
        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        });
        let found = table.edges_from(&Path::new("/a"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].to.as_str(), "/b");
    }

    #[test]
    fn multi_edge_fan_out_returns_all() {
        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/c"),
            condition: None,
            modifier: None,
        });
        let found = table.edges_from(&Path::new("/a"));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn lookup_returns_empty_for_unknown_from() {
        let table = EdgeTable::new();
        assert!(table.edges_from(&Path::new("/x")).is_empty());
    }

    /// Phase 13.5-Lifecycle-3b T2.1: `edges_to` returns every edge whose `to`
    /// equals the path, regardless of `from`.
    #[test]
    fn edges_to_returns_all_inbound_edges() {
        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/c"),
            condition: None,
            modifier: None,
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/b"),
            to: Path::new("/c"),
            condition: None,
            modifier: None,
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/d"),
            condition: None,
            modifier: None,
        });
        let into_c = table.edges_to(&Path::new("/c"));
        assert_eq!(into_c.len(), 2);
        assert!(into_c.iter().all(|e| e.to.as_str() == "/c"));
    }

    /// Phase 13.5-Lifecycle-3b T2.1: empty when no edge targets the path.
    #[test]
    fn edges_to_returns_empty_for_unknown_target() {
        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        });
        assert!(table.edges_to(&Path::new("/x")).is_empty());
    }

    /// Phase 13.5-A1 T2: Edge stores condition + modifier slots.
    #[test]
    fn edge_with_condition_and_modifier_stores_both() {
        let cond = crate::cel_eval::parse_condition("hop.x == 'y'").unwrap();
        let modif = crate::cel_eval::CompiledModifier {
            source: crate::config::ModifierSpec::default(),
            set_context: std::collections::BTreeMap::new(),
            delete_context: vec![],
            set_hop: std::collections::BTreeMap::new(),
            delete_hop: vec![],
            restore_ttl: false,
        };
        let e = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: Some(cond),
            modifier: Some(modif),
        };
        assert!(e.condition.is_some());
        assert!(e.modifier.is_some());
    }

    /// Paket-5 T8 (A1): `contains_equal` is `true` for an edge with the SAME
    /// from/to and equal condition+modifier sources, `false` when condition or
    /// modifier differs (edge identity per spec Z.265 = from+to+cond+mod, not
    /// UUID). The duplicate carries a DIFFERENT UUID — proving identity is by
    /// content, not id.
    #[test]
    fn contains_equal_matches_on_identity_not_uuid() {
        let mut table = EdgeTable::new();
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop.insert("tier".into(), "'gold'".into());
        let base = || Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: Some(crate::cel_eval::parse_condition("hop.x == 'y'").unwrap()),
            modifier: Some(crate::cel_eval::parse_modifier(&spec).unwrap()),
        };
        table.insert(base());

        // Same identity, fresh UUID → present.
        assert!(
            table.contains_equal(&base()),
            "equal from/to/condition/modifier (different UUID) → contains_equal true"
        );

        // Differing condition → absent.
        let mut diff_cond = base();
        diff_cond.condition = Some(crate::cel_eval::parse_condition("hop.x == 'z'").unwrap());
        assert!(
            !table.contains_equal(&diff_cond),
            "differing condition source → contains_equal false"
        );

        // Differing modifier → absent.
        let mut other_spec = crate::config::ModifierSpec::default();
        other_spec.set_hop.insert("tier".into(), "'silver'".into());
        let mut diff_mod = base();
        diff_mod.modifier = Some(crate::cel_eval::parse_modifier(&other_spec).unwrap());
        assert!(
            !table.contains_equal(&diff_mod),
            "differing modifier source → contains_equal false"
        );

        // Identity (no condition, no modifier) is distinct from the conditional one.
        let identity = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        };
        assert!(
            !table.contains_equal(&identity),
            "None condition/modifier differs from Some(...) → contains_equal false"
        );
    }

    /// Phase 13.5-A1 T8.5: Cascade regression-watchdog — `dec.headers_out`
    /// from `apply_edges` propagates through `build_follow_up_with`-equivalent
    /// `MessageBuilder.headers(...)` into the final `msg.headers`. Replicates
    /// `colony.rs:1093 + Z.2097-2111` without spawning Colony so a future
    /// refactor that silently drops `dec.headers_out` in the outputs-arm
    /// trips this test.
    #[test]
    fn cascade_proof_modifier_set_propagates_to_follow_up_via_build_follow_up_with() {
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, Path, Uuid,
            serde_json::{Value, json},
        };

        // Edge with modifier.set_hop.
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop.insert("tier".into(), "'gold'".into());
        let m = crate::cel_eval::parse_modifier(&spec).unwrap();
        let edge = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: Some(m),
        };
        let mut table = EdgeTable::new();
        table.insert(edge);

        // Synthetic CellEmission analogous to the outputs-arm.
        let em = CellEmission {
            input_reply_to: None,
            sender_path: Path::new("/a"),
            target: Path::new("/b"),
            content: json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
            trace_id: Uuid::now_v7(),
            parent_message_id: None,
            input_headers: meclaw_core::Headers::new(),
            input_ttl: 64,
            direct_reply: false,
        };

        let merged_headers = em.input_headers.clone();
        let decisions = apply_edges(&table, &Path::new("/a"), &merged_headers);
        assert_eq!(decisions.len(), 1);
        let dec = decisions.into_iter().next().unwrap();

        // `build_follow_up_with` is private — replicate its behaviour:
        // MessageBuilder::new(dec.target).headers(dec.headers_out).body(...).build()
        let msg = MessageBuilder::new(dec.target.clone())
            .trace_id(em.trace_id)
            .parent_message_id_opt(em.parent_message_id)
            .reply_to(em.sender_path.clone())
            .ttl(em.input_ttl)
            .headers(dec.headers_out.clone())
            .body(Body::Inline(em.content.clone()))
            .build();

        // CASCADE-ASSERT: modifier.set lands in the `hop` compartment (Slice 2).
        assert_eq!(
            msg.headers.hop.get("tier"),
            Some(&Value::String("gold".into())),
            "modifier.set must propagate through dec.headers_out → MessageBuilder.headers → msg.headers.hop"
        );
        assert_eq!(msg.target, Path::new("/b"));
    }

    /// Phase 13.5-A1 T8.5: Cascade regression-watchdog for `modifier.delete`
    /// — `dec.headers_out` after delete propagates into `msg.headers` via
    /// `build_follow_up_with`-equivalent construction.
    #[test]
    fn cascade_proof_modifier_delete_propagates_to_follow_up() {
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, Path, Uuid,
            serde_json::{Map, Value, json},
        };

        let spec = crate::config::ModifierSpec {
            delete_hop: vec!["debug".into()],
            ..crate::config::ModifierSpec::default()
        };
        let m = crate::cel_eval::parse_modifier(&spec).unwrap();
        let edge = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: Some(m),
        };
        let mut table = EdgeTable::new();
        table.insert(edge);

        // `debug` and `keep` are hop-compartment keys (Cell-Produkt/Routing).
        let mut hop = Map::new();
        hop.insert("debug".into(), Value::String("on".into()));
        hop.insert("keep".into(), Value::String("yes".into()));
        let input_headers = meclaw_core::Headers::from_parts(Map::new(), hop);

        let em = CellEmission {
            input_reply_to: None,
            sender_path: Path::new("/a"),
            target: Path::new("/b"),
            content: json!({"messages":[]}),
            trace_id: Uuid::now_v7(),
            parent_message_id: None,
            input_headers: input_headers.clone(),
            input_ttl: 64,
            direct_reply: false,
        };

        let decisions = apply_edges(&table, &Path::new("/a"), &input_headers);
        assert_eq!(decisions.len(), 1);
        let dec = decisions.into_iter().next().unwrap();

        let msg = MessageBuilder::new(dec.target.clone())
            .trace_id(em.trace_id)
            .parent_message_id_opt(em.parent_message_id)
            .reply_to(em.sender_path.clone())
            .ttl(em.input_ttl)
            .headers(dec.headers_out.clone())
            .body(Body::Inline(em.content.clone()))
            .build();

        // CASCADE-ASSERT: "debug" removed from hop, "keep" remains.
        assert!(
            msg.headers.hop.get("debug").is_none(),
            "modifier.delete must propagate through dec.headers_out → msg.headers.hop"
        );
        assert_eq!(
            msg.headers.hop.get("keep"),
            Some(&Value::String("yes".into()))
        );
    }
}

#[cfg(test)]
mod hook_tests {
    use super::*;
    use meclaw_core::Uuid;

    fn headers() -> Headers {
        let mut ctx = meclaw_core::serde_json::Map::new();
        ctx.insert("session_id".into(), meclaw_core::serde_json::json!("s1"));
        Headers::from_parts(ctx, meclaw_core::serde_json::Map::new())
    }

    #[test]
    fn evaluate_edge_returns_take_with_identity_headers() {
        let e = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        };
        let h = headers();
        let dec = evaluate_edge(&e, &h).unwrap();
        assert_eq!(dec.target.as_str(), "/b");
        assert_eq!(dec.headers_out, h);
    }

    #[test]
    fn apply_edges_returns_empty_when_no_match() {
        let table = EdgeTable::new();
        let r = apply_edges(&table, &Path::new("/a"), &headers());
        assert!(r.is_empty());
    }

    #[test]
    fn apply_edges_returns_single_for_single_match() {
        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        });
        let r = apply_edges(&table, &Path::new("/a"), &headers());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].target.as_str(), "/b");
    }

    #[test]
    fn apply_edges_returns_all_for_fan_out() {
        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: None,
            modifier: None,
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/c"),
            condition: None,
            modifier: None,
        });
        let r = apply_edges(&table, &Path::new("/a"), &headers());
        assert_eq!(r.len(), 2);
    }

    /// GH #283: an unconditional out-edge is an ALWAYS edge, not a default.
    ///
    /// `apply_edges` produces one decision per edge whose condition evaluates
    /// true, and `condition: None` means *always take*. So an unconditional
    /// edge fires **beside** every matching edge, never only when none of them
    /// matched -- there is no fallback construct in the substrate today. The
    /// fan-out test above uses two unconditional edges and therefore proves
    /// nothing about the mixed case; this one does.
    #[test]
    fn an_unconditional_edge_fires_beside_a_matching_one() {
        fn hop_tool_name(name: &str) -> Headers {
            let mut hop = meclaw_core::serde_json::Map::new();
            hop.insert(
                "tool_name".into(),
                meclaw_core::serde_json::Value::String(name.into()),
            );
            Headers::from_parts(meclaw_core::serde_json::Map::new(), hop)
        }

        let mut table = EdgeTable::new();
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: Some(
                crate::cel_eval::parse_condition("hop.tool_name == 'alpha'")
                    .expect("condition should parse"),
            ),
            modifier: None,
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/c"),
            condition: None,
            modifier: None,
        });

        let matched = apply_edges(&table, &Path::new("/a"), &hop_tool_name("alpha"));
        let matched_targets: Vec<&str> = matched.iter().map(|d| d.target.as_str()).collect();
        assert_eq!(
            matched_targets,
            vec!["/b", "/c"],
            "the unconditional edge fires BESIDE the matching one, not instead of it"
        );

        let unmatched = apply_edges(&table, &Path::new("/a"), &hop_tool_name("gamma"));
        let unmatched_targets: Vec<&str> = unmatched.iter().map(|d| d.target.as_str()).collect();
        assert_eq!(
            unmatched_targets,
            vec!["/c"],
            "with no other arm matching, only the unconditional edge remains"
        );
    }

    /// Phase 13.5-A1 T3: condition=false → edge skipped.
    #[test]
    fn evaluate_edge_skips_when_condition_false() {
        let cond = crate::cel_eval::parse_condition("hop.tier == 'gold'").unwrap();
        let e = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: Some(cond),
            modifier: None,
        };
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert(
            "tier".into(),
            meclaw_core::serde_json::Value::String("standard".into()),
        );
        let h = Headers::from_parts(meclaw_core::serde_json::Map::new(), hop);
        assert!(
            evaluate_edge(&e, &h).is_none(),
            "edge with false condition should be skipped"
        );
    }

    /// Phase 13.5-A1 T3: condition=true → edge taken with identity headers.
    #[test]
    fn evaluate_edge_takes_when_condition_true() {
        let cond = crate::cel_eval::parse_condition("hop.tier == 'gold'").unwrap();
        let e = Edge {
            id: Uuid::now_v7(),
            from: Path::new("/a"),
            to: Path::new("/b"),
            condition: Some(cond),
            modifier: None,
        };
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert(
            "tier".into(),
            meclaw_core::serde_json::Value::String("gold".into()),
        );
        let h = Headers::from_parts(meclaw_core::serde_json::Map::new(), hop);
        let dec = evaluate_edge(&e, &h).expect("edge should match");
        assert_eq!(dec.target.as_str(), "/b");
    }

    /// Phase 13.5-A1 T9 (F3-PIN): undefined-header access in a condition →
    /// CEL eval error → edge skipped. The spec is silent here; ruling A1 = the CEL
    /// standard (NOT a crash, NOT the DLQ).
    #[test]
    fn evaluate_edge_skips_when_condition_reads_undefined_header_f3() {
        let cond = crate::cel_eval::parse_condition("hop.never_was == 'x'").unwrap();
        let edge = Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: meclaw_core::Path::new("/a"),
            to: meclaw_core::Path::new("/b"),
            condition: Some(cond),
            modifier: None,
        };
        let result = evaluate_edge(&edge, &Headers::new());
        assert!(
            result.is_none(),
            "F3-PIN: undefined header access in condition → edge skipped (CEL-standard)"
        );
    }

    /// GH #82 (ruling 2026-08-13): a `modifier.restore_ttl` edge REPORTS the
    /// declaration on its decision — `ttl` is envelope, so the edge never writes
    /// it; the colony reads the flag off the decision and stamps the follow-up.
    /// An edge without the flag (and an edge without a modifier at all) reports
    /// `false`, so nothing restores by accident.
    #[test]
    fn evaluate_edge_reports_restore_ttl_from_the_modifier() {
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_context
            .insert("iter".into(), "int(context.iter) + 1".into());
        spec.restore_ttl = true;
        let restoring = Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: meclaw_core::Path::new("/collector"),
            to: meclaw_core::Path::new("/planner"),
            condition: None,
            modifier: Some(crate::cel_eval::parse_modifier(&spec).unwrap()),
        };
        let mut ctx = meclaw_core::serde_json::Map::new();
        ctx.insert("iter".into(), meclaw_core::serde_json::json!("0"));
        let h = Headers::from_parts(ctx, meclaw_core::serde_json::Map::new());
        let dec = evaluate_edge(&restoring, &h).expect("edge takes");
        assert!(dec.restore_ttl, "the decision carries the declaration");

        let plain = Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: meclaw_core::Path::new("/collector"),
            to: meclaw_core::Path::new("/planner"),
            condition: None,
            modifier: None,
        };
        assert!(
            !evaluate_edge(&plain, &h).expect("edge takes").restore_ttl,
            "an edge without the modifier must never restore"
        );
    }

    /// GH #82: edge identity is the serde-JSON of the `ModifierSpec` source
    /// (`contains_equal`, `EdgeMatchView`) and the durable-edge round-trip
    /// re-parses exactly that. The new field must therefore be INVISIBLE when it
    /// is false — otherwise every pre-existing edge silently changes identity.
    #[test]
    fn restore_ttl_default_is_omitted_from_the_serialised_modifier_source() {
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop.insert("tier".into(), "'gold'".into());
        let json = meclaw_core::serde_json::to_string(&spec).unwrap();
        assert_eq!(
            json, r#"{"set_hop":{"tier":"'gold'"}}"#,
            "a non-restoring modifier must serialise exactly as before"
        );
        spec.restore_ttl = true;
        assert_eq!(
            meclaw_core::serde_json::to_string(&spec).unwrap(),
            r#"{"set_hop":{"tier":"'gold'"},"restore_ttl":true}"#,
            "a restoring modifier carries the declaration into its identity"
        );
    }

    /// Phase 13.5-A1 T7: modifier.set inserts a new header into the edge's
    /// outgoing headers (per spec Z.829: set is applied after condition).
    #[test]
    fn apply_edges_with_modifier_set_emits_new_header() {
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop.insert("tier".into(), "'gold'".into());
        let m = crate::cel_eval::parse_modifier(&spec).unwrap();
        let edge = Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: meclaw_core::Path::new("/a"),
            to: meclaw_core::Path::new("/b"),
            condition: None,
            modifier: Some(m),
        };
        let mut table = EdgeTable::new();
        table.insert(edge);
        let decisions = apply_edges(&table, &meclaw_core::Path::new("/a"), &Headers::new());
        assert_eq!(decisions.len(), 1);
        assert_eq!(
            decisions[0].headers_out.hop.get("tier"),
            Some(&meclaw_core::serde_json::Value::String("gold".into()))
        );
    }
}
