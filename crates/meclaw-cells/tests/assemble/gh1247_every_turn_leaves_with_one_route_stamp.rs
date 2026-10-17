//! GH #1247 — talky stamps every turn: one route stamp per turn, talk when unsure.
//!
//! The owner's routing line (R-WP-1 a, R-WP-9): a cell of its own classifies every
//! turn BEFORE the round is built, it recommends and the curator decides. Four
//! classes -- `talk`, `simple`, `complex`, `research` -- as params, one mechanism. The
//! cell is `talky/route`, a `code` cell that asks `talky/decide` (an `llm` cell on
//! `provider: decisions`, the form of `presenter/decide`) ONE call per turn and turns
//! the answer into the stamp
//! `{turn_id, path, p, intent, needs, families, duration}` -- exactly these seven keys,
//! because the collector's `in_route` lane reads that form and nothing else.
//!
//! What this file pins, measured at the receiver:
//!
//! 1. the composite wires it: the keeper's `turn` reaches `./route` beside
//!    `./collector`, `./route` and `./decide` talk to each other, and the stamp leaves
//!    for `./collector` on `in_route`; `./decide` is a decider and takes a grant;
//! 2. a turn leaves `./route` as ONE call to the decider whose `path` question offers
//!    exactly the four classes of the shipped params, plus the needs and the families;
//! 3. a sure verdict becomes the seven-key stamp, the needs and families it said yes to,
//!    under the channel turn it answers;
//! 4. unsure, an error of the decider or a verdict that came too late: `talk`.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{config_of, lane, now_ms, run_cell};
use serde_json::{Value, json};

const TALKY: &str = "templates/talky/config.json";
const ROUTE: &str = "templates/talky/route/config.json";
const DECIDE: &str = "templates/talky/decide/config.json";

fn edge<'a>(edges: &'a [Value], from: &str, to: &str, needle: &str) -> &'a Value {
    edges
        .iter()
        .find(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"].as_str().unwrap_or("").contains(needle)
        })
        .unwrap_or_else(|| panic!("talky has no edge {from} -> {to} on {needle}"))
}

fn one(out: Vec<Value>, route: &str) -> Value {
    assert_eq!(out.len(), 1, "expected one emission, got {out:?}");
    let m = out.into_iter().next().expect("one emission");
    assert_eq!(m["header"]["route"], route, "{m}");
    m
}

fn decision(asked_at: i64, answers: Value) -> Value {
    let mut doc = lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": asked_at.to_string()}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    doc["decision"] = json!({"answers": answers, "missing": [], "model": "d"});
    doc
}

fn stamp(doc: Value) -> Value {
    let m = one(run_cell(ROUTE, &[], doc).0, "stamp");
    assert_eq!(m["header"]["turn_id"], "c-1", "{m}");
    assert_eq!(m["header"]["path"], m["stamp"]["path"], "{m}");
    let mut keys: Vec<&str> = m["stamp"]
        .as_object()
        .expect("stamp object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "duration", "families", "intent", "needs", "p", "path", "turn_id"
        ],
        "{m}"
    );
    assert!(
        m["stamp"]["duration"].as_i64().is_some_and(|d| d >= 0),
        "{m}"
    );
    m["stamp"].clone()
}

#[test]
fn every_turn_leaves_with_one_route_stamp() {
    // 1. the wiring
    let talky = config_of(TALKY);
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let into = edge(&edges, "./session-keeper", "./route", "'turn'");
    assert_eq!(into["modifier"]["set_hop"]["route"], "'in_turn'");
    edge(&edges, "./session-keeper", "./collector", "'turn'");
    edge(&edges, "./route", "./decide", "'decide'");
    let back = edge(&edges, "./decide", "./route", "credential_request");
    assert_eq!(back["modifier"]["set_hop"]["route"], "'in_decision'");
    let out = edge(&edges, "./route", "./collector", "'stamp'");
    assert_eq!(out["modifier"]["set_hop"]["route"], "'in_route'");
    let decide = config_of(DECIDE);
    assert_eq!(decide["params"]["provider"], "decisions");
    assert_eq!(decide["params"]["credential_grant_id"], "");

    // 2. a turn is one call over the four classes
    let classes = config_of(ROUTE)["params"]["classes"].clone();
    let mut names: Vec<&str> = classes
        .as_object()
        .expect("classes")
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["complex", "research", "simple", "talk"]);
    let turn = lane(
        "in_turn",
        json!({}),
        json!({"turn_id": "c-1"}),
        json!([{"origin": "user", "type": "text", "text": "Will it rain in Berlin?"}]),
    );
    let call = one(run_cell(ROUTE, &[], turn).0, "decide");
    assert_eq!(call["decide"]["state"], "Will it rain in Berlin?");
    let qs = &call["decide"]["questions"];
    let mut offered: Vec<&str> = qs["path"]["options"]
        .as_object()
        .expect("path options")
        .keys()
        .map(String::as_str)
        .collect();
    offered.sort_unstable();
    assert_eq!(offered, names);
    for k in [
        "needs.tool",
        "needs.reasoning",
        "needs.research",
        "families.web",
    ] {
        assert_eq!(qs[k]["kind"], "yes_no", "{k}: {qs}");
    }
    assert!(call["header"]["asked_at"].as_str().is_some(), "{call}");

    // 3. a sure verdict is the stamp
    let sure = json!({
        "path": {"choice": "simple", "p": 0.91},
        "intent": {"choice": "question", "p": 0.8},
        "needs.tool": {"yes": 0.9}, "needs.reasoning": {"yes": 0.1},
        "needs.research": {"yes": 0.2},
        "families.web": {"yes": 0.95}, "families.history": {"yes": 0.05}
    });
    // Asked an hour ahead: the duration clamps to 0, so no start time of the
    // cell's process ever makes this verdict late (dev-rules § 11).
    let s = stamp(decision(now_ms() + 3_600_000, sure.clone()));
    assert_eq!(s["turn_id"], "c-1");
    assert_eq!(s["path"], "simple");
    assert_eq!(s["p"], 0.91);
    assert_eq!(s["intent"], "question");
    assert_eq!(s["needs"], json!(["tool"]));
    assert_eq!(s["families"], json!(["web"]));

    // 4. unsure, error, late: talk
    let mut unsure = sure.clone();
    unsure["path"] = json!({"choice": "complex", "p": 0.3});
    let s = stamp(decision(now_ms(), unsure));
    assert_eq!(s["path"], "talk", "unsure: {s}");
    let mut failed = lane(
        "in_decision",
        json!({"finish_reason": "error", "error_code": "upstream_timeout",
               "asked_at": now_ms().to_string()}),
        json!({"turn_id": "c-1"}),
        json!([]),
    );
    failed["detail"] = json!("the decider timed out");
    let s = stamp(failed);
    assert_eq!(s["path"], "talk", "error: {s}");
    assert_eq!(s["p"], 0.0, "error: {s}");
    let s = stamp(decision(now_ms() - 600_000, sure));
    assert_eq!(s["path"], "talk", "late: {s}");
}
