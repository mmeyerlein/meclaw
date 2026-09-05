//! GH #562 — the recall road rides a v-lane, and the member still stamps.
//!
//! Before this change a `memory_recall` left a brain and crossed three levels to
//! reach the person's memory: the generation's own rim (`./talky -> .`), the
//! `assistants` container (`./scribe -> .`), and only then the member's door,
//! which is the one hop that DOES something — it turns `recall` into `in_query`
//! and stamps `recall_as_of`, `audience_now` and `channel` onto it, the three
//! keys the hive refuses a question without (`missing_audience`,
//! `missing_channel`).
//!
//! Under ADR-0020 the first two levels are exactly what a v-lane is for: one
//! declares nothing about the lane and is transparent, the other declared it
//! only to hand it on. So the chain is replaced by one deep edge per asker,
//! drawn where the container's addressing edges already live, and the
//! generation stops passing the lane through and starts VOUCHING for it —
//! `at: ["./talky", "./cogny"]` on the same contract entry, which is the
//! sentence that says *this lane docks on my two askers, not on my rim*.
//!
//! What this file proves, in the order the failure would happen:
//!
//! 1. **The generation vouches instead of forwarding** — the contract entry
//!    keeps the lane and gains the connect points; the pass-through edges are
//!    gone from the template's graph (a template that kept them would fan every
//!    recall out twice).
//! 2. **The v-lanes are drawn where the chain was** — in the container's own
//!    recipe, carrying `lane` and the `recall_caller` stamp each asker owes.
//! 3. **The member still stamps** — the positive proof, driven through the real
//!    router: a recall from either asker arrives at the hive's `recall` cell as
//!    `in_query`, with the three keys ON it. The member hop fired; skipping it
//!    would have delivered a question the hive refuses.
//! 4. **The bundle still finds the asker that made it** — `recall_caller` round
//!    trip for both askers, defaults included, unchanged by the migration.
//! 5. **A v-lane that would skip the member is refused** — the rule table's
//!    other direction. The member declares the lane WITHOUT a connect point for
//!    a generation's asker, so it is a mandatory hop: an edge that would carry a
//!    brain's recall past the person's level is `v_lane_mandatory_hop`, and the
//!    proof that the declaration is what does it is the same edge judged against
//!    a contract list the member has been taken out of.
//!
//! No colony, no model, no store: the templates and recipes are read off the
//! tree, the router is asked what it would do, and the mutation validator is
//! asked what it would say.
//!
//! Companion files: `gh532_two_askers_one_hive.rs` (the reply-to token itself),
//! `gh533_the_outside_asker_gets_an_answer.rs` (the outside asker's leg) and
//! `gh552_the_recall_schema_is_pinned_to_the_contract.rs`, which measures the
//! SCHEMA half of the same road and is deliberately untouched by this one.

use meclaw_colony::config::{EdgeSpec, HiveParams, LaneSpec};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::mutation::hive_contract::{HiveContract, Lane};
use meclaw_colony::mutation::port_boundary::collect_hive_port_boundary;
use meclaw_colony::mutation::rejection::MutationRejection;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

/// The member of the worked example and the generation inside it — the paths
/// `examples/organism` uses, one segment shorter, exactly as `gh532` reads them.
const MEMBER: &str = "/m";
const HIVE: &str = "/m/memory-hive";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/scribe";

// ───────────────────────────────────────────────────────────────── the harness

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn hive_params(rel: &str) -> HiveParams {
    let cfg = read_json(rel);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{rel}: no params"));
    meclaw_core::serde_json::from_value(params).unwrap_or_else(|e| panic!("{rel}: params: {e}"))
}

/// The `params.graph.edges` of a shipped template, parsed the strict way:
/// `EdgeSpec` is `deny_unknown_fields`, so an edge key the boot would refuse
/// fails here instead of in a live colony — `lane` included.
fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    hive_params(rel).graph.edges
}

/// The `add_edges` of an instantiation recipe, parsed the same strict way.
fn recipe_edges(rel: &str) -> Vec<EdgeSpec> {
    let doc = read_json(rel);
    let raw = doc["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel}: no diff.add_edges"))
        .clone();
    raw.into_iter()
        .map(|e| {
            meclaw_core::serde_json::from_value(e.clone())
                .unwrap_or_else(|err| panic!("{rel}: add_edges entry {e}: {err}"))
        })
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

