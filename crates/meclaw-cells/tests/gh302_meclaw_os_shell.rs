//! GH #302 — `meclaw-os@1.0.0`, the outermost level: the colony shell.
//!
//! The four composition templates are authored under one rule, and every one of
//! their READMEs repeats it in the same words: **a level owns what its siblings
//! must share.** The shell is the level whose siblings are the broker and the
//! control loop, so the shell owns the two lanes that reach them and the empty
//! container the organisations are instantiated into.
//!
//! There is no Rust behind any of that. A level is a directory, three
//! `config.json` files and a `template.json`, which means every claim it makes
//! is a fact about FILES — checkable here, with no colony and no runtime, the
//! same reasoning `gh173_shipped_hive_contracts` and `gh196_shipped_hive_ports`
//! are built on.
//!
//! # What is asserted, and why each one is asserted of the substrate
//!
//! 1. **Shape.** The shell is a hive with exactly three occupants: `access` and
//!    `steward` as `ref`s, and `orgs` as a real, open, empty container hive.
//! 2. **The pins resolve.** Both refs name `<name>@<version>` and the
//!    `TemplatesRegistry` — the same registry a mutation resolves against —
//!    answers with the directory on disk. A bare name would resolve to the
//!    newest version present, and a shell that silently adopts a new broker is
//!    exactly the drift #277's `template_chain` exists to make visible.
//! 3. **No `params.ports` anywhere.** The container convention (plan § Wave 4)
//!    ships every container OPEN, because the mutation that instantiates a child
//!    into it draws edges to that child and a sealed hive refuses precisely
//!    those endpoints with `hive_port_boundary`. The slot half of that
//!    convention was refuted at re-baseline time
//!    (`unbound_slot_behaviour` in `colony.rs`): a slot governs an
//!    address that does NOT exist, an existing-but-childless container hive
//!    counts as bound, and writing `params.ports` for the slot's sake would have
//!    sealed the parent. So the key is absent on both hives — asserted, because
//!    "we forgot it" and "we mean it" look identical in a file.
//! 4. **The contract is the occupants' lanes, measured.** The shell's `accepts`
//!    and `emits` are read against `templates/access/config.json` and
//!    `templates/steward/config.json` as they stand, never against a list kept
//!    here — a list would agree with itself while disagreeing with the tree.
//!    `connect` is the one subtraction: `access` requires that lane to be the
//!    only edge reaching the connector cell, and that edge is drawn where the
//!    connector stands.
//! 5. **Every declared lane has a door**, through the substrate's own
//!    `check_lane_doors` against the shell's own `params.graph`.
//! 6. **No swallowing sink** (#284, ruling Q2): nothing in here resolves to
//!    `terminal`, and every refusal lane leaves the shell instead of ending in
//!    it.
//! 7. **No second vault** (#302 ruling Q20): `access@2.0.5` carries its own
//!    interior `vault`, and the standalone `vault` template attests its inbound
//!    edges against `params.broker` — with no broker at this level it would boot
//!    locked and inert.
//! 8. **The README says the four things a reader cannot see in the JSON**: the
//!    level rule, the undeclared unbound behaviour of `orgs`, the vault finding,
//!    and that the steward of this shell talks to the colony directly.

use meclaw_colony::config::HiveParams;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::hive_contract::{HiveContract, Lane, check_lane_doors};
use meclaw_colony::templates::{
    TemplateEntry, TemplatesRegistry, parse_template_json, scan_templates_dir,
};
use meclaw_core::serde_json::Value;
use meclaw_core::{Path, Uuid};

/// Where the synthetic shell lives while its lanes are checked. Any path does;
/// what matters is that endpoints resolve the way the colony resolves them.
const HIVE: &str = "/h";

/// The template under test, and the two it references.
const SHELL: &str = "meclaw-os";
const BROKER: &str = "access";
const LOOP: &str = "steward";
/// The container the organisations are instantiated into.
const CONTAINER: &str = "orgs";
/// The one lane of the broker that is deliberately NOT re-emitted outward.
const NOT_RE_EMITTED: &str = "connect";
/// The template an organisation is grown from. Its lanes are read off the tree,
/// never listed here — see `the_org_lanes_cross_this_level_unchanged`.
const TENANT: &str = "org";

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn templates_root() -> std::path::PathBuf {
    core_root().join("templates")
}

