//! GH #466 — the recipe list a MODEL is offered and the recipe tables the
//! BUILDER renders from are one list, held together by a mechanism.
//!
//! The names live in three places, and none of them is derived from another:
//!
//!   * `templates/tools/schemas/config.json` — the `enum` of
//!     `build_topology.parameters.properties.recipe`, the only machine-readable
//!     copy a model ever sees;
//!   * `templates/builder/classify/config.json` — `RECIPES`, the switch that
//!     decides whether a NAMED recipe is rendered or refused before an
//!     inference is bought;
//!   * `templates/builder/recipes/config.json` — `RENDER`, the renderers.
//!
//! `grow_level` landed in the last two and, for one commit, not in the first:
//! the fast lane could render a level nobody could ask for, because the
//! declaration a model reads still named three recipes. That is the § 2d defect
//! exactly — prose (here: a machine-readable declaration) outliving its
//! mechanism — and this file is its lock.
//!
//! Both halves in one test, as `docs/development-rules.md` § 2d requires. The
//! lists are not repeated here: each of the three is read out of the SHIPPED
//! script by ASKING it, the way the substrate asks it — the schemas cell for
//! its declaration, the switch for the list it refuses an unknown name against,
//! the renderer for whether it has one at all. A hardcoded list in this file
//! would be a fourth copy and would pin nothing.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};
use std::collections::BTreeSet;

const SCHEMAS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/schemas/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);
const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

/// The `build_topology` declaration, as the tools hive hands it out.
fn build_topology_declaration() -> Value {
    let answer = emit_one(
        &shipped_script(SCHEMAS),
        &json!({
            "target": "/main/tools/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": ["build_topology"],
            "messages": [],
        }),
    );
    let schema = answer["schemas"][0].clone();
    assert_eq!(
        schema["name"],
        json!("build_topology"),
        "the tools hive no longer declares `build_topology` — the enum this file \
         locks has nowhere to live: {answer}"
    );
    schema
}

fn recipe_property() -> Value {
    build_topology_declaration()["parameters"]["properties"]["recipe"].clone()
}

/// The recipe names a model is OFFERED.
fn declared() -> BTreeSet<String> {
    let prop = recipe_property();
    let names: BTreeSet<String> = prop["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("`recipe` declares no enum: {prop}"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("an enum entry is not a string: {prop}"))
                .to_string()
        })
        .collect();
    assert!(
        !names.is_empty(),
        "the enum is empty. An empty result and a broken read must never look \
         alike: {prop}"
    );
    names
}

/// The recipe names the SWITCH knows — read off its own refusal, which hands
/// back `known` precisely so a caller never has to guess.
fn known_to_the_switch() -> BTreeSet<String> {
    let out = emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": json!({"request": "…",
                                         "recipe": "no_such_recipe_exists"}).to_string()}],
        }),
    );
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_unknown"),
        "the switch stopped refusing an invented recipe by name, so this file can \
         no longer read its table: {out}"
    );
    let payload: Value = meclaw_core::serde_json::from_str(
        out["messages"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("the refusal carries no payload: {out}")),
    )
    .expect("the refusal payload is json");
    let known: BTreeSet<String> = payload["known"]
        .as_array()
        .unwrap_or_else(|| panic!("the refusal names no `known` list: {payload}"))
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !known.is_empty(),
        "the switch knows no recipe at all: {payload}"
    );
    known
}

/// Whether the RENDERER has a renderer under that name. Asked with empty
/// params on purpose: every other verdict (`recipe_params_incomplete`,
/// `level_unknown`) proves the name was dispatched, and only `recipe_unknown`
/// says there is nothing behind it.
fn renderer_verdict(recipe: &str) -> String {
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": recipe, "request": "…",
                                         "params": {}}).to_string()}],
        }),
    );
    out["header"]["error_code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ═════════════════════════════════════════════════════════ the drift lock

/// § 2d, the mechanism half: the three lists are one list.
#[test]
fn the_declared_recipes_are_exactly_the_ones_the_builder_knows() {
    let offered = declared();
    let known = known_to_the_switch();

    let undeclared: Vec<&String> = known.difference(&offered).collect();
    assert!(
        undeclared.is_empty(),
        "the builder renders {undeclared:?} and no model is ever told the names \
         exist. A recipe the declaration omits is a fast lane nobody can enter — \
         every request for it buys an inference instead, which is the exact state \
         `grow_level` shipped in for one commit."
    );
    let unbuilt: Vec<&String> = offered.difference(&known).collect();
    assert!(
        unbuilt.is_empty(),
        "the declaration offers {unbuilt:?} and the switch refuses the name with \
         `recipe_unknown`. Offering a model a value the enum says is legal and the \
         cell behind it says is not turns a build order into a refusal the caller \
         cannot have foreseen."
    );

    for recipe in &offered {
        assert_ne!(
            renderer_verdict(recipe),
            "recipe_unknown",
            "`{recipe}` passes the switch and reaches a renderer that does not \
             exist. The switch and the renderer hold two tables and only one of \
             them was extended."
        );
    }
    assert_eq!(
        renderer_verdict("no_such_recipe_exists"),
        "recipe_unknown",
        "the renderer answers an invented name as though it had one — the probe \
         above proves nothing then"
    );
    assert!(
        !offered.contains("no_such_recipe_exists"),
        "the enum contains the name this file invented to probe with"
    );
}

/// § 2d, the prose half: the description a model reads NAMES every recipe the
/// enum offers. Derived from the enum rather than repeated, so adding a recipe
/// without saying what it does is red — a bare name in an enum is a value a
/// model has to guess the meaning of.
#[test]
fn the_declaration_says_what_each_recipe_does() {
    let prop = recipe_property();
    let text = prop["description"]
        .as_str()
        .unwrap_or_else(|| panic!("`recipe` carries no description: {prop}"))
        .to_string();
    for recipe in declared() {
        assert!(
            text.contains(&format!("`{recipe}`")),
            "the enum offers `{recipe}` and the description never mentions it. A \
             name with no sentence behind it is a value the model picks by its \
             spelling: {text:?}"
        );
    }
    assert!(
        !text.contains("one of the three"),
        "the description still counts the recipes it lists. § 2d: a number in \
         template prose is either derived inside a test or it stands exactly once \
         — and this one had already outlived its list: {text:?}"
    );
}
