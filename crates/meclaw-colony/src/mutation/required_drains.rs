//! GH #147 — a port that is wired must have its drain wired too.
//!
//! Some hive ports come in pairs. The memory hive's inline extraction is the
//! example the issue came from: a block the hive refuses leaves on a reject
//! egress, and a parent that wires the ingress without the drain loses every
//! refusal. The README says so in bold. Nothing enforced it, and the next
//! person to wire an inline lane is precisely the person who will not read that
//! line.
//!
//! This module lets a hive declare the pairing, next to `params.ports` and in
//! the same opt-in spirit:
//!
//! ```json
//! "params": {
//!   "ports": ["writer", "recall", "extract-glue"],
//!   "required_drains": [
//!     {"port": "extract-glue",
//!      "hop": {"route": "reject"},
//!      "because": "a rejected inline extraction leaves on this route"}
//!   ]
//! }
//! ```
//!
//! Read as: *if anything outside this hive wires into `extract-glue`, then
//! `extract-glue` must have an out-edge that takes a hop of `{route: reject}`
//! to somewhere outside the hive.*
//!
//! # Why it asks the router instead of reading the condition
//!
//! The obvious implementation compares condition source strings. That fails the
//! first time somebody writes `hop.route=='reject'` instead of
//! `hop.route == 'reject'`, or adds `&& has(context.session_id)`, or drains two
//! routes with one `in` expression — all of which are correct topologies that a
//! string comparison calls broken.
//!
//! So the check builds the hop the declaration describes and runs it through
//! [`crate::edge_table::apply_edges`] — the same function that routes the real
//! message at runtime. If the router would deliver it out of the hive, the
//! drain exists. If the router would deliver it nowhere, it does not. There is
//! no second opinion to disagree with the first.
//!
//! # The lane form (GH #237)
//!
//! The rule above keys on a PORT, and since GH #197/#228 the shipped library
//! has none: a sealed hive (`"ports": []`) cannot be addressed below its own
//! path, so `port_is_wired_from_outside` can never be true again and the
//! declaration can never fire. A rule that cannot fire reads exactly like one
//! that can, which is the defect GH #202 was filed about — so the same
//! obligation is available in the vocabulary the seal left standing:
//!
//! ```json
//! "required_drains": [
//!   {"accepts": "in_remember", "emits": "reject",
//!    "because": "a block this hive refuses leaves on the reject lane"}
//! ]
//! ```
//!
//! Read as: *a caller that sends me `in_remember` must subscribe to `reject`.*
//! Both names are lanes of this hive's own `params.contract`; a name that is in
//! neither is dropped by the reader with a warning, for the reason a deep port
//! name is dropped — a requirement nothing can satisfy is worse than none.
//!
//! The trigger is the caller's own edge: an edge from OUTSIDE that lands on the
//! hive path and stamps a literal `hop.route` equal to `accepts`. That is the
//! same reading [`crate::mutation::hive_contract`] does of an `add_edges` entry,
//! and it carries the same conservatism — an edge whose route is only knowable
//! at runtime (`'in_' + hop.kind`) names no lane here and triggers nothing.
//!
//! # The one thing the lane form cannot see
//!
//! The port form asks whether a message leaving the PORT reaches the outside.
//! The lane form has to ask whether a message leaving the HIVE PATH does — and
//! that is the caller's subscription condition, which GH #173 deliberately left
//! unchecked: shipped topologies tell lanes apart by a second hop key
//! (`hop.round_capped`), and a route-only probe does not carry it. A check that
//! refuses a correct wiring is worse than none.
//!
//! So the probe decides only what it can decide, and the third verdict is
//! silence rather than accusation:
//!
//! - the router delivers the lane out of the hive → drained;
//! - the condition fails to EVALUATE against the probe (it reads a key the
//!   probe cannot carry) → unknown, and unknown counts as drained;
//! - an unconditional out-edge → it takes everything, including this lane;
//! - every out-edge evaluates cleanly to `false` → not drained, and this is the
//!   only case that refuses.
//!
//! The residue is a subscription that guards its extra keys with `has()`, which
//! evaluates to a clean `false` and would be refused. It is named here so the
//! next person to meet it knows it is a limit and not a verdict: drop the
//! declaration, or put the lane test on an edge of its own.
//!
//! # What is deliberately NOT checked
//!
//! - **An unwired port needs nothing.** A hive whose inline ingress nobody uses
//!   is not missing a drain; it is simply not running that lane. The
//!   requirement only fires once the port is reachable from outside.
//! - **An internal drain does not count.** The refusal has to leave the hive —
//!   an edge back into the hive's own interior is the hive talking to itself,
//!   which is what it was already doing when it produced the refusal.
//! - **The drain's destination is not prescribed.** Any cell outside the hive
//!   will do. Whether it is a good drain is the parent's business; whether one
//!   exists at all is not.

