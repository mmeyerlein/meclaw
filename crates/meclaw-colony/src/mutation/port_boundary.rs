//! GH #133 — the hive port boundary.
//!
//! A hive template documents its **ports**: the endpoints a parent is meant to
//! wire. Until now that was prose. `add_edges` accepted any endpoint inside the
//! hive, so a parent scope could wire around the port and reach an interior cell
//! directly — silently bypassing whatever the hive puts in front of it (filters,
//! gates, audit). The affinity reference topology says so in its own README:
//! "Port discipline is a convention, a parent scope can reach `./store` past the
//! port with a deep endpoint."
//!
//! This module turns that convention into a contract — **opt-in**. A hive that
//! declares `params.ports` seals its scope; a hive that does not is untouched
//! (byte-identical behaviour, which is why no shipped topology changes).
//!
//! # What stays legal
//!
//! For a sealed hive `H` at path `h` with declared port names `P`:
//!
//! | Constellation | Verdict |
//! |---|---|
//! | both endpoints inside `H` (same scope, any depth) | **legal** — this is the hive's own graph |
//! | an endpoint is `h` itself | **legal** — the hive path is an address (Cell ∪ Hive symmetry, hive transit) |
//! | an endpoint is `h/<p>` with `p ∈ P` | **legal** — that IS the port |
//! | the hive marker wiring its own direct children (`h → h/x`) | **legal** — `h` counts as inside |
//! | outside ↔ `h/<non-port>` (direct child or deeper) | **rejected** — `hive_port_boundary` |
//!
//! Both directions are checked. An interior non-port node may talk to anything
//! inside its hive and to nothing outside it — reaching in and reaching out are
//! the same breach seen from two ends, and a reply lane wired straight out of an
//! interior cell bypasses the port exactly as an inbound lane does.
//!
//! # Deliberate limits
//!
//! - Only `add_edges` of a mutation diff is checked — **the bootstrap is
//!   deliberately out of scope** (ruling 2026-08-15). The birth topology is the
//!   sovereign design of the colony author: whoever writes a parent's
//!   `params.graph` has the whole tree in front of them, and that is authorship,
//!   not a breach. The seal guards what comes after — a runtime mutation,
//!   possibly written by a model, reaching into a hive it did not build. Several
//!   shipped topologies legitimately wire a deep endpoint into a hive at boot.
//!   Boot-time enforcement would arrive as its OWN opt-in switch, never by
//!   widening this one, which would retroactively invalidate birth topologies
//!   that are correct today. Subtree-internal edges stay free for the same
//!   reason (ruling 2026-08-15): they are a template's statement about itself.
//! - Endpoints are resolved with the SAME normalisation the apply side uses
//!   (`resolve_scoped_path` against the mutation's guard scope), so validate and
//!   apply cannot disagree about which node an endpoint means.
//! - A single-segment endpoint that names a hive in a FOREIGN scope (the
//!   endpoint-existence check is colony-global for hive short names) resolves
//!   here against the guard scope and therefore matches no sealed hive. The
//!   check is conservative by construction: it never rejects an edge it cannot
//!   place.

use crate::mutation::MutationError;
use meclaw_core::JsonValue;

/// A hive that opted into the port boundary, in the representation the pure
/// check consumes: absolute logical hive path plus its declared port names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedHive {
    /// Absolute logical path of the hive scope (e.g. `/main/affinity`).
    pub path: String,
    /// Declared port names — short names of DIRECT children, canonical (GH
    /// #196): the `./` a template may spell them with is gone by the time they
    /// arrive here, and an entry that could never name a direct child was
    /// reported and dropped at read. May be empty ("the hive path itself is the
    /// only address").
    pub ports: Vec<String>,
    /// GH #285 — the subset of [`ports`](Self::ports) that was declared a SLOT,
    /// with what an arriving message meets there while nothing is bound behind
    /// it.
    ///
    /// A slot is in BOTH lists on purpose: to this boundary a slot is a port,
    /// because an edge onto it is the edge the slot exists for and not an edge
    /// "past the port". The second list carries the one fact a plain port does
    /// not have — and it is a fact about the slot's INSIDE, which the boundary
    /// itself never consults. Names are canonical, exactly as in `ports`.
    pub slots: Vec<(String, crate::config::UnboundBehaviour)>,
}

impl SealedHive {
    /// True iff `abs` is the hive itself or lies anywhere below it.
    fn contains(&self, abs: &str) -> bool {
        abs == self.path || self.is_interior(abs)
    }

    /// True iff `abs` lies STRICTLY below the hive path.
    fn is_interior(&self, abs: &str) -> bool {
        if self.path == "/" {
            return abs != "/" && abs.starts_with('/');
        }
        abs.starts_with(&format!("{}/", self.path))
    }

    /// True iff `abs` is a declared port: a DIRECT child of the hive whose short
    /// name is in `ports`.
    fn is_port(&self, abs: &str) -> bool {
        let prefix = if self.path == "/" {
            "/".to_string()
        } else {
            format!("{}/", self.path)
        };
        let Some(rest) = abs.strip_prefix(&prefix) else {
            return false;
        };
        !rest.is_empty() && !rest.contains('/') && self.ports.iter().any(|p| p == rest)
    }
}

