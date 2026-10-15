//! GH #916 -- two more words an app declaration may say: `listens: ["close"]`
//! and `pins: "./<cell>"`.
//!
//! `close` draws an observer of the close batch of a session (`write` off
//! `./assistants`, the batch the member's close pass takes) into `./apps`,
//! restamped `in_close` with the close pass's context, and the binding into
//! the app carries `in_close`. `pins` draws ONE edge from the declared cell
//! onto the generation, `pin` restamped `in_pin` -- the generation hands it to
//! the curator of each brain. An app that declares neither gets neither.
//! GH #949 (review I-1): the pin edge stamps the MEMBER's round
//! (`["agent:<generation>","member:<ctx.member_person>"]`) over whatever round
//! the app's message carries, so a wish with `pins` names the person.
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted. The colony case is `gh916_an_app_hears_its_member.rs`.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::collections::BTreeSet;
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
/// The round the pin edge stamps, as a CEL literal: the generation `sam` is
/// the agent, `ctx.member_person` the person (GH #949, review I-1).
const MEMBER_ROUND: &str = r#"'["agent:sam","member:alex"]'"#;

fn golden() -> Value {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gh916_install_edges.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&raw).expect("the golden is json")
}

fn run_recipes(params: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

fn edges(params: &Value) -> Vec<Value> {
    let all = run_recipes(params);
    let first = all.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn refusal(params: &Value) -> Value {
    let all = run_recipes(params);
    let first = all.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("app_declaration_invalid"),
        "{first}"
    );
    serde_json::from_str(first["messages"][0]["text"].as_str().expect("a payload")).expect("json")
}

fn params(declaration: Value) -> Value {
    json!({"scope": MEMBER, "app": "probe-app", "template": "probe-app@1.0.0",
           "screen": "display", "generation": "sam", "ctx": {"member_person": "alex"},
           "declaration": declaration})
}

fn classify(params: Value) -> Value {
    emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": json!({"request": "install an app",
                                         "recipe": "install_app",
                                         "params": params}).to_string()}],
        }),
    )
}

fn set(edges: &[Value]) -> BTreeSet<String> {
    edges.iter().map(|e| e.to_string()).collect()
}

#[test]
fn close_and_pins_render_the_reviewed_edges() {
    let g = golden();
    let got = edges(&params(g["declaration"].clone()));
    let want: Vec<Value> = g["edges"].as_array().expect("edges").clone();
    assert_eq!(set(&got), set(&want), "got {got:#?}");
    assert_eq!(got.len(), want.len(), "no edge twice");
}

#[test]
fn close_is_the_observer_of_the_close_batch_and_pins_one_edge_from_the_cell() {
    let got = edges(&params(json!({"listens": ["close"], "pins": "./sink"})));
    let observer: Vec<&Value> = got
        .iter()
        .filter(|e| e["from"] == "./assistants" && e["to"] == "./apps")
        .collect();
    assert_eq!(observer.len(), 1, "{got:#?}");
    assert_eq!(
        observer[0]["condition"],
        "has(hop.route) && hop.route == 'write'"
    );
    assert_eq!(observer[0]["modifier"]["set_hop"]["route"], "'in_close'");
    for k in ["session_id", "audience_set", "channel"] {
        assert!(
            observer[0]["modifier"]["set_context"].get(k).is_some(),
            "{k}: the close pass's context rides along"
        );
    }
    // No guard, no default: `write` has its own regular exit at the member,
    // so the observer suppresses nothing.
    assert!(observer[0].get("default").is_none());
    let binding: Vec<&Value> = got
        .iter()
        .filter(|e| e["from"] == "./apps" && e["to"] == "./apps/probe-app")
        .collect();
    assert_eq!(binding.len(), 1);
    assert_eq!(
        binding[0]["condition"],
        "has(hop.route) && hop.route == 'in_close'"
    );
    let pin: Vec<&Value> = got
        .iter()
        .filter(|e| e["from"] == "./apps/probe-app/sink")
        .collect();
    assert_eq!(pin.len(), 1, "{got:#?}");
    assert_eq!(pin[0]["to"], "./assistants/sam");
    assert_eq!(pin[0]["condition"], "has(hop.route) && hop.route == 'pin'");
    assert_eq!(pin[0]["modifier"]["set_hop"]["route"], "'in_pin'");
    assert!(
        pin[0].get("lane").is_none(),
        "a plain edge onto the generation's rim"
    );
}

