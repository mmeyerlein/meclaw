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
/// template, the public export a subset — `PUBLIC_TEMPLATES` in the
/// maintainers' export script), so a count that is honest in one
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
            is_default: false,
            lane: None,
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
        context: l.context.clone(),
        at: Vec::new(),
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

/// Driver ruling **W7-R1** (GH #286, 2026-08-25) — the inward mirror of the
/// GH #176 carve-out above: a door that narrows WITHIN a declared lane on a key
/// the bare route probe cannot carry.
///
/// `probe(route)` builds a hop that holds `route` and nothing else. A door
/// conditioned on `has(hop.tool_name) && hop.tool_name == 'bash'` can therefore
/// never fire under it — not because the lane is undeclared, but because the
/// probe is not the message the condition is about. The sweep below would read
/// that as "opens on a lane no `accepts` entry names", which is a false
/// positive: the lane IS declared (`tool_call`), and the condition only decides
/// WHICH occupant serves it.
///
/// The emit side has had this exemption since GH #176 — `edge_states_lane`'s
/// third verdict, *an expression that reads the message is not judged and
/// counts*. The accepts side did not, and that asymmetry was the defect. This
/// function closes it, and it is deliberately narrower than the emit-side one:
/// a door with no condition, or one that reads only `hop.route`, is still put
/// through the router unchanged.
///
/// **The gate gives up no check it could have made alone.** The substrate keeps
/// judging these doors at mutation time, where the probe is not a bare route
/// either: `hive_contract::door_exists` runs the same lane through the real
/// `apply_edges` against the real edge table, defaults and all.
///
/// The case that forced it, and the proof that the exemption still bites — a
/// door discriminating on `hop.route` itself is condemned as before — live in
/// `crates/meclaw-cells/tests/gh286_one_call_reaches_exactly_one_tool.rs`,
/// module `door_sweep`. **A change here belongs in both files:** integration
/// tests compile into separate binaries, so that module is a reconstruction of
/// this one rather than a caller of it.
fn condition_reads_a_hop_key_the_probe_cannot_carry(
    spec: &meclaw_colony::config::EdgeSpec,
) -> bool {
    let Some(src) = spec.condition.as_deref() else {
        return false;
    };
    let mut rest = src;
    while let Some(at) = rest.find("hop.") {
        let after = &rest[at + "hop.".len()..];
        let key: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !key.is_empty() && key != "route" {
            return true;
        }
        rest = &after[key.len()..];
    }
    false
}

/// GH #469 — the outward mirror of
/// [`condition_reads_a_hop_key_the_probe_cannot_carry`]: an exit that narrows
/// WITHIN a declared lane on a PROMOTED `context` key the bare probe cannot
/// carry.
///
/// `probe(route)` builds an EMPTY context compartment. An exit conditioned on
/// `has(context.build_caller) && context.build_caller == 'operator'` can
/// therefore never fire under it — not because the lane is undeclared, but
/// because the probe is not the message the condition is about. `meclaw-os`
/// stamps that key at its own door and tells the baumeister's two answers
/// apart by it; both answers travel a lane the level declares, and only the
/// DESTINATION differs. Reading such an edge as "carries a lane no `emits`
/// entry names" is the same false positive W7-R1 closed on the inward half,
/// one compartment over.
///
/// Deliberately narrow, exactly like its inward twin: an exit with no
/// condition, or one that reads only `hop.*`, still goes through the router
/// unchanged, so an exit whose LANE is genuinely undeclared is condemned as
/// before. Two other readers keep the case honest — the substrate's own
/// `hive_contract::exit_exists` at mutation time, and
/// `gh302_meclaw_os_shell::every_edge_is_a_door_or_an_exit_and_every_one_carries_a_declared_lane`,
/// which probes the shell's edges WITH the context each one demands rather
/// than exempting them.
fn condition_reads_a_context_key(spec: &meclaw_colony::config::EdgeSpec) -> bool {
    let Some(src) = spec.condition.as_deref() else {
        return false;
    };
    let mut rest = src;
    while let Some(at) = rest.find("context.") {
        let after = &rest[at + "context.".len()..];
        let key: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !key.is_empty() {
            return true;
        }
        rest = after;
    }
    false
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
            }) || condition_reads_a_hop_key_the_probe_cannot_carry(spec);
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
            }) || condition_reads_a_context_key(spec);
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

// ── GH #368/#366 — the bundle reply is a property of the CELL TYPE `store` ───