/// GH #196/#202 — the one place that decides what a port name in a hive's
/// `params` NAMES.
///
/// Two spellings, one node: `./policy` and `policy` denote the same direct child
/// (Befund 6), and every other reader on the mutation surface already strips the
/// canonical prefix before deciding anything (#189, #193). The boundary compares
/// SHORT names, so an entry written the first way used to compare equal to
/// nothing at all — a hive that believed it had ports and was sealed shut.
///
/// `None` means the entry can never name a direct child, whatever the topology:
/// a deep name, `.`, `..`, or nothing. The caller reports it rather than keeping
/// it, because an entry that matches nothing is indistinguishable from an entry
/// that was never written — and that silence is the whole defect of #196.
///
/// Shared with [`crate::mutation::required_drains`] since #202, because
/// `params.required_drains[].port` is documented as the same shape and had
/// re-derived a stricter rule of its own: two readers of one documented shape
/// now agree by construction rather than by coincidence.
pub(crate) fn canonical_port_name(raw: &str) -> Option<&str> {
    let name = raw.strip_prefix("./").unwrap_or(raw);
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return None;
    }
    Some(name)
}

/// PURE — no FS, no DB. Reject every `add_edges` entry that crosses a sealed
/// hive's port boundary (GH #133).
///
/// `guard_scope` is the mutation's scope; endpoints resolve against it exactly
/// as the apply side resolves them. `sealed` carries only the hives that opted
/// in — an empty slice makes the check vacuous, which is the state of every
/// topology that ships today.
///
/// Pre-destructive by contract: the caller runs this before staging, so a reject
/// leaves the colony (registry, edge table, filesystem) untouched.
///
/// GH #293 — this is the thin `Result` face of [`addressed_port_boundary`]: the
/// FIRST breach the collecting core produces is, by construction, the one this
/// function returned before, so every verdict it ever gave is byte-identical.
///
/// GH #559 — this face judges the SEAL and nothing else, and it is LANE-BLIND
/// by construction: it is handed no contracts (`None` below), so the v-lane
/// rule table does not run at all and an edge that declares a `lane` is
/// measured exactly as an undeclared one would be.
///
/// That is deliberate, and it is what its callers need tomorrow as much as
/// today. Every one of them is a template AUDIT (`gh203_documented_port_addresses`,
/// `gh311_ports_slot_addresses`, `gh196_shipped_hive_ports`, `access_template`,
/// `w13_memory_hive_is_sealed`): each synthesises an `{from, to}` edge out of a
/// documented endpoint and asks ONE question — can an outsider reach in past
/// the port. None of them synthesises a `lane`, and once R-V3 has migrated the
/// shipped chains they still will not: the lane is a property of the mutation
/// that draws the edge, not of the address the README documents. A function
/// handed no contracts cannot honestly judge a contract rule, so it does not
/// try — with an empty contract slice the target-level row would have fired
/// `v_lane_no_connect_point` on every lane-carrying edge, which is a verdict
/// about a declaration nobody showed it.
///
/// The v-lane rules ride on [`collect_hive_port_boundary`], which IS handed the
/// contracts.
pub fn validate_hive_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
) -> Result<(), MutationError> {
    addressed_port_boundary(diff, guard_scope, sealed, None)
        .into_iter()
        .next()
        .map_or(Ok(()), |(error, _)| Err(error))
}

/// GH #293 — stage 6 ([`Stage::ContractLocality`]) as a COLLECTING check, the
/// port-boundary third of it: EVERY `add_edges` entry that wires past a sealed
/// hive's port is named, not only the first one.
///
/// Reaching in and reaching out are the same breach seen from two ends, so a
/// diff that does both used to cost two round trips to learn one mistake.
///
/// **This changes no verdict** — see [`validate_hive_port_boundary`], which is
/// now the first-error face of the same core.
///
/// [`Stage::ContractLocality`]: crate::mutation::rejection::Stage::ContractLocality
///
/// GH #559 — and the v-lane third of it, on the same pass. An `add_edges` entry
/// that declares a `lane` is a v-lane, and the levels it crosses are judged by
/// the rule table in [`v_lane_verdict`]: a level that declares the lane and
/// permits this endpoint waives its seal for THIS edge, a level that declares
/// the lane and permits nothing may not be skipped, and the target hive owes a
/// connect point. `contracts` carries what every hive declared — an empty slice
/// still RUNS the rule table (a colony where nobody declared a contract owes a
/// v-lane a connect point just the same); it is the seal-only face above that
/// skips it entirely.
pub fn collect_hive_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
    contracts: &[crate::mutation::hive_contract::HiveContract],
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address) in addressed_port_boundary(diff, guard_scope, sealed, Some(contracts)) {
        into.push(Violation::from_error(
            Stage::ContractLocality,
            &error,
            Some(address),
        ));
    }
}

