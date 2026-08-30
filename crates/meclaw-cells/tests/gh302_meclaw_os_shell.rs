//! GH #302 — `meclaw-os@1.5.0`, the outermost level: the colony shell.
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
//!    `argus` as `ref`s, and `orgs` as a real, open, empty container hive.
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
//!    `templates/argus/config.json` as they stand, never against a list kept
//!    here — a list would agree with itself while disagreeing with the tree.
//!    `connect` is the one subtraction: `access` requires that lane to be the
//!    only edge reaching the connector cell, and that edge is drawn where the
//!    connector stands.
//! 5. **Every declared lane has a door**, through the substrate's own
//!    `check_lane_doors` against the shell's own `params.graph`.
//! 6. **No swallowing sink** (#284, ruling Q2): nothing in here resolves to
//!    `terminal`, and every refusal lane leaves the shell instead of ending in
//!    it.
//! 7. **No second vault** (#302 ruling Q20): `access@2.3.2` carries its own
//!    interior `vault`, and the standalone `vault` template attests its inbound
//!    edges against `params.broker` — with no broker at this level it would boot
//!    locked and inert.
//! 8. **The README says the four things a reader cannot see in the JSON**: the
//!    level rule, the undeclared unbound behaviour of `orgs`, the vault finding,
//!    and that the argus of this shell talks to the colony directly.

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
const LOOP: &str = "argus";

/// GH #425 / R6 — the two halves of the ONE authoring path a colony has: the
/// baumeister that drafts, and the submitter that is the only cell in the tree
/// with a reach onto the mutation door. Both pass the question ADR-0013 asks
/// (do all occupants of the level beneath share it?): one colony, one manifest
/// audit trail — yes.
const BAUMEISTER: &str = "builder";
const SUBMITTER: &str = "submit";
/// GH #446 / R4 — the ONE front door a person addresses the OS through. It
/// passes the same question ADR-0013 asks: a colony has one operator surface,
/// and the identity a request acquires there is the identity every organisation
/// under this level sees. It stands here and not one level down for the same
/// reason the submitter does — it has to be a sibling of the submitter to hand
/// it anything at all.
const FRONT_DOOR: &str = "operator";
/// The container the organisations are instantiated into.
const CONTAINER: &str = "orgs";
/// The one lane of the broker that is deliberately NOT re-emitted outward.
const NOT_RE_EMITTED: &str = "connect";

