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
pub fn validate_hive_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
) -> Result<(), MutationError> {
    addressed_port_boundary(diff, guard_scope, sealed)
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
pub fn collect_hive_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address) in addressed_port_boundary(diff, guard_scope, sealed) {
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
fn addressed_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
) -> Vec<(MutationError, String)> {
    let mut violations = Vec::new();
    if sealed.is_empty() {
        return violations;
    }
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
        for hive in sealed {
            check_endpoint_pair(
                hive,
                from,
                from_abs.as_str(),
                to_abs.as_str(),
                "from",
                &mut violations,
            );
            check_endpoint_pair(
                hive,
                to,
                to_abs.as_str(),
                from_abs.as_str(),
                "to",
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
