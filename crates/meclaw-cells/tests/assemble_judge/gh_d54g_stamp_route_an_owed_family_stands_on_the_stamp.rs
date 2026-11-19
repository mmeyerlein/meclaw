//! d54g peerfix (GH #1568) -- a turn that carries a duty is offered the family
//! that fulfils it (`owed_families` of `talky/route`).
//!
//! WHY this file exists: a turn of another member's companion (origin
//! `peer`) carries the door's duty to pass on what is meant for the person
//! with the tool `peer`. The family `peer` reached the brain only when the
//! decider said yes to `families.peer` at `family_threshold`; measured on a
//! contact turn (K-189, d54g diagnosis 0.67 section 4) the decider said 0.676
//! and 0.823 raw, its calibration curve of the catalog row made 0.024 and
//! 0.091 of it, and the brain carried the duty in every request without the
//! tool to fulfil it. The bank behind the curve holds 185 points of
//! `families.peer` with no yes (OR-54g-295): the curve is right, a duty
//! without its tool is the fault. `owed_families` `{<origin>: [<family>]}`
//! is a composition setting, no model parameter: a turn with a message of
//! that origin has those families on its stamp whatever the decider said --
//! judged, unsure or failed. Only a family the composition offers
//! (`families`) is owed. Shipped `{}`: every stamp as before. The origin is
//! seen on `in_turn` and rides out and back on the hop (`owed`), like the
//! request text does (`state`).

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const TALKY: &str = "templates/talky/config.json";

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The composition of the cases: a family `peer` offered beside the shipped
/// ones, and owed to every turn with a message of origin `peer`.
fn owing() -> Vec<(&'static str, Value)> {
    vec![
        (
            "families",
            json!({"web": "searching or reading the web",
                   "history": "earlier conversations with this person",
                   "peer": "passing something on to the person"}),
        ),
        ("owed_families", json!({"peer": ["peer"]})),
    ]
}

/// A contact turn: the words of another member's companion.
fn peer_turn() -> Value {
    json!([{"origin": "peer", "type": "text", "speaker": "peer:1a2b3c4d",
            "text": "The result you waited for: the file says 'Lavendel'."}])
}

/// The person's own turn.
fn own_turn() -> Value {
    json!([{"origin": "user", "type": "text", "text": "Will it rain tomorrow?"}])
}