/// GH #425 — the lanes an occupant of this level ships and this level
/// deliberately does NOT declare, because a sibling INSIDE the level is the one
/// that produces or consumes them.
///
/// ADR-0013: *"a level declares the union of its occupants' lanes that CROSS it
/// … minus what a sibling inside the level consumes itself."* The builder pair
/// crosses nothing at this boundary, and that is not a saving — it is the rule
/// applied: the lane is invisible at the rim precisely because the baumeister
/// stands INSIDE.
///
/// GH #469 took `in_build` OUT of this list, and the retraction is the
/// interesting half. The argument above holds for `build` and
/// `in_build_result`, which are an ORGANISATION's names for the round. It broke
/// for the one caller it had to hold for: the first build of a colony, where
/// `./orgs` is empty by construction, so the edge the whole subtraction rested
/// on has no sender. `in_build` is what an operator asks for at the rim, and a
/// lane the level does not declare is a lane no caller can be told about.
///
/// | lane | who ships it | who inside answers for it |
/// |---|---|---|
/// | `build` | `org`, emitted | `./builder` / `./operator`, by class |
/// | `in_build_result` | `org`, accepted | `./builder` / `./operator`, on the way back |
/// | `in_apply` | `submit`, accepted | produced by the `./operator -> ./submit` edge |
/// | `in_submit` | `operator`, accepted | a RIM lane, and also produced by the `./orgs -> ./operator` edge |
/// | `manifest` | `builder`, emitted | consumed by the `./builder -> ./orgs` edge, or by `./builder -> ./operator` when the round came in at the rim |
/// | `in_receipt` | `builder` and `operator`, accepted | produced by the `./submit -> ./builder` and `./submit -> ./operator` edges |
/// | `apply` | `operator`, emitted | consumed by the `./operator -> ./submit` edge |
/// | `export` | `operator`, emitted | re-stamped `in_export` by the `./operator -> ./orgs` edge |
/// | `export_done` | `operator`, accepted | produced by the `./orgs -> ./operator` edge |
///
/// `receipt` used to stand in this list and no longer does (GH #446). It has
/// TWO producers now: the submitter, whose receipts are taken by `./builder`
/// and by the front door inside, and the front door itself, whose receipts are
/// the answer whoever asked is owed. A lane with one producer inside and one
/// that crosses is a lane that crosses — subtracting it would take away the
/// only exit the front door has.
///
/// R-Zielfluss (a): the front door's `receipt` has TWO destinations at this
/// level and one lane to be told apart by. An assistant's goes back down
/// (`./operator -> ./orgs`, re-stamped `in_build_result`), a person's leaves on
/// the rim, and `hop.submitter_kind == 'agent'` is the discriminator on both
/// edges. `in_submit` is where the two callers meet: a rim door for the person
/// and the `./orgs -> ./operator` edge for the assistant.
///
/// `mutate` is deliberately NOT here: the submitter emits it and the shell
/// re-emits it, because it has to leave the level to reach the mutation door.
/// That is the one lane of this pair that crosses, and it is the whole guardrail.
const CONSUMED_INSIDE: &[&str] = &[
    "build",
    "in_build_result",
    "in_apply",
    "manifest",
    // GH #446 — the front door's three internal lanes. `apply` reaches the
    // submitter, `export` is re-stamped onto the organisation's `in_export`,
    // and `export_done` comes back the same way. Producer and consumer are
    // siblings here, so none of the three names reaches the rim.
    "apply",
    "export",
    "export_done",
    // GH #435: the submitter asks the broker whether a manifest may be applied,
    // and both halves of that question are siblings at THIS level. `ask` leaves
    // the submitter and reaches the broker as `in_request`; the verdict leaves
    // the broker as `grant` and reaches the submitter as `in_verdict`. Neither
    // name crosses the rim, which is the same reason `build`/`in_apply` do not.
    "ask",
    "in_verdict",
    // The refine lane of the builder's tool loop, and the exact mirror of
    // `build`/`in_build`: the submitter emits `receipt`, the
    // `./submit -> ./builder` edge of THIS level renames it to `in_receipt`,
    // and the baumeister takes the refusal back into a repair round. Producer
    // and consumer are both siblings here, so the name never reaches the rim —
    // declaring it outward would offer a caller a lane no edge of this level
    // would ever fill.
    "in_receipt",
    // GH #474 — the halt. The baumeister raises `manifest`, this level's own
    // `./builder -> ./operator` edge re-stamps it to `in_draft`, and the front
    // door parks it. Producer and consumer are siblings here, so the name never
    // reaches the rim: an operator asks on `in_build` and says yes on
    // `in_submit`, and both of those ARE rim lanes. A caller that could post
    // `in_draft` from outside could park a manifest nobody drafted.
    "in_draft",
    // GH #504 — the corpus nudge, and the exact mirror of `in_receipt` one
    // lane over. The submitter raises `receipt`, this level's own
    // `./submit -> ./builder` edge re-stamps it to `in_ingest` when the
    // committed diff registered a class, and the baumeister forwards it to the
    // librarian it refs. Producer and consumer are siblings here, so the name
    // never reaches the rim. Its answer `catalogue` is NOT in this list: the
    // builder emits it and this level re-emits it, because nothing inside
    // consumes a report.
    "in_ingest",
];
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
/// check build — plus every `context.<key> == '<value>'` clause `condition`
/// demands.
///
/// GH #469. A door or an exit may discriminate on a PROMOTED context key rather
/// than on the lane alone — the level stamps `context.build_caller` on the way
/// in, and the builder's two answers are told apart by it on the way out. A
/// bare lane probe reads such an edge as one that never fires, and the checks
/// below would then report an edge that demonstrably carries a declared lane as
/// carrying none. What is asked here is whether an edge NAMES a lane, so the
/// probe carries what the edge asks for; whether it fires for a given message
/// is a statement about the messages the inside produces, and the substrate's
/// own `door_states_lane` refuses to judge that for the same reason.
fn probe_guarded(route: &str, condition: Option<&str>) -> meclaw_core::Headers {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".to_string(), Value::String(route.to_string()));
    let mut context = meclaw_core::serde_json::Map::new();
    for (key, value) in context_equalities(condition.unwrap_or_default()) {
        context.insert(key, Value::String(value));
    }
    meclaw_core::Headers::from_parts(context, hop)
}

