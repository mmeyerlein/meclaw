//! GH #1157 — the lane-door check, made local per contract, judges exactly as
//! the global one did.
//!
//! The check used to run the router's fan-out ([`run_probe`]) from EVERY
//! interior source of a hive for every emitted lane, and it reads every edge of
//! the colony per contract. That made each mutation pay for the size of the
//! colony. The local form asks only the sources that own an edge onto the hive
//! path — a decision can only land where one of the source's own out-edges
//! points, so no other source can ever satisfy the exit — and it reads the
//! edges onto a hive path from one inbound map per check.
//!
//! Three pins:
//! - (a) EQUIVALENCE: the frozen pre-#1157 implementation below
//!   ([`reference_lane_doors`]) and the live one return the SAME violations,
//!   in the same order, across every mutation form — named cases and a
//!   deterministic random walk over a pool of edges — including a pre-state
//!   whose standing violation must refuse every mutation before and after.
//! - (b) ADVERSARIAL: a check that skipped a foreign hive, or a source whose
//!   exit only fires in the default phase, goes red here.
//! - (c) COST: the number of probe runs does not grow with the colony's edges
//!   or sources, only with the number of contracts.

use super::*;
use crate::cel_eval::parse_condition;
use crate::edge_table::Edge;
use meclaw_core::Uuid;

// ---- the frozen reference: the lane-door core as it stood at 28c948e15 ----

fn reference_hive_path_is_wired(c: &HiveContract, edges: &EdgeTable) -> bool {
    edges
        .iter()
        .any(|e| e.from.as_str() == c.hive_path || e.to.as_str() == c.hive_path)
}

fn reference_door_exists(c: &HiveContract, route: &str, edges: &EdgeTable) -> bool {
    let hive = Path::new(&c.hive_path);
    crate::edge_table::apply_edges(edges, &hive, &HiveContract::probe(route))
        .iter()
        .any(|d| c.is_interior(d.target.as_str()))
        || edges
            .edges_from(&hive)
            .iter()
            .any(|e| c.is_interior(e.to.as_str()) && door_names_lane(e, route))
}

fn reference_exit_exists(c: &HiveContract, route: &str, edges: &EdgeTable) -> bool {
    if edges.iter().any(|e| {
        c.is_interior(e.from.as_str()) && e.to.as_str() == c.hive_path && door_states_lane(e, route)
    }) {
        return true;
    }
    let probe = HiveContract::probe(route);
    let mut sources: Vec<&Path> = edges
        .iter()
        .filter(|e| c.is_interior(e.from.as_str()))
        .map(|e| &e.from)
        .collect();
    sources.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    sources.dedup_by(|a, b| a.as_str() == b.as_str());
    sources.into_iter().any(|src| {
        crate::edge_table::apply_edges(edges, src, &probe)
            .iter()
            .any(|d| d.target.as_str() == c.hive_path)
    })
}

fn reference_lane_doors(
    contracts: &[HiveContract],
    edges: &EdgeTable,
) -> Vec<(MutationError, String, String)> {
    let mut violations = Vec::new();
    for c in contracts {
        if !reference_hive_path_is_wired(c, edges) {
            continue;
        }
        for lane in &c.accepts {
            if docks_below_the_rim(lane) || reference_door_exists(c, &lane.route, edges) {
                continue;
            }
            violations.push((
                MutationError::HiveContract(format!(
                    "hive '{hive}' declares it accepts the lane '{route}' ({because}), but \
                     nothing routes a message arriving at '{hive}' with hop.route='{route}' to a \
                     cell inside it. Either the contract names a lane the hive lost, or the door \
                     edge {{\"from\": \".\"}} is missing.",
                    hive = c.hive_path,
                    route = lane.route,
                    because = lane.because,
                )),
                c.hive_path.clone(),
                lane.because.clone(),
            ));
        }
        for lane in &c.emits {
            if docks_below_the_rim(lane) || reference_exit_exists(c, &lane.route, edges) {
                continue;
            }
            violations.push((
                MutationError::HiveContract(format!(
                    "hive '{hive}' declares it emits the lane '{route}' ({because}), but no cell \
                     inside it routes a message with hop.route='{route}' back out through \
                     '{hive}'. A lane a caller cannot receive is not part of an interface.",
                    hive = c.hive_path,
                    route = lane.route,
                    because = lane.because,
                )),
                c.hive_path.clone(),
                lane.because.clone(),
            ));
        }
    }
    violations
}

// ---- fixture ----

