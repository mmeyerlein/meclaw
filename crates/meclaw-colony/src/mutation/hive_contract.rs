//! GH #173 — a hive declares its lanes, and the declaration is checked.
//!
//! A template is supposed to be a class: you instantiate it, you wire to its
//! interface, and you can later swap it for another implementation with a
//! different inside. For hive templates none of that held. `contract` was a
//! CELL property; a hive had `description` prose, and the prose named cells
//! three levels down ("Ingress: `./keeper/stamp`"). So every instantiation
//! copied the template's internal layout into the caller's own topology, and
//! the first rearrangement of the inside broke every caller.
//!
//! `params.contract` says the same thing in the vocabulary that survives a
//! reimplementation — LANES:
//!
//! ```json
//! "params": {
//!   "ports": [],
//!   "contract": {
//!     "accepts": [{"route": "in_batch", "context": ["session_id"],
//!                  "because": "one closed session as a single write batch"}],
//!     "emits": [{"route": "episode", "because": "one message per turn"}]
//!   }
//! }
//! ```
//!
//! Read as: *send me a message at MY path whose `hop.route` is `in_batch`, and
//! I will hand you back messages at MY path whose `hop.route` is `episode`.* No
//! cell of the hive appears anywhere, which is what makes the inside free to
//! change.
//!
//! # The two halves of the check
//!
//! **Outward** ([`check_inbound_lanes`]) — the caller is checked against the
//! contract. An `add_edges` entry that targets a contracted hive and stamps a
//! literal `hop.route` must name a lane the hive accepts. Without it a typo is
//! a dead letter at runtime that reads like a model failure.
//!
//! **Inward** ([`check_lane_doors`]) — the hive is checked against its own
//! contract. Every accepted lane must be routable from the hive path to
//! somewhere inside it; every emitted lane must be routable from somewhere
//! inside back out through the hive path. This is the half that keeps the
//! declaration from rotting into decoration: rearranging the inside is free,
//! but rearranging it so a declared lane loses its door is not. One kind of
//! lane is exempt, and says so itself: a lane that names connect points (`at`,
//! GH #559) docks BELOW the rim by declaration, so its door is that address and
//! not an edge out of the hive path — see [`docks_below_the_rim`].
//!
//! # Why it asks the router instead of reading the text
//!
//! Same reasoning as [`crate::mutation::required_drains`]. Comparing condition
//! source strings breaks the first time somebody writes `hop.route=='in_batch'`
//! instead of `hop.route == 'in_batch'`, or opens a family of lanes with one
//! `startsWith('in_')` — which is exactly how the migrated templates are
//! written. So the check BUILDS the hop the lane describes and runs it through
//! [`crate::edge_table::apply_edges`], the same function that routes the real
//! message. There is no second opinion to disagree with the first.
//!
//! One thing the router alone cannot answer, though (GH #176): an exit that
//! does not CARRY the lane but CREATES it. A hive's failure exit recognises
//! something only the inside knows — an `llm` cell's `hop.finish_reason` — and
//! translates it into a lane, which is the entire reason the boundary exists.
//! A probe carrying `hop.route` never satisfies that condition, so the exit
//! check reads the out-door's own `set_hop.route` as well: a door that STATES
//! the declared lane and crosses the hive path is an exit for it, whether or
//! not a route probe can make it fire. Stating a different lane is not, and
//! stating the lane on an edge that stays inside is not either — a caller
//! cannot receive what never crosses the boundary. See [`door_states_lane`].
//!
//! The caller's stamped route is read the same way: the edge's own
//! `set_hop.route` expression is compiled and evaluated against EMPTY headers.
//! A literal (`'in_batch'`) yields a string; anything that reads the incoming
//! message (`hop.upstream_route`, `'in_' + hop.kind`) fails to evaluate and is
//! skipped. A check that cannot say which lane an edge means must never reject
//! it — the same conservatism the port boundary is built on.
//!
//! # What is deliberately NOT checked
//!
//! - **A disconnected hive.** A contract is a statement about the hive PATH; if
//!   no edge in the colony touches that path, the hive is an island — freshly
//!   instantiated and not yet wired, or disconnected by `remove_nodes` — and
//!   its contract is dormant, not broken. Without this a contracted hive could
//!   not be taken out of a colony at all: `remove_nodes` drops the hive path's
//!   own edges and would leave a contract with no doors behind.
//! - **`accepts[].context` — no longer (GH #291).** This bullet claimed a
//!   promotion three edges upstream was indistinguishable from a missing one.
//!   It is not: GH #185's backwards reachability walk answers exactly that
//!   question, and the lane requirement is now checked with it
//!   ([`crate::mutation::validate::collect_hive_lane_context`], projected from
//!   these contracts by [`lane_requirements`]). What stays unchecked is an edge
//!   whose route is COMPUTED rather than stated, and an edge whose caller side
//!   is a hive path with no inbound edge — a dormant requirement, not a broken
//!   one.
//! - **A caller's subscription condition.** Checking that an edge OUT of the
//!   hive names a declared emit would have to evaluate the caller's condition
//!   against a probe hop, and the shipped topologies tell lanes apart by a
//!   second hop key (`hop.round_capped`) that a route-only probe does not
//!   carry. That check would refuse correct wirings, so it is not made. See the
//!   issue thread.
//! - **The bootstrap.** Boot WARNS ([`warn_on_broken_contracts`]) and never
//!   refuses, for the reason the port boundary leaves boot alone: the birth
//!   topology is the colony author's sovereign design, and a colony that cannot
//!   boot is worse than one that boots with a loud line in its log.

