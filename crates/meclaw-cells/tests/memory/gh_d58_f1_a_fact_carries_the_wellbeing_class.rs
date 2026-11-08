//! GH #1456 (D58 F1) -- a sidecar fact may carry the closed class `wellbeing`.
//!
//! WHY this file exists: the curator's wellbeing line keyed on the predicate
//! `health`, which the sidecar never writes (its predicates are open keys).
//! The model classifies the fact itself instead: `wellbeing: true` when the
//! fact says how the person is doing right now, in body or mind. The offered
//! schema declares the field OPTIONAL (an old payload without it stays valid,
//! and missing reads as false); the contract names it; the ingress
//! (`extract-glue`) stages it only as the boolean `true` and interprets
//! nothing -- a fact without it is staged as before, byte for byte. Bare
//! script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const SCHEMAS: &str = "../../templates/memory-hive/schemas/config.json";
const CONTRACT: &str = "../../templates/memory-hive/inline-contract.md";

fn offered_fact_item() -> Value {
    let out = meclaw_testing::emit_one(
        &meclaw_testing::shipped_script(SCHEMAS),
        &json!({"tools": ["*"], "messages": []}),
    );
    out["sidecar"][0]["schema"]["properties"]["facts"]["items"].clone()
}

/// Pin: the schema offers `wellbeing` as an optional boolean.
#[test]
fn the_schema_offers_the_class_as_an_optional_boolean() {
    let item = offered_fact_item();
    assert_eq!(
        item["properties"]["wellbeing"],
        json!({"type": "boolean"}),
        "{item}"
    );
    let required = item["required"].as_array().expect("required");
    assert!(
        !required.iter().any(|k| k == "wellbeing"),
        "optional: a payload of before stays valid: {item}"
    );
}

/// Pin: the contract block the model reads names the class.
#[test]
fn the_contract_names_the_class() {
    let contract = std::fs::read_to_string(CONTRACT).expect("contract");
    let block = contract
        .split("```text\n")
        .nth(1)
        .and_then(|b| b.split("\n```").next())
        .expect("the contract block");
    assert!(block.contains("`wellbeing"), "{block}");
}

/// Pin: the ingress stages `wellbeing: true` as given; false, missing or a
/// non-bool class stage the fact without the key, and the fact is kept.
#[test]
fn the_ingress_stages_the_class_only_as_true() {
    if !shipped() {
        return;
    }
    let out = m7a_arrive(
        json!([
            m7a_fact("twinge", json!({"wellbeing": true})),
            m7a_fact("tomatoes", json!({"wellbeing": false})),
            m7a_fact("plain", json!({})),
            m7a_fact("worded", json!({"wellbeing": "yes"})),
        ]),
        false,
    );
    assert!(m7a_rejects(&out).is_empty(), "{out:?}");
    let staged = m7a_staged(&out);
    let by = |claim: &str| {
        staged
            .iter()
            .find(|f| f["claim"] == claim)
            .unwrap_or_else(|| panic!("{claim} is staged: {staged:?}"))
    };
    assert_eq!(by("twinge")["wellbeing"], json!(true), "{staged:?}");
    for claim in ["tomatoes", "plain", "worded"] {
        assert!(by(claim).get("wellbeing").is_none(), "{claim}: {staged:?}");
    }
}