/// One edge of the pool: `from`, `to`, condition, stamped route, phase.
#[derive(Clone, Copy)]
struct Spec {
    from: &'static str,
    to: &'static str,
    cond: Option<&'static str>,
    stamp: Option<&'static str>,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Regular,
    Default,
    Tap,
}

const fn s(from: &'static str, to: &'static str, cond: Option<&'static str>) -> Spec {
    Spec {
        from,
        to,
        cond,
        stamp: None,
        kind: Kind::Regular,
    }
}

const fn stamp(
    from: &'static str,
    to: &'static str,
    cond: Option<&'static str>,
    route: &'static str,
) -> Spec {
    Spec {
        from,
        to,
        cond,
        stamp: Some(route),
        kind: Kind::Regular,
    }
}

const fn phase(mut spec: Spec, kind: Kind) -> Spec {
    spec.kind = kind;
    spec
}

fn build(spec: &Spec) -> Edge {
    let modifier = spec.stamp.map(|route| {
        let mut m = crate::config::ModifierSpec::default();
        m.set_hop.insert("route".to_string(), route.to_string());
        crate::cel_eval::parse_modifier(&m).expect("modifier parses")
    });
    Edge {
        id: Uuid::now_v7(),
        from: Path::new(spec.from),
        to: Path::new(spec.to),
        condition: spec
            .cond
            .map(|c| parse_condition(c).expect("condition parses")),
        modifier,
        is_default: spec.kind == Kind::Default,
        lane: None,
        tap: spec.kind == Kind::Tap,
    }
}

/// The colony every case starts from: five contracted hives (one nested in
/// another, one asleep on an island), each sound, plus wiring far away.
const BASE: &[Spec] = &[
    stamp("/caller", "/mem", None, "'in_batch'"),
    s(
        "/mem",
        "/mem/glue",
        Some("has(hop.route) && hop.route.startsWith('in_')"),
    ),
    s(
        "/mem/glue",
        "/mem",
        Some("has(hop.route) && !hop.route.startsWith('in_')"),
    ),
    s("/mem/glue", "/mem/log", None),
    s("/mem/log", "/mem/archive", Some("hop.route == 'episode'")),
    s("/app", "/app/front", Some("hop.route == 'ask'")),
    stamp("/app/front", "/app/inner", None, "'work'"),
    s(
        "/app/inner",
        "/app/inner/worker",
        Some("hop.route == 'work'"),
    ),
    s(
        "/app/inner/worker",
        "/app/inner",
        Some("hop.route == 'done'"),
    ),
    s("/app/inner", "/app/back", Some("hop.route == 'done'")),
    s("/app/back", "/app", Some("hop.route == 'answer'")),
    s("/gate", "/gate/s", Some("hop.route == 'in'")),
    phase(s("/gate/s", "/gate", None), Kind::Default),
    s("/gate/s", "/gate/t", Some("hop.route == 'never'")),
    s("/stamp", "/stamp/llm", Some("hop.route == 'go'")),
    stamp(
        "/stamp/llm",
        "/stamp",
        Some("has(hop.finish_reason) && hop.finish_reason == 'error'"),
        "'failed'",
    ),
    s("/a", "/b", None),
    s("/b", "/c", Some("hop.route == 'x'")),
];

/// Edges a mutation may add (or, once added, take away again).
const EXTRA: &[Spec] = &[
    // breaks ANOTHER hive's exit: a regular hit suppresses /gate's default exit
    s("/gate/s", "/gate/t", None),
    s("/a", "/c", None),
    // a node is born: /n, wired in and out
    s("/a", "/n", None),
    s("/n", "/b", None),
    // edges across a hive boundary that miss the rim
    s("/mem/glue", "/sink", None),
    s("/caller", "/mem/glue", None),
    // a second exit source, and one that crosses two rims at once
    s("/mem/log", "/mem", Some("hop.route == 'episode'")),
    s("/app/inner/worker", "/app", Some("hop.route == 'answer'")),
    s("/app", "/app/inner", Some("hop.route == 'work'")),
    // the island hive wakes up, and gets a door
    s("/x", "/island", None),
    s(
        "/island",
        "/island/c",
        Some("hop.route == 'z' && has(hop.route)"),
    ),
    s("/island/c", "/island", Some("hop.route == 'z2'")),
    // a computed exit, a self edge on the rim, a tap onto the rim
    stamp("/stamp/llm", "/stamp", None, "hop.upstream_route"),
    s("/mem", "/mem", Some("hop.route == 'in_batch'")),
    phase(s("/gate/s", "/gate", Some("hop.route == 'out'")), Kind::Tap),
    // a newborn hive /n with a contract and no door (see `contracts`)
    s("/n", "/n/inner", Some("hop.route == 'other'")),
    // edges from OUTSIDE that stamp the emit lane onto the rim (constant and
    // computed): never an exit, the source is not interior
    stamp("/caller", "/mem", None, "'episode'"),
    stamp("/x", "/app/inner", None, "hop.upstream_route"),
    // a door the CEL probe misses but the syntax names (`has()` after the lane)
    s(
        "/gate",
        "/gate/s",
        Some("hop.route == 'in' && has(hop.organ) && hop.organ == 'x'"),
    ),
];