use crate::edge_table::EdgeTable;
use crate::mutation::MutationError;
use meclaw_core::{Headers, JsonValue, Path};

/// One lane of a hive contract, resolved for checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    /// The `hop.route` value that IS this lane.
    pub route: String,
    /// GH #291 — `context` keys a caller must have promoted by the time a
    /// message enters on this lane. Carried here so the lane-context check
    /// (`crate::mutation::validate::validate_hive_lane_context`) can be handed
    /// the requirement without reading `config.json` a second time. Empty ⇒ the
    /// lane requires nothing, which is also what a lane that omits the key
    /// says — `LaneSpec::context` is `#[serde(default)]`, so an absent list and
    /// an explicit `[]` are the same statement.
    pub context: Vec<String>,
    /// GH #559 — the connect points this lane declares
    /// ([`crate::config::LaneSpec::at`]): the relative paths below the hive a
    /// v-lane on this lane may end on. Carried here for the same reason
    /// `context` is — the v-lane verdict
    /// ([`crate::mutation::port_boundary::v_lane_verdict`]) is handed the
    /// declaration instead of reading `config.json` a second time.
    pub at: Vec<String>,
    /// The hive's own sentence about the lane, quoted verbatim in a rejection.
    pub because: String,
}

/// A hive that declared `params.contract`, with its path resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiveContract {
    /// Absolute logical path of the hive scope (e.g. `/main/memory`).
    pub hive_path: String,
    /// Lanes a caller may send into the hive path.
    pub accepts: Vec<Lane>,
    /// Lanes the hive sends back out through its own path.
    pub emits: Vec<Lane>,
}

impl HiveContract {
    /// True iff `abs` lies STRICTLY below the hive path.
    fn is_interior(&self, abs: &str) -> bool {
        if self.hive_path == "/" {
            return abs != "/" && abs.starts_with('/');
        }
        abs.starts_with(&format!("{}/", self.hive_path))
    }

    /// The headers a message on `route` carries, as the router would see them.
    fn probe(route: &str) -> Headers {
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("route".to_string(), JsonValue::String(route.to_string()));
        Headers::from_parts(meclaw_core::serde_json::Map::new(), hop)
    }
}

/// True iff anything at all in the colony's topology touches the hive path.
///
/// A hive nobody addresses is an island, and an island's contract is dormant
/// (see the module note on what is not checked).
fn hive_path_is_wired(c: &HiveContract, edges: &EdgeTable) -> bool {
    edges
        .iter()
        .any(|e| e.from.as_str() == c.hive_path || e.to.as_str() == c.hive_path)
}

/// True iff a message arriving at the hive path on `route` reaches a cell
/// inside the hive — the hive has a door for that lane.
fn door_exists(c: &HiveContract, route: &str, edges: &EdgeTable) -> bool {
    let hive = Path::new(&c.hive_path);
    crate::edge_table::apply_edges(edges, &hive, &HiveContract::probe(route))
        .iter()
        .any(|d| c.is_interior(d.target.as_str()))
}

/// True iff this edge's own modifier PRODUCES `route` on the message it takes.
///
/// GH #176. The route probe below can only find a lane a message ALREADY
/// carries. The out-door of a failure lane does not work that way: it
/// recognises something the inside knows — an `llm` cell's `hop.finish_reason`
/// — and TRANSLATES it into a lane, which is the whole point of the boundary.
/// A probe carrying `hop.route` never satisfies that condition, so without
/// reading the modifier such a door is invisible and the hive is refused a lane
/// it demonstrably has.
///
/// The condition is deliberately NOT evaluated. Whether the door ever fires is
/// a statement about the messages the inside produces; whether it names the
/// lane is a statement about the door, and only the second one is checkable
/// here.
///
/// Three verdicts, and the middle one is the guard:
/// - no `set_hop.route` at all → this edge names no lane, `false`;
/// - a constant that is a DIFFERENT lane → it names someone else's, `false`;
/// - an expression that reads the message (`hop.upstream_route`) → which lane
///   it means is knowable only once that message exists, so it is not judged
///   and counts. Same conservatism as [`stated_route`] on the outward half: a
///   check that cannot say which lane an edge means must never reject it.
fn door_states_lane(edge: &crate::edge_table::Edge, route: &str) -> bool {
    let Some(src) = edge
        .modifier
        .as_ref()
        .and_then(|m| m.source.set_hop.get("route"))
    else {
        return false;
    };
    match constant_route(src) {
        Some(stated) => stated == route,
        None => true,
    }
}