/// The collecting core: every boundary breach, with the RESOLVED interior
/// endpoint it concerns as its address.
///
/// The address is the resolved absolute path rather than the raw endpoint
/// spelling, because that is the node the seal is about and two different
/// spellings (`./aff/store`, `aff/store`) name the same breach.
///
/// GH #559: for an edge that declares a lane the v-lane verdict runs FIRST, and
/// it produces two things — the violations of the rule table, and the set of
/// (hive, endpoint) pairs whose seal that same table waived. The waiver is why
/// the two checks cannot be two passes: a v-lane that a level opened for is not
/// a port breach at that level, and a second pass judging the seal alone would
/// report one.
///
/// `contracts` is an `Option` rather than a possibly-empty slice, because the
/// two states are different questions and not a matter of degree: `None` means
/// "do not judge lanes at all" (the seal-only face), `Some(&[])` means "judge
/// them against a colony in which nobody declared anything". The waiver list is
/// rebuilt PER EDGE — a level that opened for one edge's lane has said nothing
/// about the next edge.
fn addressed_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
    contracts: Option<&[crate::mutation::hive_contract::HiveContract]>,
) -> Vec<(MutationError, String)> {
    let mut violations = Vec::new();
    let Some(obj) = diff.as_object() else {
        // Shape errors are surfaced by schema validation; nothing to bound.
        return violations;
    };
    let Some(adds) = obj.get("add_edges").and_then(|v| v.as_array()) else {
        return violations;
    };
    for e in adds {
        let (Some(from), Some(to)) = (
            e.get("from").and_then(|v| v.as_str()),
            e.get("to").and_then(|v| v.as_str()),
        ) else {
            // Missing endpoints are a `schema` reject of the edge validation.
            continue;
        };
        let from_abs = crate::mutation::resolve_scoped_path(guard_scope, from);
        let to_abs = crate::mutation::resolve_scoped_path(guard_scope, to);

        // GH #559: the declared lane, if this is a v-lane. Its verdict comes
        // before the seal so that the waivers it grants are known when the seal
        // is measured, and so that a refusal leads with the sentence that says
        // what to do about it.
        let mut waived: Vec<(String, String)> = Vec::new();
        if let (Some(cs), Some(lane)) = (contracts, e.get("lane").and_then(|v| v.as_str())) {
            for (endpoint_abs, other_abs) in [
                (from_abs.as_str(), to_abs.as_str()),
                (to_abs.as_str(), from_abs.as_str()),
            ] {
                let verdict = v_lane_verdict(lane, endpoint_abs, other_abs, sealed, cs);
                waived.extend(verdict.waived);
                violations.extend(verdict.violations);
            }
        }

        if sealed.is_empty() {
            continue;
        }
        for hive in sealed {
            check_endpoint_pair(
                hive,
                from,
                from_abs.as_str(),
                to_abs.as_str(),
                "from",
                &waived,
                &mut violations,
            );
            check_endpoint_pair(
                hive,
                to,
                to_abs.as_str(),
                from_abs.as_str(),
                "to",
                &waived,
                &mut violations,
            );
        }
    }
    violations
}

/// One half of the symmetric check: `endpoint_abs` is the endpoint under
/// scrutiny, `other_abs` its partner on the same edge.
///
/// GH #293: pushes instead of returning. The three early exits are unchanged —
/// they are the constellations that are LEGAL, and a legal endpoint contributes
/// nothing either way.
fn check_endpoint_pair(
    hive: &SealedHive,
    endpoint_raw: &str,
    endpoint_abs: &str,
    other_abs: &str,
    side: &str,
    waived: &[(String, String)],
    out: &mut Vec<(MutationError, String)>,
) {
    if !hive.is_interior(endpoint_abs) {
        return; // not inside this hive — no boundary involved
    }
    if hive.is_port(endpoint_abs) {
        return; // that IS the port
    }
    if hive.contains(other_abs) {
        return; // both ends inside the hive (or the hive marker itself)
    }
    // GH #559: the fourth legal constellation. This hive declared the edge's
    // lane and named THIS endpoint as a connect point for it — the one
    // exception the template pronounces about itself, so it is not a breach of
    // its own seal. `ports: []` stays literally true for every other lane.
    if waived
        .iter()
        .any(|(h, ep)| h == &hive.path && ep == endpoint_abs)
    {
        return;
    }
    let ports = if hive.ports.is_empty() {
        "none (transit through the hive path only)".to_string()
    } else {
        hive.ports.join(", ")
    };
    out.push((
        MutationError::HivePortBoundary(format!(
            "add_edges[].{side}='{endpoint_raw}' resolves to '{endpoint_abs}', an interior node of \
             the sealed hive '{hive_path}', while the edge's other endpoint '{other_abs}' lies \
             outside it — that wires past the port. Declared ports of '{hive_path}': {ports}. \
             Wire the hive path itself or one of its ports.",
            hive_path = hive.path
        )),
        endpoint_abs.to_string(),
    ));
}

/// GH #559 — what [`v_lane_verdict`] decided about ONE endpoint of a v-lane.
#[derive(Debug, Default)]
pub struct VLaneVerdict {
    /// `(hive path, endpoint)` pairs whose seal this lane's declaration opened.
    /// The port boundary treats such a pair as legal — see
    /// [`check_endpoint_pair`].
    pub waived: Vec<(String, String)>,
    /// Rule-table refusals, each with the endpoint it concerns as its address.
    pub violations: Vec<(MutationError, String)>,
}

/// The parent scope of `abs`, or `None` for the root itself.
///
/// A registered node's parent IS a hive scope: only a hive holds members
/// (`docs/meclaw-overview.md` § The hive boundary), so "the hive an endpoint sits
/// in" needs no registry lookup — it is one path operation. That is what lets
/// this rule table stay pure.
fn parent_scope(abs: &str) -> Option<&str> {
    match abs.rfind('/') {
        None | Some(0) if abs == "/" => None,
        Some(0) => Some("/"),
        Some(i) => Some(&abs[..i]),
        None => None,
    }
}