fn lane(route: &str) -> Lane {
    Lane {
        route: route.into(),
        context: Vec::new(),
        at: Vec::new(),
        required: false,
        because: format!("the {route} lane"),
    }
}

fn contract(hive: &str, accepts: &[&str], emits: &[&str]) -> HiveContract {
    HiveContract {
        hive_path: hive.into(),
        accepts: accepts.iter().map(|r| lane(r)).collect(),
        emits: emits.iter().map(|r| lane(r)).collect(),
    }
}

fn contracts(with_newborn: bool) -> Vec<HiveContract> {
    let mut v = vec![
        contract("/mem", &["in_batch"], &["episode"]),
        contract("/app", &["ask"], &["answer"]),
        contract("/app/inner", &["work"], &["done"]),
        contract("/gate", &["in"], &["out"]),
        contract("/stamp", &["go"], &["failed"]),
        contract("/island", &["z"], &["z2"]),
    ];
    if with_newborn {
        v.push(contract("/n", &["q"], &[]));
    }
    v
}

/// A colony under mutation: the table plus the id of every pool edge it holds.
struct Colony {
    table: EdgeTable,
    held: Vec<Option<Uuid>>,
}

fn pool() -> Vec<Spec> {
    BASE.iter().chain(EXTRA.iter()).copied().collect()
}

impl Colony {
    fn base() -> Self {
        let mut c = Colony {
            table: EdgeTable::new(),
            held: vec![None; BASE.len() + EXTRA.len()],
        };
        for i in 0..BASE.len() {
            c.toggle(i);
        }
        c
    }

    fn toggle(&mut self, i: usize) {
        match self.held[i].take() {
            Some(id) => self.table.remove(&id),
            None => {
                let e = build(&pool()[i]);
                self.held[i] = Some(e.id);
                self.table.insert(e);
            }
        }
    }

    fn index_of(from: &str, to: &str, extra: bool) -> usize {
        let range = if extra {
            BASE.len()..BASE.len() + EXTRA.len()
        } else {
            0..BASE.len()
        };
        let p = pool();
        range
            .into_iter()
            .find(|&i| p[i].from == from && p[i].to == to)
            .unwrap_or_else(|| panic!("no pool edge {from} -> {to}"))
    }

    /// Takes away every pool edge that touches `node` — a removed node.
    fn remove_node(&mut self, node: &str) {
        let p = pool();
        for (i, e) in p.iter().enumerate() {
            if self.held[i].is_some() && (e.from == node || e.to == node) {
                self.toggle(i);
            }
        }
    }
}

fn assert_same(contracts: &[HiveContract], colony: &Colony, case: &str) {
    assert_eq!(
        addressed_lane_doors(contracts, &colony.table),
        reference_lane_doors(contracts, &colony.table),
        "{case}: the local check must judge exactly as the global one"
    );
}

// ---- (a) equivalence ----

#[test]
fn the_base_colony_is_sound_under_both() {
    let colony = Colony::base();
    assert!(reference_lane_doors(&contracts(false), &colony.table).is_empty());
    assert_same(&contracts(false), &colony, "base");
}

