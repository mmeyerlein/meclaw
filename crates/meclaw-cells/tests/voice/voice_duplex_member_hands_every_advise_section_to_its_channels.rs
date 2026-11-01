//! D48 (live-2) -- the member hands every advise section of the voice
//! contract to its channels.
//!
//! The voice cell takes five sections on `in_advise` (`fact`, `context`,
//! `correction`, `interrupt`, `renew`; `templates/voice/config.json`
//! `consumes.hop.section`). The member's edge from `./assistants` to
//! `./channels` is the one shipped way a sidecar section reaches a voice
//! channel, and until the review of this wave it let three of them through:
//! `interrupt` and `renew` never reached a cell that understands them.
//!
//! Measured on the SHIPPED edge, compiled and evaluated by the colony's own
//! CEL evaluator, so a condition that reads right and evaluates wrong fails
//! here too.

use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use meclaw_core::serde_json::{Map, Value, json};

const MEMBER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/member/config.json"
);

const ASSISTANT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/assistant/config.json"
);

/// The sections the voice cell's `in_advise` takes, in the order its
/// contract names them.
const SECTIONS: [&str; 5] = ["fact", "context", "correction", "interrupt", "renew"];

/// The shipped condition of the sidecar edge `./assistants` -> `./channels`.
fn advise_condition() -> String {
    let raw = std::fs::read_to_string(MEMBER).expect("templates/member/config.json is readable");
    let member: Value =
        meclaw_core::serde_json::from_str(&raw).expect("the member template parses");
    let edges = member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member declares its edges");
    let mut found: Vec<String> = edges
        .iter()
        .filter(|e| e["from"] == json!("./assistants") && e["to"] == json!("./channels"))
        .filter_map(|e| e["condition"].as_str())
        .filter(|c| c.contains("hop.route == 'sidecar'"))
        .map(str::to_string)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "exactly one sidecar edge to the channels: {found:?}"
    );
    found.remove(0)
}

/// Whether a sidecar message of `section` for a bound channel passes.
fn passes(condition: &str, section: &str) -> bool {
    let cond = parse_condition(condition).expect("the shipped condition compiles");
    let mut hop = Map::new();
    hop.insert("route".into(), json!("sidecar"));
    hop.insert("section".into(), json!(section));
    let mut context = Map::new();
    context.insert("channel_node".into(), json!("/m/channels/voice"));
    evaluate_condition(&cond, &context, &hop).expect("the condition evaluates")
}

#[test]
fn every_advise_section_reaches_the_channels() {
    let condition = advise_condition();
    for section in SECTIONS {
        assert!(
            passes(&condition, section),
            "the section `{section}` of the voice contract never reaches a channel: {condition}"
        );
    }
}

/// The filter stays a filter: a section the voice cell does not take, such
/// as the memory's own, stays off the channels.
#[test]
fn a_section_outside_the_contract_stays_off_the_channels() {
    let condition = advise_condition();
    assert!(!passes(&condition, "memory"), "{condition}");
}

/// The shipped condition of the assistant's sidecar edge `./cogny` -> `.`.
fn cogny_up_condition() -> String {
    let raw =
        std::fs::read_to_string(ASSISTANT).expect("templates/assistant/config.json is readable");
    let assistant: Value =
        meclaw_core::serde_json::from_str(&raw).expect("the assistant template parses");
    let edges = assistant["params"]["graph"]["edges"]
        .as_array()
        .expect("the assistant declares its edges");
    let mut found: Vec<String> = edges
        .iter()
        .filter(|e| e["from"] == json!("./cogny") && e["to"] == json!("."))
        .filter_map(|e| e["condition"].as_str())
        .filter(|c| c.contains("hop.route == 'sidecar'"))
        .map(str::to_string)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "exactly one sidecar edge up from cogny: {found:?}"
    );
    found.remove(0)
}

/// Review re-check Re-k1: the reasoning core's advice stays inside the
/// assistant -- all five sections, not only the three there were before D48.
/// Otherwise an `interrupt` or a `renew` of cogny's would leave the level and
/// pass the member's filter above into the call.
#[test]
fn cogny_keeps_every_advise_section_inside_the_assistant() {
    let condition = cogny_up_condition();
    for section in SECTIONS {
        assert!(
            !passes(&condition, section),
            "cogny's `{section}` leaves the assistant: {condition}"
        );
    }
    assert!(passes(&condition, "memory"), "{condition}");
}
