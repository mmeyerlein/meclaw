//! GH #425 — the composer's answer is READ, not forwarded.
//!
//! Four checks, and each one stops something that would otherwise be discovered
//! halfway through an application that has **no rollback**:
//!
//! | check | prevents |
//! |---|---|
//! | first balanced `{…}` run instead of `json.loads` | a model writes prose around its JSON however hard you ask it not to |
//! | `declarations` is a non-empty LIST of objects | a single object that looks like a list becomes a manifest of length 1 |
//! | every `diff` key is one that exists | an invented operation name is refused at position k, after k−1 applied |
//! | no endpoint begins with `/colony` | the draft proposes what the validator refuses anyway — and does it late |
//!
//! The last one is **not** the guardrail. The guardrail is the missing edge
//! (`gh425_the_builder_cannot_reach_the_mutation_door`). This is courtesy
//! towards the ordering.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const NORMALISE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/normalise/config.json"
);

const COMPOSE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/compose/config.json"
);

fn run_normalise(answer: &str) -> Value {
    emit_one(
        &shipped_script(NORMALISE),
        &json!({
            "target": "/os/builder/normalise",
            "header": {"hop": {"finish_reason": "stop"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "text", "id": "", "text": answer}],
        }),
    )
}

#[test]
fn prose_around_the_json_is_survived() {
    let out = run_normalise(
        "Sure! Here you go:\n```json\n{\"declarations\":[\
         {\"scope\":\"/os\",\"ctx\":{},\"diff\":{\"add_edges\":[{\"from\":\"./a\",\"to\":\"./b\"}]}}]}\n\
         ```\nHope that helps!",
    );
    assert_eq!(out["header"]["manifest_class"], json!("composed"));
    assert_eq!(out["manifest"].as_array().expect("array").len(), 1);
    assert_eq!(out["header"]["declaration_count"], json!(1));
}

#[test]
fn an_answer_with_no_object_is_refused_and_not_shipped_empty() {
    let out = run_normalise("I would rather not.");
    assert_eq!(out["header"]["error_code"], json!("no_manifest_in_answer"));
    assert!(
        out.get("manifest").is_none(),
        "an empty manifest is a failure wearing the face of an honest answer (GH #308)"
    );
}

#[test]
fn a_single_declaration_that_forgot_the_list_is_refused_rather_than_wrapped() {
    let out = run_normalise("{\"declarations\": {\"scope\":\"/os\",\"diff\":{}}}");
    assert_eq!(
        out["header"]["error_code"],
        json!("declarations_not_a_list")
    );
}

#[test]
fn an_invented_operation_name_is_refused_here_and_not_at_position_k() {
    let out = run_normalise(
        "{\"declarations\":[{\"scope\":\"/os\",\"diff\":{\"update_params\":[{\"name\":\"a\"}]}}]}",
    );
    assert_eq!(out["header"]["error_code"], json!("declaration_malformed"));
    let reason = out["messages"][0]["text"].as_str().expect("a turn");
    assert!(
        reason.contains("update_params") && reason.contains("move_nodes"),
        "the refusal names the invented key AND the ones that exist: {reason}"
    );
}

#[test]
fn a_declaration_naming_the_control_plane_is_refused_before_anything_is_applied() {
    let out = run_normalise(
        "{\"declarations\":[{\"scope\":\"/os\",\"ctx\":{},\"diff\":\
         {\"add_edges\":[{\"from\":\"./a\",\"to\":\"/colony/mutations\"}]}}]}",
    );
    assert_eq!(
        out["header"]["error_code"],
        json!("declaration_targets_control_plane")
    );
}

#[test]
fn the_control_plane_is_also_looked_for_inside_a_match_pattern() {
    // `remove_edges` and `remove_nodes` carry their endpoints one level down,
    // under `match`. A check that only read the top level would wave through
    // exactly the declarations that name an existing privileged edge.
    let out = run_normalise(
        "{\"declarations\":[{\"scope\":\"/\",\"ctx\":{},\"diff\":\
         {\"remove_edges\":[{\"match\":{\"from\":\"./os\",\"to\":\"/colony/mutations\"}}]}}]}",
    );
    assert_eq!(
        out["header"]["error_code"],
        json!("declaration_targets_control_plane")
    );
}

#[test]
fn a_manifest_past_the_cap_is_refused_by_name() {
    let one = "{\"scope\":\"/os\",\"diff\":{\"add_edges\":[{\"from\":\"./a\",\"to\":\"./b\"}]}}";
    let many: Vec<&str> = std::iter::repeat_n(one, 65).collect();
    let out = run_normalise(&format!("{{\"declarations\":[{}]}}", many.join(",")));
    assert_eq!(out["header"]["error_code"], json!("manifest_too_large"));
}

#[test]
fn the_composer_speaks_the_same_wire_as_every_other_shipped_llm_cell() {
    let raw = std::fs::read_to_string(COMPOSE).expect("compose config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    assert_eq!(cfg["cell"]["type"], json!("llm"));
    assert_eq!(
        cfg["params"]["provider"],
        json!("openai"),
        "`provider` is the WIRE PROTOCOL, not the vendor (#387, ruled 2026-08-25)"
    );
    assert_eq!(cfg["params"]["temperature"], json!(0));
    assert_eq!(cfg["params"]["system_order"], json!(["instructions"]));
    assert!(
        cfg["params"]["max_tokens"].as_u64().expect("max_tokens") >= 2048,
        "a manifest is longer than a spec — 512 truncates it into a refusal"
    );
}