#[test]
fn every_named_mutation_form_judges_alike() {
    type Step = fn(&mut Colony);
    let cases: &[(&str, bool, Step)] = &[
        ("add a far edge", false, |c| {
            c.toggle(Colony::index_of("/a", "/c", true))
        }),
        ("remove a far edge", false, |c| {
            c.toggle(Colony::index_of("/a", "/b", false))
        }),
        ("add a node", false, |c| {
            c.toggle(Colony::index_of("/a", "/n", true));
            c.toggle(Colony::index_of("/n", "/b", true));
        }),
        ("remove a node inside a hive", false, |c| {
            c.remove_node("/mem/glue")
        }),
        ("remove a hive's rim wiring", false, |c| {
            c.remove_node("/gate")
        }),
        ("an edge across a hive boundary", false, |c| {
            c.toggle(Colony::index_of("/mem/glue", "/sink", true));
            c.toggle(Colony::index_of("/caller", "/mem/glue", true));
        }),
        ("nested: the inner hive loses its exit", false, |c| {
            c.toggle(Colony::index_of("/app/inner/worker", "/app/inner", false))
        }),
        ("nested: an inner cell exits both rims", false, |c| {
            c.toggle(Colony::index_of("/app/inner/worker", "/app", true));
            c.toggle(Colony::index_of("/app/back", "/app", false));
        }),
        ("an edge that breaks ANOTHER hive's exit", false, |c| {
            c.toggle(Colony::index_of("/gate/s", "/gate/t", true))
        }),
        ("an edge that breaks ANOTHER hive's door", false, |c| {
            c.toggle(Colony::index_of("/a", "/c", true));
            c.toggle(Colony::index_of("/mem", "/mem/glue", false));
        }),
        ("an island wakes up without a door", false, |c| {
            c.toggle(Colony::index_of("/x", "/island", true))
        }),
        ("a newborn hive without a door", true, |c| {
            c.toggle(Colony::index_of("/a", "/n", true));
            c.toggle(Colony::index_of("/n", "/n/inner", true));
        }),
        ("the stamped exit goes computed", false, |c| {
            c.toggle(Colony::index_of("/stamp/llm", "/stamp", false));
            c.toggle(Colony::index_of("/stamp/llm", "/stamp", true));
        }),
    ];
    for (name, newborn, step) in cases {
        let mut colony = Colony::base();
        step(&mut colony);
        assert_same(&contracts(*newborn), &colony, name);
    }
}

/// A tiny deterministic generator — no dependency, same walk on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound
    }
}

#[test]
fn a_random_walk_of_mutations_judges_alike_at_every_step() {
    for seed in 1..=8u64 {
        let mut rng = Lcg(seed);
        let mut colony = Colony::base();
        let mut newborn = false;
        for step in 0..300 {
            let pick = rng.next(BASE.len() + EXTRA.len() + 1);
            if pick == BASE.len() + EXTRA.len() {
                newborn = !newborn;
            } else {
                colony.toggle(pick);
            }
            assert_same(
                &contracts(newborn),
                &colony,
                &format!("seed {seed} step {step}"),
            );
        }
    }
}

#[test]
fn a_standing_violation_refuses_every_mutation_before_and_after() {
    // The STOPP finding of the #1157 diagnosis, pinned: a hive whose door
    // was never checked (a boot that only warns, or a newborn that missed
    // its own birth check) stands broken. Every later mutation — however
    // far away — is refused for it, by the old check and by the new one.
    let standing = contract("/broken", &["k"], &[]);
    let mut all = contracts(false);
    all.push(standing);
    let mut colony = Colony::base();
    colony.table.insert(build(&s(
        "/broken",
        "/broken/c",
        Some("hop.route == 'other'"),
    )));
    let mut rng = Lcg(42);
    for step in 0..200 {
        colony.toggle(rng.next(BASE.len() + EXTRA.len()));
        let new = addressed_lane_doors(&all, &colony.table);
        assert_eq!(
            new,
            reference_lane_doors(&all, &colony.table),
            "step {step}"
        );
        assert!(
            new.iter().any(|(_, hive, _)| hive == "/broken"),
            "step {step}: the standing violation refuses this mutation too: {new:?}"
        );
    }
}

// ---- (b) adversarial ----

#[test]
fn a_foreign_hive_is_never_left_out() {
    // The mutation touches nothing near /gate; /gate's exit was broken by an
    // edge that suppresses its default. A check that only looked at hives the
    // diff touched would let this through.
    let mut colony = Colony::base();
    colony.toggle(Colony::index_of("/gate/s", "/gate/t", true));
    colony.toggle(Colony::index_of("/a", "/c", true));
    let v = addressed_lane_doors(&contracts(false), &colony.table);
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].1, "/gate");
    assert_eq!(v[0].0.error_code(), "hive_contract");
}

/// A hive `/h` emitting `out`, with `n` interior sources that all own an
/// edge onto the rim, and only the LAST (by path) carrying the lane.
fn last_source_table(n: usize, last: Spec) -> EdgeTable {
    let mut t = EdgeTable::new();
    t.insert(build(&s("/c", "/h", None)));
    for i in 0..n {
        let from: &'static str = Box::leak(format!("/h/s{i:03}").into_boxed_str());
        t.insert(build(&s(from, "/h", Some("hop.route == 'nope'"))));
    }
    t.insert(build(&last));
    t
}

