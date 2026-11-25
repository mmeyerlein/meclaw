//! d54s specaus (GH #1584, R-54-9) -- every call of the quick voice lands on
//! `./quick`.
//!
//! WHY this file exists: the speculation (#1275) ran on a sibling cell of its
//! own (`./quick_spec`, #1398), and a speculating talky sent its bridge to
//! `./quick_bridge`. The speculation is gone (0.72 shipped it off), and with
//! it the reason for the siblings; the bridge stays (`bridge_target` `*`).
//! This pin holds what 0.72 did and does: the tool round, its bridge and a
//! complex turn's bridge all leave the shipped curator onto `./quick` and
//! carry no `hop.spec`.
//!
//! The claims are routing claims over the SHIPPED talky edges, evaluated by the
//! colony's own router (`apply_edges`), fed with the calls the SHIPPED curator
//! emits (the curator-hive harness runs its scripts synchronously). No clock
//! is involved, so there is no wait point for a paused clock to hold.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

const TALKY: &str = "templates/talky/config.json";
const BASE: &str = "/t";

fn plain_hive() -> Hive {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    h.check_emits = true;
    h
}

/// The normal curate of the turn at its route stamp.
fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, families: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": needs, "families": families});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let system = json!({"instructions": {"mode": {"text": "Be brief."},
                                         "sidecar": {"text": "End with a sidecar block."}}});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(text)], "system": system}),
    );
}

fn is_bridge(m: &Msg) -> bool {
    m.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .ends_with("-bridge")
}

/// talky's shipped edges as the colony's router holds them, rooted at `/t`.
fn talky_table() -> EdgeTable {
    let params = read_json(&repo(TALKY))["params"].clone();
    let hp: HiveParams = sj::from_value(params).expect("talky params");
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let abs = |ep: &str| match ep {
            "." => BASE.to_string(),
            other => format!("{BASE}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

/// The cells one message reaches from `from` (`/t/<cell>`), with each hop.
fn reach(
    t: &EdgeTable,
    from: &str,
    ctx: &Map<String, Value>,
    hop: &Map<String, Value>,
) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(ctx.clone(), hop.clone());
    apply_edges(t, &Path::new(&format!("{BASE}/{from}")), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out))
        .collect()
}

/// The model cells a curator call lands on (the curator's out hop, its context).
fn cells_of(t: &EdgeTable, call: &Msg) -> Vec<String> {
    let mut hop = call.hop.clone();
    hop.insert("iter".into(), json!("0"));
    reach(t, "curator", &call.context, &hop)
        .into_iter()
        .map(|(to, _)| to)
        .collect()
}

/// Pin: the tool round and its bridge leave in one emission, a complex turn
/// sends its bridge at the stamp -- all three onto `./quick`, none with
/// `hop.spec`.
#[test]
fn every_quick_call_lands_on_quick() {
    if !shipped() {
        return;
    }
    let t = talky_table();
    let mut h = plain_hive();
    seam(
        &mut h,
        "t1",
        "simple",
        "tool",
        "orga",
        "move my dentist to friday",
    );
    let mut calls = h.routed("quick");
    assert_eq!(
        calls.len(),
        2,
        "call and bridge together: {:?} {:?}",
        h.out,
        h.stderr
    );
    seam(&mut h, "t2", "complex", "", "web", "plan my trip to Rome");
    calls.extend(h.routed("quick"));
    assert!(
        calls.iter().filter(|c| is_bridge(c)).count() == 2 && calls.len() == 3,
        "{calls:?}"
    );
    for c in &calls {
        assert!(c.hop.get("spec").is_none(), "no spec mark: {:?}", c.hop);
        assert_eq!(cells_of(&t, c), vec!["/t/quick".to_string()], "{:?}", c.hop);
    }
    assert_eq!(h.emit_violations, Vec::<String>::new());
}