/// Add one template's (or one recipe's) edges to the table, rebased under
/// `base`. The declared `lane` travels with the edge: routing does not read it,
/// but an edge table that dropped it would let this file pass on a topology the
/// mutation validator refuses.
fn add_edges(table: &mut EdgeTable, base: &str, specs: &[EdgeSpec], label: &str) {
    for spec in specs {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("{label}: condition {src:?}: {e}"))
        });
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{label}: modifier {k}: {e}"))
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition,
            modifier,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
        });
    }
}

/// The levels a recall round trip crosses, in one edge table: the member, its
/// memory hive, the `assistants` container as `examples/organism` wires it, and
/// one generation.
fn shipped_table() -> EdgeTable {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        MEMBER,
        &hive_edges("templates/member/config.json"),
        "member",
    );
    add_edges(
        &mut t,
        HIVE,
        &hive_edges("templates/memory-hive/config.json"),
        "memory-hive",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    add_edges(
        &mut t,
        BOX,
        &recipe_edges("examples/organism/grow-assistant.json"),
        "grow-assistant",
    );
    t
}

fn headers(context: &[(&str, &str)], hop: &[(&str, &str)]) -> Headers {
    let map = |pairs: &[(&str, &str)]| -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    };
    Headers::from_parts(map(context), map(hop))
}

fn hop_of(h: &Headers, key: &str) -> String {
    h.hop.get(key).and_then(|v| v.as_str()).unwrap_or("").into()
}

fn ctx_of(h: &Headers, key: &str) -> String {
    h.context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .into()
}

/// Follow one message through the table until nothing takes it further,
/// insisting at every step that exactly ONE edge does — a fan-out on this road
/// is two questions where the asker asked one, which is the failure a chain left
/// half-migrated would produce.
fn walk(table: &EdgeTable, from: &str, headers: Headers) -> (Vec<String>, Headers) {
    let mut trace = vec![from.to_string()];
    let mut here = Path::new(from);
    let mut hs = headers;
    for _ in 0..24 {
        let out = apply_edges(table, &here, &hs);
        if out.is_empty() {
            return (trace, hs);
        }
        assert_eq!(
            out.len(),
            1,
            "at {} the message fans out to {:?} — a recall has exactly one addressee",
            here.as_str(),
            out.iter().map(|d| d.target.as_str()).collect::<Vec<_>>()
        );
        let d = out.into_iter().next().expect("checked non-empty");
        here = d.target;
        hs = d.headers_out;
        trace.push(here.as_str().to_string());
    }
    panic!("the walk did not settle in 24 hops: {trace:?}");
}

/// The request an asker inside the generation raises — the shape
/// `collector/assemble`'s `recall_ask` builds, every key present and empty
/// rather than absent.
fn recall_request() -> Headers {
    headers(
        &[
            ("assistant", "scribe"),
            ("session_id", "S-42"),
            ("audience_set", "[\"alex\"]"),
            ("channel", "chat:1"),
            ("turn_id", "S-42#7"),
        ],
        &[
            ("route", "recall"),
            ("recall_query", "what did we decide"),
            ("memory_tier", "1"),
            ("recall_window_from", ""),
            ("recall_window_to", ""),
            ("turn_id", "S-42#7"),
        ],
    )
}

fn lane_spec<'a>(lanes: &'a [LaneSpec], route: &str) -> &'a LaneSpec {
    lanes
        .iter()
        .find(|l| l.route == route)
        .unwrap_or_else(|| panic!("no lane '{route}' in the contract"))
}

// ───────────────────────────────── 1. the generation vouches, it does not carry