/// True iff SOME node inside the hive routes `route` back out through the hive
/// path — the hive has an exit for that lane.
///
/// Every interior source is tried rather than a declared one, because which
/// cell produces a lane is precisely the fact the contract exists to hide.
///
/// Two ways to be an exit, and both have to cross the hive path: an edge that
/// CARRIES the lane out (the probe), or an edge that CREATES it on the way out
/// (the modifier, GH #176 — see [`door_states_lane`]). Producing the lane on an
/// edge that stays inside is not an exit: a caller cannot receive a message
/// that never crosses the boundary.
fn exit_exists(c: &HiveContract, route: &str, edges: &EdgeTable) -> bool {
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

/// INWARD — every declared lane of every wired, contracted hive has a door (or,
/// for an emit, an exit) in the POST-state topology.
///
/// `edges` is the table as it will be once the diff is applied: the mutation
/// that instantiates a hive brings its own internal graph along, and a
/// pre-state check would refuse exactly that mutation.
///
/// GH #293 — this is the thin `Result` face of [`addressed_lane_doors`]: the
/// FIRST violation the collecting core produces is, by construction, the one
/// this function returned before, so every verdict it ever gave is
/// byte-identical.
pub fn check_lane_doors(
    contracts: &[HiveContract],
    edges: &EdgeTable,
) -> Result<(), MutationError> {
    addressed_lane_doors(contracts, edges)
        .into_iter()
        .next()
        .map_or(Ok(()), |(error, _, _)| Err(error))
}

/// GH #293 — the lane-door check as a COLLECTING one: every declared lane of
/// every wired, contracted hive that lost its door, in one refusal.
///
/// This is where the hive's own `because` becomes structured (spec § Part 3:
/// "`required_drains[].because` and `LaneSpec.because` exist precisely to travel
/// verbatim into a refusal"). The sentence stays in the message, exactly where
/// it was, AND rides along in [`Violation::because`] — so a reader that wants
/// the hive's reason no longer has to fish it out of prose with a bracket
/// match.
///
/// Tagged [`Stage::ContractLocality`] because that is what it decides: whether
/// the lanes a hive's contract promises are still local to it. It runs later in
/// the pipeline than the other two contract-locality checks — it needs the
/// POST-state edge table, which does not exist until the diff's edges are in RAM
/// — and it keeps its own rollback path there.
///
/// **This changes no verdict** — see [`check_lane_doors`], which is now the
/// first-error face of the same core.
///
/// [`Violation::because`]: crate::mutation::rejection::Violation::because
/// [`Stage::ContractLocality`]: crate::mutation::rejection::Stage::ContractLocality
pub fn collect_lane_doors(
    contracts: &[HiveContract],
    edges: &EdgeTable,
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address, because) in addressed_lane_doors(contracts, edges) {
        into.push(Violation::from_error_because(
            Stage::ContractLocality,
            &error,
            Some(address),
            because,
        ));
    }
}

/// GH #559 / ADR-0020 — true iff this lane docks BELOW the rim by declaration.
///
/// A lane that names connect points (`at`) says where in the hive it lands, and
/// that address is never the hive path: `at` is a list of paths strictly below
/// it, and the v-lane rule table refuses a lane-carrying edge that ends anywhere
/// else ([`crate::mutation::port_boundary::v_lane_verdict`], rows 3 and 5). So
/// the door of such a lane is the connect point, not an edge out of `.`, and
/// demanding a rim door for it would refuse exactly the migration ADR-0020
/// describes: the level whose pass-through hop was replaced by one deep edge no
/// longer has — and must no longer have — a rim edge for the lane it vouches
/// for.
///
/// Only the DOOR obligation lifts, and it is REPLACED rather than dropped:
/// [`addressed_inbound_lanes`] refuses an edge that delivers such a lane AT the
/// hive path, because the door it would arrive at is precisely the one the
/// migration struck. Everything else about the lane stays where it was: the
/// connect points are still policed per edge at mutation time, the declaration
/// still makes the level a mandatory hop for every address it does NOT name,
/// and a lane with no `at` is judged exactly as it always was.
fn docks_below_the_rim(lane: &Lane) -> bool {
    !lane.at.is_empty()
}