/// Every `context.<key> == '<value>'` pair a CEL condition states, in order.
///
/// Deliberately literal: it reads equalities and nothing else, so a negated
/// guard (`context.x != 'y'`) contributes nothing and the probe stays the bare
/// one for it — which is right, because that edge fires without the key.
fn context_equalities(condition: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = condition;
    while let Some(at) = rest.find("context.") {
        rest = &rest[at + "context.".len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        let (key, after) = rest.split_at(end);
        rest = after;
        let Some(tail) = rest.strip_prefix(" == '") else {
            continue;
        };
        let Some(close) = tail.find('\'') else {
            continue;
        };
        out.push((key.to_string(), tail[..close].to_string()));
        rest = &tail[close..];
    }
    out
}

fn readme() -> String {
    let p = shell_dir().join("README.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The shipped library, as the registry a mutation resolves against.
fn registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&templates_root()).expect("the templates directory scans");
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("gh302:{}", s.filesystem_path.display()),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn template_json() -> Value {
    let p = shell_dir().join("template.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ─────────────────────────────────────────────────────────────── the shape

#[test]
fn the_shell_holds_five_refs_and_one_empty_container() {
    let dir = shell_dir();
    let _ = hive_params(&dir); // it is a hive, and its params parse

    let mut want = vec![
        BROKER.to_string(),
        BAUMEISTER.to_string(),
        CONTAINER.to_string(),
        FRONT_DOOR.to_string(),
        LOOP.to_string(),
        SUBMITTER.to_string(),
    ];
    want.sort();
    assert_eq!(
        children(&dir),
        want,
        "the shell's occupants are exactly `{BROKER}`, `{LOOP}`, `{BAUMEISTER}`, \
         `{SUBMITTER}`, `{FRONT_DOOR}` and the `{CONTAINER}` container — a level that \
         grows a seventh sibling has taken on something its siblings do not share"
    );

    // A ref directory holds nothing besides its own `config.json` (the
    // substrate refuses anything else with `schema`,
    // `mutation/subtree.rs:612-634`).
    for name in [BROKER, LOOP, BAUMEISTER, SUBMITTER] {
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
    let registry = registry();

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
fn the_shells_contract_is_its_occupants_lanes_minus_the_ones_that_stay_inside() {
    let (broker_in, broker_out) = occupant_routes(BROKER);
    let (loop_in, loop_out) = occupant_routes(LOOP);
    let (builder_in, builder_out) = occupant_routes(BAUMEISTER);
    let (submit_in, submit_out) = occupant_routes(SUBMITTER);
    let (door_in, door_out) = occupant_routes(FRONT_DOOR);

    // Five occupants, and the one in the container — the organisation — has no
    // contract of its own to read here, because it does not exist until
    // somebody instantiates one. Its lanes are the `org` template's, read off
    // the tree exactly like the rest.
    let (tenant_in, tenant_out) = occupant_routes(TENANT);

    let expect_in = sorted(
        [
            broker_in, loop_in, tenant_in, builder_in, submit_in, door_in,
        ]
        .concat()
        .into_iter()
        .filter(|r| !CONSUMED_INSIDE.contains(&r.as_str()))
        .collect::<Vec<_>>(),
    );
    let expect_out = sorted(
        [
            broker_out,
            loop_out,
            tenant_out,
            builder_out,
            submit_out,
            door_out,
        ]
        .concat()
        .into_iter()
        .filter(|r| r != NOT_RE_EMITTED && !CONSUMED_INSIDE.contains(&r.as_str()))
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
///
/// GH #454 is the measurement that gives this test its most interesting green.
/// A channel stopped belonging to one generation of an agent and started
/// belonging to the person, which re-cut the turn path inside `member@1.3.0`
/// and bumped `assistant` to `2.0.0`. Two levels further out, HERE, not one
/// entry of either list moved: `turn` was never a rim lane of an organisation
/// and still is not, and `answer` was always one — it merely carries an
/// answered turn now beside what it carried before. That is the property a
/// namespace is for, and it is only an observation because the lists on both
/// sides are re-derived from the tree in the same run.
#[test]
fn the_org_lanes_cross_this_level_unchanged() {
    let (tenant_in, tenant_out) = occupant_routes(TENANT);
    let shell = hive_params(&shell_dir());
    let c = shell.contract.expect("the shell declares a contract");
    let mine_in = routes(&c.accepts);
    let mine_out = routes(&c.emits);

    for lane in tenant_in
        .iter()
        .filter(|l| !CONSUMED_INSIDE.contains(&l.as_str()))
    {
        assert!(
            mine_in.contains(lane),
            "`{TENANT}` accepts `{lane}` and this level does not: nothing can reach an \
             organisation on that lane. The level declares the union of its occupants' lanes \
             (minus what a sibling inside consumes), so a lane that moved in `{TENANT}` moves \
             here in the same commit. This level accepts {mine_in:?}."
        );
    }
    for lane in tenant_out
        .iter()
        .filter(|l| !CONSUMED_INSIDE.contains(&l.as_str()))
    {
        assert!(
            mine_out.contains(lane),
            "`{TENANT}` emits `{lane}` and this level does not: the answer stops at the \
             container. This level emits {mine_out:?}."
        );
    }

    // GH #454, stated rather than implied: the re-layering two levels down did
    // not reach this rim. `turn` is consumed inside a member and is not a lane
    // of this shell; `answer` is how an answered turn leaves it.
    assert!(
        !mine_out.iter().chain(mine_in.iter()).any(|l| l == "turn"),
        "the shell declares `turn`. A member consumes its own turn internally \
         (`./channels -> ./firewall` since GH #454), so an organisation never raises the \
         lane and an exit carrying it here could never fire: {mine_out:?}"
    );
    assert!(
        mine_out.iter().any(|l| l == "answer"),
        "the shell dropped `answer` — the one lane an answered turn leaves an organisation \
         on: {mine_out:?}"
    );

    // The subtractions are decisions, so they are asserted as ones: each lane in
    // CONSUMED_INSIDE that the TENANT ships must still be shipped by it, and
    // must still not be declared here. A lane the org stopped raising would
    // otherwise sit in the table forever, silently exempting nothing.
    for lane in ["build", "in_build_result"] {
        let owned = tenant_out.iter().any(|l| l == lane) || tenant_in.iter().any(|l| l == lane);
        assert!(
            owned,
            "the subtraction of `{lane}` is stale: `{TENANT}` no longer ships it"
        );
        assert!(
            !mine_out.iter().chain(mine_in.iter()).any(|l| l == lane),
            "the shell declares `{lane}`, which a sibling INSIDE it answers for. On this \
             level the builder lane pair crosses NOTHING: `./orgs` raises it, `./builder` \
             or `./submit` takes it, and back. Declaring it at the rim would promise a lane \
             whose messages never leave."
        );
    }

    // And the other direction for the tenant's half, which is what catches a
    // lane this level KEPT after the tenant stopped raising it — the exact shape
    // of the `turn` defect: an exit edge that can never fire, declared as if it
    // could.
    //
    // GH #469 widened the list of occupants read here to ALL of them. It used
    // to name four, which was enough only while the authoring path raised
    // nothing at the rim; `in_build` is raised by the baumeister and reaches
    // the rim, so a check that never opened the baumeister's contract would
    // have called a lane with an occupant behind it a lane without one.
    let raises_or_takes = |lane: &String| -> bool {
        [BROKER, LOOP, TENANT, FRONT_DOOR, BAUMEISTER, SUBMITTER]
            .into_iter()
            .any(|t| {
                let (accepts, emits) = occupant_routes(t);
                accepts.contains(lane) || emits.contains(lane)
            })
    };
    for lane in mine_out.iter().chain(mine_in.iter()) {
        assert!(
            raises_or_takes(lane),
            "this level declares `{lane}`, and no occupant raises or takes it — \
             `{BROKER}`, `{LOOP}`, `{TENANT}`, `{FRONT_DOOR}`, `{BAUMEISTER}` and `{SUBMITTER}` \
             were all read at the tree. A lane with no occupant behind it is an edge that can \
             never fire, declared as if it could."
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

    // GH #425 — until R6 every edge of this shell touched the rim, and the
    // assertion below read that as a rule: "a level routes, it does not wire its
    // occupants to each other". It was a coincidence of who lived here. ADR-0013
    // says a level OWNS what its siblings must share, and owning a baumeister
    // means the container is wired to it — exactly as `assistant` wires
    // `./cogny -> ./tools` and `member` wires `./channels -> ./firewall`
    // (`./assistants -> ./firewall` until GH #454 moved the channel up a level).
    //
    // What survives of the old rule is the part that was load-bearing: an
    // occupant-to-occupant edge must name a lane, so it can be read, and it must
    // not be the container talking to itself.
    for e in &hp.graph.edges {
        if e.from == "." || e.to == "." {
            continue;
        }
        assert_ne!(
            e.from, e.to,
            "the edge {} -> {} is an occupant wired to itself",
            e.from, e.to
        );
        assert!(
            e.condition
                .as_deref()
                .is_some_and(|c| c.contains("hop.route ==")),
            "the internal edge {} -> {} states no lane — an edge between two occupants of a \
             level is the level exercising what it owns, and it says on which lane or it is \
             not readable",
            e.from,
            e.to
        );
    }

    for e in hp.graph.edges.iter().filter(|e| e.from == ".") {
        let target = format!("{HIVE}/{}", e.to.trim_start_matches("./"));
        let covered = spec.accepts.iter().any(|l| {
            let headers = probe_guarded(&l.route, e.condition.as_deref());
            meclaw_colony::edge_table::apply_edges(&table, &hive, &headers)
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
            let headers = probe_guarded(&l.route, e.condition.as_deref());
            meclaw_colony::edge_table::apply_edges(&table, &src, &headers)
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
    // The builder pair is skipped here and asserted in its own shape below: its
    // door is not the rim but a SIBLING, which is the whole of what "a level
    // owns what its siblings must share" buys.
    for route in tenant_in
        .iter()
        .filter(|r| !CONSUMED_INSIDE.contains(&r.as_str()))
    {
        assert!(
            carries(".", &child, route),
            "no `. -> {child}` edge on `{route}` — `{TENANT}` accepts that lane and this level \
             declares it, so the container needs a door for it"
        );
    }
    for route in tenant_out
        .iter()
        .filter(|r| !CONSUMED_INSIDE.contains(&r.as_str()))
    {
        assert!(
            carries(&child, ".", route),
            "no `{child} -> .` edge on `{route}` — `{TENANT}` emits that lane and this level \
             declares it, so the container needs an exit for it"
        );
    }

    // GH #425 — the container reaches the baumeister and, since R-Zielfluss (a),
    // the FRONT DOOR rather than the submitter, and both answer it back. Four
    // edges, and the two downward ones discriminate on `hop.build_op` rather
    // than on the lane, because the lane is one and the classes are two.
    //
    // `./orgs -> ./submit` is deliberately absent: one submission front door
    // and not two. An assistant's apply becomes `in_submit` at the front door,
    // travels the submitter, its gate and the broker exactly as before, and the
    // receipt comes back through the front door as well.
    for (to, op) in [
        (format!("./{BAUMEISTER}"), "draft"),
        (format!("./{FRONT_DOOR}"), "apply"),
    ] {
        assert!(
            hp.graph.edges.iter().any(|e| e.from == child
                && e.to == to
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains("'build'") && c.contains(&format!("'{op}'")))),
            "no `{child} -> {to}` edge on `build` with build_op `{op}` — the container \
             cannot reach the one baumeister the colony shares"
        );
        assert!(
            hp.graph.edges.iter().any(|e| e.from == to
                && e.to == child
                && e.modifier
                    .as_ref()
                    .and_then(|m| m.set_hop.get("route"))
                    .is_some_and(|v| v.contains("in_build_result"))),
            "nothing comes back from {to} — a round that answers into nothing is a tool \
             call the brain waits out"
        );
    }

    // And the one edge that leaves the shell for the mutation door.
    let senders: Vec<&str> = hp
        .graph
        .edges
        .iter()
        .filter(|e| {
            e.to == "."
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains("'mutate'"))
        })
        .map(|e| e.from.as_str())
        .collect();
    assert_eq!(
        senders.len(),
        2,
        "the `mutate` lane carries two senders and no more: {senders:?}"
    );
    assert!(
        senders.contains(&format!("./{LOOP}").as_str())
            && senders.contains(&format!("./{SUBMITTER}").as_str()),
        "the control loop and the submitter, and nobody else: {senders:?}"
    );
    assert!(
        !senders.contains(&format!("./{BAUMEISTER}").as_str()),
        "R6: the builder gets NO edge to the mutation door. If this ever passes with \
         ./{BAUMEISTER} in it, the guardrail is gone and the README is fiction"
    );
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
        if e.to != "." {
            // GH #425 — one exception, and it is an assertion rather than a
            // silence: a refusal may stay inside this level if it is being
            // carried BACK TO WHOEVER ASKED, re-stamped onto a lane the
            // receiving occupant accepts. The builder's `error` is exactly
            // that: it becomes `in_build_result` and lands in the chat that
            // raised the request, as a tool_result naming the code. What this
            // rule protects against is a refusal reaching a place that answers
            // nothing — so the test is that the target TAKES the lane, not that
            // the edge points outwards.
            let stamped = e
                .modifier
                .as_ref()
                .and_then(|m| m.set_hop.get("route"))
                .map(|v| v.trim_matches('\'').to_string())
                .unwrap_or_else(|| {
                    panic!(
                        "the refusal edge {} -> {} stays inside the shell and re-stamps no \
                         lane — a refusal that keeps its own name inside a level is one \
                         nobody downstream declared to take",
                        e.from, e.to
                    )
                });
            let occupant = e.to.trim_start_matches("./");
            let (accepts, _) = occupant_routes(if occupant == CONTAINER {
                TENANT
            } else {
                occupant
            });
            assert!(
                accepts.contains(&stamped),
                "the refusal edge {} -> {} re-stamps `{stamped}`, which `{occupant}` does not \
                 accept: {accepts:?}. A refusal delivered onto a lane nobody takes is a \
                 refusal turned into silence, which is what this rule exists to prevent",
                e.from,
                e.to
            );
            continue;
        }
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

    // W6 moved the argus's counting out of `colony.db` and onto a virtual
    // endpoint. Those two lanes travel with the ref; this shell writes no edge
    // for them, and must not look as though it sealed them away.
    assert!(
        text.contains("/colony/ledger"),
        "the README does not say that this shell's argus talks to the colony directly — two \
         absolute lanes travel with the ref and no edge here draws them"
    );
    // And the lane that is deliberately not passed on.
    assert!(
        text.contains(NOT_RE_EMITTED),
        "the README does not account for the `{NOT_RE_EMITTED}` lane"
    );
}

#[test]
fn the_shell_substitutes_nothing_in_its_own_config_values() {
    // The shell has no cell, so it configures nothing: neither `${ctx.*}` nor
    // `${ENV}` occurs in a value it owns. The `ctx` half of that is gated in
    // both directions by `gh292_every_ctx_key_is_declared`, so a `requires.ctx`
    // entry without a use would be red there rather than harmless here — which
    // is why this level declares `env` and no `ctx` at all.
    let dir = shell_dir();
    let mut values = String::new();
    for sub in [None, Some(BROKER), Some(LOOP), Some(CONTAINER)] {
        let d = sub.map_or_else(|| dir.clone(), |s| dir.join(s));
        values.push_str(&std::fs::read_to_string(d.join("config.json")).unwrap());
    }
    assert!(
        !values.contains("${"),
        "the shell's own config values substitute something — then this level configures a cell, \
         and it is no longer only a boundary"
    );
    let requires = template_json();
    let requires = requires
        .get("requires")
        .expect("GH #465: the shell declares its environment surface");
    assert!(
        requires.get("ctx").is_none(),
        "the shell declares a `ctx` requirement while substituting no `${{ctx.*}}`; a mutation \
         would be refused for a key nothing here consumes"
    );
}

/// GH #465 — `requires.env` on the shell is the ROLL-UP of what its refs
/// substitute, derived rather than transcribed, in both directions.
///
/// This is the deliberate exception to the rule `gh292_every_ctx_key_is_declared`
/// states for `ctx` ("a template declares what IT uses"), and the exception is
/// the whole point of the issue: a `ctx` key is supplied by the mutation that
/// instantiates, so the walk over `ref_chain` can collect it at the door and
/// nobody has to know beforehand. An `env` key is supplied by the COLONY, hours
/// earlier, by a person editing a file — and until this block existed the only
/// way to learn that the shell wants `OPENROUTER_API_KEY` was to grow it and
/// watch the control loop fail at its first cycle. So the level says it, and
/// the saying is gated from both sides:
///
/// - **undeclared** — a key one of the refs substitutes and this block omits is
///   a shell that boots and dies later. An occupant bump that adds a key and
///   forgets this file is red here.
/// - **superfluous** — a key declared here that nothing under the shell
///   substitutes is a leaflet: it asks an operator for a value that goes
///   nowhere, and nothing else would ever notice.
/// - **`required` is derived, never chosen.** Exactly the keys written with no
///   POSIX default (`${VAR}`, not `${VAR:-x}`) are required, because those are
///   precisely the ones whose absence the substitution cannot survive.
///
/// The keys come out of `mutation::substitute::collect_env_keys`, the
/// substrate's own scanner, and the declaration out of `templates::read_requires`,
/// the reader the enforcement point uses. A regex of this test's own would be
/// free to disagree with both.
#[test]
fn the_shells_requires_env_is_the_rollup_of_what_its_refs_substitute() {
    use meclaw_colony::mutation::substitute::collect_env_keys;
    use meclaw_colony::templates::read_requires;

    let registry = registry();

    /// Every `${VAR}` reachable from `dir`, following `ref` markers through the
    /// registry — name → "this occurrence carried a default".
    fn walk(
        dir: &std::path::Path,
        registry: &TemplatesRegistry,
        seen: &mut Vec<String>,
        out: &mut std::collections::BTreeMap<String, bool>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut kids: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        kids.sort();
        if dir.join("config.json").is_file() {
            let val = config_at(dir);
            if let Some(reference) = ref_target(dir) {
                if !seen.contains(&reference) {
                    seen.push(reference.clone());
                    let entry = registry
                        .resolve(&reference)
                        .unwrap_or_else(|e| panic!("{reference}: {e}"));
                    walk(&entry.filesystem_path, registry, seen, out);
                }
                return; // a marker's own file carries nothing else.
            }
            for (name, has_default) in collect_env_keys(&val).expect("the config scans") {
                out.entry(name)
                    .and_modify(|d| *d = *d && has_default)
                    .or_insert(has_default);
            }
        }
        for kid in kids {
            if kid.is_dir() {
                walk(&kid, registry, seen, out);
            }
        }
    }

    let mut used = std::collections::BTreeMap::new();
    walk(&shell_dir(), &registry, &mut Vec::new(), &mut used);

    // Anti-vacuity: a walk that found nothing would agree with an empty block.
    assert!(
        used.len() > 10,
        "the ref walk found only {} keys — it stopped following refs",
        used.len()
    );

    let declared = read_requires(&shell_dir()).expect("the shell's `requires` block parses");

    let used_names: Vec<&String> = used.keys().collect();
    let declared_names: Vec<&String> = declared.env.keys().collect();
    assert_eq!(
        declared_names, used_names,
        "`requires.env` and the environment surface under the shell disagree; declared-but-unused \
         asks an operator for nothing, used-but-undeclared is a shell that boots and fails later"
    );

    for (name, has_default) in &used {
        let decl = &declared.env[name];
        assert_eq!(
            decl.required, !has_default,
            "`{name}`: `required` is derived from the tree — a key written `${{{name}}}` with no \
             default is required, one written `${{{name}:-…}}` is not"
        );
        assert!(
            decl.because.as_ref().is_some_and(|b| !b.trim().is_empty()),
            "`{name}` is declared without a `because`; the sentence is what a refusal quotes"
        );
    }

    // The one required key is a fact of the tree, not of this list — but the
    // COUNT is asserted, because a walk that silently stopped finding plain
    // tokens would satisfy every assertion above.
    let required: Vec<&String> = used
        .iter()
        .filter(|(_, has_default)| !*has_default)
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        required.len(),
        1,
        "exactly one value under this shell is written with no default, and it is the reason \
         `requirement_missing` can be pre-destructive at all; found {required:?}"
    );

    // The prose half of the same promise (`docs/development-rules.md` § 2d): the
    // README tells a reader what the shell needs, and the key it names is READ
    // OFF the tree here rather than typed in.
    let text = readme();
    assert!(
        text.contains(required[0].as_str()),
        "the README does not name `{}`, the one key without which this shell refuses to grow",
        required[0]
    );
    assert!(
        text.contains("requirement_missing"),
        "the README does not name the code the refusal carries, so a reader who meets it cannot \
         look it up"
    );
    assert!(
        text.contains(".env.example"),
        "the README does not point at the copy-ready file that lists the surface"
    );
    // The countable half (§ 2d): the sentence stands only while the tree makes
    // it true, and the condition is the derived set two assertions up.
    assert!(
        text.contains("Exactly **one** of those keys is required"),
        "the tree has exactly one required key and the README does not say so"
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
