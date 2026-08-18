//! GH #173 — the shipped hive templates state their interface in lanes, and the
//! statement is true of their own graph.
//!
//! A colony-side check (`meclaw-colony`, `mutation::hive_contract`) can only see
//! a hive once it has been instantiated. A template is a class before that, and
//! whether a class's declared interface matches its implementation is a fact
//! about the FILE — checkable here, with no colony and no runtime.
//!
//! Three properties, in the order they matter:
//!
//! 1. **A sealed hive declares one.** `ports: []` plus `{"from": "."}` doors is
//!    a template that has been brought behind its boundary (overview § Die
//!    Hive-Grenze). That is exactly the template a caller now addresses by path
//!    and lane, so it is exactly the template that owes a contract.
//! 2. **Every declared lane exists.** Run through the real router, against the
//!    template's own `params.graph`.
//! 3. **Every lane the graph opens is declared.** The other direction, and the
//!    one that keeps a contract from being a truthful half-story: a door the
//!    contract does not mention is an undocumented lane, which is where the
//!    prose-in-`description` era started.
//!
//! It also refuses the one mistake that would undo the whole change: a route
//! that is really a cell name.

use meclaw_colony::config::HiveParams;
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::mutation::hive_contract::{HiveContract, Lane, check_lane_doors};
use meclaw_core::serde_json::Value;
use meclaw_core::{Headers, Path, Uuid};

/// Where the synthetic hive lives while it is being checked. Any path does; the
/// point is that endpoints resolve the way the colony resolves them.
const HIVE: &str = "/h";

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The top-level template directories whose OWN `config.json` is a hive with
/// `params` — read straight out of the root, with none of the recursion the
/// sweep uses.
///
/// This is the sweep's floor, and it is derived rather than declared. A number
/// cannot be: the library ships in two sizes (the private tree carries every
/// template, the public export a subset — `PUBLIC_TEMPLATES` in
/// `plans/export-fixtures/make_export.py`), so a count that is honest in one
/// tree is either red or vacuous in the other, and two hard-coded numbers are
/// two chances to pick the wrong one. What the floor is actually for survives
/// the derivation intact: the sweep must not be able to pass by finding
/// nothing. Every hive template the tree carries at its top level has to turn
/// up in the recursive walk, measured by a second, independent read of the same
/// directory — a walk that lost a subtree loses these first.
fn root_hive_templates() -> Vec<String> {
    let root = templates_root();
    let mut out: Vec<String> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .filter_map(|entry| {
            let p = entry.unwrap().path();
            let raw = std::fs::read_to_string(p.join("config.json")).ok()?;
            let val: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
            let is_hive = val
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(|t| t.as_str())
                == Some("hive");
            let has_params = val.get("params").is_some_and(|p| !p.is_null());
            (is_hive && has_params).then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out
}

/// Every `config.json` in the shipped tree whose cell type is `hive`, with its
/// parsed `params`. Sub-copies inside composite templates ride along on purpose:
/// a copy that drifted from its original is a copy with a different contract.
fn shipped_hives() -> Vec<(String, HiveParams)> {
    let mut out = Vec::new();
    walk(&templates_root(), &templates_root(), &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));

    let roots = root_hive_templates();
    // A library with no hive template at all is a root the sweep was pointed at
    // wrongly, not a tree that happens to be small. That case is a failure in
    // BOTH trees, and it is the only thing the derived floor cannot say by
    // itself.
    assert!(
        !roots.is_empty(),
        "no hive template under {} at all — wrong root",
        templates_root().display()
    );
    let found: std::collections::HashSet<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    let missed: Vec<&String> = roots
        .iter()
        .filter(|r| !found.contains(r.as_str()))
        .collect();
    assert!(
        missed.is_empty(),
        "the sweep walked past hive templates this tree carries: {missed:?} \
         (found {} hive configs in total)",
        out.len()
    );
    out
}

fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, HiveParams)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            walk(root, &p, out);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap();
        let val: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if val
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            != Some("hive")
        {
            continue;
        }
        let params = val.get("params").cloned().unwrap_or(Value::Null);
        if params.is_null() {
            continue;
        }
        let hp: HiveParams = meclaw_core::serde_json::from_value(params)
            .unwrap_or_else(|e| panic!("{}: params: {e}", p.display()));
        let rel = p.strip_prefix(root).unwrap().parent().unwrap();
        out.push((rel.display().to_string(), hp));
    }
}

/// The template's `params.graph`, resolved into the edge table the colony would
/// build from it: `.` is the hive itself, `./x` a child of the hive.
fn table_for(hp: &HiveParams) -> EdgeTable {
    let abs = |ep: &str| -> String {
        match ep {
            "." => HIVE.to_string(),
            other => format!("{HIVE}/{}", other.trim_start_matches("./")),
        }
    };
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
        });
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition,
            // Modifiers do not decide WHETHER an edge is taken, and every set
            // expression here reads keys a bare route probe does not carry.
            modifier: None,
        });
    }
    t
}

fn probe(route: &str) -> Headers {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), Value::String(route.into()));
    Headers::from_parts(meclaw_core::serde_json::Map::new(), hop)
}

fn contract_of(hp: &HiveParams) -> Option<HiveContract> {
    let spec = hp.contract.as_ref()?;
    let lane = |l: &meclaw_colony::config::LaneSpec| Lane {
        route: l.route.clone(),
        because: l.because.clone(),
    };
    Some(HiveContract {
        hive_path: HIVE.to_string(),
        accepts: spec.accepts.iter().map(lane).collect(),
        emits: spec.emits.iter().map(lane).collect(),
    })
}

