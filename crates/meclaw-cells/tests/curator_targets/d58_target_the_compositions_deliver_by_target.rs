//! D58 target (GH #1483, contract points 1, 2, 5, 8, 10, 12, 13) -- the three
//! composites that ship a curator deliver its calls by the handle it names.
//!
//! WHY this file exists: the curator names no way and no cell any more. Every
//! model call leaves on `model_call` with `hop.target`, an errand on
//! `errand`; each model cell of a composite reports its window profile over
//! its package line and answers the curator's ask, with or without a handle
//! (OR-d58-2.target.15). The composite is where the names live: talky,
//! cogny and simple-agent wire `hop.target` to their cells, lift the target
//! and a bridge's kind into the context of the call (`curator_target`,
//! `bridge`), name the target of a curate where no lead does (cogny its one
//! brain, simple-agent `brain` or `strong` as its door names it), and carry the value
//! set of their former role in the profile of each model cell alone -- no
//! knob `role` and no value of a set as a knob of the curator instance
//! (OR-d58-2.target.60).
//!
//! The claims are routing claims over the SHIPPED edges, evaluated by the
//! colony's own router (`apply_edges`), and claims over the shipped params.
//! No clock, no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Headers, Path, Uuid};

const BASE: &str = "/t";

/// A composite's shipped edges as the colony's router holds them, at `/t`.
fn table(composite: &str) -> EdgeTable {
    let params = read_json(&repo(&format!("templates/{composite}/config.json")))["params"].clone();
    let hp: HiveParams = sj::from_value(params).expect("hive params");
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

/// The cells one message from `/t/<from>` reaches, each with its headers.
fn reach(t: &EdgeTable, from: &str, ctx: Value, hop: Value) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(obj(ctx), obj(hop));
    let mut out: Vec<(String, Headers)> =
        apply_edges(t, &Path::new(&format!("{BASE}/{from}")), &hs)
            .into_iter()
            .map(|d| (d.target.as_str().to_string(), d.headers_out))
            .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn cells(v: &[(String, Headers)]) -> Vec<String> {
    v.iter().map(|(to, _)| to.clone()).collect()
}

fn at(cell: &str) -> String {
    format!("{BASE}/{cell}")
}

/// A model call of the curator as `./policy` emits it.
fn call(target: Option<&str>, extra: Value) -> Value {
    let mut hop = json!({"route": "model_call", "curator_call": "c1", "turn_id": "t1",
                         "session_id": "s1", "iter": "0"});
    if let Some(t) = target {
        hop["target"] = json!(t);
    }
    if let Value::Object(m) = extra {
        for (k, v) in m {
            hop[k] = v;
        }
    }
    hop
}

/// The model cells of each composite, by the name its curator knows them.
const MODEL_CELLS: [(&str, &[&str]); 3] = [
    ("talky", &["brain", "quick", "quick_spec", "quick_bridge"]),
    ("cogny", &["brain"]),
    ("simple-agent", &["brain", "strong"]),
];

#[test]
fn d58_target_talky_delivers_each_call_by_its_target() {
    if !shipped() {
        return;
    }
    let t = table("talky");
    let mut failures = Vec::new();
    for cell in ["brain", "quick", "quick_spec", "quick_bridge"] {
        let got = reach(&t, "curator", json!({}), call(Some(cell), json!({})));
        if cells(&got) != vec![at(cell)] {
            failures.push(format!("{cell}: {:?}", cells(&got)));
            continue;
        }
        let ctx = &got[0].1.context;
        if ctx.get("curator_target") != Some(&json!(cell)) {
            failures.push(format!(
                "{cell}: curator_target {:?}",
                ctx.get("curator_target")
            ));
        }
        if ctx.get("call_kind") != Some(&json!("")) {
            failures.push(format!("{cell}: call_kind {:?}", ctx.get("call_kind")));
        }
    }
    // D58 moveout (contract point 3): a side call names its kind; the
    // composite lifts it for the curator and the collector.
    let got = reach(
        &t,
        "curator",
        json!({}),
        call(
            Some("quick_bridge"),
            json!({"curator_call": "c1-bridge", "call_kind": "side"}),
        ),
    );
    if cells(&got) != vec![at("quick_bridge")]
        || got[0].1.context.get("call_kind") != Some(&json!("side"))
    {
        failures.push(format!("bridge: {got:?}"));
    }
    // A call without a target is the brain's (the target that ends the turn).
    let got = reach(&t, "curator", json!({}), call(None, json!({})));
    if cells(&got) != vec![at("brain")] {
        failures.push(format!("no target: {:?}", cells(&got)));
    }
    // The old routes reach no model cell.
    for old in ["brain", "quick"] {
        let mut hop = call(Some(old), json!({}));
        hop["route"] = json!(old);
        let got = reach(&t, "curator", json!({}), hop);
        if !got.is_empty() {
            failures.push(format!("route {old}: {:?}", cells(&got)));
        }
    }
    // An errand leaves the composite and opens the collector's departure.
    let got = reach(
        &t,
        "curator",
        json!({}),
        json!({"route": "errand", "consult_class": "consult", "curator_call": "c2"}),
    );
    if cells(&got) != vec![BASE.to_string(), at("collector")] {
        failures.push(format!("errand: {:?}", cells(&got)));
    }
    // A bridge's error goes to `./errors` (it has no copy that would take the
    // round to the brain); a quick call's error does not.
    let err = json!({"finish_reason": "error", "model": "m"});
    let bridge_err = reach(
        &t,
        "quick",
        json!({"curator_call": "c1-bridge", "call_kind": "side"}),
        err.clone(),
    );
    let quick_err = reach(
        &t,
        "quick",
        json!({"curator_call": "c1", "call_kind": ""}),
        err,
    );
    if !cells(&bridge_err).contains(&at("errors")) || cells(&quick_err).contains(&at("errors")) {
        failures.push(format!(
            "errors: bridge {:?} quick {:?}",
            cells(&bridge_err),
            cells(&quick_err)
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn d58_target_cogny_and_simple_agent_name_the_target_of_a_curate() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    let curate = json!({"route": "curate", "iter": "0"});
    let got = reach(&table("cogny"), "collector", json!({}), curate.clone());
    match got.iter().find(|(to, _)| to == &at("curator")) {
        Some((_, h)) if h.hop.get("target") == Some(&json!("brain")) => {}
        other => failures.push(format!("cogny curate: {other:?}")),
    }
    let sa = table("simple-agent");
    // D58 proof (GH #1475): the door names the turn's target (`sa_target`),
    // the composite carries the name to the curate unchanged.
    for (named, want) in [
        (json!("strong"), "strong"),
        (json!("brain"), "brain"),
        (Value::Null, "brain"),
    ] {
        let ctx = if named.is_null() {
            json!({})
        } else {
            json!({"sa_target": named})
        };
        let got = reach(&sa, "collector", ctx, curate.clone());
        match got.iter().find(|(to, _)| to == &at("curator")) {
            Some((_, h)) if h.hop.get("target") == Some(&json!(want)) => {}
            other => failures.push(format!("simple-agent target {named}: {other:?}")),
        }
    }
    // The model call goes by its target, not by the name in the context.
    for (target, ctx) in [
        ("strong", json!({"sa_target": "brain"})),
        ("brain", json!({"sa_target": "strong"})),
    ] {
        let got = reach(&sa, "curator", ctx, call(Some(target), json!({})));
        if cells(&got) != vec![at(target)]
            || got[0].1.context.get("curator_target") != Some(&json!(target))
        {
            failures.push(format!("simple-agent {target}: {got:?}"));
        }
    }
    let got = reach(
        &table("cogny"),
        "curator",
        json!({}),
        call(Some("brain"), json!({})),
    );
    if cells(&got) != vec![at("brain")] {
        failures.push(format!("cogny brain: {:?}", cells(&got)));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn d58_target_every_model_cell_reports_its_profile_and_answers_both_asks() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    for (composite, names) in MODEL_CELLS {
        let t = table(composite);
        for cell in names.iter().copied() {
            let cfg = read_json(&repo(&format!("templates/{composite}/{cell}/config.json")));
            let p = &cfg["params"];
            if p["package_line"] != json!(true) {
                failures.push(format!("{composite}/{cell}: no package line"));
            }
            if !p["window_profile"]
                .as_object()
                .is_some_and(|m| !m.is_empty())
            {
                failures.push(format!("{composite}/{cell}: no window profile"));
            }
            let line = reach(
                &t,
                cell,
                json!({}),
                json!({"route": "package", "package_of": format!("/t/{cell}")}),
            );
            if cells(&line) != vec![at("curator")]
                || line[0].1.hop.get("route") != Some(&json!("in_catalog"))
            {
                failures.push(format!("{composite}/{cell}: package line {line:?}"));
            }
            let named = reach(
                &t,
                "curator",
                json!({}),
                json!({"route": "catalog_ask", "catalog_for": cell}),
            );
            if cells(&named) != vec![at(cell)] {
                failures.push(format!("{composite}/{cell}: named ask {:?}", cells(&named)));
            }
        }
        let all = reach(&t, "curator", json!({}), json!({"route": "catalog_ask"}));
        let mut want: Vec<String> = names.iter().map(|c| at(c)).collect();
        want.sort();
        if cells(&all) != want {
            failures.push(format!(
                "{composite}: ask without a handle {:?}",
                cells(&all)
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The value sets of the former roles talky and consult (curator policy
/// `ROLES`), as they stood.
fn value_set(role: &str) -> Value {
    match role {
        "talky" => json!({"compress_at": 0.5, "rebuild_to": 0.45,
              "keep_recent": 40, "keep_recent_tokens": 80000,
              "horizon": "day", "tiers": "raw,summary,none",
              "keep_rounds": 1, "gc": "1", "gc_scaffold": "1", "stub_tools_after": 20,
              "broadcast_mode": "tail", "recall_push": "1", "recall_budget": 200,
              "short_ids": "1", "identity_budget": "full", "window_target": 100000,
              "notes_share": 0.02, "state_lines": 8}),
        "consult" => json!({"compress_at": 0.5, "rebuild_to": 0.35,
                "keep_recent": 10,
                "horizon": "task", "tiers": "raw,summary",
                "keep_rounds": 2, "gc": "0", "gc_scaffold": "1", "stub_tools_after": 10,
                "broadcast_mode": "tail", "recall_push": "", "recall_budget": 0,
                "short_ids": "1", "identity_budget": "full", "window_target": 0,
                "notes_share": 0, "state_lines": 0}),
        other => panic!("no value set {other}"),
    }
}

#[test]
fn d58_target_the_value_sets_moved_into_the_profiles_alone() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    let tree_holders = [
        ("talky", "brain", "talky"),
        ("cogny", "brain", "consult"),
        ("simple-agent", "brain", "talky"),
        ("simple-agent", "strong", "talky"),
    ];
    for (composite, cell, role) in tree_holders {
        let p = &read_json(&repo(&format!("templates/{composite}/{cell}/config.json")))["params"]["window_profile"];
        if p["system"] != json!("delta") {
            failures.push(format!("{composite}/{cell}: holds no tree: {p}"));
        }
        for (k, v) in value_set(role).as_object().expect("set") {
            if &p[k] != v {
                failures.push(format!("{composite}/{cell}: {k} {} != {v}", p[k]));
            }
        }
    }
    // A fast target carries every value its call's cut reads and no value of
    // the wall (OR-d58-2.target.16, .60): the short window, no note, the
    // rest of the call's values as the main target's.
    for cell in ["quick", "quick_spec", "quick_bridge"] {
        let p = &read_json(&repo(&format!("templates/talky/{cell}/config.json")))["params"]["window_profile"];
        let mut values: Vec<&String> = p
            .as_object()
            .expect("profile")
            .keys()
            .filter(|k| value_set("talky").get(k.as_str()).is_some())
            .collect();
        values.sort();
        let call = [
            "broadcast_mode",
            "identity_budget",
            "keep_recent",
            "notes_share",
            "short_ids",
            "state_lines",
            "window_target",
        ];
        if values != call.iter().collect::<Vec<_>>()
            || call[..]
                .iter()
                .filter(|k| !["keep_recent", "notes_share"].contains(k))
                .any(|k| p[*k] != value_set("talky")[*k])
            || p["keep_recent"] != json!(4)
            || p["notes_share"] != json!(0)
            || p["system"] != json!("full")
        {
            failures.push(format!("talky/{cell}: {p}"));
        }
    }
    for (composite, role) in [
        ("talky", "talky"),
        ("cogny", "consult"),
        ("simple-agent", "talky"),
    ] {
        let pol = &read_json(&repo(&format!("templates/{composite}/curator/config.json")))["override_params"]
            ["policy"];
        if pol.get("role").is_some() {
            failures.push(format!("{composite}: the curator still has a role: {pol}"));
        }
        for k in value_set(role).as_object().expect("set").keys() {
            if pol.get(k).is_some() {
                failures.push(format!(
                    "{composite}: the curator instance sets {k}: {}",
                    pol[k]
                ));
            }
        }
    }
    // The lead is its own hive (d58-2 lead): talky sets the lead's knobs on
    // its `./lead` marker, the intake's on the curator's.
    let ov = &read_json(&repo("templates/talky/curator/config.json"))["override_params"];
    let lead = &read_json(&repo("templates/talky/lead/config.json"))["override_params"]["lead"];
    if lead["path_routes"]
        .as_str()
        .is_none_or(|s| !s.contains("*:brain"))
        || ov["intake"]["errand_tool"] != lead["errand_tool"]
    {
        failures.push(format!("talky lead/intake: {ov} {lead}"));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