use crate::edge_table::EdgeTable;
use crate::mutation::MutationError;
use meclaw_core::{Headers, Path};
use std::collections::BTreeMap;

/// What a hive pairs, once the declaration is resolved against its own path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainKind {
    /// GH #147 — a PORT of the hive, and the hop a message leaving it carries.
    Port {
        /// Absolute logical path of the port (e.g. `/main/memory/extract-glue`).
        port_path: String,
        /// The hop compartment a message on the drain route would carry.
        hop: BTreeMap<String, String>,
    },
    /// GH #237 — two LANES of the hive's contract: sending the first obliges
    /// the caller to take the second.
    Lane {
        /// The inbound lane whose wiring triggers the obligation.
        accepts: String,
        /// The outbound lane somebody outside must then take.
        emits: String,
    },
}

/// One `required_drains` entry of one hive, resolved to absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainRequirement {
    /// Absolute logical path of the declaring hive (e.g. `/main/memory`).
    pub hive_path: String,
    /// Which of the two shapes this entry is.
    pub kind: DrainKind,
    /// The declaring hive's own sentence about why this pairing exists. It ends
    /// up verbatim in the rejection, because a refusal that cannot say what it
    /// is protecting is a refusal people route around.
    pub because: String,
}

impl DrainRequirement {
    fn inside(&self, abs: &str) -> bool {
        abs == self.hive_path || abs.starts_with(&format!("{}/", self.hive_path))
    }
}

/// The headers a message carrying exactly this hop would arrive with.
fn probe_headers(hop: &BTreeMap<String, String>) -> Headers {
    let mut out = meclaw_core::serde_json::Map::new();
    for (k, v) in hop {
        out.insert(k.clone(), meclaw_core::serde_json::Value::String(v.clone()));
    }
    Headers::from_parts(meclaw_core::serde_json::Map::new(), out)
}

/// True iff something OUTSIDE the hive can send to this port.
fn port_is_wired_from_outside(req: &DrainRequirement, port_path: &str, edges: &EdgeTable) -> bool {
    edges
        .iter()
        .any(|e| e.to.as_str() == port_path && !req.inside(e.from.as_str()))
}

/// True iff a message leaving the port with the declared hop would be routed to
/// a target outside the hive.
fn drain_exists(
    req: &DrainRequirement,
    port_path: &str,
    hop: &BTreeMap<String, String>,
    edges: &EdgeTable,
) -> bool {
    let port = Path::new(port_path);
    crate::edge_table::apply_edges(edges, &port, &probe_headers(hop))
        .iter()
        .any(|d| !req.inside(d.target.as_str()))
}

/// True iff something OUTSIDE the hive sends the `accepts` lane INTO the hive
/// path — the obligation's trigger (GH #237).
///
/// The lane is read off the caller's own `set_hop.route`, exactly as
/// [`crate::mutation::hive_contract`] reads an `add_edges` entry: a literal
/// names the lane, an expression that reads the incoming message does not name
/// it here and triggers nothing. A trigger that had to guess would refuse
/// wirings nobody can see the reason for.
fn lane_is_wired_from_outside(req: &DrainRequirement, accepts: &str, edges: &EdgeTable) -> bool {
    edges.iter().any(|e| {
        e.to.as_str() == req.hive_path
            && !req.inside(e.from.as_str())
            && e.modifier
                .as_ref()
                .and_then(|m| m.source.set_hop.get("route"))
                .and_then(|src| crate::mutation::hive_contract::constant_route(src))
                .is_some_and(|stated| stated == accepts)
    })
}