#[test]
fn the_generation_vouches_for_its_two_askers_and_forwards_nothing() {
    let hp = hive_params("templates/assistant/config.json");
    let c = hp
        .contract
        .as_ref()
        .expect("the assistant declares a contract");

    for (lanes, route) in [(&c.emits, "recall"), (&c.accepts, "in_bundle")] {
        let lane = lane_spec(lanes, route);
        assert_eq!(
            lane.at,
            vec!["./talky".to_string(), "./cogny".to_string()],
            "the '{route}' entry must name the two askers as its connect points: a v-lane docks \
             where the target says it docks, and a declaration WITHOUT `at` is a mandatory hop — \
             which is precisely the pass-through this change removed"
        );
    }

    // The other half: a template that kept the chain AND gained the corridor
    // would deliver every recall twice.
    for (from, to, needle) in [
        ("./talky", ".", "'recall'"),
        ("./cogny", ".", "'recall'"),
        (".", "./talky", "in_bundle"),
        (".", "./cogny", "in_bundle"),
    ] {
        let leftovers: Vec<&EdgeSpec> = hp
            .graph
            .edges
            .iter()
            .filter(|e| {
                e.from == from
                    && e.to == to
                    && e.condition.as_deref().is_some_and(|c| c.contains(needle))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "the assistant still ships the pass-through edge {from} -> {to} on {needle} — the \
             chain and the v-lane are the same delivery twice"
        );
    }
}

// ───────────────────────────────── 2. the v-lanes are drawn where the chain was

#[test]
fn the_container_draws_one_v_lane_per_asker_in_each_direction() {
    let specs = recipe_edges("examples/organism/grow-assistant.json");

    for (asker, stamp) in [("talky", "'talky'"), ("cogny", "'cogny'")] {
        let up: Vec<&EdgeSpec> = specs
            .iter()
            .filter(|e| e.from == format!("./scribe/{asker}") && e.to == ".")
            .collect();
        assert_eq!(
            up.len(),
            1,
            "exactly one recall v-lane out of ./scribe/{asker}, found {}",
            up.len()
        );
        assert_eq!(
            up[0].lane.as_deref(),
            Some("recall"),
            "the deep edge must NAME its lane — an unnamed one is an ordinary edge the seal check \
             judges without ever consulting the generation's connect points"
        );
        let m = up[0]
            .modifier
            .as_ref()
            .unwrap_or_else(|| panic!("./scribe/{asker} -> . must carry a modifier"));
        assert_eq!(
            m.set_context.get("recall_caller").map(String::as_str),
            Some(stamp),
            "the stamp the retired assistant edge carried moves ONTO the v-lane, or the bundle \
             loses the asker that made it"
        );

        let down: Vec<&EdgeSpec> = specs
            .iter()
            .filter(|e| e.from == "." && e.to == format!("./scribe/{asker}"))
            .collect();
        assert_eq!(
            down.len(),
            1,
            "exactly one in_bundle v-lane into ./scribe/{asker}, found {}",
            down.len()
        );
        assert_eq!(
            down[0].lane.as_deref(),
            Some("in_bundle"),
            "the way back is a v-lane too, and it says so"
        );
    }

    // The surface stays the default door, the core the guarded one — the GH #532
    // rule, one level up from where it used to live.
    let core = specs
        .iter()
        .find(|e| e.to == "./scribe/cogny" && e.from == ".")
        .expect("checked above");
    let cond = core
        .condition
        .as_deref()
        .expect("the core's door is guarded");
    assert!(
        cond.contains("hop.recall_caller"),
        "the core's door reads the token off the HOP — a door guarded on context alone can never \
         fire under the template gate's probe (gh173) — got {cond:?}"
    );
    assert!(!core.is_default, "the guarded door is the REGULAR one");
    let surface = specs
        .iter()
        .find(|e| e.to == "./scribe/talky" && e.from == ".")
        .expect("checked above");
    assert!(
        surface.is_default,
        "the surface's door stays the DEFAULT, so a bundle with no token, an empty one or an \
         unknown one lands where every bundle landed before"
    );

    // And the chain the v-lanes replace is gone from the recipe.
    for (from, to, needle) in [
        ("./scribe", ".", "'recall'"),
        (".", "./scribe", "in_bundle"),
    ] {
        assert!(
            !specs.iter().any(|e| e.from == from
                && e.to == to
                && e.condition.as_deref().is_some_and(|c| c.contains(needle))),
            "the recipe still draws the chain edge {from} -> {to} on {needle}"
        );
    }
}

// ─────────────────────────────────────── 3. the member still stamps (the point)

#[test]
fn a_recall_from_either_asker_reaches_the_hive_carrying_the_members_stamps() {
    let t = shipped_table();

    for (asker, token) in [("talky", "talky"), ("cogny", "cogny")] {
        let (trace, arrived) = walk(&t, &format!("{GEN}/{asker}"), recall_request());
        assert_eq!(
            trace.last().map(String::as_str),
            Some("/m/memory-hive/recall"),
            "the {asker} recall must reach the hive's recall cell: {trace:?}"
        );
        assert_eq!(
            hop_of(&arrived, "route"),
            "in_query",
            "and it must arrive on the lane the member's door makes of it"
        );

        // THE proof of this file: the member hop fired. These three keys exist
        // nowhere upstream of it — the hive refuses a question without them
        // (`missing_audience`, `missing_channel`), so a v-lane that skipped the
        // member would turn every recall into a typed refusal.
        assert_eq!(
            ctx_of(&arrived, "audience_now"),
            "[\"alex\"]",
            "the member's door promotes the round's audience: {trace:?}"
        );
        assert_eq!(
            ctx_of(&arrived, "channel"),
            "chat:1",
            "and the room it happened in: {trace:?}"
        );
        assert!(
            arrived.context.contains_key("recall_as_of"),
            "and it sets `recall_as_of` — empty is a VALUE here (the hive reads 'now' from it), so \
             the assertion is about the key being present: {trace:?}"
        );

        assert_eq!(
            ctx_of(&arrived, "recall_caller"),
            token,
            "the v-lane carries the reply-to token the assistant's rim used to stamp"
        );
        assert_eq!(ctx_of(&arrived, "session_id"), "S-42");

        // The road is one hop shorter, and it is the ASSISTANT's rim that fell
        // out — not the member's.
        assert!(
            trace.contains(&BOX.to_string()),
            "the container is still on the road, because the member's stamping door starts there: \
             {trace:?}"
        );
        assert!(
            !trace.contains(&GEN.to_string()),
            "and the generation's rim is not, or nothing was migrated: {trace:?}"
        );
    }
}

// ────────────────────────────────── 4. the bundle comes home to the right asker

#[test]
fn the_bundle_still_finds_the_asker_that_made_it() {
    let t = shipped_table();

    // Both legs leave at once from the two occupants of ONE generation. Routing
    // is per message, so "at the same time" is exactly this: two chains, two
    // contexts, one table.
    let (_, core_at_hive) = walk(&t, &format!("{GEN}/cogny"), recall_request());
    let (_, surface_at_hive) = walk(&t, &format!("{GEN}/talky"), recall_request());

    let answer = |at_hive: &Headers| -> Headers {
        Headers::from_parts(
            at_hive.context.clone(),
            [("route", "bundle"), ("turn_id", "S-42#7")]
                .iter()
                .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
                .collect(),
        )
    };

    let hive_recall = format!("{HIVE}/recall");
    let (core_trace, core_home) = walk(&t, &hive_recall, answer(&core_at_hive));
    let (surface_trace, surface_home) = walk(&t, &hive_recall, answer(&surface_at_hive));

    assert_eq!(
        core_trace.last().map(String::as_str),
        Some("/m/assistants/scribe/cogny"),
        "the core's bundle must reach the core: {core_trace:?}"
    );
    assert_eq!(
        surface_trace.last().map(String::as_str),
        Some("/m/assistants/scribe/talky"),
        "and the surface's the surface: {surface_trace:?}"
    );
    assert_eq!(hop_of(&core_home, "route"), "in_bundle");
    assert_eq!(hop_of(&surface_home, "route"), "in_bundle");
    assert!(
        !core_trace.contains(&GEN.to_string()),
        "and the way back skips the generation's rim as well: {core_trace:?}"
    );
}

#[test]
fn a_bundle_with_no_token_still_goes_where_every_bundle_went() {
    // The compatibility half, and the reason the surface's door is a `default`:
    // an outside caller, an older instance, a lane nobody stamped. None of them
    // dead-letters at the container's path.
    let t = shipped_table();
    for token in ["", "talky", "somebody-else"] {
        let mut hop = vec![("route", "in_bundle"), ("turn_id", "S-42#7")];
        if !token.is_empty() {
            hop.push(("recall_caller", token));
        }
        let (trace, _) = walk(
            &t,
            BOX,
            headers(&[("assistant", "scribe"), ("session_id", "S-42")], &hop),
        );
        assert_eq!(
            trace.last().map(String::as_str),
            Some("/m/assistants/scribe/talky"),
            "token {token:?} must land on the surface: {trace:?}"
        );
    }
}

// ─────────────────────── 5. a v-lane that would skip the member is refused

/// The contracts of the two levels the rule table asks about, resolved onto the
/// paths of the worked example: the person's level and one generation inside it.
///
/// Read off the shipped templates, `at` included — a reconstruction that dropped
/// `at` would prove nothing about the files that ship.
fn contracts_at(member_abs: &str, gen_abs: &str) -> Vec<HiveContract> {
    let of = |rel: &str, path: &str| -> HiveContract {
        let hp = hive_params(rel);
        let spec = hp.contract.expect("declares a contract");
        let lane = |l: &LaneSpec| Lane {
            route: l.route.clone(),
            context: l.context.clone(),
            at: l.at.clone(),
            because: l.because.clone(),
        };
        HiveContract {
            hive_path: path.to_string(),
            accepts: spec.accepts.iter().map(lane).collect(),
            emits: spec.emits.iter().map(lane).collect(),
        }
    };
    vec![
        of("templates/member/config.json", member_abs),
        of("templates/assistant/config.json", gen_abs),
    ]
}

/// Everything the rule table refused about one diff, as `(code, address)`.
///
/// No sealed hive is handed in: this file is about the LANE half of the check,
/// and both endpoints below are hive paths, which a seal never refuses anyway.
fn lane_verdicts(scope: &str, diff: &Value, contracts: &[HiveContract]) -> Vec<(String, String)> {
    let mut rejection = MutationRejection::new();
    collect_hive_port_boundary(diff, scope, &[], contracts, &mut rejection);
    rejection
        .entries()
        .iter()
        .map(|v| (v.code.to_string(), v.address.clone().unwrap_or_default()))
        .collect()
}

#[test]
fn a_v_lane_that_carries_a_recall_past_the_member_is_refused() {
    const ORG: &str = "/os/orgs/acme";
    const ALEX: &str = "/os/orgs/acme/members/alex";
    const SCRIBE: &str = "/os/orgs/acme/members/alex/assistants/scribe";

    let contracts = contracts_at(ALEX, SCRIBE);

    // An org-level author who read "the recall lane is a v-lane now" and drew
    // one straight out of the brain, past the level that stamps the audience,
    // the room and the as-of.
    let skip = json!({
        "add_edges": [{
            "from": "./members/alex/assistants/scribe/talky",
            "to": ".",
            "lane": "recall",
            "condition": "has(hop.route) && hop.route == 'recall'"
        }]
    });
    let verdicts = lane_verdicts(ORG, &skip, &contracts);
    assert_eq!(
        verdicts,
        vec![(
            "v_lane_mandatory_hop".to_string(),
            format!("{SCRIBE}/talky")
        )],
        "the person's level declares the recall lane and names no connect point below it for a \
         generation's asker, so it may not be bypassed — and the generation itself vouches for \
         `./talky`, so it must NOT contribute a second refusal"
    );

    // The declaration is what refuses it: the same edge, judged against a
    // colony in which the member said nothing about the lane, passes.
    let without_member: Vec<HiveContract> = contracts
        .iter()
        .filter(|c| c.hive_path != ALEX)
        .cloned()
        .collect();
    assert!(
        lane_verdicts(ORG, &skip, &without_member).is_empty(),
        "with the member's declaration gone the rule table has nothing to object to — which is \
         what makes that declaration, and not the shape of the tree, the thing that protects the \
         stamp"
    );
}

#[test]
fn the_shipped_v_lane_is_the_one_the_rule_table_allows() {
    const BOX_ABS: &str = "/os/orgs/acme/members/alex/assistants";
    const ALEX: &str = "/os/orgs/acme/members/alex";
    const SCRIBE: &str = "/os/orgs/acme/members/alex/assistants/scribe";

    let contracts = contracts_at(ALEX, SCRIBE);
    let diff = json!({
        "add_edges": recipe_edges("examples/organism/grow-assistant.json")
            .iter()
            .filter(|e| e.lane.is_some())
            .map(|e| json!({"from": e.from, "to": e.to, "lane": e.lane}))
            .collect::<Vec<Value>>()
    });
    assert_eq!(
        diff["add_edges"].as_array().map(Vec::len),
        Some(4),
        "four v-lanes: one recall and one in_bundle per asker"
    );
    assert!(
        lane_verdicts(BOX_ABS, &diff, &contracts).is_empty(),
        "the recipe the tree ships must pass its own rule table: {:?}",
        lane_verdicts(BOX_ABS, &diff, &contracts)
    );
}

// ────────── 6. the other half of the corridor: the rim is CLOSED for the lane

/// Everything the INBOUND-lane check refused about one diff, as
/// `(code, address)`.
///
/// The v-lane rule table is only half of what an `at` says. The other half is
/// what it takes AWAY: a lane that docks below the rim no longer arrives at the
/// rim, so the door that used to be there is gone and an edge still addressed
/// to the hive path is a dead letter waiting to happen. That is the half this
/// helper measures — `check_inbound_lanes`, the outward face of the contract.
fn inbound_verdicts(
    scope: &str,
    diff: &Value,
    contracts: &[HiveContract],
) -> Vec<(String, String)> {
    let mut rejection = MutationRejection::new();
    meclaw_colony::mutation::hive_contract::collect_inbound_lanes(
        diff,
        scope,
        contracts,
        &mut rejection,
    );
    rejection
        .entries()
        .iter()
        .map(|v| (v.code.to_string(), v.address.clone().unwrap_or_default()))
        .collect()
}

#[test]
fn an_edge_that_delivers_the_lane_at_the_rim_is_refused_by_name() {
    const BOX_ABS: &str = "/os/orgs/acme/members/alex/assistants";
    const ALEX: &str = "/os/orgs/acme/members/alex";
    const SCRIBE: &str = "/os/orgs/acme/members/alex/assistants/scribe";
    let contracts = contracts_at(ALEX, SCRIBE);

    // The edge that WORKED before the migration and silently stops working
    // after it: a bundle addressed at the generation's own path. The lane is
    // still declared, so the inbound check would wave it through on the lane
    // name alone — and nothing inside routes an `in_bundle` arriving there any
    // more, so it would dead-letter at runtime, which is exactly the class
    // GH #173 exists to refuse ("a lane a caller cannot receive is not part of
    // an interface").
    let at_the_rim = json!({
        "add_edges": [{
            "from": ".",
            "to": "./scribe",
            "condition": "has(hop.route) && hop.route == 'in_bundle'",
            "modifier": {"set_hop": {"route": "'in_bundle'"}}
        }]
    });
    let verdicts = inbound_verdicts(BOX_ABS, &at_the_rim, &contracts);
    assert_eq!(
        verdicts,
        vec![("hive_contract".to_string(), SCRIBE.to_string())],
        "an edge that states `in_bundle` INTO the generation's path must be refused: the lane \
         names connect points, so it does not arrive here any more"
    );

    // And the same lane, ending where the contract says it docks, is fine —
    // which is what makes the refusal a redirection rather than a wall.
    let at_the_connect_point = json!({
        "add_edges": [{
            "from": ".",
            "to": "./scribe/talky",
            "lane": "in_bundle",
            "condition": "has(hop.route) && hop.route == 'in_bundle'",
            "modifier": {"set_hop": {"route": "'in_bundle'"}}
        }]
    });
    assert!(
        inbound_verdicts(BOX_ABS, &at_the_connect_point, &contracts).is_empty(),
        "the v-lane form of the same delivery is the one the contract asks for: {:?}",
        inbound_verdicts(BOX_ABS, &at_the_connect_point, &contracts)
    );

    // A lane WITHOUT connect points is judged exactly as it always was: it
    // arrives at the rim, and the rim is where its door is.
    let ordinary = json!({
        "add_edges": [{
            "from": ".",
            "to": "./scribe",
            "condition": "has(hop.route) && hop.route == 'in_turn'",
            "modifier": {"set_hop": {"route": "'in_turn'"}}
        }]
    });
    assert!(
        inbound_verdicts(BOX_ABS, &ordinary, &contracts).is_empty(),
        "an ordinary lane still docks at the path it always docked at"
    );
}
