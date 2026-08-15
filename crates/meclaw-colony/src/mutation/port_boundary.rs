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
    /// Declared port names — short names of DIRECT children. May be empty
    /// ("the hive path itself is the only address").
    pub ports: Vec<String>,
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
pub fn validate_hive_port_boundary(
    diff: &JsonValue,
    guard_scope: &str,
    sealed: &[SealedHive],
) -> Result<(), MutationError> {
    if sealed.is_empty() {
        return Ok(());
    }
    let Some(obj) = diff.as_object() else {
        // Shape errors are surfaced by schema validation; nothing to bound.
        return Ok(());
    };
    let Some(adds) = obj.get("add_edges").and_then(|v| v.as_array()) else {
        return Ok(());
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
            check_endpoint_pair(hive, from, from_abs.as_str(), to_abs.as_str(), "from")?;
            check_endpoint_pair(hive, to, to_abs.as_str(), from_abs.as_str(), "to")?;
        }
    }
    Ok(())
}

/// One half of the symmetric check: `endpoint_abs` is the endpoint under
/// scrutiny, `other_abs` its partner on the same edge.
fn check_endpoint_pair(
    hive: &SealedHive,
    endpoint_raw: &str,
    endpoint_abs: &str,
    other_abs: &str,
    side: &str,
) -> Result<(), MutationError> {
    if !hive.is_interior(endpoint_abs) {
        return Ok(()); // not inside this hive — no boundary involved
    }
    if hive.is_port(endpoint_abs) {
        return Ok(()); // that IS the port
    }
    if hive.contains(other_abs) {
        return Ok(()); // both ends inside the hive (or the hive marker itself)
    }
    let ports = if hive.ports.is_empty() {
        "none (transit through the hive path only)".to_string()
    } else {
        hive.ports.join(", ")
    };
    Err(MutationError::HivePortBoundary(format!(
        "add_edges[].{side}='{endpoint_raw}' resolves to '{endpoint_abs}', an interior node of the \
         sealed hive '{hive_path}', while the edge's other endpoint '{other_abs}' lies outside it \
         — that wires past the port. Declared ports of '{hive_path}': {ports}. Wire the hive path \
         itself or one of its ports.",
        hive_path = hive.path
    )))
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
            out.push(SealedHive {
                path: s.to_string(),
                ports,
            });
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
            }],
            "only the hive that declared ports is sealed"
        );
    }
}