/// True iff somebody outside takes the `emits` lane off the hive path.
///
/// Three ways to be a drain, and the second and third are the reason this does
/// not use `apply_edges`: the router SKIPS an edge whose condition fails to
/// evaluate, and "the condition reads a key my probe cannot carry" must not be
/// read as "there is no drain" (see the module note on what this cannot see).
fn lane_drain_exists(req: &DrainRequirement, emits: &str, edges: &EdgeTable) -> bool {
    let hop = BTreeMap::from([("route".to_string(), emits.to_string())]);
    let headers = probe_headers(&hop);
    edges
        .edges_from(&Path::new(&req.hive_path))
        .iter()
        .filter(|e| !req.inside(e.to.as_str()))
        .any(|e| match &e.condition {
            // Unconditional: it takes everything that leaves, this lane too.
            None => true,
            // An error is UNKNOWN and never judged — the same conservatism the
            // outward half of the contract check is built on. `unwrap_or(true)`
            // says exactly that, and says it in one place.
            Some(c) => crate::cel_eval::evaluate_condition(
                c,
                &meclaw_core::serde_json::Map::new(),
                &headers.hop,
            )
            .unwrap_or(true),
        })
}

/// The pure check: every wired port that declared a drain must have one.
///
/// `edges` is the POST-state — the table as it will be once the diff is
/// applied. Checking the pre-state would refuse the very mutation that wires
/// both edges together, which is the mutation this rule wants people to write.
pub fn check_required_drains(
    reqs: &[DrainRequirement],
    edges: &EdgeTable,
) -> Result<(), MutationError> {
    for req in reqs {
        if let Some(msg) = unmet(req, edges) {
            return Err(MutationError::RequiredDrainMissing(msg));
        }
    }
    Ok(())
}

/// The one sentence a requirement has to say when it is not met, or `None`
/// when it is met or dormant. Shared by the mutation half (which refuses with
/// it) and the boot half (which only says it).
fn unmet(req: &DrainRequirement, edges: &EdgeTable) -> Option<String> {
    match &req.kind {
        DrainKind::Port { port_path, hop } => {
            if !port_is_wired_from_outside(req, port_path, edges)
                || drain_exists(req, port_path, hop, edges)
            {
                return None;
            }
            let hop = hop
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!(
                "the port '{port_path}' of hive '{hive}' is wired from outside, but nothing takes \
                 a message leaving it with hop {{{hop}}} out of the hive — {because}. Wire the \
                 drain in the SAME mutation as the ingress: the two edges are one decision.",
                hive = req.hive_path,
                because = req.because,
            ))
        }
        DrainKind::Lane { accepts, emits } => {
            if !lane_is_wired_from_outside(req, accepts, edges)
                || lane_drain_exists(req, emits, edges)
            {
                return None;
            }
            Some(format!(
                "an edge sends hop.route='{accepts}' into hive '{hive}', but nothing outside it \
                 takes hop.route='{emits}' back off '{hive}' — {because}. Wire the subscription \
                 in the SAME mutation as the ingress: the two edges are one decision.",
                hive = req.hive_path,
                because = req.because,
            ))
        }
    }
}