#[test]
fn an_app_without_close_or_pins_draws_neither() {
    let got = edges(&params(json!({
        "offers": [{"kind": "sidecar", "at": "./sink", "section": "probe"}],
        "listens": ["answer"]})));
    assert!(
        !got.iter().any(|e| e["condition"]
            .as_str()
            .is_some_and(|c| c.contains("'write'") || c.contains("in_close"))),
        "{got:#?}"
    );
    assert!(
        !got.iter().any(|e| e.to_string().contains("in_pin")),
        "{got:#?}"
    );
}

#[test]
fn the_vocabulary_knows_close_and_pins() {
    let p = refusal(&params(json!({"listens": ["close", "gossip"]})));
    assert_eq!(p["field"], "listens");
    assert!(
        p["known"].as_array().unwrap().contains(&json!("close")),
        "{p}"
    );
    let p = refusal(&params(json!({"listens": [], "pinz": "./sink"})));
    assert_eq!(p["field"], "declaration");
    assert!(
        p["known"].as_array().unwrap().contains(&json!("pins")),
        "{p}"
    );
}

#[test]
fn pins_names_a_cell_inside_the_app() {
    for bad in [
        json!("sink"),
        json!("./"),
        json!("./../x"),
        json!(3),
        json!(["./sink"]),
    ] {
        let p = refusal(&params(json!({"pins": bad.clone()})));
        assert_eq!(p["field"], "pins", "{bad}: {p}");
    }
}

#[test]
fn pins_needs_the_generation_at_the_switch() {
    let mut p = params(json!({"pins": "./sink"}));
    p.as_object_mut().unwrap().remove("generation");
    let out = classify(p.clone());
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{out}"
    );
    let payload: Value =
        serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
            .expect("json");
    assert_eq!(payload["missing"], json!(["generation"]), "{payload}");
    // `close` alone ends at the container: no generation needed.
    let mut q = params(json!({"listens": ["close"]}));
    q.as_object_mut().unwrap().remove("generation");
    let out = classify(q);
    assert_eq!(out["header"]["route"], json!("recipe"), "{out}");
}

/// GH #949 (review I-1) -- a pin leaves in its MEMBER's round. The round an
/// app's message carries is written by the app's own template, so the edge
/// the builder draws stamps `["agent:<generation>","member:<person>"]` over
/// it; a wish with `pins` that names no person renders nothing and is asked,
/// at the renderer and at the switch, with one question. `close` alone stamps
/// no round and needs no person. The forged round on the real edge table is
/// `gh916_an_app_hears_its_member.rs` (`a_pin_reaches_the_curator_of_every_
/// brain_once`).
///
/// Red before the fix: the pin edge carries no `set_context`.
#[test]
fn a_pin_leaves_in_the_members_round_and_the_wish_names_the_person() {
    let got = edges(&params(json!({"pins": "./sink"})));
    let pin: Vec<&Value> = got
        .iter()
        .filter(|e| e["from"] == "./apps/probe-app/sink")
        .collect();
    assert_eq!(pin.len(), 1, "{got:#?}");
    assert_eq!(
        pin[0]["modifier"]["set_context"]["audience_set"],
        json!(MEMBER_ROUND),
        "the pin edge stamps the member's round: {got:#?}"
    );

    let mut p = params(json!({"pins": "./sink"}));
    p.as_object_mut().unwrap().remove("ctx");
    let rendered = run_recipes(&p);
    let first = rendered.first().expect("an emission");
    assert_eq!(
        first["header"]["error_code"],
        json!("wish_incomplete"),
        "{first}"
    );
    assert!(first["manifest"].is_null(), "nothing is rendered: {first}");
    let asked: Value =
        serde_json::from_str(first["messages"][0]["text"].as_str().expect("a payload"))
            .expect("json");
    assert_eq!(asked["missing"], json!(["ctx.member_person"]), "{asked}");
    let out = classify(p);
    assert_eq!(
        out["header"]["error_code"],
        json!("wish_incomplete"),
        "{out}"
    );
    let switch: Value =
        serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
            .expect("json");
    assert_eq!(switch, asked, "one question at the switch and the renderer");

    let mut q = params(json!({"listens": ["close"]}));
    q.as_object_mut().unwrap().remove("ctx");
    assert!(!edges(&q).is_empty(), "`close` alone needs no person");
}