/// The deepest scope both paths lie in — the level whose graph an edge between
/// them belongs to (design § 1: "an edge lives in the graph of the deepest
/// common ancestor").
fn lowest_common_scope(a: &str, b: &str) -> String {
    let mut common = String::from("/");
    let mut it_a = a.split('/').filter(|s| !s.is_empty());
    let mut it_b = b.split('/').filter(|s| !s.is_empty());
    // The LCA of the two ENDPOINTS is one level above their last shared
    // segment when they differ, and the endpoint's own parent when one is an
    // ancestor of the other. Walking the shared prefix and stopping at the
    // first difference gives the first; the callers below only ever ask about
    // levels STRICTLY between this and an endpoint, which gives the second.
    loop {
        match (it_a.next(), it_b.next()) {
            (Some(x), Some(y)) if x == y => {
                if common == "/" {
                    common = format!("/{x}");
                } else {
                    common.push('/');
                    common.push_str(x);
                }
            }
            _ => break,
        }
    }
    common
}

/// The relative path from `scope` down to `abs`, in the `./x` spelling a
/// contract writes (`docs/config.md` § `params.contract`). `None` when `abs`
/// does not lie below `scope`.
fn relative_to(scope: &str, abs: &str) -> Option<String> {
    let prefix = if scope == "/" {
        "/".to_string()
    } else {
        format!("{scope}/")
    };
    abs.strip_prefix(&prefix)
        .filter(|rest| !rest.is_empty())
        .map(|rest| format!("./{rest}"))
}

/// GH #559 — THE rule table of a v-lane (ruling R-V1), for one endpoint.
///
/// A v-lane is a deep edge that names the lane it carries. `endpoint_abs` is
/// the end under scrutiny, `other_abs` its partner; the levels judged are the
/// hives strictly between the two ends' common scope and `endpoint_abs`, from
/// the outermost inwards, with the endpoint's own parent as the TARGET hive.
///
/// | crossed level | what its contract says about the lane | verdict |
/// |---|---|---|
/// | unsealed | nothing | transparent — skipped |
/// | unsealed | declared, no `at` naming this endpoint | `v_lane_mandatory_hop` |
/// | sealed | `at` names this endpoint | allowed — the seal is waived |
/// | sealed | nothing, or `at` without a hit | the existing `hive_port_boundary` stays |
/// | the target hive | `at` does not name this endpoint | `v_lane_no_connect_point` |
///
/// The two halves are one rule read from two sides. A level that DECLARES a
/// lane has said it takes part in it — it stamps, filters, guards — so skipping
/// it drops something nobody would notice was missing (`v_lane_mandatory_hop`).
/// A level that declares an `at` for the lane has said the opposite about that
/// one target: pass. And the target owes the connect point, because `ports: []`
/// has to stay literally true — the v-lane is the exception a template
/// pronounces about ITSELF, never one a caller helps itself to.
///
/// PURE: paths and declarations, no FS and no registry. An endpoint that is a
/// direct member of the common scope is not deep and is not judged at all,
/// which is what keeps an ordinary edge that happens to carry a lane honest.
#[must_use]
pub fn v_lane_verdict(
    lane: &str,
    endpoint_abs: &str,
    other_abs: &str,
    sealed: &[SealedHive],
    contracts: &[crate::mutation::hive_contract::HiveContract],
) -> VLaneVerdict {
    let mut out = VLaneVerdict::default();
    let scope = lowest_common_scope(endpoint_abs, other_abs);
    let Some(target) = parent_scope(endpoint_abs) else {
        return out; // the root is nobody's deep endpoint
    };
    // Not deep on this side: the endpoint is a member of the very scope the
    // edge lives in, so it crosses no level and there is nothing to declare.
    if target == scope || relative_to(&scope, endpoint_abs).is_none() {
        return out;
    }

    // The crossed levels, outermost first: every scope strictly below the
    // common one and at or above the target hive.
    let mut levels: Vec<&str> = Vec::new();
    let mut cur = Some(target);
    while let Some(level) = cur {
        if level == scope || relative_to(&scope, level).is_none() {
            break;
        }
        levels.push(level);
        cur = parent_scope(level);
    }
    levels.reverse();

    for level in levels {
        let is_target = level == target;
        // Every crossed level is an ancestor of the endpoint by construction —
        // but this runs inside the colony task, where a wrong assumption is not
        // a failed check, it is the whole colony gone (invariant: the colony
        // hot path is panic-free). A level that cannot name the endpoint relatively can
        // make no statement about it, so it is skipped, which is the same
        // answer the transparent row gives.
        let Some(rel) = relative_to(level, endpoint_abs) else {
            continue;
        };
        let declared = contracts
            .iter()
            .find(|c| c.hive_path == level)
            .and_then(|c| {
                c.accepts
                    .iter()
                    .chain(c.emits.iter())
                    .find(|l| l.route == lane)
            });
        let permits = declared.is_some_and(|l| l.at.contains(&rel));
        let is_sealed = sealed.iter().any(|h| h.path == level);

        if permits {
            // Row 3: the level opened for this lane at this address. The waiver
            // is only meaningful for a sealed level, but recording it
            // unconditionally keeps the two halves independent.
            out.waived
                .push((level.to_string(), endpoint_abs.to_string()));
            continue;
        }
        if is_target {
            // Row 5: the target owes the connect point, and nothing else can
            // supply it — a caller that could pick its own docking point is the
            // port bypass this whole boundary exists to refuse.
            let because = declared.map_or_else(
                || format!("'{lane}' is not in its contract at all"),
                |l| {
                    format!(
                        "it declares '{lane}' ({because}) but its connect points are: {at}",
                        because = l.because,
                        at = if l.at.is_empty() {
                            "none".to_string()
                        } else {
                            l.at.join(", ")
                        }
                    )
                },
            );
            out.violations.push((
                MutationError::VLaneNoConnectPoint(format!(
                    "add_edges[] declares lane '{lane}' onto '{endpoint_abs}', but the hive \
                     '{level}' names no connect point for it — {because}. A v-lane docks where \
                     the target says it docks: add '{rel}' to that lane's `at` in \
                     '{level}'.params.contract."
                )),
                endpoint_abs.to_string(),
            ));
            continue;
        }
        if let Some(l) = declared.filter(|_| !is_sealed) {
            // Row 2: an open level that speaks about this lane is a MANDATORY
            // hop. It is open, so the seal has nothing to say — and skipping it
            // would silently drop whatever it contributes to the lane.
            out.violations.push((
                MutationError::VLaneMandatoryHop(format!(
                    "add_edges[] declares lane '{lane}' onto '{endpoint_abs}' and would skip the \
                     hive '{level}', which declares that lane itself ({because}) — a level that \
                     takes part in a lane may not be bypassed. Route through '{level}', or let it \
                     waive the hop by adding '{rel}' to that lane's `at`.",
                    because = l.because
                )),
                endpoint_abs.to_string(),
            ));
        }
        // Rows 1 and 4 produce nothing here: an open level that says nothing is
        // transparent, and a sealed level that says nothing keeps the
        // `hive_port_boundary` the seal check is about to raise.
    }
    out
}