/// Call-site adapter (NOT pure — reads `config.json`): collect the drain
/// requirements every hive in the colony declared.
///
/// Same source and same reasoning as
/// [`crate::mutation::port_boundary::collect_sealed_hives`]: the declaration
/// lives in the hive's own birth config, so it survives a reboot without a
/// `colony.db` schema change, and a hive whose config is missing or unparseable
/// contributes nothing rather than an invented rule.
pub fn collect_required_drains<'a>(
    root: &std::path::Path,
    hive_paths: impl Iterator<Item = &'a Path>,
) -> Vec<DrainRequirement> {
    let mut out = Vec::new();
    for logical in hive_paths {
        let s = logical.as_str();
        if s == "/" {
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
        let Ok(val) = meclaw_core::serde_json::from_str::<meclaw_core::JsonValue>(&raw) else {
            continue;
        };
        let params = val
            .get("params")
            .cloned()
            .unwrap_or(meclaw_core::JsonValue::Null);
        if params.is_null() {
            continue;
        }
        let Ok(hp) = meclaw_core::serde_json::from_value::<crate::config::HiveParams>(params)
        else {
            continue;
        };
        // The hive's own contract, for the lane form: a pairing that names a
        // lane the hive does not have is a pairing nothing can satisfy.
        let lanes =
            |pick: fn(&crate::config::HiveContractSpec) -> &Vec<crate::config::LaneSpec>| {
                hp.contract
                    .as_ref()
                    .map(|c| pick(c).iter().map(|l| l.route.clone()).collect::<Vec<_>>())
                    .unwrap_or_default()
            };
        let accepted = lanes(|c| &c.accepts);
        let emitted = lanes(|c| &c.emits);
        for d in hp.required_drains.clone().unwrap_or_default() {
            match d {
                crate::config::DrainSpec::Port(d) => {
                    // GH #202: a port name is a short name of a direct child,
                    // exactly like `params.ports` — so it is decided by the same
                    // function and not by a second, stricter opinion. The
                    // re-derived rule here used to refuse every `/`, which
                    // dropped the `./recall` spelling that `params.ports`
                    // accepts: the hive kept its declaration and lost its
                    // guarantee, silently, in the lenient direction.
                    let Some(port) = crate::mutation::port_boundary::canonical_port_name(&d.port)
                    else {
                        tracing::warn!(
                            hive = %s,
                            port = %d.port,
                            "required_drains[].port must be the short name of a direct child — \
                             this entry can never name a port, ignoring"
                        );
                        continue;
                    };
                    out.push(DrainRequirement {
                        hive_path: s.to_string(),
                        kind: DrainKind::Port {
                            port_path: format!("{s}/{port}"),
                            hop: d.hop,
                        },
                        because: d.because,
                    });
                }
                crate::config::DrainSpec::Lane(d) => {
                    // GH #237: both halves have to be lanes this hive declares,
                    // for the same reason a deep port name is dropped — a
                    // requirement nothing can satisfy looks exactly like one
                    // that bites, and the second is what people rely on.
                    if !accepted.contains(&d.accepts) || !emitted.contains(&d.emits) {
                        tracing::warn!(
                            hive = %s,
                            accepts = %d.accepts,
                            emits = %d.emits,
                            "required_drains names a lane this hive's params.contract does not \
                             declare — this entry can never fire, ignoring"
                        );
                        continue;
                    }
                    out.push(DrainRequirement {
                        hive_path: s.to_string(),
                        kind: DrainKind::Lane {
                            accepts: d.accepts,
                            emits: d.emits,
                        },
                        because: d.because,
                    });
                }
            }
        }
    }
    out
}

/// Boot-time reporting half: the same rule, applied to the topology a colony
/// woke up with, and only ever as a WARNING.
///
/// The mutation path rejects, and it should — a mutation is somebody changing a
/// running colony, usually with far less of the tree in front of them than its
/// author had. The birth topology is the opposite case: it is authorship, the
/// same sovereignty argument that keeps the bootstrap out of the port boundary
/// (GH #133, ruling 2026-08-15). Refusing to boot a tree that has been running
/// for weeks would be the substrate overruling the person who built it.
///
/// So this says the sentence and gets out of the way. Silence remains the one
/// thing it must not do.
///
/// Edges arrive as the graph DTO (source strings), which is the only edge view
/// a caller outside the colony task has. Conditions are re-parsed here; one
/// that no longer parses is skipped with a warning of its own rather than
/// silently counting as "no drain".
///
/// The modifier travels along since GH #237, for the reason the contract check
/// needs it (`BootEdge`): the lane form's trigger is the caller's own
/// `set_hop.route`, and an edge stripped of its modifier states no lane — boot
/// would then be silent about exactly the colonies the rule is written for.
pub fn warn_on_missing_drains(
    reqs: &[DrainRequirement],
    edges: &[crate::mutation::hive_contract::BootEdge],
) {
    if reqs.is_empty() {
        return;
    }
    let mut table = EdgeTable::new();
    for (from, to, cond, modifier) in edges {
        let condition = match cond {
            None => None,
            Some(src) => match crate::cel_eval::parse_condition(src) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(
                        from = %from, to = %to, error = %e,
                        "drain check: edge condition does not parse — edge ignored for this check"
                    );
                    continue;
                }
            },
        };
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
        });
    }
    for req in reqs {
        let Some(msg) = unmet(req, &table) else {
            continue;
        };
        tracing::warn!(
            hive = %req.hive_path,
            reason = %msg,
            "a hive's declared drain pairing is not met — messages on that lane reach nothing \
             (the mutation path refuses this; the birth topology is yours)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_table::Edge;
    use meclaw_core::Uuid;

    fn req() -> DrainRequirement {
        DrainRequirement {
            hive_path: "/main/memory".into(),
            kind: DrainKind::Port {
                port_path: "/main/memory/glue".into(),
                hop: BTreeMap::from([("route".to_string(), "reject".to_string())]),
            },
            because: "a rejected block leaves here".into(),
        }
    }

    /// The same hive, stating the same obligation in lanes (GH #237).
    fn lane_req() -> DrainRequirement {
        DrainRequirement {
            hive_path: "/main/memory".into(),
            kind: DrainKind::Lane {
                accepts: "in_remember".into(),
                emits: "reject".into(),
            },
            because: "a block this hive refuses leaves on the reject lane".into(),
        }
    }

    /// An edge that STAMPS a route on the message it takes — how a caller says
    /// which lane it is sending into.
    fn stamping_edge(from: &str, to: &str, route_expr: &str) -> Edge {
        let mut spec = crate::config::ModifierSpec::default();
        spec.set_hop
            .insert("route".to_string(), route_expr.to_string());
        Edge {
            modifier: Some(crate::cel_eval::parse_modifier(&spec).expect("modifier parses")),
            ..edge(from, to, None)
        }
    }

    fn edge(from: &str, to: &str, condition: Option<&str>) -> Edge {
        Edge {
            id: Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition: condition
                .map(|c| crate::cel_eval::parse_condition(c).expect("test condition parses")),
            modifier: None,
        }
    }

    fn table(edges: Vec<Edge>) -> EdgeTable {
        let mut t = EdgeTable::new();
        for e in edges {
            t.insert(e);
        }
        t
    }

    // ---- the config.json reader ----

    /// Plant one hive `config.json` in a throwaway colony root and let the real
    /// reader say which requirements it sees. The reader is the whole subject
    /// here: a requirement it drops is a requirement that never runs.
    fn collect_from(params: &str) -> Vec<DrainRequirement> {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("main/mem")).unwrap();
        std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
        std::fs::write(
            root.join("main/mem/config.json"),
            format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
        )
        .unwrap();
        let paths = [Path::new("/mem")];
        collect_required_drains(root, paths.iter())
    }

    #[test]
    fn collect_canonicalises_a_drain_port_written_with_the_dot_slash_prefix() {
        // GH #202: `port` is documented as the same shape as `params.ports`,
        // which since #196 accepts both spellings of one node. This reader used
        // to refuse anything containing a `/`, so `./recall` was warned about
        // and dropped and the hive that insisted on a drain silently had no
        // insistence left — lenient in the direction that removes a guarantee.
        let got = collect_from(
            r#"{"required_drains":[{"port":"./recall","hop":{"route":"reject"},
                "because":"a half window leaves here"}]}"#,
        );
        assert_eq!(
            got,
            vec![DrainRequirement {
                hive_path: "/mem".into(),
                kind: DrainKind::Port {
                    port_path: "/mem/recall".into(),
                    hop: BTreeMap::from([("route".to_string(), "reject".to_string())]),
                },
                because: "a half window leaves here".into(),
            }],
            "both spellings name one port, and the port path is one a resolved endpoint can equal"
        );
    }

    #[test]
    fn collect_drops_a_drain_port_that_could_never_name_a_direct_child() {
        // A deep name is not a port and never can be. Dropping keeps the check
        // honest: a requirement whose port path no endpoint can equal would sit
        // there looking enforced while enforcing nothing.
        let got = collect_from(
            r#"{"required_drains":[
                {"port":"recall/leg","hop":{"route":"reject"},"because":"deep"},
                {"port":"..","hop":{"route":"reject"},"because":"dots"},
                {"port":"","hop":{"route":"reject"},"because":"empty"},
                {"port":"glue","hop":{"route":"reject"},"because":"the one that can match"}]}"#,
        );
        assert_eq!(
            got.iter().map(port_path_of).collect::<Vec<_>>(),
            vec!["/mem/glue"],
            "only the entry that can name a child survives"
        );
    }

    #[test]
    fn an_unwired_port_requires_nothing() {
        // The hive's own interior talks to the port. Nobody outside does. That
        // is a hive not running the lane, not a hive missing a drain.
        let t = table(vec![edge("/main/memory/writer", "/main/memory/glue", None)]);
        assert!(check_required_drains(&[req()], &t).is_ok());
    }

    #[test]
    fn a_wired_port_without_a_drain_is_refused() {
        let t = table(vec![edge("/main/talky", "/main/memory/glue", None)]);
        let err = check_required_drains(&[req()], &t).unwrap_err();
        assert_eq!(err.error_code(), "required_drain_missing");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("a rejected block leaves here"),
            "the hive's own sentence travels into the refusal: {msg}"
        );
    }

    #[test]
    fn a_wired_port_with_a_drain_passes() {
        let t = table(vec![
            edge("/main/talky", "/main/memory/glue", None),
            edge(
                "/main/memory/glue",
                "/main/sink",
                Some("has(hop.route) && hop.route == 'reject'"),
            ),
        ]);
        assert!(check_required_drains(&[req()], &t).is_ok());
    }

    #[test]
    fn spelling_of_the_condition_does_not_matter() {
        // The router is asked, not the source text. All three of these route a
        // reject out of the hive, and all three are correct topologies.
        for cond in [
            "hop.route=='reject'",
            "has(hop.route) && hop.route in ['reject', 'error']",
            "has(hop.route) && hop.route != 'bundle'",
        ] {
            let t = table(vec![
                edge("/main/talky", "/main/memory/glue", None),
                edge("/main/memory/glue", "/main/sink", Some(cond)),
            ]);
            assert!(
                check_required_drains(&[req()], &t).is_ok(),
                "condition `{cond}` drains the reject route"
            );
        }
    }

    #[test]
    fn a_drain_that_stays_inside_the_hive_does_not_count() {
        // The refusal has to LEAVE. An edge back into the interior is the hive
        // talking to itself, which is what produced the refusal in the first
        // place.
        let t = table(vec![
            edge("/main/talky", "/main/memory/glue", None),
            edge(
                "/main/memory/glue",
                "/main/memory/store",
                Some("has(hop.route) && hop.route == 'reject'"),
            ),
        ]);
        assert!(check_required_drains(&[req()], &t).is_err());
    }

    #[test]
    fn an_out_edge_for_a_different_route_is_not_the_drain() {
        // The recall bundle leaves the hive; the reject does not. A check that
        // only asked "has any out-edge" would pass this and lose every refusal.
        let t = table(vec![
            edge("/main/talky", "/main/memory/glue", None),
            edge(
                "/main/memory/glue",
                "/main/talky",
                Some("has(hop.route) && hop.route == 'bundle'"),
            ),
        ]);
        assert!(check_required_drains(&[req()], &t).is_err());
    }
    /// The port path of a port-form requirement, for the assertions that are
    /// about the reader rather than about the check.
    fn port_path_of(r: &DrainRequirement) -> &str {
        match &r.kind {
            DrainKind::Port { port_path, .. } => port_path.as_str(),
            DrainKind::Lane { .. } => "<lane form>",
        }
    }

    // ---- GH #237: the lane form ----

    #[test]
    fn collect_reads_the_lane_form_when_the_contract_has_both_lanes() {
        let got = collect_from(
            r#"{"ports":[],
                "contract":{"accepts":[{"route":"in_remember","because":"a block"}],
                            "emits":[{"route":"reject","because":"a refusal"}]},
                "required_drains":[{"accepts":"in_remember","emits":"reject",
                                    "because":"a refused block leaves here"}]}"#,
        );
        assert_eq!(
            got,
            vec![DrainRequirement {
                hive_path: "/mem".into(),
                kind: DrainKind::Lane {
                    accepts: "in_remember".into(),
                    emits: "reject".into(),
                },
                because: "a refused block leaves here".into(),
            }],
            "a sealed hive states the pairing in lanes, and the reader keeps it"
        );
    }

    #[test]
    fn collect_drops_a_lane_pairing_the_contract_does_not_declare() {
        // Same reasoning as the deep port name above: a pairing that names a
        // lane the hive does not have can never fire, and a rule that cannot
        // fire reads exactly like one that can (GH #202).
        let got = collect_from(
            r#"{"ports":[],
                "contract":{"accepts":[{"route":"in_remember","because":"a block"}],
                            "emits":[{"route":"reject","because":"a refusal"}]},
                "required_drains":[
                  {"accepts":"in_typo","emits":"reject","because":"no such inbound lane"},
                  {"accepts":"in_remember","emits":"rejekt","because":"no such outbound lane"},
                  {"accepts":"in_remember","emits":"reject","because":"the one that can fire"}]}"#,
        );
        assert_eq!(
            got.iter().map(|r| r.because.as_str()).collect::<Vec<_>>(),
            vec!["the one that can fire"],
            "only the pairing whose two halves are lanes of this hive survives"
        );
    }

    #[test]
    fn a_hive_nobody_sends_the_lane_to_requires_nothing() {
        // The declaration is dormant until a caller uses the lane — the same
        // shape as an unwired port, in the vocabulary the seal left standing.
        let t = table(vec![stamping_edge(
            "/main/talky",
            "/main/memory",
            "'in_episode'",
        )]);
        assert!(check_required_drains(&[lane_req()], &t).is_ok());
    }

    #[test]
    fn sending_the_lane_without_subscribing_to_its_answer_is_refused() {
        let t = table(vec![stamping_edge(
            "/main/talky",
            "/main/memory",
            "'in_remember'",
        )]);
        let err = check_required_drains(&[lane_req()], &t).unwrap_err();
        assert_eq!(err.error_code(), "required_drain_missing");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("a block this hive refuses leaves on the reject lane"),
            "the hive's own sentence travels into the refusal: {msg}"
        );
    }

    #[test]
    fn sending_the_lane_and_taking_the_answer_passes() {
        let t = table(vec![
            stamping_edge("/main/talky", "/main/memory", "'in_remember'"),
            edge(
                "/main/memory",
                "/main/sink",
                Some("has(hop.route) && hop.route == 'reject'"),
            ),
        ]);
        assert!(check_required_drains(&[lane_req()], &t).is_ok());
    }

    #[test]
    fn a_subscription_to_a_different_lane_is_not_the_drain() {
        // The bundle leaves the hive; the refusal does not. A check that only
        // asked "does anything leave" would pass this and lose every refusal.
        let t = table(vec![
            stamping_edge("/main/talky", "/main/memory", "'in_remember'"),
            edge(
                "/main/memory",
                "/main/talky",
                Some("has(hop.route) && hop.route == 'bundle'"),
            ),
        ]);
        assert!(check_required_drains(&[lane_req()], &t).is_err());
    }

    #[test]
    fn a_subscription_that_stays_inside_the_hive_does_not_count() {
        let t = table(vec![
            stamping_edge("/main/talky", "/main/memory", "'in_remember'"),
            edge(
                "/main/memory",
                "/main/memory/store",
                Some("has(hop.route) && hop.route == 'reject'"),
            ),
        ]);
        assert!(check_required_drains(&[lane_req()], &t).is_err());
    }

    #[test]
    fn an_ingress_whose_lane_is_only_known_at_runtime_triggers_nothing() {
        // `'in_' + hop.kind` names no lane before the message exists. The
        // outward half of the contract check refuses to judge such an edge for
        // the same reason, and a trigger that guessed would refuse wirings
        // nobody can see the reason for.
        let t = table(vec![stamping_edge(
            "/main/talky",
            "/main/memory",
            "'in_' + hop.kind",
        )]);
        assert!(check_required_drains(&[lane_req()], &t).is_ok());
    }

    #[test]
    fn a_subscription_the_probe_cannot_judge_counts_as_a_drain() {
        // GH #237, the documented limit — and the direction it errs in. This
        // condition reads a key the lane probe does not carry, so the router
        // would SKIP the edge; reading that as "no drain" would refuse a
        // correct wiring, which is worse than not checking at all.
        let t = table(vec![
            stamping_edge("/main/talky", "/main/memory", "'in_remember'"),
            edge(
                "/main/memory",
                "/main/sink",
                Some("hop.route == 'reject' && context.session_id != ''"),
            ),
        ]);
        assert!(check_required_drains(&[lane_req()], &t).is_ok());
    }

    #[test]
    fn an_unconditional_subscription_is_a_drain() {
        // It takes everything that leaves the hive, this lane included.
        let t = table(vec![
            stamping_edge("/main/talky", "/main/memory", "'in_remember'"),
            edge("/main/memory", "/main/sink", None),
        ]);
        assert!(check_required_drains(&[lane_req()], &t).is_ok());
    }
}