/// Every `config.json` in the shipped tree whose cell type is `store`, as
/// (relative directory, its `contract.emits`). Discovered by walking the tree,
/// never from a list: a template added tomorrow has to be caught by the sweep
/// that runs today, which is the whole point of a drift lock.
fn shipped_stores() -> Vec<(String, Value)> {
    let root = templates_root();
    let mut out: Vec<(String, Value)> = Vec::new();
    walk_stores(&root, &root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    // The floor, derived the way GH #173's is: a second, independent, NON-
    // recursive read of `templates/*/*/config.json`. Every store one level under
    // a template must turn up in the recursive walk — a walk that lost a subtree
    // loses these first — and a tree with no store at all is a wrong root, not a
    // small library. Today the floor happens to be the whole set: a composite
    // template names its sub-templates with a `cell.type: "ref"` marker instead
    // of copying their cell directories, so no store config sits deeper than
    // `templates/*/*/`. The floor is a lower bound, not an equality assertion —
    // a template that ever does nest a store one level further is caught by the
    // walk alone, and this read stays the independent witness that the walk runs
    // at all.
    let mut floor: Vec<String> = Vec::new();
    for template in std::fs::read_dir(&root).unwrap() {
        let template = template.unwrap().path();
        if !template.is_dir() {
            continue;
        }
        for cell in std::fs::read_dir(&template).unwrap() {
            let dir = cell.unwrap().path();
            if is_store_config(&dir.join("config.json")).is_some() {
                floor.push(dir.strip_prefix(&root).unwrap().display().to_string());
            }
        }
    }
    assert!(
        !floor.is_empty(),
        "no store under {} at all — wrong root",
        root.display()
    );
    let found: std::collections::HashSet<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    let missed: Vec<&String> = floor
        .iter()
        .filter(|f| !found.contains(f.as_str()))
        .collect();
    assert!(
        missed.is_empty(),
        "the sweep walked past shipped stores: {missed:?}"
    );
    out
}

/// `Some(contract.emits)` iff this path is a `config.json` declaring a `store`.
/// A store without a `contract` at all yields the empty object, so the sweep
/// below reports it as a missing declaration rather than skipping it.
fn is_store_config(p: &std::path::Path) -> Option<Value> {
    let raw = std::fs::read_to_string(p).ok()?;
    let val: Value =
        meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let is_store = val
        .get("cell")
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        == Some("store");
    is_store.then(|| {
        val.get("contract")
            .and_then(|c| c.get("emits"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()))
    })
}

fn walk_stores(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Value)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            walk_stores(root, &p, out);
            continue;
        }
        if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
            continue;
        }
        if let Some(emits) = is_store_config(&p) {
            let rel = p.strip_prefix(root).unwrap().parent().unwrap();
            out.push((rel.display().to_string(), emits));
        }
    }
}

/// GH #368 — a `store` cell answers N operations in ONE bundle reply (GH #295)
/// and stamps `duration_ms` on every reply it has ever sent. Fifteen of the
/// sixteen shipped stores still declared the pre-bundle contract when #368 found
/// them, because each template was written against the store of its own day and
/// nothing re-read the older ones afterwards.
///
/// This is the lock against the next one rotting the same way: the sweep finds
/// the store configs itself, so a template added tomorrow is checked by the test
/// written today. `emits` is the promise surface a reader wires against, and
/// `emits.hop` is what the locality validator reads to decide whether a
/// downstream edge may condition on a header — an undeclared key is a header no
/// edge may name.
#[test]
fn every_shipped_store_tells_the_bundle_truth() {
    let stores = shipped_stores();
    for (name, emits) in &stores {
        let declared = |compartment: &str, key: &str| -> bool {
            emits.get(compartment).and_then(|c| c.get(key)).is_some()
        };
        // `rows_affected` rides along because it is the key #368 found furthest
        // out: `builder-librarian`'s store had never declared it at all — the
        // very number a bundle reply sums. A lock that checks only the two keys
        // the issue was named after lets that one rot back to green.
        for key in ["operation", "rows_affected", "duration_ms", "bundle_errors"] {
            assert!(
                declared("hop", key),
                "{name}: contract.emits.hop does not declare `{key}` — the store cell \
                 stamps it and an undeclared header is one no edge may condition on"
            );
        }
        assert!(
            declared("body", "results"),
            "{name}: contract.emits.body does not declare the bundle slot `results` — \
             the cell writes it on every bundle reply (GH #295)"
        );
        // GH #369 — `operation` is the one hop key of the five that is not
        // optional. Every emit path of the `store` cell stamps it, the error
        // surface included (GH #331), and since GH #370 the degraded substitute
        // does too. `required: false` on it would say a reply may arrive without
        // the field, which is untrue and is exactly what a return edge
        // conditioned on `hop.operation` reads to decide it may not be relied on.
        assert_eq!(
            emits
                .get("hop")
                .and_then(|h| h.get("operation"))
                .and_then(|o| o.get("required")),
            Some(&Value::Bool(true)),
            "{name}: contract.emits.hop.operation is not `required: true` — every \
             store reply carries it, the error surface included (GH #331/#370)"
        );
    }
    assert!(
        stores.len() >= 8,
        "the sweep checked almost nothing: {} stores",
        stores.len()
    );
}
