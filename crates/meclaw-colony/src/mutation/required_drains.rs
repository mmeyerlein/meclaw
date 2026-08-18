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

/// One `required_drains` entry of one hive, resolved to absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainRequirement {
    /// Absolute logical path of the declaring hive (e.g. `/main/memory`).
    pub hive_path: String,
    /// Absolute logical path of the port (e.g. `/main/memory/extract-glue`).
    pub port_path: String,
    /// The hop compartment a message on the drain route would carry.
    pub hop: BTreeMap<String, String>,
    /// The declaring hive's own sentence about why this pairing exists. It ends
    /// up verbatim in the rejection, because a refusal that cannot say what it
    /// is protecting is a refusal people route around.
    pub because: String,
}

impl DrainRequirement {
    fn inside(&self, abs: &str) -> bool {
        abs == self.hive_path || abs.starts_with(&format!("{}/", self.hive_path))
    }

    /// The hop the declaration describes, as the router would see it.
    fn probe_headers(&self) -> Headers {
        let mut hop = meclaw_core::serde_json::Map::new();
        for (k, v) in &self.hop {
            hop.insert(k.clone(), meclaw_core::serde_json::Value::String(v.clone()));
        }
        Headers::from_parts(meclaw_core::serde_json::Map::new(), hop)
    }
}

/// True iff something OUTSIDE the hive can send to this port.
fn port_is_wired_from_outside(req: &DrainRequirement, edges: &EdgeTable) -> bool {
    edges
        .iter()
        .any(|e| e.to.as_str() == req.port_path && !req.inside(e.from.as_str()))
}

/// True iff a message leaving the port with the declared hop would be routed to
/// a target outside the hive.
fn drain_exists(req: &DrainRequirement, edges: &EdgeTable) -> bool {
    let port = Path::new(&req.port_path);
    crate::edge_table::apply_edges(edges, &port, &req.probe_headers())
        .iter()
        .any(|d| !req.inside(d.target.as_str()))
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
        if !port_is_wired_from_outside(req, edges) {
            continue;
        }
        if drain_exists(req, edges) {
            continue;
        }
        let hop = req
            .hop
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(MutationError::RequiredDrainMissing(format!(
            "the port '{port}' of hive '{hive}' is wired from outside, but nothing takes a \
             message leaving it with hop {{{hop}}} out of the hive — {because}. Wire the drain \
             in the SAME mutation as the ingress: the two edges are one decision.",
            port = req.port_path,
            hive = req.hive_path,
            because = req.because,
        )));
    }
    Ok(())
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
        for d in hp.required_drains.unwrap_or_default() {
            // GH #202: a port name is a short name of a direct child, exactly
            // like `params.ports` — so it is decided by the same function and
            // not by a second, stricter opinion. The re-derived rule here used
            // to refuse every `/`, which dropped the `./recall` spelling that
            // `params.ports` accepts: the hive kept its declaration and lost
            // its guarantee, silently, in the lenient direction.
            let Some(port) = crate::mutation::port_boundary::canonical_port_name(&d.port) else {
                tracing::warn!(
                    hive = %s,
                    port = %d.port,
                    "required_drains[].port must be the short name of a direct child — this entry \
                     can never name a port, ignoring"
                );
                continue;
            };
            out.push(DrainRequirement {
                hive_path: s.to_string(),
                port_path: format!("{s}/{port}"),
                hop: d.hop,
                because: d.because,
            });
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
pub fn warn_on_missing_drains(
    reqs: &[DrainRequirement],
    edges: &[(String, String, Option<String>)],
) {
    if reqs.is_empty() {
        return;
    }
    let mut table = EdgeTable::new();
    for (from, to, cond) in edges {
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
        table.insert(crate::edge_table::Edge {
            id: meclaw_core::Uuid::now_v7(),
            from: Path::new(from),
            to: Path::new(to),
            condition,
            modifier: None,
        });
    }
    for req in reqs {
        if !port_is_wired_from_outside(req, &table) || drain_exists(req, &table) {
            continue;
        }
        tracing::warn!(
            hive = %req.hive_path,
            port = %req.port_path,
            because = %req.because,
            "a wired hive port has no drain for its declared route — messages leaving it that \
             way reach nothing (the mutation path refuses this; the birth topology is yours)"
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
            port_path: "/main/memory/glue".into(),
            hop: BTreeMap::from([("route".to_string(), "reject".to_string())]),
            because: "a rejected block leaves here".into(),
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
                port_path: "/mem/recall".into(),
                hop: BTreeMap::from([("route".to_string(), "reject".to_string())]),
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
            got.iter().map(|r| r.port_path.as_str()).collect::<Vec<_>>(),
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
}