/// The decider's answer for turn `c-1` with `path` at `p`; `families.peer` at
/// the calibrated value of the measured contact turn (0.091), under every bar.
fn decision(path: &str, p: f64, owed: Option<&str>) -> Value {
    let answers = json!({
        "path": {"choice": path, "p": p},
        "intent": {"choice": "question", "p": 0.8},
        "needs.tool": {"yes": 0.2}, "needs.reasoning": {"yes": 0.1},
        "needs.research": {"yes": 0.0},
        "families.web": {"yes": 0.7}, "families.history": {"yes": 0.1},
        "families.peer": {"yes": 0.091}
    });
    let mut hop = json!({"finish_reason": "stop", "asked_at": now_ms().to_string()});
    if let Some(o) = owed {
        hop["owed"] = json!(o);
    }
    let mut doc = lane("in_decision", hop, json!({"turn_id": "c-1"}), json!([]));
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

/// The one stamp of `out`: its families and its fallback reason ("" judged).
fn stamped(out: &[Value]) -> (Value, String) {
    let m = on_route(out, "stamp");
    (m["stamp"]["families"].clone(), head_str(m, "fallback"))
}

/// A header key of `m` as text, "" when the header carries none (nothing owed
/// leaves the hop as it was).
fn head_str(m: &Value, key: &str) -> String {
    m["header"][key].as_str().unwrap_or("").to_string()
}

/// `None` when a set's `over` on `talky/route` passes the door, else why not.
fn refused(over: Value) -> Option<String> {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    let registry = TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    );
    let parsed = parse_subtree(&core_root().join("templates/talky"), &registry)
        .unwrap_or_else(|e| panic!("the shipped talky template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "route")
        .expect("talky has a route cell");
    check_override_params(&node, Some("route"), "talky", &over)
        .err()
        .map(|why| format!("a set's {over} on talky/route must pass the door: {why:?}"))
}

#[test]
fn gh_d54g_stamp_route_an_owed_family_stands_on_the_stamp() {
    // Declared, shipped {} (nothing owed), with a setting that says what it does.
    let cfg = config_of(ROUTE);
    assert_eq!(
        cfg["params"]["owed_families"],
        json!({}),
        "shipped owed_families"
    );
    let setting = &cfg["contract"]["settings"]["owed_families"];
    assert_eq!(
        setting["default"],
        json!({}),
        "the setting names the shipped default"
    );
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );
    assert_eq!(refused(json!({"owed_families": {"peer": ["peer"]}})), None);

    // in_turn: a turn of the owing origin names its owed families on the
    // decide call; the person's own turn and the shipped composition do not.
    let ask = |over: &[(&str, Value)], msgs: Value| {
        let out = run_cell(
            ROUTE,
            over,
            lane("in_turn", json!({}), json!({"turn_id": "c-1"}), msgs),
        )
        .0;
        head_str(on_route(&out, "decide"), "owed")
    };
    assert_eq!(
        ask(&owing(), peer_turn()),
        "peer",
        "a contact turn carries its duty out"
    );
    assert_eq!(
        ask(&owing(), own_turn()),
        "",
        "the person's turn owes nothing"
    );
    assert_eq!(
        ask(&[], peer_turn()),
        "",
        "shipped: nothing owed, the call of before"
    );

    // in_decision, judged: the decider's 0.091 bars peer; the duty stamps it.
    let owing = owing();
    let (fams, why) = stamped(&run_cell(ROUTE, &owing, decision("talk", 0.9, Some("peer"))).0);
    assert_eq!(
        (fams, why.as_str()),
        (json!(["peer", "web"]), ""),
        "judged with a duty"
    );
    let (fams, _) = stamped(&run_cell(ROUTE, &owing, decision("talk", 0.9, None)).0);
    assert_eq!(
        fams,
        json!(["web"]),
        "judged without a duty: the decider's families alone"
    );

    // Unsure path (unsure_families off as shipped): no family of the decider,
    // the owed one still.
    let (fams, why) = stamped(&run_cell(ROUTE, &owing, decision("talk", 0.3, Some("peer"))).0);
    assert_eq!(
        (fams, why.as_str()),
        (json!(["peer"]), "unsure"),
        "unsure with a duty"
    );

    // A failed decide call: the fallback stamp keeps the duty's tool.
    let mut failed = lane(
        "in_decision",
        json!({"finish_reason": "error", "error_code": "decide_timeout",
               "asked_at": now_ms().to_string(), "owed": "peer"}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    failed["decision"] = json!({});
    let (fams, why) = stamped(&run_cell(ROUTE, &owing, failed).0);
    assert_eq!(
        (fams, why.as_str()),
        (json!(["peer"]), "error"),
        "error with a duty"
    );

    // Only a family the composition offers is owed.
    let (fams, _) =
        stamped(&run_cell(ROUTE, &owing, decision("talk", 0.9, Some("peer,unoffered"))).0);
    assert_eq!(
        fams,
        json!(["peer", "web"]),
        "an unoffered family is never stamped"
    );

    // The duty rides on through the view the route orders and back to decide.
    let viewing = [owing.clone(), vec![("context_turns", json!(2))]].concat();
    let out = run_cell(
        ROUTE,
        &viewing,
        lane("in_turn", json!({}), json!({"turn_id": "c-1"}), peer_turn()),
    )
    .0;
    assert_eq!(
        head_str(on_route(&out, "view_ask"), "owed"),
        "peer",
        "the view ask carries it"
    );
    let mut viewed = lane(
        "in_view",
        json!({"state": "the result", "asked_at": now_ms().to_string(), "owed": "peer"}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    viewed["messages"] = json!([]);
    let out = run_cell(ROUTE, &viewing, viewed).0;
    assert_eq!(
        head_str(on_route(&out, "decide"), "owed"),
        "peer",
        "and the decide call after it"
    );

    // And through the second judge: out with its call, back with its verdict.
    let judging = [owing.clone(), vec![("second_judge", json!("second_judge"))]].concat();
    let first = json!({"path": {"choice": "talk", "p": 0.3}, "families.peer": {"yes": 0.091}});
    let now = now_ms().to_string();
    let mut second_view = lane(
        "in_second_view",
        json!({"state": "the result", "asked_at": now, "second_at": now,
               "first": first.to_string(), "owed": "peer"}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    second_view["system"] = json!({});
    let out = run_cell(ROUTE, &judging, second_view).0;
    assert_eq!(
        head_str(on_route(&out, "second_judge"), "owed"),
        "peer",
        "the second judge's call carries it"
    );
    let second = lane(
        "in_second",
        json!({"finish_reason": "stop", "asked_at": now, "second_at": now,
               "first": first.to_string(), "owed": "peer"}),
        json!({"turn_id": "c-1"}),
        json!([{"origin": "assistant", "type": "text",
                "text": "{\"path\": {\"choice\": \"talk\", \"p\": 0.9}}"}]),
    );
    let (fams, _) = stamped(&run_cell(ROUTE, &judging, second).0);
    assert_eq!(
        fams,
        json!(["peer"]),
        "the second verdict's stamp keeps the duty"
    );

    // The composite carries the hop key out and back on every leg the moment of
    // the ask (`asked_at`) travels: into the context where the route leaves, back
    // onto the hop where it arrives, and out of the context there.
    let talky = config_of(TALKY);
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .expect("talky edges");
    let mut out_legs = 0;
    let mut back_legs = 0;
    for e in edges {
        let m = &e["modifier"];
        let from_route = e["from"] == "./route" && e["to"] != ".";
        if from_route && m["set_context"].get("route_asked_at").is_some() {
            assert_eq!(
                m["set_context"]["route_owed"],
                json!("has(hop.owed) ? hop.owed : ''"),
                "a leg out of the route that carries the ask carries the duty: {e}"
            );
            out_legs += 1;
        }
        let deletes_state = m["delete_context"]
            .as_array()
            .is_some_and(|d| d.contains(&json!("route_asked_at")));
        if e["to"] == "./route" && deletes_state {
            assert_eq!(
                m["set_hop"]["owed"],
                json!("has(context.route_owed) ? context.route_owed : ''"),
                "a leg into the route that brings the ask back brings the duty: {e}"
            );
            assert!(
                m["delete_context"]
                    .as_array()
                    .is_some_and(|d| d.contains(&json!("route_owed"))),
                "and drops it from the context: {e}"
            );
            back_legs += 1;
        }
    }
    assert!(
        out_legs >= 5 && back_legs >= 5,
        "check of the setup: legs {out_legs}/{back_legs}"
    );
}