/// A template that has been brought behind its boundary: sealed to its own path
/// and distributing inbound traffic itself.
fn is_sealed_with_doors(hp: &HiveParams) -> bool {
    hp.ports.as_ref().is_some_and(|p| p.is_empty()) && hp.graph.edges.iter().any(|e| e.from == ".")
}

/// GH #176 — the second way an edge can be an exit: it CREATES the lane on its
/// way out instead of carrying it.
///
/// A failure lane recognises something only the inside knows (an `llm` cell's
/// `hop.finish_reason`, a store echo's `hop.operation`, a `hop.msg_type`) and
/// TRANSLATES it into a route on the boundary edge. A probe carrying
/// `hop.route` never satisfies such a condition, so the router alone cannot see
/// the exit. The substrate reads the edge's own `set_hop.route` for exactly
/// this case (`hive_contract::door_states_lane`); the template sweep has to
/// read it the same way, or it refuses a lane a live colony accepts.
///
/// Mirrors the substrate's three verdicts: no `set_hop.route` names no lane; a
/// constant that is a different lane names someone else's; an expression that
/// reads the message is not judged and counts.
fn edge_states_lane(spec: &meclaw_colony::config::EdgeSpec, route: &str) -> bool {
    let Some(src) = spec.modifier.as_ref().and_then(|m| m.set_hop.get("route")) else {
        return false;
    };
    match constant_route(src) {
        Some(stated) => stated == route,
        None => true,
    }
}

/// A single-quoted CEL string literal, or `None` for anything computed.
fn constant_route(src: &str) -> Option<&str> {
    let t = src.trim();
    let inner = t.strip_prefix('\'')?.strip_suffix('\'')?;
    (!inner.contains('\'')).then_some(inner)
}

/// True iff SOME edge crossing the hive path outward names `route` on itself.
fn an_exit_names_lane(hp: &HiveParams, route: &str) -> bool {
    hp.graph
        .edges
        .iter()
        .any(|e| e.to == "." && edge_states_lane(e, route))
}

#[test]
fn every_sealed_hive_template_declares_its_lanes() {
    let missing: Vec<String> = shipped_hives()
        .into_iter()
        .filter(|(_, hp)| is_sealed_with_doors(hp) && hp.contract.is_none())
        .map(|(name, _)| name)
        .collect();
    assert!(
        missing.is_empty(),
        "these templates are addressed by path and lane but say nowhere which lanes: {missing:?}"
    );
}

#[test]
fn every_declared_lane_has_a_door_in_the_templates_own_graph() {
    let mut checked = 0usize;
    for (name, hp) in shipped_hives() {
        let Some(mut c) = contract_of(&hp) else {
            continue;
        };
        // An emit the boundary edge NAMES is already accounted for (GH #176),
        // and the router cannot be asked about it: the probe would have to
        // satisfy a condition written about the hive's insides. Everything else
        // goes through the real check unchanged.
        c.emits.retain(|l| !an_exit_names_lane(&hp, &l.route));
        check_lane_doors(std::slice::from_ref(&c), &table_for(&hp))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        checked += 1;
    }
    assert!(checked >= 8, "the sweep checked almost nothing: {checked}");
}

#[test]
fn every_lane_the_graph_opens_is_declared() {
    // The reverse direction. A door the contract does not mention is a lane a
    // caller can only learn about by reading the inside — which is the practice
    // this whole change exists to end.
    for (name, hp) in shipped_hives() {
        let Some(c) = contract_of(&hp) else { continue };
        let table = table_for(&hp);
        let hive = Path::new(HIVE);
        for spec in hp.graph.edges.iter().filter(|e| e.from == ".") {
            let covered = c.accepts.iter().any(|l| {
                apply_edges(&table, &hive, &probe(&l.route))
                    .iter()
                    .any(|d| {
                        d.target.as_str() == format!("{HIVE}/{}", spec.to.trim_start_matches("./"))
                    })
            });
            assert!(
                covered,
                "{name}: the door {} -> {} opens on a lane no `accepts` entry names",
                spec.from, spec.to
            );
        }
        for spec in hp.graph.edges.iter().filter(|e| e.to == ".") {
            let src = Path::new(&format!("{HIVE}/{}", spec.from.trim_start_matches("./")));
            let covered = c.emits.iter().any(|l| {
                edge_states_lane(spec, &l.route)
                    || apply_edges(&table, &src, &probe(&l.route))
                        .iter()
                        .any(|d| d.target.as_str() == HIVE)
            });
            assert!(
                covered,
                "{name}: the exit {} -> {} carries a lane no `emits` entry names",
                spec.from, spec.to
            );
        }
    }
}

#[test]
fn a_lane_is_a_route_never_a_cell_name() {
    // The failure mode that would put the boundary back where it was: declaring
    // `./writer` as if it were a lane. `params.ports` already made that mistake
    // once (GH #173, finding 3).
    for (name, hp) in shipped_hives() {
        let Some(spec) = hp.contract.as_ref() else {
            continue;
        };
        for l in spec.accepts.iter().chain(spec.emits.iter()) {
            assert!(
                !l.route.is_empty()
                    && !l.route.contains('/')
                    && !l.route.starts_with('.')
                    && !l.because.is_empty(),
                "{name}: '{}' is a path, not a lane",
                l.route
            );
        }
    }
}
