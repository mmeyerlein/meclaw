//! GH #803 — a lane the member stamps at a CONTAINER has to be carried one more
//! hop, or the child never hears it.
//!
//! `member@1.9.0` wired two lanes across the level for a channel whose model
//! answers on its own timeline: a `delegation` goes from `./channels` straight
//! to `./assistants` as `in_delegation`, and the three advice sections come back
//! down to `./channels` as `in_advise`. Both edges end at a CONTAINER, and a
//! container is not a pass-through in this substrate: `Edge.to` is a static
//! path, so every lane a child receives costs one more edge named for that child
//! (the address rule, GH #454/#478).
//!
//! Measured on a built colony before this file existed:
//! `<member>/assistants -> <member>/assistants/<gen>` stood for `in_turn`,
//! `in_tool`, `in_menu`, `in_build_result`, `in_export`, `in_import` and
//! `mutation_committed` — and not for `in_delegation`. Every delegation died at
//! the container as `hive_no_route`, and the one manifest that wired a duplex
//! channel had to draw the missing hop by hand.
//!
//! What this file pins is the SEAM rather than a word: the lane names are read
//! out of `templates/member/config.json` — the side that stamps them — and the
//! rendered level has to carry a door for exactly those. A lane renamed in the
//! member and forgotten in the recipe is red here, which is the failure mode
//! `gh466_grow_level_renders_the_level.rs` cannot see: it compares the table
//! against examples generated out of the same table.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

/// The one declaration a `grow_level` wish renders.
fn grow(params: Value) -> Value {
    let wish = json!({"recipe": "grow_level", "request": "…", "params": params});
    let all = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": wish.to_string()}],
        }),
    );
    let out = all
        .into_iter()
        .find(|m| m["header"]["operation"] == json!("recipe"))
        .expect("a rendered manifest");
    out["manifest"]
        .as_array()
        .and_then(|d| d.first())
        .cloned()
        .unwrap_or_else(|| panic!("no manifest: {out}"))
}

/// The member's own edge table.
fn member_edges() -> Vec<Value> {
    let raw = std::fs::read_to_string(repo("templates/member/config.json"))
        .expect("the member template ships");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("the member config is json");
    v["params"]["graph"]["edges"]
        .as_array()
        .expect("the member draws edges")
        .clone()
}

/// Every lane the member RE-STAMPS onto `container`, whatever they are called
/// today. A `set_hop.route` of `in_*` on an edge INTO a container is the
/// member saying "from here this is addressed inward" — and an address that
/// stops at the container is no address at all. Read off the tree, never
/// written down here (§ 2d).
fn stamped_onto(container: &str) -> Vec<String> {
    let mut found: Vec<String> = member_edges()
        .into_iter()
        .filter(|e| e["to"] == json!(container))
        .filter_map(|e| {
            e["modifier"]["set_hop"]["route"]
                .as_str()
                .map(|s| s.trim_matches('\'').to_string())
        })
        .filter(|lane| lane.starts_with("in_"))
        .collect();
    found.sort();
    found.dedup();
    assert!(
        !found.is_empty(),
        "the member stamps no inbound lane onto {container} any more — this \
         file is about the hop AFTER that stamp, so the seam moved"
    );
    found
}

/// Every lane a rendered level opens a door for, anywhere under the child. It
/// is the SUBTREE and not the child's own path, because two of these lanes are
/// v-lanes that end at a brain rim two storeys down (GH #562) — what is being
/// asserted is that the message arrives, not where inside it lands.
fn doors_into(decl: &Value, child: &str) -> Vec<String> {
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .filter(|e| {
            e["from"] == json!(".")
                && e["to"]
                    .as_str()
                    .is_some_and(|t| t == child || t.starts_with(&format!("{child}/")))
        })
        .filter_map(|e| e["condition"].as_str())
        .filter_map(|c| {
            let at = c.find("hop.route == '")? + "hop.route == '".len();
            let rest = &c[at..];
            Some(rest[..rest.find('\'')?].to_string())
        })
        .collect()
}

/// What the level renders, and what the member stamps at its container, held
/// to one set.
fn assert_every_stamped_lane_arrives(container: &str, params: Value, child: &str) {
    let decl = grow(params);
    let doors = doors_into(&decl, child);
    for lane in stamped_onto(container) {
        assert!(
            doors.contains(&lane),
            "a child grown from the table has no door for `{lane}` — the member \
             stamps it onto `{container}` and the container carries only \
             {doors:?}, so every message on that lane dies one hop short of the \
             child it is addressed to"
        );
    }
}

#[test]
fn a_grown_generation_receives_the_delegation_the_member_stamps() {
    assert_every_stamped_lane_arrives(
        "./assistants",
        json!({"scope": "/os/orgs/acme/members/alex", "level": "assistant",
               "name": "scribe", "template": "assistant@2.8.0",
               "ctx": {"model": "m", "model_fast": "m", "model_surface": "m"}}),
        "./scribe",
    );
}

#[test]
fn a_grown_channel_receives_the_advice_the_member_stamps() {
    assert_every_stamped_lane_arrives(
        "./channels",
        json!({"scope": "/os/orgs/acme/members/alex", "level": "channel",
               "name": "telegram", "template": "telegram-connector@2.0.1",
               "assistant": "scribe", "ctx": {"member_person": "alex"}}),
        "./telegram",
    );
}

#[test]
fn every_door_of_a_grown_child_names_the_child_it_is_for() {
    // The address rule (GH #454/#478) read once more, and the reason the two
    // hops above could not simply be widened: a container may hold two
    // children, and a door under one condition delivers to BOTH. Every door
    // this level renders therefore carries a guard beyond the lane name — and
    // the guard has to read the KEY the parent stamps the address into.
    // Matching the bare name would pass a door that spelled the key wrong, and
    // a misspelt key is `has(...)` false on every hop: the door is shut, not
    // widened.
    for (params, child, key) in [
        (
            json!({"scope": "/os/orgs/acme/members/alex", "level": "assistant",
                   "name": "scribe", "template": "assistant@2.8.0",
                   "ctx": {"model": "m", "model_fast": "m", "model_surface": "m"}}),
            "./scribe",
            "context.assistant",
        ),
        (
            json!({"scope": "/os/orgs/acme/members/alex", "level": "channel",
                   "name": "telegram", "template": "telegram-connector@2.0.1",
                   "assistant": "scribe", "ctx": {"member_person": "alex"}}),
            "./telegram",
            "context.channel_node",
        ),
    ] {
        let decl = grow(params);
        for edge in decl["diff"]["add_edges"].as_array().expect("add_edges") {
            if edge["to"] != json!(child) || edge["from"] != json!(".") {
                continue;
            }
            let cond = edge["condition"].as_str().unwrap_or_default();
            let name = child.trim_start_matches("./");
            // Two spellings of one address, and no third: the context key the
            // parent stamps, or — for the one turn that arrives with no
            // `context.assistant` on it, the v-lane's way back — the owner
            // path, where the name stands between separators.
            let by_key = cond.contains(&format!("{key} == '{name}'"));
            let by_owner =
                cond.contains("hop.owner.contains('") && cond.contains(&format!("/{name}/'"));
            assert!(
                by_key || by_owner,
                "a door into {child} is addressed by lane alone ({cond:?}) — it \
                 names neither `{key} == '{name}'` nor an owner path holding \
                 `{name}`, so a second child of the same container would be \
                 handed every message on it"
            );
        }
    }
}