fn shell_dir() -> std::path::PathBuf {
    templates_root().join(SHELL)
}

/// One `config.json`, as raw JSON.
fn config_at(dir: &std::path::Path) -> Value {
    let p = dir.join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The `params` block of a hive `config.json`, read by the SUBSTRATE's own
/// reader — which is `deny_unknown_fields`, so a stray key is a failure here
/// and not a surprise at boot.
fn hive_params(dir: &std::path::Path) -> HiveParams {
    let val = config_at(dir);
    assert_eq!(
        val.get("cell")
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str),
        Some("hive"),
        "{}/config.json is not a hive",
        dir.display()
    );
    // A hive without a `params` block is valid and means "declares nothing"
    // (`crates/meclaw-colony/src/bootstrap.rs`, the empty-object substitution).
    // The container ships exactly that, so this reader has to say the same thing
    // the substrate says rather than treating absence as a defect.
    let params = match val.get("params") {
        None | Some(Value::Null) => Value::Object(Default::default()),
        Some(other) => other.clone(),
    };
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}/config.json: params: {e}", dir.display()))
}

/// The direct child directories of a template directory, sorted.
fn children(dir: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| {
            let e = e.unwrap();
            e.path()
                .is_dir()
                .then(|| e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out
}

/// `Some("<name>@<version>")` iff this directory holds a bare `ref` marker.
fn ref_target(dir: &std::path::Path) -> Option<String> {
    let val = config_at(dir);
    let cell = val.get("cell")?;
    if cell.get("type").and_then(Value::as_str) != Some("ref") {
        return None;
    }
    cell.get("template")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The routes of a contract half, in declaration order.
fn routes(lanes: &[meclaw_colony::config::LaneSpec]) -> Vec<String> {
    lanes.iter().map(|l| l.route.clone()).collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// The headers a message on `route` carries, as the router would see them —
/// the same probe `gh173_shipped_hive_contracts` and the substrate's own lane
/// check build.
fn probe(route: &str) -> meclaw_core::Headers {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".to_string(), Value::String(route.to_string()));
    meclaw_core::Headers::from_parts(meclaw_core::serde_json::Map::new(), hop)
}

fn readme() -> String {
    let p = shell_dir().join("README.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn template_json() -> Value {
    let p = shell_dir().join("template.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ─────────────────────────────────────────────────────────────── the shape

#[test]
fn the_shell_holds_the_broker_the_control_loop_and_one_empty_container() {
    let dir = shell_dir();
    let _ = hive_params(&dir); // it is a hive, and its params parse

    assert_eq!(
        children(&dir),
        vec![BROKER.to_string(), CONTAINER.to_string(), LOOP.to_string()],
        "the shell's occupants are exactly `{BROKER}`, `{LOOP}` and the `{CONTAINER}` container — \
         a level that grows a fourth sibling has taken on something its siblings do not share"
    );

    // A ref directory holds nothing besides its own `config.json` (the
    // substrate refuses anything else with `schema`,
    // `mutation/subtree.rs:612-634`).
    for name in [BROKER, LOOP] {
        let d = dir.join(name);
        assert!(
            ref_target(&d).is_some(),
            "templates/{SHELL}/{name} is not a `ref` marker"
        );
        assert!(
            children(&d).is_empty(),
            "templates/{SHELL}/{name} holds directories beside its ref marker"
        );
    }

    // The container is a real hive and it is EMPTY: an org is instantiated into
    // it, it is not shipped with one.
    let container = dir.join(CONTAINER);
    let _ = hive_params(&container);
    assert!(
        children(&container).is_empty(),
        "the `{CONTAINER}` container ships with children — it is the address an org is \
         instantiated AT, not a place that already holds one"
    );
}

#[test]
fn both_refs_pin_an_exact_version_the_registry_can_resolve() {
    let dir = shell_dir();
    let scanned = scan_templates_dir(&templates_root()).expect("the templates directory scans");
    let registry = TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("gh302:{}", s.filesystem_path.display()),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    );

    for name in [BROKER, LOOP] {
        let reference = ref_target(&dir.join(name))
            .unwrap_or_else(|| panic!("templates/{SHELL}/{name} names no template"));
        let (named, version) = reference.split_once('@').unwrap_or_else(|| {
            panic!(
                "{reference}: a bare name resolves to whatever version is \
                 newest on disk, and a shell that silently adopts a new occupant is the drift \
                 `template_chain` exists to make visible — pin `<name>@<version>`"
            )
        });
        assert_eq!(named, name, "the ref in `{name}/` names `{named}`");
        assert_eq!(
            version.split('.').count(),
            3,
            "{reference}: a pin is `major.minor.patch`; ranges do not exist in this substrate"
        );
        let entry = registry
            .resolve(&reference)
            .unwrap_or_else(|e| panic!("{reference} does not resolve: {e:?}"));
        assert_eq!(entry.name, name);
        assert_eq!(entry.version.as_deref(), Some(version));
    }
}

#[test]
fn neither_the_shell_nor_the_container_carries_a_ports_key() {
    // The container convention: OPEN, because the mutation that instantiates an
    // org draws edges to that org, and a sealed hive refuses exactly those
    // endpoints. And no slot: it governs an address that does not exist, while
    // a container hive does exist — `unbound_slot_behaviour` in `colony.rs`
    // § 5. Writing `params.ports` for the slot's sake would have sealed the
    // shell, which is worse than useless.
    for dir in [shell_dir(), shell_dir().join(CONTAINER)] {
        let raw = config_at(&dir);
        assert!(
            raw.get("params").and_then(|p| p.get("ports")).is_none(),
            "{}/config.json carries a `params.ports` key — the key's mere presence SEALS the \
             hive, and this one has to stay open",
            dir.display()
        );
        assert!(
            hive_params(&dir).ports.is_none(),
            "{}: the substrate's own reader still sees a seal",
            dir.display()
        );
    }
}

// ──────────────────────────────────────────────────────────── the contract

/// The contract half of a shipped template, by route.
fn occupant_routes(name: &str) -> (Vec<String>, Vec<String>) {
    let hp = hive_params(&templates_root().join(name));
    let c = hp
        .contract
        .unwrap_or_else(|| panic!("templates/{name} declares no contract"));
    (routes(&c.accepts), routes(&c.emits))
}

#[test]
fn the_shells_contract_is_its_occupants_lanes_minus_the_one_that_stays_inside() {
    let (broker_in, broker_out) = occupant_routes(BROKER);
    let (loop_in, loop_out) = occupant_routes(LOOP);

    // Three occupants, and the third one — the organisation in the container —
    // has no contract of its own to read here, because it does not exist until
    // somebody instantiates one. Its lanes are the `org` template's, read off
    // the tree exactly like the other two.
    let (tenant_in, tenant_out) = occupant_routes(TENANT);

    let expect_in = sorted([broker_in, loop_in, tenant_in].concat());
    let expect_out = sorted(
        [broker_out, loop_out, tenant_out]
            .concat()
            .into_iter()
            .filter(|r| r != NOT_RE_EMITTED)
            .collect::<Vec<_>>(),
    );

    let hp = hive_params(&shell_dir());
    let c = hp.contract.expect("the shell declares a contract");
    assert_eq!(
        sorted(routes(&c.accepts)),
        expect_in,
        "the shell accepts something other than what its occupants accept — a level owns what \
         its siblings must share, and an inbound lane it does not carry is a lane no caller can \
         reach"
    );
    assert_eq!(
        sorted(routes(&c.emits)),
        expect_out,
        "the shell emits something other than its occupants' lanes minus `{NOT_RE_EMITTED}`"
    );

    // The subtraction is a decision, so it is asserted as one: the broker DOES
    // emit it, and the shell deliberately does not pass it on.
    assert!(
        occupant_routes(BROKER)
            .1
            .iter()
            .any(|r| r == NOT_RE_EMITTED),
        "`{BROKER}` no longer emits `{NOT_RE_EMITTED}` — the subtraction below has lost its subject"
    );
    assert!(
        !routes(&c.emits).iter().any(|r| r == NOT_RE_EMITTED),
        "the shell re-emits `{NOT_RE_EMITTED}` — `{BROKER}` requires that lane to be the ONLY edge \
         reaching the connector cell, and that edge is drawn where the connector stands"
    );

    // Both inbound lanes of the broker demand a promoted requester (R-AC-1). A
    // level that forwards them without saying so hands the requirement to a
    // caller who cannot read it.
    let broker = hive_params(&templates_root().join(BROKER))
        .contract
        .unwrap();
    for lane in &broker.accepts {
        if lane.context.is_empty() {
            continue;
        }
        let mine = c
            .accepts
            .iter()
            .find(|l| l.route == lane.route)
            .unwrap_or_else(|| panic!("the shell lost the `{}` lane", lane.route));
        for key in &lane.context {
            assert!(
                mine.context.contains(key),
                "the shell accepts `{}` without demanding `context.{key}`, which `{BROKER}` \
                 requires — the promotion has to happen outside this level",
                lane.route
            );
        }
    }
}

/// The shell's `params.graph`, resolved into the edge table the colony would
/// build from it.
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
            modifier: None,
            is_default: spec.is_default,
        });
    }
    t
}

#[test]
fn every_declared_lane_has_a_door_in_the_shells_own_graph() {
    let hp = hive_params(&shell_dir());
    let spec = hp.contract.as_ref().expect("the shell declares a contract");
    let lane = |l: &meclaw_colony::config::LaneSpec| Lane {
        route: l.route.clone(),
        context: l.context.clone(),
        because: l.because.clone(),
    };
    let contract = HiveContract {
        hive_path: HIVE.to_string(),
        accepts: spec.accepts.iter().map(lane).collect(),
        emits: spec.emits.iter().map(lane).collect(),
    };
    // The substrate's own check, not a second opinion about what a door is.
    check_lane_doors(std::slice::from_ref(&contract), &table_for(&hp))
        .unwrap_or_else(|e| panic!("templates/{SHELL}: {e:?}"));
}

/// W7-R2 — the container declares nothing, and the LEVEL carries its lanes.
///
/// The trap this pins is a declaration that hides itself. A hive contract is
/// checked inward: an accepted lane must route from the hive path to a cell
/// INSIDE it. The container is empty by construction, so no lane it declared
/// could ever have a door — and the check does not fire while nothing addresses
/// the hive, because `hive_path_is_wired` treats an unaddressed contract as
/// dormant. Such a declaration therefore ships GREEN and turns red the moment
/// somebody draws the first edge to the container, at which point EVERY mutation
/// of the colony is refused with `hive_contract` until the first organisation
/// stands inside. Green only because nobody is looking is the same defect class
/// as the slot this wave removed from all four levels.
#[test]
fn the_container_declares_nothing_and_the_level_carries_its_lanes() {
    let container = shell_dir().join(CONTAINER);
    let raw = config_at(&container);
    assert!(
        raw.get("params").is_none(),
        "the `{CONTAINER}` container carries a `params` block — it declares nothing at all \
         (W7-R2), and a hive without `params` is valid"
    );

    let hp = hive_params(&container);
    assert!(
        hp.contract.is_none(),
        "the `{CONTAINER}` container declares a `params.contract`. It is empty by construction, so \
         every lane it names has a door that cannot exist; the declaration is green only while \
         nobody addresses the hive, and the first edge drawn to it refuses every mutation of the \
         colony with `hive_contract`. The LEVEL declares the transit lanes instead."
    );
    assert!(
        hp.graph.edges.is_empty(),
        "the `{CONTAINER}` container ships internal edges — below it there is nothing to route to \
         until an org is instantiated, and the mutation that instantiates one draws its own"
    );

    // The other half of the rule is asserted next door, in
    // `the_org_lanes_cross_this_level_unchanged`: the level declares what the
    // container cannot, and it declares it from the tenant template's own lanes.
}

/// **The level boundary, pinned rather than hand-made.**
///
/// This is the assertion whose absence let the defect through the first time.
/// The shell's transit lanes were written against an `org` whose own list was
/// written against a `member` that did not exist yet, and the result was a
/// declared `turn` lane with an exit edge that could never fire — invisible,
/// because nothing compared the two files.
///
/// So: read `templates/org/config.json` AT THE TREE and require containment in
/// both directions. Every lane an organisation accepts must be a lane this level
/// accepts (otherwise nothing can reach an organisation on it), and every lane an
/// organisation emits must be one this level emits (otherwise the answer stops at
/// the container). Moving a lane in `org` is then red HERE instead of silently
/// disappearing.
///
/// The union rule (Task 13 Step 2, orchestrator ruling 2026-08-25) allows one
/// direction of slack and only one: a level may subtract a lane a sibling INSIDE
/// it consumes. Nothing inside this shell consumes an organisation's output, so
/// on this level containment is exact for the emit half too.
#[test]
fn the_org_lanes_cross_this_level_unchanged() {
    let (tenant_in, tenant_out) = occupant_routes(TENANT);
    let shell = hive_params(&shell_dir());
    let c = shell.contract.expect("the shell declares a contract");
    let mine_in = routes(&c.accepts);
    let mine_out = routes(&c.emits);

    for lane in &tenant_in {
        assert!(
            mine_in.contains(lane),
            "`{TENANT}` accepts `{lane}` and this level does not: nothing can reach an \
             organisation on that lane. The level declares the union of its occupants' lanes \
             (minus what a sibling inside consumes), so a lane that moved in `{TENANT}` moves \
             here in the same commit. This level accepts {mine_in:?}."
        );
    }
    for lane in &tenant_out {
        assert!(
            mine_out.contains(lane),
            "`{TENANT}` emits `{lane}` and this level does not: the answer stops at the \
             container. This level emits {mine_out:?}."
        );
    }

    // And the other direction for the tenant's half, which is what catches a
    // lane this level KEPT after the tenant stopped raising it — the exact shape
    // of the `turn` defect: an exit edge that can never fire, declared as if it
    // could.
    let raises_or_takes = |lane: &String| -> bool {
        [BROKER, LOOP, TENANT].into_iter().any(|t| {
            let (accepts, emits) = occupant_routes(t);
            accepts.contains(lane) || emits.contains(lane)
        })
    };
    for lane in mine_out.iter().chain(mine_in.iter()) {
        assert!(
            raises_or_takes(lane),
            "this level declares `{lane}`, and no occupant raises or takes it — \
             `{BROKER}`, `{LOOP}` and `{TENANT}` were all read at the tree. A lane with no \
             occupant behind it is an edge that can never fire, declared as if it could."
        );
    }
}

/// The pin under W7-R2's second half: every lane the level declares is carried
/// by an edge the level itself ships, and every edge it ships carries a declared
/// lane. Both directions, because a promise with no edge is a dead lane and an
/// edge with no promise is an undocumented one.
#[test]
fn every_edge_is_a_door_or_an_exit_and_every_one_carries_a_declared_lane() {
    let hp = hive_params(&shell_dir());
    let spec = hp.contract.as_ref().expect("the shell declares a contract");
    let table = table_for(&hp);
    let hive = Path::new(HIVE);

    for e in &hp.graph.edges {
        assert!(
            e.from == "." || e.to == ".",
            "the edge {} -> {} is internal to the shell — a level routes, it does not wire its \
             occupants to each other",
            e.from,
            e.to
        );
    }

    for e in hp.graph.edges.iter().filter(|e| e.from == ".") {
        let target = format!("{HIVE}/{}", e.to.trim_start_matches("./"));
        let covered = spec.accepts.iter().any(|l| {
            meclaw_colony::edge_table::apply_edges(&table, &hive, &probe(&l.route))
                .iter()
                .any(|d| d.target.as_str() == target)
        });
        assert!(
            covered,
            "the door {} -> {} opens on a lane no `accepts` entry names",
            e.from, e.to
        );
    }

    for e in hp.graph.edges.iter().filter(|e| e.to == ".") {
        let src = Path::new(&format!("{HIVE}/{}", e.from.trim_start_matches("./")));
        let covered = spec.emits.iter().any(|l| {
            meclaw_colony::edge_table::apply_edges(&table, &src, &probe(&l.route))
                .iter()
                .any(|d| d.target.as_str() == HIVE)
        });
        assert!(
            covered,
            "the exit {} -> {} carries a lane no `emits` entry names",
            e.from, e.to
        );
    }

    // And the container's lanes specifically, by the edge that carries each: the
    // container is inside the shell, which is what gives them a door at all. The
    // lane names come from the `org` template on disk, so this cannot drift into
    // agreeing with itself.
    let carries = |from: &str, to: &str, route: &str| -> bool {
        hp.graph.edges.iter().any(|e| {
            e.from == from
                && e.to == to
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains(&format!("'{route}'")))
        })
    };
    let child = format!("./{CONTAINER}");
    let (tenant_in, tenant_out) = occupant_routes(TENANT);
    for route in &tenant_in {
        assert!(
            carries(".", &child, route),
            "no `. -> {child}` edge on `{route}` — `{TENANT}` accepts that lane and this level \
             declares it, so the container needs a door for it"
        );
    }
    for route in &tenant_out {
        assert!(
            carries(&child, ".", route),
            "no `{child} -> .` edge on `{route}` — `{TENANT}` emits that lane and this level \
             declares it, so the container needs an exit for it"
        );
    }
}

// ───────────────────────────────────────────── the subtractions (Q2, Q20)

#[test]
fn nothing_in_the_shell_swallows_a_refusal() {
    // Q2 (GH #284): the DLQ is the record. No shipped topology routes
    // `reject`/`error` into a `terminal`.
    let dir = shell_dir();
    for name in children(&dir) {
        if let Some(reference) = ref_target(&dir.join(&name)) {
            assert!(
                !reference.starts_with("terminal@"),
                "templates/{SHELL}/{name} references `{reference}` — a sink that accepts \
                 everything and emits nothing turns a refusal into silence"
            );
        }
    }
    assert!(
        !children(&dir).iter().any(|c| c == "terminal"),
        "the shell carries a `terminal` occupant"
    );

    let hp = hive_params(&dir);
    for e in &hp.graph.edges {
        let Some(cond) = e.condition.as_deref() else {
            continue;
        };
        if !(cond.contains("'error'") || cond.contains("'reject'")) {
            continue;
        }
        assert_eq!(
            e.to, ".",
            "the refusal edge {} -> {} ends INSIDE the shell; a refusal lane leaves the level it \
             was raised in, or it is not a refusal any more",
            e.from, e.to
        );
    }
}

#[test]
fn the_shell_adds_no_second_vault_and_the_readme_states_the_finding() {
    let dir = shell_dir();
    assert!(
        !children(&dir).iter().any(|c| c == "vault"),
        "the shell carries its own `vault` — `{BROKER}` already contains one, and the standalone \
         template attests its inbound edges against `params.broker`, so a second one at this \
         level boots locked and inert (ruling Q20)"
    );
    for name in children(&dir) {
        if let Some(reference) = ref_target(&dir.join(&name)) {
            assert!(
                !reference.starts_with("vault@"),
                "templates/{SHELL}/{name} references `{reference}` (ruling Q20)"
            );
        }
    }
    let text = readme();
    assert!(
        text.contains("vault") && text.contains("params.broker"),
        "the README does not state the vault finding — a subtraction nobody wrote down is \
         indistinguishable from an omission"
    );
}

// ──────────────────────────────────────────────────── what only prose says

#[test]
fn the_readme_carries_the_version_the_level_rule_and_the_two_undeclared_facts() {
    let text = readme();
    let version = template_json()
        .get("version")
        .and_then(Value::as_str)
        .expect("template.json declares a version as a STRING (GH #221)")
        .to_string();

    let h1 = text
        .lines()
        .find(|l| l.starts_with("# "))
        .expect("the README has an H1");
    assert!(
        h1.contains(&format!("{SHELL}@{version}")),
        "the README H1 is {h1:?} and `template.json` says {version} (GH #335)"
    );

    // The one rule all four levels repeat in the same words. Case is not part
    // of the rule — a heading capitalises its first letter and a descriptor
    // shouts it — but the words are.
    assert!(
        text.to_lowercase()
            .contains("a level owns what its siblings must share"),
        "the README does not carry the level rule verbatim"
    );

    // The unbound behaviour of the container is undeclared, and the README says
    // so with its reason and a pointer to the receipt — not to an issue that is
    // closed.
    assert!(
        text.contains("undeclared") && text.contains(CONTAINER),
        "the README does not say that the unbound behaviour of `{CONTAINER}` is undeclared"
    );
    assert!(
        text.contains("unbound_slot_behaviour"),
        "the README does not point at the resolution code that measured why there is no slot here"
    );

    // W6 moved the steward's counting out of `colony.db` and onto a virtual
    // endpoint. Those two lanes travel with the ref; this shell writes no edge
    // for them, and must not look as though it sealed them away.
    assert!(
        text.contains("/colony/ledger"),
        "the README does not say that this shell's steward talks to the colony directly — two \
         absolute lanes travel with the ref and no edge here draws them"
    );
    // And the lane that is deliberately not passed on.
    assert!(
        text.contains(NOT_RE_EMITTED),
        "the README does not account for the `{NOT_RE_EMITTED}` lane"
    );
}

#[test]
fn the_shell_declares_no_requirement_because_its_own_values_use_none() {
    // W3's validator collects the union across the refs itself
    // (`addressed_requires` resolves each `ref_chain`), so a level declares only
    // what its OWN config values use. This one substitutes nothing — and the
    // `ctx` half of that is gated in both directions by
    // `gh292_every_ctx_key_is_declared`, so a declaration without a use would be
    // red there rather than harmless here.
    let dir = shell_dir();
    let mut values = String::new();
    for sub in [None, Some(BROKER), Some(LOOP), Some(CONTAINER)] {
        let d = sub.map_or_else(|| dir.clone(), |s| dir.join(s));
        values.push_str(&std::fs::read_to_string(d.join("config.json")).unwrap());
    }
    let substitutes = values.contains("${");
    assert!(
        !substitutes,
        "the shell's config values substitute something — then it owes a `requires` declaration"
    );
    assert!(
        template_json().get("requires").is_none(),
        "the shell declares `requires` while substituting nothing; the refs' own requirements are \
         collected through the ref chain and are not this template's to repeat"
    );
}

#[test]
fn the_descriptor_carries_the_four_description_slots() {
    let val = template_json();
    assert_eq!(
        val.get("name").and_then(Value::as_str),
        Some(SHELL),
        "the descriptor's name is what a reference resolves against"
    );
    assert!(
        val.get("version").and_then(Value::as_str).is_some(),
        "`version` must be a STRING — the reader takes strings only and a number reaches the \
         registry as no version at all (GH #221)"
    );
    let d = val.get("description").expect("a description block");
    for slot in ["purpose", "use_when", "not_in_scope", "examples"] {
        assert!(d.get(slot).is_some(), "the descriptor has no `{slot}` slot");
    }
    assert!(
        d.get("not_in_scope")
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains(NOT_RE_EMITTED)),
        "`not_in_scope` does not name the `{NOT_RE_EMITTED}` lane the shell refuses to pass on"
    );
    // The descriptor has to parse for the substrate too, or the row reaches the
    // registry versionless.
    let scanned = parse_template_json(&shell_dir().join("template.json"))
        .expect("the substrate's own reader parses the descriptor");
    assert_eq!(scanned.name, SHELL);
    assert!(scanned.version.is_some());
}
