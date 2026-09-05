//! GH #470 — a member grown by `grow_level` can export its own memory.
//!
//! The builder's fast lane renders the transit edge set of a composition level
//! from a table (`templates/builder/recipes/config.json`, `_container_level`).
//! That table was written against a `member` that did not yet have a memory
//! export, and it stayed put while the level grew four lanes: `in_export` down,
//! and `close_report`, `export_done` and `pack_ack` back up. Thirteen edges
//! where the level declares eighteen.
//!
//! Nothing was red, and the reason is worth naming: the table is pinned byte
//! for byte against `examples/organism/grow-*.json`
//! (`gh466_grow_level_renders_the_level.rs`), and the examples carry the same
//! stale set — so the pin compared the table with a copy of itself.
//!
//! This file is the drift lock the pin could not be (development rules § 2d).
//! The second generator of that same edge set is a shipped tool,
//! `examples/memory-import/build_import.py`, whose `edges()` writes the set out
//! by hand for one member — and writes it correctly. It is read here by
//! CALLING it, so the two truths in this tree are compared element for element
//! rather than described beside each other.
//!
//! `in_import` is the one lane both agree to leave out, and that is a decision
//! rather than an omission: an import addresses the member's own path, so a
//! container edge for it could never deliver anything. The script says so at
//! its own call site; this test holds it to it.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::path::PathBuf;
use std::process::Command;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

/// The `add_edges` of one rendered level, as the shipped script produces them.
///
/// The FIRST manifest, because since GH #543 a member wish renders two — the
/// level, and then the screen and the app that member always gets. This file is
/// about the level's own export lanes, which are in the first one.
fn rendered_edges(params: Value) -> Vec<Value> {
    let out = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe", "member_index": "0"},
                       "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
    .into_iter()
    .find(|m| m["header"]["operation"] == json!("recipe"))
    .expect("a first manifest");
    out["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("no rendered edges: {out}"))
        .clone()
}

fn member_level() -> Vec<Value> {
    rendered_edges(json!({"scope": "/os/orgs/acme", "level": "member",
                          "name": "alex", "template": "a-template@1.0.0"}))
}

/// The lane each edge carries, read off its guard. One lane per edge at a
/// container level — no folding here, unlike the screen and the app.
///
/// It reads `hop.route` by name rather than taking the last comparison in the
/// string: since GH #478 a down-edge carries a second one, the member's own
/// address (`context.member == '<name>'`), and a lane reader that grabbed the
/// tail would report every door as the member.
fn lane(edge: &Value) -> String {
    let c = edge["condition"].as_str().unwrap_or_default();
    c.split_once("hop.route == '")
        .and_then(|(_, tail)| tail.split_once('\''))
        .map(|(lane, _)| lane.to_string())
        .unwrap_or_default()
}

/// Since GH #503 a level declares itself AT its container, so the container is
/// `.` — the declaration's own scope root — and the child is named bare. The
/// absolute edge is the one it always was; the spelling is one storey shorter.
fn down(edges: &[Value], name: &str) -> Vec<String> {
    let to = format!("./{name}");
    edges
        .iter()
        .filter(|e| e["from"] == json!(".") && e["to"] == json!(to))
        .map(lane)
        .collect()
}

fn up(edges: &[Value], name: &str) -> Vec<String> {
    let from = format!("./{name}");
    edges
        .iter()
        .filter(|e| e["from"] == json!(from) && e["to"] == json!("."))
        .map(lane)
        .collect()
}

#[test]
fn a_member_grown_from_the_table_can_be_asked_for_its_memory() {
    // The positive receipt, and the reason this issue exists: the demand that a
    // member hand out everything it remembers enters the level on `in_export`.
    // Without the edge it stops at the container — the org above carries the
    // lane that far and no further — and the failure looks exactly like a
    // member that has nothing to say.
    let edges = member_level();
    let doors = down(&edges, "alex");
    assert!(
        doors.iter().any(|l| l == "in_export"),
        "a member grown by grow_level has no in_export door, so it cannot be \
         asked for its memory: the doors are {doors:?}"
    );
    // And the two receipts that fall into the same hole: the close pass's and
    // the identity push's. Both are emitted by something INSIDE the member and
    // consumed by nothing inside it, so an absent edge is a message that dies
    // at its own boundary.
    let exits = up(&edges, "alex");
    for receipt in ["close_report", "export_done", "pack_ack"] {
        assert!(
            exits.iter().any(|l| l == receipt),
            "a member grown by grow_level cannot emit {receipt}: the exits are \
             {exits:?}"
        );
    }
}

#[test]
fn the_recipe_and_the_import_tool_render_the_same_member_edges() {
    // Two generators of one edge set live in this tree. This is the only place
    // they meet. `edges()` is CALLED rather than transcribed — a copy of it
    // here would be a third truth, which is the defect one level up.
    let script = repo("examples/memory-import/build_import.py");
    if !script.is_file() {
        return; // R2b: a tree without the tool cannot make this assertion
    }
    let program = concat!(
        "import importlib.util, json, sys\n",
        "spec = importlib.util.spec_from_file_location('bi', sys.argv[1])\n",
        "m = importlib.util.module_from_spec(spec)\n",
        "spec.loader.exec_module(m)\n",
        "json.dump(m.edges('alex'), sys.stdout)\n",
    );
    let Ok(out) = Command::new("python3")
        .arg("-c")
        .arg(program)
        .arg(&script)
        .output()
    else {
        return; // R2b: no python3, no verdict — and no false green either
    };
    assert!(
        out.status.success(),
        "build_import.edges() did not run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let want: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("edges() returns json");
    let want = want.as_array().expect("a list of edges").clone();
    assert_eq!(
        want.len(),
        20,
        "the import tool stopped writing twenty edges — re-read it before \
         trusting this comparison"
    );
    assert_eq!(
        member_level(),
        want,
        "the fast lane and examples/memory-import/build_import.py disagree about \
         the edges one member costs. They are two generators of one set; when \
         they part, one of them is quietly growing a member that cannot do \
         something the level promises"
    );
}

#[test]
fn the_two_container_levels_are_still_the_same_shape_one_level_apart() {
    // `_container_level` renders `org` and `member` from ONE table. That is a
    // claim about the two contracts, not a convenience: org@1.4.0 and
    // member@1.5.0 declare the same seven accepts and the same eleven emits,
    // and each one's parent carries every one of those lanes into the container
    // the child is grown into. The day they diverge, the renderers split — so
    // the claim is asserted rather than assumed.
    let org = rendered_edges(json!({"scope": "/os", "level": "org",
                                    "name": "acme", "template": "a-template@1.0.0"}));
    let member = member_level();
    assert_eq!(
        (down(&org, "acme"), up(&org, "acme"), org.len()),
        (down(&member, "alex"), up(&member, "alex"), member.len()),
        "org and member no longer render the same lanes — either a contract \
         moved and the table did not follow, or the two levels have parted and \
         `_container_level` owes them one renderer each"
    );
    // `in_import` is the lane both levels accept and neither wires, and it is
    // deliberate: an import addresses the member's own path, so an edge from
    // the container could never deliver one. A table that grew it by accident
    // would draw an edge nothing can ever use.
    assert!(
        !down(&member, "alex").iter().any(|l| l == "in_import"),
        "in_import got an edge: the lane is addressed at the level's own path \
         (examples/memory-import/build_import.py, edges()), so a container edge \
         for it is one nothing can ever deliver to"
    );
}
