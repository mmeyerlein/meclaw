//! D58 lead (GH #1474, contract points 1, 2 and 3) -- every shipped
//! composite with a lead has its five lanes.
//!
//! WHY this file exists: a curator whose knob says a lead is wired asks it
//! on every call and waits in `win` for the verdict; if one of the edges
//! between the two hives is missing, that turn waits for ever (or a note
//! ends as a dead letter at the boundary). So the knob and the wiring go
//! together, in both directions: every shipped composite whose curator
//! sets the knob carries a `lead` hive and the edges for the three lanes to
//! it, the lane home (`in_lead`) and the lane of the lead's orders
//! (`in_feed`, D58 moveout, GH #1476: the early interrupt, which the lead
//! orders on the curator's own lane), and no composite draws those edges
//! without setting the knob. The lead hive itself carries its inner edges
//! and its own contract with the same lanes. talky is one such composite;
//! cogny is none.
//!
//! Reads the shipped templates; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_core::serde_json::Value;

fn mentions(e: &Value, route: &str) -> bool {
    e["condition"]
        .as_str()
        .is_some_and(|c| c.contains(&format!("'{route}'")))
}

fn edge(edges: &[Value], from: &str, to: &str) -> Vec<Value> {
    edges
        .iter()
        .filter(|e| e["from"] == from && e["to"] == to)
        .cloned()
        .collect()
}

/// The composites of this tree whose child `curator` is the template
/// `curator` (any version), with whether its knob says a lead is wired --
/// read by the policy's own rule (`switch_on`: "1", "true", "on" or JSON
/// true), so a composite that writes the knob in another form the policy
/// reads as on is held to the edges as well (review B M1).
fn composites() -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(repo("templates")).expect("templates") {
        let dir = entry.expect("a dir entry").path();
        let child = dir.join("curator").join("config.json");
        if !child.is_file() || !dir.join("config.json").is_file() {
            continue;
        }
        let cfg = read_json(&child);
        let is_ref = cfg["cell"]["type"] == "ref"
            && cfg["cell"]["template"]
                .as_str()
                .is_some_and(|t| t.starts_with("curator@"));
        if !is_ref {
            continue;
        }
        let wired = switch_on(&cfg["override_params"]["policy"][LEAD_KNOB]);
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        out.push((name, wired));
    }
    out.sort();
    out
}

#[test]
fn gh_d58_lead_every_shipped_composition_with_a_lead_has_its_five_lanes() {
    if !shipped() {
        return;
    }
    let all = composites();
    assert!(
        all.iter().any(|(c, w)| c == "talky" && *w),
        "talky wires a lead (`{LEAD_KNOB}` \"1\" at its curator's policy): {all:?}"
    );
    assert!(
        all.iter().any(|(c, w)| c == "cogny" && !*w),
        "cogny wires none: {all:?}"
    );
    let home_route = format!("'{FROM_LEAD}'");
    // The routes the lead hive emits: first its verdict -- the home edge of
    // a composite conditions on it -- and since D58 moveout (GH #1476) its
    // orders on `in_feed`.
    let hive = lead_hive();
    let emits = contract_routes(&hive, "emits");
    assert_eq!(
        emits.len(),
        2,
        "the lead hive emits two routes, its verdict and its orders: {emits:?}"
    );
    assert_ne!(
        emits[0], ORDERS_FROM_LEAD,
        "the verdict comes first: {emits:?}"
    );
    for (comp, wired) in &all {
        let cfg = read_json(&repo(&format!("templates/{comp}/config.json")));
        let edges = edges_of(&cfg);
        let out_edges = edge(&edges, "./curator", "./lead");
        let home_edges = edge(&edges, "./lead", "./curator");
        if !*wired {
            assert!(
                out_edges.is_empty() && home_edges.is_empty(),
                "{comp} draws edges between curator and lead without `{LEAD_KNOB}` \"1\""
            );
            continue;
        }
        let child = read_json(&repo(&format!("templates/{comp}/lead/config.json")));
        assert!(
            child["cell"]["template"]
                .as_str()
                .is_some_and(|t| t.starts_with("lead@")),
            "{comp}: the child `lead` is the template `lead`: {}",
            child["cell"]
        );
        for lane in TO_LEAD {
            assert!(
                out_edges.iter().any(|e| mentions(e, lane)),
                "{comp}: an edge `./curator -> ./lead` carries `{lane}`"
            );
        }
        // Contract point 2: the home edge takes the lead's verdict, brings it
        // home on `in_lead` and resets what the old inner edge reset
        // (`cur_origin`), on one edge (review B M1).
        assert!(
            home_edges.iter().any(|e| {
                mentions(e, &emits[0])
                    && e["modifier"]["set_hop"]["route"].as_str() == Some(home_route.as_str())
                    && e["modifier"]["delete_context"]
                        .as_array()
                        .is_some_and(|keys| keys.iter().any(|k| k == "cur_origin"))
            }),
            "{comp}: the edge `./lead -> ./curator` takes `{}`, brings it home on `{FROM_LEAD}` \
             and deletes `cur_origin`: {home_edges:?}",
            emits[0]
        );
        assert!(
            home_edges.iter().any(|e| mentions(e, ORDERS_FROM_LEAD)),
            "{comp}: an edge `./lead -> ./curator` passes the lead's orders on `{ORDERS_FROM_LEAD}`"
        );
    }

    // The lead hive: its inner edges and its contract.
    let inner = edges_of(&hive);
    for (from, to) in [
        (".", "./lead"),
        ("./lead", "./state"),
        ("./state", "./lead"),
        ("./lead", "."),
    ] {
        assert!(
            !edge(&inner, from, to).is_empty(),
            "the lead hive draws `{from} -> {to}`"
        );
    }
    for lane in TO_LEAD {
        assert!(
            edge(&inner, ".", "./lead")
                .iter()
                .any(|e| mentions(e, lane)),
            "the lead hive takes `{lane}` to `./lead`"
        );
    }
    let accepts = contract_routes(&hive, "accepts");
    for lane in TO_LEAD {
        assert!(
            accepts.iter().any(|r| r == lane),
            "the lead hive's contract accepts `{lane}`: {accepts:?}"
        );
    }
    let emits = contract_routes(&hive, "emits");
    assert_eq!(
        emits.len(),
        2,
        "the lead hive emits two routes, its verdict and its orders: {emits:?}"
    );
    assert!(
        emits.iter().any(|r| r == ORDERS_FROM_LEAD),
        "the lead hive's contract emits `{ORDERS_FROM_LEAD}`: {emits:?}"
    );
    for route in &emits {
        assert!(
            edge(&inner, "./lead", ".")
                .iter()
                .any(|e| mentions(e, route)),
            "the lead hive's edge `./lead -> .` carries `{route}`"
        );
    }
}
