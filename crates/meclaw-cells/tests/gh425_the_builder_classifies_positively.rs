//! GH #425 — the fast lane is chosen POSITIVELY, from a named recipe whose
//! parameters validate, and never by exclusion. "Anything that is not X" is the
//! shape that reads an error reply as a fresh request
//! (`workshop/cookbook/reply-to-fallback-loops.md`).
//!
//! The switch calls no model. The chat model that is already in the round fills
//! the tool arguments; if it named a recipe and the parameters are complete, the
//! class is DECIDED — R6's "vordefinierte parametrisierte Rezepte ohne
//! Modellaufruf" is one inference fewer, not one guess more.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);

fn run_classify(args: Value) -> Value {
    emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                          "text": args.to_string()}],
        }),
    )
}

#[test]
fn a_named_recipe_with_valid_params_takes_the_fast_lane() {
    let out = run_classify(json!({
        "request": "hang the notes edge on the archive instead",
        "recipe": "rewire_edge",
        "params": {"scope": "/os", "from": "./a", "to": "./b", "old_to": "./c"}
    }));
    assert_eq!(out["header"]["route"], json!("recipe"));
    assert_eq!(out["header"]["operation"], json!("classify"));
}

#[test]
fn a_request_without_a_recipe_takes_the_design_lane() {
    let out = run_classify(json!({"request": "build me a pipeline that summarises the day"}));
    assert_eq!(out["header"]["route"], json!("design"));
    assert_eq!(out["header"]["operation"], json!("classify"));
}

#[test]
fn a_named_recipe_with_missing_params_is_refused_by_name_not_downgraded() {
    // The important one. A half-filled recipe must NOT fall into the model lane:
    // a typo in one argument would otherwise silently buy an inference and
    // answer a different question than the one that was asked.
    let out = run_classify(json!({
        "request": "rewire", "recipe": "rewire_edge", "params": {"scope": "/os"}
    }));
    assert_eq!(out["header"]["route"], json!("error"));
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete")
    );
}

#[test]
fn a_recipe_nobody_ships_is_refused_and_names_the_ones_that_exist() {
    let out = run_classify(json!({"request": "do it", "recipe": "teleport_node"}));
    assert_eq!(out["header"]["error_code"], json!("recipe_unknown"));
    let text = out["messages"][0]["text"].as_str().expect("a turn");
    assert!(
        text.contains("rewire_edge"),
        "a refusal that does not say what IS available makes the caller guess: {text}"
    );
}

#[test]
fn a_tool_call_with_no_request_text_is_refused_rather_than_composed() {
    let out = run_classify(json!({"recipe": null}));
    assert_eq!(out["header"]["error_code"], json!("request_missing"));
}