#[test]
fn the_last_source_is_asked_too() {
    let c = [contract("/h", &[], &["out"])];
    let sound = last_source_table(20, s("/h/zz", "/h", Some("hop.route == 'out'")));
    assert!(addressed_lane_doors(&c, &sound).is_empty());
    let broken = last_source_table(20, s("/h/zz", "/h", Some("hop.route == 'gone'")));
    assert_eq!(addressed_lane_doors(&c, &broken).len(), 1);
}

#[test]
fn a_source_is_asked_with_all_its_edges_not_only_the_rim_edge() {
    // The exit is a DEFAULT edge: it fires only when no regular edge of the
    // same source does. Asking the rim edge alone would see an exit that the
    // router never takes.
    let c = [contract("/h", &[], &["out"])];
    let mut t = EdgeTable::new();
    t.insert(build(&s("/c", "/h", None)));
    t.insert(build(&phase(s("/h/s", "/h", None), Kind::Default)));
    t.insert(build(&s("/h/s", "/h/t", Some("hop.route == 'never'"))));
    assert!(
        addressed_lane_doors(&c, &t).is_empty(),
        "default exit fires"
    );
    t.insert(build(&s("/h/s", "/h/u", None)));
    let v = addressed_lane_doors(&c, &t);
    assert_eq!(v, reference_lane_doors(&c, &t));
    assert_eq!(
        v.len(),
        1,
        "a regular hit suppresses the default exit: {v:?}"
    );
}

// ---- (c) cost ----

/// `k` contracted hives, each with a family-filter door and one exit source,
/// `chain` further interior sources per hive that never touch the rim, and
/// `far` edges nowhere near any hive.
fn grown(k: usize, chain: usize, far: usize) -> (Vec<HiveContract>, EdgeTable) {
    let mut t = EdgeTable::new();
    let mut cs = Vec::new();
    for h in 0..k {
        let hive = format!("/h{h}");
        cs.push(HiveContract {
            hive_path: hive.clone(),
            accepts: vec![lane("in_a")],
            emits: vec![lane("out")],
        });
        let leak = |p: String| -> &'static str { Box::leak(p.into_boxed_str()) };
        t.insert(build(&s(
            leak(hive.clone()),
            leak(format!("{hive}/glue")),
            Some("has(hop.route) && hop.route.startsWith('in_')"),
        )));
        t.insert(build(&s(
            leak(format!("{hive}/glue")),
            leak(hive.clone()),
            Some("hop.route == 'out'"),
        )));
        let mut prev = format!("{hive}/glue");
        for i in 0..chain {
            let next = format!("{hive}/a{i:03}");
            t.insert(build(&s(leak(prev.clone()), leak(next.clone()), None)));
            prev = next;
        }
    }
    for i in 0..far {
        let from: &'static str = Box::leak(format!("/far/{i}").into_boxed_str());
        let to: &'static str = Box::leak(format!("/far/{}", i + 1).into_boxed_str());
        t.insert(build(&s(from, to, Some("hop.route == 'x'"))));
    }
    (cs, t)
}

fn probe_runs(contracts: &[HiveContract], edges: &EdgeTable) -> usize {
    PROBE_RUNS.with(|n| n.set(0));
    let v = addressed_lane_doors(contracts, edges);
    assert!(v.is_empty(), "the grown colony is sound: {v:?}");
    PROBE_RUNS.with(|n| n.get())
}

#[test]
fn probe_runs_grow_with_contracts_not_with_the_colony() {
    let (c4, small) = grown(4, 0, 0);
    let (c4b, big) = grown(4, 60, 400);
    let (c8, _) = grown(8, 0, 0);
    let (_, small8) = grown(8, 0, 0);

    let base = probe_runs(&c4, &small);
    let grown_colony = probe_runs(&c4b, &big);
    let more_contracts = probe_runs(&c8, &small8);
    eprintln!(
        "GH #1157 probe runs: 4 contracts small={base} grown(+240 interior sources, \
         +400 far edges)={grown_colony}; 8 contracts={more_contracts}"
    );
    assert_eq!(
        base, grown_colony,
        "probe runs must not grow with the colony's edges and sources"
    );
    assert_eq!(
        more_contracts,
        2 * base,
        "linear in the number of contracts"
    );
    assert_eq!(
        base,
        4 * 2,
        "one door probe and one exit source per contract"
    );
}
