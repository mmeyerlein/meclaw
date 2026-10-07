//! `examples/organism/grow-assistant.json` as the `assistants` container reads it.
//!
//! Since GH #1061 the shipped generation is the CREDENTIALLED one: ONE declaration
//! that stands at the MEMBER (an edge lives in the graph of the lowest common
//! ancestor of its two ends, and a brain and the member's broker `./access` share
//! only that one), names its node through the container (`assistants/scribe`),
//! carries the grant ids in `override_params`, the credential v-lanes behind the
//! level's own transit edges, and the grants as `seed_rows`.
//!
//! The trees these tests build by hand stand the generation in an `assistants`
//! hive of their own and give their brains a key directly, with no broker beside
//! them. What they need from the example is the LEVEL: its transit edges as the
//! container draws them, and the node with the parameters the wish set. That is
//! exactly what the recipe renders for the same wish WITHOUT the credential block
//! (`gh466_grow_level_renders_the_level.rs` pins both), so the translation below
//! is an address change and two removals, never a second copy of the example.
#![allow(dead_code)]

use meclaw_core::serde_json::{Map, Value, json};

/// The container the level is instantiated into, as the member names it.
const CONTAINER: &str = "./assistants";

/// `./assistants` -> `.`, `./assistants/x` -> `./x`, anything else -> `None`
/// (an endpoint outside the container: a credential v-lane to `./access`).
fn from_the_container(endpoint: &str) -> Option<String> {
    if endpoint == CONTAINER {
        return Some(".".to_string());
    }
    endpoint
        .strip_prefix(CONTAINER)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(|rest| format!("./{rest}"))
}

/// Whether `decl` is the member-standing form (its node is named through the
/// container). A declaration already at the container passes through unchanged.
pub fn stands_at_the_member(decl: &Value) -> bool {
    decl["diff"]["add_nodes"][0]["name"]
        .as_str()
        .is_some_and(|n| n.starts_with("assistants/"))
}

/// The level's transit edges, the node with the wish's own parameters (no
/// `api_key: ""`, no `credential_grant_id`), and nothing of the credential road:
/// no v-lane to `./access`, no `seed_rows`.
pub fn at_the_container(decl: &Value) -> Value {
    if !stands_at_the_member(decl) {
        return decl.clone();
    }
    let scope = decl["scope"].as_str().expect("a scope");
    let node = &decl["diff"]["add_nodes"][0];
    let name = node["name"]
        .as_str()
        .and_then(|n| n.strip_prefix("assistants/"))
        .expect("a node named through the container");

    let mut out_node = Map::new();
    out_node.insert("name".into(), json!(name));
    out_node.insert("template".into(), node["template"].clone());
    let mut overrides = Map::new();
    if let Some(over) = node["override_params"].as_object() {
        for (cell, params) in over {
            let mut kept = params.as_object().cloned().unwrap_or_default();
            kept.remove("api_key");
            kept.remove("credential_grant_id");
            if !kept.is_empty() {
                overrides.insert(cell.clone(), Value::Object(kept));
            }
        }
    }
    if !overrides.is_empty() {
        out_node.insert("override_params".into(), Value::Object(overrides));
    }

    let edges: Vec<Value> = decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .filter_map(|e| {
            let from = from_the_container(e["from"].as_str()?)?;
            let to = from_the_container(e["to"].as_str()?)?;
            let mut e = e.clone();
            e["from"] = json!(from);
            e["to"] = json!(to);
            Some(e)
        })
        .collect();

    let mut out = Map::new();
    out.insert("scope".into(), json!(format!("{scope}/assistants")));
    if decl.get("ctx").is_some_and(Value::is_object) {
        out.insert("ctx".into(), decl["ctx"].clone());
    }
    out.insert(
        "diff".into(),
        json!({"add_nodes": [Value::Object(out_node)], "add_edges": edges}),
    );
    Value::Object(out)
}

/// The credential v-lanes alone: every edge of the declaration with an end
/// outside the container, in order.
pub fn credential_edges(decl: &Value) -> Vec<Value> {
    decl["diff"]["add_edges"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|e| {
                    e["from"].as_str().and_then(from_the_container).is_none()
                        || e["to"].as_str().and_then(from_the_container).is_none()
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