/// Call-site adapter (NOT pure — reads `config.json`): collect the hives that
/// declared `params.ports`.
///
/// The declaration lives in the hive's own `config.json`, which is the
/// bootstrap snapshot and semantically frozen after instantiation
/// (`docs/config.md` § Access). Reading it per mutation keeps the boundary
/// truthful for hives that were registered before this field existed and
/// survives a reboot, where hive scopes are rehydrated from `colony.db` and the
/// `params` hints are otherwise ignored — no `colony.db` schema change is
/// needed for an opt-in marker.
///
/// A hive whose `config.json` is missing, unreadable, or does not parse as
/// [`crate::config::HiveParams`] contributes nothing: bootstrap already reports
/// such a tree loudly, and a validation gate must not invent a boundary out of
/// an unreadable file.
pub fn collect_sealed_hives<'a>(
    root: &std::path::Path,
    hive_paths: impl Iterator<Item = &'a meclaw_core::Path>,
) -> Vec<SealedHive> {
    let mut out = Vec::new();
    for logical in hive_paths {
        let s = logical.as_str();
        if s == "/" {
            // The root scope has no enclosing parent that could reach "in from
            // outside" — sealing it would be vacuous.
            continue;
        }
        let (scope, name) = match s.rfind('/') {
            Some(0) => ("/", &s[1..]),
            Some(i) => (&s[..i], &s[i + 1..]),
            None => continue,
        };
        let cfg_path = crate::path_truth::resolve_cell_dir(root, scope, name).join("config.json");
        let Ok(raw) = std::fs::read_to_string(&cfg_path) else {
            continue;
        };
        let Ok(val) = meclaw_core::serde_json::from_str::<JsonValue>(&raw) else {
            continue;
        };
        let params = val.get("params").cloned().unwrap_or(JsonValue::Null);
        if params.is_null() {
            continue;
        }
        let Ok(hp) = meclaw_core::serde_json::from_value::<crate::config::HiveParams>(params)
        else {
            continue;
        };
        if let Some(ports) = hp.ports {
            let mut canonical = Vec::with_capacity(ports.len());
            let mut slots = Vec::new();
            for spec in &ports {
                let raw = spec.name();
                match canonical_port_name(raw) {
                    Some(name) => {
                        canonical.push(name.to_string());
                        // GH #285: a slot is a port AND a slot. The behaviour
                        // rides on the canonical name so both lists name the
                        // same node — a slot findable under one spelling and
                        // sealed under the other is #196 all over again.
                        if let crate::config::PortSpec::Slot { unbound, .. } = spec {
                            slots.push((name.to_string(), *unbound));
                        }
                    }
                    // GH #196: loud, not inert. A declaration nobody could
                    // match sealed two shipped templates shut in silence, and
                    // the same silence is what `required_drains` produced for
                    // its own port names until #202 put both readers on this
                    // function. Dropping keeps the seal fail-closed: an entry
                    // that opens no door must not be read as one that opens
                    // every door.
                    None => tracing::warn!(
                        hive = %s,
                        port = %raw,
                        "params.ports[] must be the short name of a direct child — this entry can \
                         never match an endpoint, ignoring"
                    ),
                }
            }
            out.push(SealedHive {
                path: s.to_string(),
                ports: canonical,
                slots,
            });
        }
    }
    out
}

/// GH #285 — the ABSOLUTE addresses the given hives declared as slots.
///
/// One derivation, two readers: the boot check
/// ([`crate::declared_slot_endpoints`], which plans the tree first) and the
/// mutation edge check (which asks the hives the colony is running right now).
/// Both need "hive path plus one segment" and both need it to mean the same
/// node — a second copy of the prefix rule is how a slot becomes wireable at
/// boot and unknown at mutation time, or the reverse.
///
/// Names arrive canonical from [`collect_sealed_hives`], so the address is the
/// hive path plus the short name; the root scope is the one path that already
/// ends in the separator (and it is never sealed, so it never contributes).
#[must_use]
pub(crate) fn slot_endpoint_addresses(sealed: &[SealedHive]) -> std::collections::HashSet<String> {
    slot_unbound_behaviours(sealed)
        .into_iter()
        .map(|(address, _)| address)
        .collect()
}