/// The collecting core: every lane without a door, with the hive path it
/// concerns and the hive's own sentence about that lane.
fn addressed_lane_doors(
    contracts: &[HiveContract],
    edges: &EdgeTable,
) -> Vec<(MutationError, String, String)> {
    let mut violations = Vec::new();
    for c in contracts {
        if !hive_path_is_wired(c, edges) {
            continue;
        }
        for lane in &c.accepts {
            if docks_below_the_rim(lane) || door_exists(c, &lane.route, edges) {
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
            if docks_below_the_rim(lane) || exit_exists(c, &lane.route, edges) {
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

/// The `hop.route` an edge's own modifier STATES, if it states a constant.
///
/// Compiles just the `set_hop.route` expression and runs it against empty
/// headers with the real evaluator. `'in_batch'` is a constant and yields
/// `Some("in_batch")`; `hop.upstream_route` reads a message that does not exist
/// yet, fails, and yields `None` — unknown, therefore unjudged.
fn stated_route(edge: &JsonValue) -> Option<String> {
    constant_route(
        edge.get("modifier")?
            .get("set_hop")?
            .get("route")?
            .as_str()?,
    )
}

/// The constant a `set_hop.route` EXPRESSION states, if it states one.
///
/// Compiles just that one expression and runs it against empty headers with the
/// real evaluator, so there is no second opinion about what `'in_batch'` means.
/// Anything that reads a message which does not exist yet fails to evaluate and
/// yields `None` — unknown, not wrong. Shared by both halves of the file:
/// outward it decides whether an `add_edges` entry may be judged at all,
/// inward ([`door_states_lane`]) whether an out-door names the lane it claims.
pub(crate) fn constant_route(src: &str) -> Option<String> {
    let mut spec = crate::config::ModifierSpec::default();
    spec.set_hop.insert("route".to_string(), src.to_string());
    let compiled = crate::cel_eval::parse_modifier(&spec).ok()?;
    let out = crate::cel_eval::apply_modifier(&compiled, &Headers::new()).ok()?;
    out.hop.get("route")?.as_str().map(|s| s.to_string())
}

/// OUTWARD — PURE, no FS, no DB. Refuse an `add_edges` entry that sends a
/// stated lane into a contracted hive that does not accept it.
///
/// `guard_scope` is the mutation's scope; endpoints resolve against it exactly
/// as the apply side resolves them, so validate and apply cannot disagree about
/// which node an endpoint means.
///
/// Pre-destructive by contract: the caller runs this before staging, so a
/// reject leaves the colony untouched.
///
/// GH #293 — this is the thin `Result` face of [`addressed_inbound_lanes`]: the
/// FIRST violation the collecting core produces is, by construction, the one
/// this function returned before, so every verdict it ever gave is
/// byte-identical.
pub fn check_inbound_lanes(
    diff: &JsonValue,
    guard_scope: &str,
    contracts: &[HiveContract],
) -> Result<(), MutationError> {
    addressed_inbound_lanes(diff, guard_scope, contracts)
        .into_iter()
        .next()
        .map_or(Ok(()), |(error, _)| Err(error))
}

/// GH #293 — stage 6 ([`Stage::ContractLocality`]) as a COLLECTING check, the
/// inbound-lane third of it: EVERY `add_edges` entry that sends a stated lane
/// into a hive that does not accept it is named, not only the first one.
///
/// **This changes no verdict** — see [`check_inbound_lanes`], which is now the
/// first-error face of the same core.
///
/// No `because` travels with these entries, and that is not an omission: the
/// lane being refused is one the hive precisely does NOT declare, so there is no
/// contract sentence about it to quote. The `because` of the lanes it DOES
/// declare is already in the message, as the list of what is on offer.
///
/// [`Stage::ContractLocality`]: crate::mutation::rejection::Stage::ContractLocality
pub fn collect_inbound_lanes(
    diff: &JsonValue,
    guard_scope: &str,
    contracts: &[HiveContract],
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address) in addressed_inbound_lanes(diff, guard_scope, contracts) {
        into.push(Violation::from_error(
            Stage::ContractLocality,
            &error,
            Some(address),
        ));
    }
}

/// The collecting core: every stated lane an `add_edges` entry sends into a hive
/// that does not accept it, with the RESOLVED hive path it was sent to as its
/// address.
fn addressed_inbound_lanes(
    diff: &JsonValue,
    guard_scope: &str,
    contracts: &[HiveContract],
) -> Vec<(MutationError, String)> {
    let mut violations = Vec::new();
    if contracts.is_empty() {
        return violations;
    }
    let Some(adds) = diff.get("add_edges").and_then(|v| v.as_array()) else {
        return violations;
    };
    for e in adds {
        let Some(to) = e.get("to").and_then(|v| v.as_str()) else {
            continue; // a missing endpoint is an `edge_schema` reject elsewhere
        };
        let to_abs = crate::mutation::resolve_scoped_path(guard_scope, to);
        let Some(c) = contracts.iter().find(|c| c.hive_path == to_abs.as_str()) else {
            continue;
        };
        let Some(route) = stated_route(e) else {
            continue; // the lane is only knowable at runtime — not this check's business
        };
        if let Some(lane) = c.accepts.iter().find(|l| l.route == route) {
            if !docks_below_the_rim(lane) {
                continue;
            }
            // GH #562 — the other half of a connect point, and the reason the
            // door check may lift its obligation at all (`docks_below_the_rim`).
            // The lane IS declared, so the check above would wave this edge
            // through on the name alone; but `at` says it docks below the rim,
            // the rim door was struck with the pass-through it belonged to, and
            // nothing inside routes the lane arriving here any more. That is a
            // silent dead letter at runtime, which is the class GH #173 exists
            // to refuse — *a lane a caller cannot receive is not part of an
            // interface* — and it is a REDIRECTION rather than a wall: the same
            // delivery, ended on one of the connect points and naming its lane,
            // is the edge the contract asks for.
            //
            // `hive_contract` and not a fourth `v_lane_*` code, deliberately:
            // the three v-lane codes judge a DEEP edge against the rule table,
            // and this edge is not deep at all. What it gets wrong is which
            // address a declared lane arrives at — the question this check has
            // always answered.
            violations.push((
                MutationError::HiveContract(format!(
                    "add_edges[].to='{to}' sends hop.route='{route}' into hive '{hive}', which \
                     declares that lane with CONNECT POINTS ({because}) — so it does not arrive \
                     at '{hive}' at all. A lane with `at` docks below the rim: end the edge on \
                     one of {at} and name it with \"lane\": \"{route}\", which is the v-lane \
                     the connect point permits (ADR-0020).",
                    hive = c.hive_path,
                    because = lane.because,
                    at = lane.at.join(", "),
                )),
                c.hive_path.clone(),
            ));
            continue;
        }
        let offered = if c.accepts.is_empty() {
            "none — this hive accepts nothing at its path".to_string()
        } else {
            c.accepts
                .iter()
                .map(|l| l.route.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        violations.push((
            MutationError::HiveContract(format!(
                "add_edges[].to='{to}' sends hop.route='{route}' into hive '{hive}', which does \
                 not accept that lane. Lanes '{hive}' accepts: {offered}. A hive's interface is \
                 its lanes, not its cells — pick one, or the hive has to grow a door for yours.",
                hive = c.hive_path,
            )),
            c.hive_path.clone(),
        ));
    }
    violations
}

/// GH #291 — project the collected contracts into the lane requirements the
/// pure lane-context check consumes
/// ([`crate::mutation::validate::collect_hive_lane_context`]).
///
/// One requirement per `accepts[]` entry, so the check module keeps knowing
/// nothing about `config.json`. Both call sites — the mutation pipeline's
/// fourth stage-6 check and the boot report — go through THIS function, which
/// is the whole point: two projections would be two chances to answer the same
/// question differently.
///
/// The one conversion that happens here: [`Lane::because`] is a `String` and
/// [`HiveLaneRequirement::because`] is an `Option<String>`, because a lane that
/// says nothing must render as nothing. An empty sentence becomes `None`, so a
/// lane without a `because` never produces `lane 'x'()` or a dangling dash in a
/// refusal.
pub fn lane_requirements(
    contracts: &[HiveContract],
) -> Vec<crate::mutation::validate::HiveLaneRequirement> {
    let mut out = Vec::new();
    for contract in contracts {
        for lane in &contract.accepts {
            out.push(crate::mutation::validate::HiveLaneRequirement {
                hive_path: contract.hive_path.clone(),
                route: lane.route.clone(),
                context: lane.context.clone(),
                because: (!lane.because.is_empty()).then(|| lane.because.clone()),
            });
        }
    }
    out
}

/// Call-site adapter (NOT pure — reads `config.json`): collect the contract
/// every hive in the colony declared.
///
/// Same source and same reasoning as
/// [`crate::mutation::port_boundary::collect_sealed_hives`]: the declaration
/// lives in the hive's own birth config, so it survives a reboot without a
/// `colony.db` schema change, and a hive whose config is missing or unparseable
/// contributes nothing rather than an invented rule.
pub fn collect_hive_contracts<'a>(
    root: &std::path::Path,
    hive_paths: impl Iterator<Item = &'a Path>,
) -> Vec<HiveContract> {
    let mut out = Vec::new();
    for logical in hive_paths {
        let s = logical.as_str();
        if s == "/" {
            // The root scope has no outside; a contract there would face nobody.
            continue;
        }
        let (scope, name) = match s.rfind('/') {
            Some(0) => ("/", &s[1..]),
            Some(i) => (&s[..i], &s[i + 1..]),
            None => continue,
        };
        let cell_dir = crate::path_truth::resolve_cell_dir(root, scope, name);
        if let Some(c) = contract_from_cell_dir(&cell_dir, s) {
            out.push(c);
        }
    }
    out
}

/// The contract ONE hive declares, read out of the `config.json` in `cell_dir`
/// and labelled with the logical path `hive_path` it will stand at.
///
/// Split out of [`collect_hive_contracts`] for GH #559: a `swap_nodes` has to
/// ask the SUCCESSOR's contract about a v-lane before the successor exists on
/// disk, and the only place that declaration lives at that moment is the
/// TEMPLATE directory. Two readers of one declaration would be free to
/// disagree about what `params.contract` means; one reader cannot.
///
/// `None` when the config is missing, unreadable, unparseable, or simply
/// declares no contract — the same conservatism the collector has always had.
#[must_use]
pub fn contract_from_cell_dir(cell_dir: &std::path::Path, hive_path: &str) -> Option<HiveContract> {
    let raw = std::fs::read_to_string(cell_dir.join("config.json")).ok()?;
    let val = meclaw_core::serde_json::from_str::<JsonValue>(&raw).ok()?;
    let params = val.get("params").cloned().unwrap_or(JsonValue::Null);
    if params.is_null() {
        return None;
    }
    let hp = meclaw_core::serde_json::from_value::<crate::config::HiveParams>(params).ok()?;
    let spec = hp.contract?;
    let lane = |l: &crate::config::LaneSpec| Lane {
        route: l.route.clone(),
        context: l.context.clone(),
        at: l.at.clone(),
        because: l.because.clone(),
    };
    Some(HiveContract {
        hive_path: hive_path.to_string(),
        accepts: spec.accepts.iter().map(lane).collect(),
        emits: spec.emits.iter().map(lane).collect(),
    })
}

/// One edge as the boot check receives it from `/colony/graph`: endpoints, the
/// `condition` source, the `modifier` source spec as JSON, and the routing
/// phase.
///
/// GH #283: the fifth term is the default phase. The boot checks rebuild an
/// [`EdgeTable`] from these tuples and then ask it routing questions — so a
/// term this tuple does not carry is a property the check judges the topology
/// WITHOUT, on a table that is not the one the colony runs.
///
/// GH #367 made the term real: `GraphEdgeDto` names the phase, so
/// [`crate::boot_edges_from_graph`] fills it from the graph instead of the
/// literal `false` the producer carried while the DTO was silent.
pub type BootEdge = (String, String, Option<String>, Option<JsonValue>, bool);

/// GH #173, the boot half: say it out loud when a hive's own graph no longer
/// carries a lane its contract promises.
///
/// A warning, never a refusal — the mutation path is where this rule bites,
/// because that is somebody changing a colony they did not necessarily build.
/// The birth topology is authorship, the same reason the port boundary and the
/// required drains leave the bootstrap alone. And a colony held hostage by its
/// own documentation would be a worse substrate than one that says so in the log.
pub fn warn_on_broken_contracts(contracts: &[HiveContract], edges: &[BootEdge]) {
    if contracts.is_empty() {
        return;
    }
    let table = edge_table_from_boot_edges(edges);
    if let Err(e) = check_lane_doors(contracts, &table) {
        tracing::warn!(reason = %format!("{e:?}"), "a hive contract does not match its own graph");
    }
}

/// Rebuild a routable [`EdgeTable`] from what `/colony/graph` reported.
///
/// This is the table both boot probes ask their routing questions of, so it has
/// to be the table the colony runs — an edge whose phase or modifier is dropped
/// here is a verdict taken over a topology that does not exist. An edge whose
/// condition does not parse is skipped with a warning rather than guessed at.
pub fn edge_table_from_boot_edges(edges: &[BootEdge]) -> EdgeTable {
    let mut table = EdgeTable::new();
    for (from, to, cond, modifier, is_default) in edges {
        let condition = match cond {
            None => None,
            Some(src) => match crate::cel_eval::parse_condition(src) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(
                        from = %from, to = %to, error = %e,
                        "hive contract check: edge condition does not parse — edge ignored here"
                    );
                    continue;
                }
            },
        };
        // GH #176: the modifier travels with the edge here, because an out-door
        // that TRANSLATES a model field into a lane is only visible through it
        // (see `door_states_lane`). Dropping it would make boot warn about
        // exactly the hives that got their failure lane right.
        let modifier = modifier.as_ref().and_then(|raw| {
            let spec =
                meclaw_core::serde_json::from_value::<crate::config::ModifierSpec>(raw.clone())
                    .ok()?;
            crate::cel_eval::parse_modifier(&spec).ok()
        });
        table.insert(crate::edge_table::Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition,
            modifier,
            // GH #283: the phase as the caller read it off the running graph.
            is_default: *is_default,
            lane: None,
        });
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel_eval::parse_condition;
    use meclaw_core::Uuid;
    use meclaw_core::serde_json::json;

    fn edge(from: &str, to: &str, cond: Option<&str>) -> crate::edge_table::Edge {
        crate::edge_table::Edge {
            id: Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: cond.map(|c| parse_condition(c).expect("condition parses")),
            modifier: None,
            is_default: false,
            lane: None,
        }
    }

    /// The same edge with a `set_hop.route` on it — a door that STAMPS a lane
    /// rather than one that reads it.
    fn stamping_edge(
        from: &str,
        to: &str,
        cond: Option<&str>,
        route_expr: &str,
    ) -> crate::edge_table::Edge {
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop
            .insert("route".to_string(), route_expr.to_string());
        crate::edge_table::Edge {
            modifier: Some(crate::cel_eval::parse_modifier(&spec).expect("modifier parses")),
            ..edge(from, to, cond)
        }
    }

    /// The shape GH #176 needs: an inner failure edge that recognises the model
    /// field and translates it into a lane on the way out.
    fn translating_table(route_expr: &str) -> EdgeTable {
        let mut t = EdgeTable::new();
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route.startsWith('in_')"),
        ));
        t.insert(stamping_edge(
            "/mem/glue",
            "/mem",
            Some("has(hop.finish_reason) && hop.finish_reason == 'error'"),
            route_expr,
        ));
        t
    }

    fn lane(route: &str) -> Lane {
        Lane {
            route: route.into(),
            context: Vec::new(),
            at: Vec::new(),
            because: format!("the {route} lane"),
        }
    }

    fn drain_contract() -> Vec<HiveContract> {
        vec![HiveContract {
            hive_path: "/mem".into(),
            accepts: vec![lane("in_batch")],
            emits: vec![lane("episode")],
        }]
    }

    /// The shape the migrated templates have: one door, one exit, both written
    /// as a family filter rather than a per-lane equality.
    fn sound_table() -> EdgeTable {
        let mut t = EdgeTable::new();
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route.startsWith('in_')"),
        ));
        t.insert(edge(
            "/mem/glue",
            "/mem",
            Some("has(hop.route) && !hop.route.startsWith('in_')"),
        ));
        t
    }

    // ---- inward: the hive against its own contract ----

    #[test]
    fn a_family_filter_door_satisfies_the_lane_it_covers() {
        // The reason this check runs the router instead of reading the text: no
        // string in `startsWith('in_')` contains `in_batch`.
        assert!(check_lane_doors(&drain_contract(), &sound_table()).is_ok());
    }

    #[test]
    fn an_accepted_lane_without_a_door_is_named() {
        let mut t = EdgeTable::new();
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route == 'in_other'"),
        ));
        t.insert(edge("/mem/glue", "/mem", None));
        let err = check_lane_doors(&drain_contract(), &t).expect_err("no door for in_batch");
        assert!(format!("{err:?}").contains("in_batch"), "{err:?}");
    }

    /// GH #559 / #562 — a lane that names connect points owes no rim door.
    ///
    /// This is the shape a migrated level has: the pass-through hop is gone, one
    /// deep edge delivers, and the contract entry that used to describe the hop
    /// now says where the lane docks instead. Demanding a door out of `.` here
    /// would refuse the migration itself.
    #[test]
    fn a_lane_that_names_its_connect_points_needs_no_door_at_the_rim() {
        let vouching = |route: &str| Lane {
            at: vec!["./glue".into()],
            ..lane(route)
        };
        let c = vec![HiveContract {
            hive_path: "/mem".into(),
            accepts: vec![vouching("in_batch")],
            emits: vec![vouching("episode")],
        }];
        let mut t = EdgeTable::new();
        // The rim is wired for some OTHER lane, so the contract is not dormant.
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route == 'in_other'"),
        ));
        assert!(
            check_lane_doors(&c, &t).is_ok(),
            "{:?}",
            check_lane_doors(&c, &t)
        );

        // And the exemption is the `at` and nothing else: the same two lanes
        // without connect points are the refusal they always were.
        assert!(check_lane_doors(&drain_contract(), &t).is_err());
    }

    /// GH #562 — the connect point CLOSES the rim for its lane.
    ///
    /// The mirror of the test above: the door obligation is not dropped, it is
    /// replaced. An edge that states the lane at the hive path is refused by
    /// name, and the same delivery at the connect point is not.
    #[test]
    fn an_edge_that_states_a_docked_lane_at_the_rim_is_refused() {
        let c = vec![HiveContract {
            hive_path: "/mem".into(),
            accepts: vec![Lane {
                at: vec!["./glue".into()],
                ..lane("in_batch")
            }],
            emits: Vec::new(),
        }];
        let at_the_rim = json!({"add_edges": [
            {"from": "/outside", "to": "/mem",
             "modifier": {"set_hop": {"route": "'in_batch'"}}}
        ]});
        let err = check_inbound_lanes(&at_the_rim, "/", &c)
            .expect_err("the lane docks below the rim, so it does not arrive here");
        let msg = format!("{err:?}");
        assert!(msg.contains("CONNECT POINTS"), "{msg}");
        assert!(
            msg.contains("./glue"),
            "the refusal must name where it DOES dock: {msg}"
        );

        // The same lane at the connect point, and an ordinary lane at the rim:
        // both untouched.
        let at_the_point = json!({"add_edges": [
            {"from": "/outside", "to": "/mem/glue", "lane": "in_batch",
             "modifier": {"set_hop": {"route": "'in_batch'"}}}
        ]});
        assert!(check_inbound_lanes(&at_the_point, "/", &c).is_ok());
        let plain = vec![HiveContract {
            hive_path: "/mem".into(),
            accepts: vec![lane("in_batch")],
            emits: Vec::new(),
        }];
        assert!(check_inbound_lanes(&at_the_rim, "/", &plain).is_ok());
    }

    #[test]
    fn an_emitted_lane_without_an_exit_is_named() {
        let mut t = EdgeTable::new();
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route.startsWith('in_')"),
        ));
        // The exit leads to a sibling INSIDE the hive: the caller never sees it.
        t.insert(edge("/mem/glue", "/mem/store", None));
        let err = check_lane_doors(&drain_contract(), &t).expect_err("no exit for episode");
        assert!(format!("{err:?}").contains("episode"), "{err:?}");
    }

    #[test]
    fn a_hive_nobody_addresses_is_dormant_not_broken() {
        // `remove_nodes` on a contracted hive drops the hive path's own edges.
        // Refusing that would make a contracted hive impossible to take out.
        let mut t = EdgeTable::new();
        t.insert(edge("/mem/glue", "/mem/store", None));
        assert!(check_lane_doors(&drain_contract(), &t).is_ok());
    }

    // ---- inward: a door that PRODUCES the lane (GH #176) ----

    #[test]
    fn a_door_that_states_a_different_lane_is_not_this_lanes_exit() {
        // The guard on the modifier-aware probe below. This edge leaves the
        // hive and stamps a route, but not the declared one, and its condition
        // is unreachable for a route probe — so nothing here produces
        // `episode`, and the refusal that stands today has to keep standing.
        let err = check_lane_doors(&drain_contract(), &translating_table("'other'"))
            .expect_err("a stamped lane that is not the declared one is not its exit");
        assert!(format!("{err:?}").contains("episode"), "{err:?}");
        assert_eq!(err.error_code(), "hive_contract");
    }

    #[test]
    fn a_door_that_states_the_lane_but_stays_inside_is_not_an_exit() {
        // The other half of the same guard: producing the lane is not enough,
        // it has to leave through the hive path. A caller cannot receive a
        // message that never crosses the boundary.
        let mut t = EdgeTable::new();
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route.startsWith('in_')"),
        ));
        t.insert(stamping_edge("/mem/glue", "/mem/store", None, "'episode'"));
        let err = check_lane_doors(&drain_contract(), &t)
            .expect_err("an interior hop that stamps the lane is not an exit");
        assert!(format!("{err:?}").contains("episode"), "{err:?}");
    }

    #[test]
    fn an_out_door_that_states_the_lane_is_its_exit() {
        // GH #176. The door recognises a model field — a shape no route probe
        // can reach — and CREATES the lane. Reading the modifier is the only
        // way to see that the hive can honour what it declared.
        assert!(check_lane_doors(&drain_contract(), &translating_table("'episode'")).is_ok());
    }

    #[test]
    fn an_out_door_whose_lane_is_computed_is_not_judged() {
        // Same conservatism as the outward half: which lane this door names is
        // knowable only once a message exists, and a check that cannot say
        // which lane an edge means must never reject it.
        for expr in ["hop.upstream_route", "'ep' + hop.kind", "context.lane"] {
            assert!(
                check_lane_doors(&drain_contract(), &translating_table(expr)).is_ok(),
                "{expr} must be left alone"
            );
        }
    }

    #[test]
    fn a_door_that_stamps_something_other_than_a_route_is_not_an_exit() {
        // `set_hop` without a `route` key says nothing about a lane.
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop
            .insert("kind".to_string(), "'failure'".to_string());
        let mut t = EdgeTable::new();
        t.insert(edge(
            "/mem",
            "/mem/glue",
            Some("has(hop.route) && hop.route.startsWith('in_')"),
        ));
        t.insert(crate::edge_table::Edge {
            modifier: Some(crate::cel_eval::parse_modifier(&spec).expect("modifier parses")),
            ..edge(
                "/mem/glue",
                "/mem",
                Some("has(hop.finish_reason) && hop.finish_reason == 'error'"),
            )
        });
        let err = check_lane_doors(&drain_contract(), &t).expect_err("no lane is named here");
        assert!(format!("{err:?}").contains("episode"), "{err:?}");
    }

    // ---- outward: the caller against the contract ----

    fn stamped(to: &str, route_expr: &str) -> JsonValue {
        json!({"add_edges": [
            {"from": "./caller", "to": to,
             "modifier": {"set_hop": {"route": route_expr}}}
        ]})
    }

    #[test]
    fn a_stated_lane_the_hive_accepts_passes() {
        assert!(
            check_inbound_lanes(&stamped("./mem", "'in_batch'"), "/", &drain_contract()).is_ok()
        );
    }

    #[test]
    fn a_stated_lane_the_hive_does_not_accept_is_refused() {
        let err = check_inbound_lanes(&stamped("./mem", "'in_bath'"), "/", &drain_contract())
            .expect_err("in_bath is not a lane");
        let msg = format!("{err:?}");
        assert!(msg.contains("in_bath") && msg.contains("in_batch"), "{msg}");
    }

    #[test]
    fn a_computed_route_is_not_judged() {
        // Only knowable once a message exists. Unknown is not the same as wrong.
        for expr in ["hop.upstream_route", "'in_' + hop.kind", "context.lane"] {
            assert!(
                check_inbound_lanes(&stamped("./mem", expr), "/", &drain_contract()).is_ok(),
                "{expr} must be left alone"
            );
        }
    }

    #[test]
    fn an_edge_that_stamps_nothing_is_not_judged() {
        // The lane may have been stamped further upstream and carried here.
        let diff = json!({"add_edges": [{"from": "./caller", "to": "./mem"}]});
        assert!(check_inbound_lanes(&diff, "/", &drain_contract()).is_ok());
    }

    #[test]
    fn an_edge_into_an_uncontracted_hive_is_not_judged() {
        assert!(
            check_inbound_lanes(&stamped("./other", "'whatever'"), "/", &drain_contract()).is_ok()
        );
    }

    #[test]
    fn a_deep_endpoint_is_not_this_checks_business() {
        // Reaching past the boundary is the PORT boundary's refusal (GH #133);
        // two checks naming the same breach would report it twice.
        assert!(
            check_inbound_lanes(&stamped("./mem/glue", "'in_bath'"), "/", &drain_contract())
                .is_ok()
        );
    }

    /// GH #291 — a lane that says nothing must carry no sentence at all.
    ///
    /// `Lane::because` is a `String` (an absent `because` deserialises to `""`)
    /// and `HiveLaneRequirement::because` is an `Option<String>`. Handing the
    /// empty string through would put `lane 'in_x'()` — an empty parenthesis,
    /// or a dangling dash — into a refusal an author has to read.
    #[test]
    fn a_lane_without_a_because_projects_to_none() {
        let contract = HiveContract {
            hive_path: "/mem".into(),
            accepts: vec![
                Lane {
                    route: "in_quiet".into(),
                    context: vec!["k".into()],
                    at: Vec::new(),
                    because: String::new(),
                },
                Lane {
                    route: "in_loud".into(),
                    context: vec!["k".into()],
                    at: Vec::new(),
                    because: "because it matters".into(),
                },
            ],
            emits: vec![],
        };
        let reqs = lane_requirements(std::slice::from_ref(&contract));
        assert_eq!(reqs.len(), 2, "one requirement per accepted lane");
        assert_eq!(reqs[0].because, None, "an empty sentence is no sentence");
        assert_eq!(reqs[0].hive_path, "/mem");
        assert_eq!(reqs[0].context, vec!["k".to_string()]);
        assert_eq!(reqs[1].because.as_deref(), Some("because it matters"));
    }

    /// And the emitted half is not a caller requirement: only `accepts[]` says
    /// what a caller must have promoted on the way IN.
    #[test]
    fn only_accepted_lanes_become_requirements() {
        let contract = HiveContract {
            hive_path: "/mem".into(),
            accepts: vec![],
            emits: vec![Lane {
                route: "episode".into(),
                context: vec!["session_id".into()],
                at: Vec::new(),
                because: "one message per turn".into(),
            }],
        };
        assert!(lane_requirements(std::slice::from_ref(&contract)).is_empty());
    }
}
