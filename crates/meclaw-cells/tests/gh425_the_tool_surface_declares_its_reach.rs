//! GH #425 — a tool surface that contains a tool reaching OUT of the assistant
//! says so in its contract.
//!
//! `templates/tools/template.json` argued, and still argues, that a second
//! outward lane would put the choice of tool back into the caller's edge table.
//! That argument is about tool RESULTS and is untouched: `tool_result` is the
//! one result lane, whatever the tool was.
//!
//! `build` is a different class — the REACH of the surface. Until R6 this
//! surface reached nowhere; now it carries a tool whose whole job is to address
//! `/os/builder`, four levels up. The honest place for that is the contract, and
//! the precedent is one level down: `sandbox_union` exists because a process
//! radius existed COLLECTIVELY and was invisible while it was spread over four
//! `config.json` files.

use meclaw_core::serde_json::{Value, json};

const TOOLS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/config.json"
);

fn tools_config() -> Value {
    let raw = std::fs::read_to_string(TOOLS).expect("tools config");
    meclaw_core::serde_json::from_str(&raw).expect("json")
}

fn routes(cfg: &Value, slot: &str) -> Vec<String> {
    cfg.pointer(slot)
        .and_then(Value::as_array)
        .expect(slot)
        .iter()
        .map(|e| e["route"].as_str().expect("route").to_string())
        .collect()
}

#[test]
fn the_tool_surface_declares_the_lane_that_leaves_the_assistant() {
    let cfg = tools_config();
    let emits = routes(&cfg, "/params/contract/emits");
    assert!(
        emits.iter().any(|r| r == "build"),
        "a tool surface with a tool that reaches out of the assistant declares it"
    );
    let accepts = routes(&cfg, "/params/contract/accepts");
    assert!(accepts.iter().any(|r| r == "in_build_result"));
}

#[test]
fn the_outward_build_lane_is_paired_with_its_return() {
    let cfg = tools_config();
    let drains = cfg
        .pointer("/params/required_drains")
        .and_then(Value::as_array)
        .expect("required_drains")
        .clone();
    assert!(
        drains
            .iter()
            .any(|d| d["emits"] == json!("build") && d["accepts"] == json!("in_build_result")),
        "a caller that carries the request out and does not carry the answer back \
         has a tool round that never ends: the brain waits for a turn that was \
         produced somewhere it cannot reach"
    );
}

#[test]
fn the_reach_is_declared_once_and_the_result_lane_stays_alone() {
    let cfg = tools_config();
    let emits = routes(&cfg, "/params/contract/emits");
    assert_eq!(
        emits.iter().filter(|r| *r == "tool_result").count(),
        1,
        "one result lane, whatever the tool was"
    );
    assert_eq!(
        emits.len(),
        3,
        "the surface emits a result, reaches once, and — since GH #464 — hands out its own \
         declarations. A FOURTH outward lane needs its own argument, in this contract, \
         before it exists in an edge: {emits:?}"
    );
    assert_eq!(
        emits.iter().filter(|r| *r == "tool_schemas").count(),
        1,
        "the declaration lane is the third, and it is not a result: a result belongs to a \
         call somebody made, a declaration to a start-up question, and folding the two \
         would make every collector guess which of them it was handed: {emits:?}"
    );
}

#[test]
fn every_new_occupant_is_declared_in_the_reentrancy_block() {
    // GH #286's hazard is the entry nobody declared: a swap that quietly turns a
    // parallel tool round sequential. A declaration with a hole cannot catch it.
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/tools/template.json"
    ))
    .expect("tools template.json");
    let t: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let reentrancy = t["reentrancy"].as_object().expect("reentrancy block");
    for occupant in [
        "bash",
        "web_fetch",
        "web_search",
        "unknown",
        "build",
        "apply",
    ] {
        let entry = reentrancy
            .get(occupant)
            .unwrap_or_else(|| panic!("no reentrancy entry for {occupant}"));
        assert!(entry["reentrant"].is_boolean(), "{occupant}: no verdict");
        assert!(
            !entry["because"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "{occupant}: a verdict with no argument is a verdict nobody can check"
        );
    }
}

#[test]
fn the_sandbox_union_counts_the_new_occupants_among_its_posts() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/tools/template.json"
    ))
    .expect("tools template.json");
    let t: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let because = t["sandbox_union"]["because"]
        .as_str()
        .expect("sandbox_union.because");
    assert!(
        because.contains("build") && because.contains("apply"),
        "a union that leaves half its posts out of the calculation is the defect \
         this block exists to prevent"
    );
}

#[test]
fn a_result_that_lost_its_class_marker_still_reaches_a_cell() {
    // `hop` is one-hop. If the class marker is lost anywhere on the eight hops
    // home, a conditioned door alone would drop the answer at the hive path and
    // the tool round would wait forever. There is a DEFAULT door, and it leads
    // to the draft side — a draft applies nothing, which is the benign reading
    // of "I no longer know which of the two this was".
    let cfg = tools_config();
    let edges = cfg
        .pointer("/params/graph/edges")
        .and_then(Value::as_array)
        .expect("edges");
    let fallback = edges.iter().find(|e| {
        e["from"] == json!(".")
            && e["default"] == json!(true)
            && e["condition"]
                .as_str()
                .unwrap_or_default()
                .contains("in_build_result")
    });
    let fallback = fallback.expect("no default door for in_build_result");
    assert_eq!(
        fallback["to"],
        json!("./build"),
        "the fallback leads to the side that applies nothing"
    );
}