/// GH #285 (W4 T11) — the same addresses [`slot_endpoint_addresses`] forms,
/// each paired with the behaviour its hive declared for the UNBOUND state.
///
/// The delivery sites need the pair: an address alone answers "may an edge end
/// here", and only the word answers "and what happens to the message that
/// arrives while nothing does". Both readers share this one derivation for the
/// reason named above — an address the edge check knows and the delivery filter
/// spells differently is a slot that is wireable and then behaves like a typo.
#[must_use]
pub(crate) fn slot_unbound_behaviours(
    sealed: &[SealedHive],
) -> Vec<(String, crate::config::UnboundBehaviour)> {
    let mut out = Vec::new();
    for hive in sealed {
        let prefix = if hive.path == "/" {
            String::from("/")
        } else {
            format!("{}/", hive.path)
        };
        for (name, unbound) in &hive.slots {
            out.push((format!("{prefix}{name}"), *unbound));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn sealed() -> Vec<SealedHive> {
        vec![SealedHive {
            path: "/aff".into(),
            ports: vec!["brief".into(), "gate".into()],
            slots: vec![],
        }]
    }

    fn edge(from: &str, to: &str) -> JsonValue {
        json!({"add_edges": [{"from": from, "to": to}]})
    }

    // ---- legal constellations ----

    #[test]
    fn an_edge_onto_the_hive_path_itself_stays_legal() {
        assert!(validate_hive_port_boundary(&edge("./caller", "./aff"), "/", &sealed()).is_ok());
    }

    #[test]
    fn an_edge_onto_a_declared_port_stays_legal() {
        assert!(
            validate_hive_port_boundary(&edge("./caller", "./aff/brief"), "/", &sealed()).is_ok()
        );
        assert!(
            validate_hive_port_boundary(&edge("./aff/gate", "./caller"), "/", &sealed()).is_ok()
        );
    }

    #[test]
    fn an_edge_within_the_same_scope_stays_legal() {
        // The hive's own graph: gate -> store, both interior, `store` is no port.
        assert!(validate_hive_port_boundary(&edge("./gate", "./store"), "/aff", &sealed()).is_ok());
        // Deeper: interior to interior, any depth.
        assert!(
            validate_hive_port_boundary(&edge("./aff/store", "./aff/inner/x"), "/", &sealed())
                .is_ok()
        );
    }

    #[test]
    fn the_hive_marker_may_wire_its_own_direct_children() {
        assert!(validate_hive_port_boundary(&edge("./aff", "./aff/store"), "/", &sealed()).is_ok());
    }

    #[test]
    fn an_edge_that_never_touches_the_hive_stays_legal() {
        assert!(validate_hive_port_boundary(&edge("./a", "./b"), "/", &sealed()).is_ok());
    }

    #[test]
    fn a_hive_without_a_port_declaration_is_not_sealed_at_all() {
        // The opt-in switch: an empty `sealed` slice makes every edge legal,
        // which is the state of every topology that ships today.
        assert!(validate_hive_port_boundary(&edge("./caller", "./aff/store"), "/", &[]).is_ok());
    }

    // ---- illegal constellations ----

    #[test]
    fn wiring_from_outside_onto_a_non_port_child_rejects() {
        let err = validate_hive_port_boundary(&edge("./caller", "./aff/store"), "/", &sealed())
            .expect_err("deep endpoint past the port must reject");
        assert_eq!(err.error_code(), "hive_port_boundary");
        let MutationError::HivePortBoundary(detail) = err else {
            panic!("wrong variant");
        };
        assert!(
            detail.contains("/aff/store"),
            "names the endpoint: {detail}"
        );
        assert!(detail.contains("/aff"), "names the hive: {detail}");
        assert!(detail.contains("brief"), "names the ports: {detail}");
    }

    #[test]
    fn wiring_out_of_a_non_port_child_to_the_outside_rejects() {
        let err = validate_hive_port_boundary(&edge("./aff/store", "./caller"), "/", &sealed())
            .expect_err("a reply lane out of an interior cell is the same breach");
        assert_eq!(err.error_code(), "hive_port_boundary");
    }

    #[test]
    fn wiring_from_outside_onto_a_node_below_a_port_rejects() {
        // A port is a DIRECT child; `brief/sub` is not the port.
        let err = validate_hive_port_boundary(&edge("./caller", "./aff/brief/sub"), "/", &sealed())
            .expect_err("a node below the port is not the port");
        assert_eq!(err.error_code(), "hive_port_boundary");
    }

    #[test]
    fn an_empty_port_list_seals_everything_but_the_hive_path() {
        let sealed = vec![SealedHive {
            path: "/aff".into(),
            ports: vec![],
            slots: vec![],
        }];
        assert!(validate_hive_port_boundary(&edge("./caller", "./aff"), "/", &sealed).is_ok());
        let err = validate_hive_port_boundary(&edge("./caller", "./aff/brief"), "/", &sealed)
            .expect_err("no port declared means no interior address");
        assert_eq!(err.error_code(), "hive_port_boundary");
    }

    #[test]
    fn a_scoped_mutation_resolves_endpoints_against_its_guard_scope() {
        // Scope `/aff`, endpoint `./store` → `/aff/store` (interior), partner
        // `../caller` would be out of bounds and is rejected earlier by
        // `validate_scope_containment`; a sibling scope reaches in via a depth
        // endpoint from root instead.
        let err = validate_hive_port_boundary(
            &json!({"add_edges": [{"from": "./sub/aff/store", "to": "./sub/caller"}]}),
            "/",
            &[SealedHive {
                path: "/sub/aff".into(),
                ports: vec!["brief".into()],
                slots: vec![],
            }],
        )
        .expect_err("nested hive is sealed too");
        assert_eq!(err.error_code(), "hive_port_boundary");
    }

    #[test]
    fn a_bare_short_name_endpoint_resolves_against_the_scope() {
        // `store` without `./` denotes the same scope-local node (Befund 6).
        let err = validate_hive_port_boundary(
            &json!({"add_edges": [{"from": "caller", "to": "aff/store"}]}),
            "/",
            &sealed(),
        )
        .expect_err("bare endpoints resolve identically");
        assert_eq!(err.error_code(), "hive_port_boundary");
    }

    #[test]
    fn a_diff_without_add_edges_is_vacuous() {
        assert!(
            validate_hive_port_boundary(&json!({"add_nodes": [{"name": "x"}]}), "/", &sealed())
                .is_ok()
        );
    }

    // ---- the config.json reader ----

    #[test]
    fn collect_canonicalises_a_port_written_with_the_dot_slash_prefix() {
        // GH #196: `./policy` and `policy` denote the same node (Befund 6), and
        // the boundary compares SHORT names — so a declaration written the first
        // way matched nothing and sealed the hive as strictly as `ports: []`,
        // while its own README presented the ports as the way in.
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("main/aff")).unwrap();
        std::fs::write(
            root.join("main/aff/config.json"),
            r#"{"cell":{"type":"hive"},"params":{"ports":["./brief","gate"]}}"#,
        )
        .unwrap();
        std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();

        let paths = [meclaw_core::Path::new("/aff")];
        let got = collect_sealed_hives(root, paths.iter());
        assert_eq!(
            got,
            vec![SealedHive {
                path: "/aff".into(),
                ports: vec!["brief".into(), "gate".into()],
                slots: vec![],
            }],
            "both spellings land on one short name"
        );
    }

    #[test]
    fn collect_drops_a_port_declaration_that_could_never_match() {
        // A deep name is not a port and never can be: a port is a member of the
        // scope, not a node somewhere below it. Dropping it keeps the seal
        // fail-closed, and the warning is what makes the drop findable — an
        // inert declaration that says nothing is the whole defect of #196.
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("main/aff")).unwrap();
        std::fs::write(
            root.join("main/aff/config.json"),
            r#"{"cell":{"type":"hive"},"params":{"ports":["brief/sub","..","","gate"]}}"#,
        )
        .unwrap();
        std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();

        let paths = [meclaw_core::Path::new("/aff")];
        let got = collect_sealed_hives(root, paths.iter());
        assert_eq!(
            got,
            vec![SealedHive {
                path: "/aff".into(),
                ports: vec!["gate".into()],
                slots: vec![],
            }],
            "only the entry that can match survives, and the hive stays sealed"
        );
    }

    // ---- GH #559: the path arithmetic the rule table stands on ----

    /// The three helpers are pure path arithmetic, and pure path arithmetic is
    /// exactly where an off-by-one hides: every verdict of the rule table is a
    /// statement about WHICH levels lie between two endpoints, so a wrong
    /// answer here is a seal opened for the wrong hive.
    #[test]
    fn the_crossed_levels_are_derived_from_the_two_endpoints_alone() {
        assert_eq!(parent_scope("/"), None, "the root has no parent");
        assert_eq!(parent_scope("/caller"), Some("/"));
        assert_eq!(parent_scope("/outer/inner/target"), Some("/outer/inner"));

        // Two endpoints in different branches meet at their shared prefix.
        assert_eq!(lowest_common_scope("/caller", "/outer/inner/target"), "/");
        assert_eq!(lowest_common_scope("/a/b/x", "/a/b/y/z"), "/a/b");
        // One endpoint an ancestor of the other: the shallower one IS the
        // common scope, which is what makes `h -> h/child` not a v-lane.
        assert_eq!(lowest_common_scope("/a/b", "/a/b/c"), "/a/b");

        assert_eq!(relative_to("/", "/outer"), Some("./outer".to_string()));
        assert_eq!(
            relative_to("/outer", "/outer/inner/target"),
            Some("./inner/target".to_string())
        );
        // Not below it, and not itself: a level that is not an ancestor names
        // no relative path at all.
        assert_eq!(relative_to("/outer", "/outer"), None);
        assert_eq!(relative_to("/outer", "/other/x"), None);
        // The suffix rule, not a prefix rule: `/outer-2` is a SIBLING.
        assert_eq!(relative_to("/outer", "/outer-2/x"), None);
    }

    /// An edge that declares a lane but is not DEEP on either side is not a
    /// v-lane at all — it crosses no level, so the rule table has nothing to
    /// say and says nothing. Pinned because the opposite (judging every
    /// lane-carrying edge) would refuse the ordinary rim-to-rim edge every
    /// migrated chain still needs.
    #[test]
    fn a_lane_between_two_members_of_one_scope_is_not_judged() {
        let v = v_lane_verdict("recall", "/a", "/b", &sealed(), &[]);
        assert!(v.violations.is_empty(), "{:?}", v.violations);
        assert!(v.waived.is_empty());
    }

    /// The seal-only face is LANE-BLIND, and that is a promise its callers
    /// rely on rather than an accident of an empty slice.
    ///
    /// Every caller is a template audit synthesising `{from, to}` out of a
    /// documented endpoint. Handed no contracts, this face cannot know whether
    /// a lane has a connect point — so it does not ask: a lane-carrying edge
    /// gets exactly the verdict the same edge without the key would get, in
    /// both directions. (The collecting face, which IS handed the contracts,
    /// asks; that is what the integration suite pins.)
    #[test]
    fn the_seal_only_face_does_not_judge_lanes() {
        let lane_edge = |from: &str, to: &str| json!({"add_edges": [{"from": from, "to": to, "lane": "recall"}]});
        // A deep endpoint under an UNSEALED hive: legal without the key, and
        // legal with it — no `v_lane_no_connect_point` invented out of a
        // declaration nobody showed this function.
        assert!(
            validate_hive_port_boundary(&lane_edge("./caller", "./open/deep"), "/", &sealed())
                .is_ok(),
            "a lane the seal has no opinion about must pass"
        );
        // And where the seal DID refuse, it still refuses, with its own code.
        assert_eq!(
            validate_hive_port_boundary(&lane_edge("./caller", "./aff/store"), "/", &sealed())
                .expect_err("the seal is untouched by the key")
                .error_code(),
            "hive_port_boundary"
        );
    }

    #[test]
    fn a_port_name_is_a_short_name_in_either_spelling() {
        assert_eq!(canonical_port_name("./brief"), Some("brief"));
        assert_eq!(canonical_port_name("brief"), Some("brief"));
        // What can never name a direct child of the hive.
        assert_eq!(canonical_port_name("brief/sub"), None);
        assert_eq!(canonical_port_name("./brief/sub"), None);
        assert_eq!(canonical_port_name("."), None);
        assert_eq!(canonical_port_name(".."), None);
        assert_eq!(canonical_port_name("./"), None);
        assert_eq!(canonical_port_name(""), None);
    }

    #[test]
    fn collect_reads_the_opt_in_declaration_and_ignores_the_open_hive() {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        let write = |rel: &str, body: &str| {
            let dir = root.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("config.json"), body).unwrap();
        };
        // root cell dir `main` (path_truth strips it from logical paths)
        write("main", r#"{"cell":{"type":"hive"}}"#);
        write(
            "main/aff",
            r#"{"cell":{"type":"hive"},"params":{"ports":["brief","gate"]}}"#,
        );
        write("main/open", r#"{"cell":{"type":"hive"},"params":{}}"#);
        write("main/nop", r#"{"cell":{"type":"hive"}}"#);

        let paths = [
            meclaw_core::Path::new("/"),
            meclaw_core::Path::new("/aff"),
            meclaw_core::Path::new("/open"),
            meclaw_core::Path::new("/nop"),
        ];
        let got = collect_sealed_hives(root, paths.iter());
        assert_eq!(
            got,
            vec![SealedHive {
                path: "/aff".into(),
                ports: vec!["brief".into(), "gate".into()],
                slots: vec![],
            }],
            "only the hive that declared ports is sealed"
        );
    }

    /// GH #285: a declared slot IS a port of this boundary. It appears in
    /// `ports` like any other, because an edge onto a slot is not an edge "past
    /// the port" — it is the edge the slot exists for. What the slot adds is a
    /// second fact, in `slots`: what a message meets there while nothing is
    /// bound behind it.
    #[test]
    fn collect_reads_a_slot_as_a_port_and_remembers_its_unbound_behaviour() {
        use crate::config::UnboundBehaviour;

        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("main/aff")).unwrap();
        std::fs::write(
            root.join("main/aff/config.json"),
            r#"{"cell":{"type":"hive"},"params":{"ports":[
                 "brief",
                 {"name": "gen", "slot": true, "unbound": "park"}
               ]}}"#,
        )
        .unwrap();
        std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();

        let paths = [meclaw_core::Path::new("/aff")];
        let got = collect_sealed_hives(root, paths.iter());
        assert_eq!(
            got,
            vec![SealedHive {
                path: "/aff".into(),
                ports: vec!["brief".into(), "gen".into()],
                slots: vec![("gen".into(), UnboundBehaviour::Park)],
            }],
            "a slot is a port for the boundary, and a slot besides"
        );

        // And that is not a statement about a list: the real boundary lets an
        // outside edge land on the slot.
        assert!(validate_hive_port_boundary(&edge("./caller", "./aff/gen"), "/", &got).is_ok());
        assert_eq!(
            validate_hive_port_boundary(&edge("./caller", "./aff/store"), "/", &got)
                .expect_err("a non-port child stays sealed")
                .error_code(),
            "hive_port_boundary"
        );
    }

    /// A slot written with the canonical `./` prefix lands on the same short
    /// name in BOTH lists — one canonicalisation, applied once (GH #196/#285).
    #[test]
    fn collect_canonicalises_a_slot_name_like_any_other_port() {
        use crate::config::UnboundBehaviour;

        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("main/aff")).unwrap();
        std::fs::write(
            root.join("main/aff/config.json"),
            r#"{"cell":{"type":"hive"},"params":{"ports":[
                 {"name": "./gen", "slot": true, "unbound": "error"}
               ]}}"#,
        )
        .unwrap();
        std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();

        let paths = [meclaw_core::Path::new("/aff")];
        assert_eq!(
            collect_sealed_hives(root, paths.iter()),
            vec![SealedHive {
                path: "/aff".into(),
                ports: vec!["gen".into()],
                slots: vec![("gen".into(), UnboundBehaviour::Error)],
            }]
        );
    }
}
