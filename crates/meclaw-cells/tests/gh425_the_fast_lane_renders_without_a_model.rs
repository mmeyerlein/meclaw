//! GH #425 — R6's fast lane: predefined parameterised recipes, rendered without
//! a model and without a network, sub-second.
//!
//! What is under test is not speed for its own sake. It is that the trivial
//! class of structural wish — rewire an edge, hang a drain, grow a cell and
//! wire it in — never reaches an inference at all, and that the ORDER of the
//! declarations it produces is semantics rather than taste: a manifest rolls
//! forward and stops at the first refusal, with NO ROLLBACK (R5).

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, run_shipped_script, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn run_recipes(payload: Value) -> Value {
    emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
}

/// The digest, computed INDEPENDENTLY of the script under test: the same
/// canonical bytes, hashed by a python that never saw the shipped source. A
/// helper that drifted would agree with itself; this does not.
fn sha256_of_canonical(manifest: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    let out = run_shipped_script(program, &manifest.to_string());
    assert!(out.status.success(), "digest helper failed");
    String::from_utf8(out.stdout).expect("hex")
}

#[test]
fn rewire_edge_removes_the_old_edge_before_it_draws_the_new_one() {
    let out = run_recipes(json!({"recipe": "rewire_edge", "request": "…",
        "params": {"scope": "/os", "from": "./a", "to": "./b", "old_to": "./c",
                   "condition": "has(hop.route) && hop.route == 'note'"}}));
    let decls = out["manifest"]
        .as_array()
        .expect("the manifest is an array");
    assert_eq!(
        decls.len(),
        2,
        "remove first, add second — a manifest has no rollback, so an add that \
         lands before the remove leaves BOTH edges live if the remove is refused"
    );
    assert!(decls[0]["diff"]["remove_edges"].is_array());
    assert!(decls[1]["diff"]["add_edges"].is_array());
    assert_eq!(
        decls[0]["diff"]["remove_edges"][0]["match"]["to"],
        json!("./c"),
        "remove_edges takes a `match` pattern, not a bare edge (docs/rewiring.md)"
    );
    assert_eq!(
        decls[1]["diff"]["add_edges"][0]["condition"],
        json!("has(hop.route) && hop.route == 'note'")
    );
    assert_eq!(out["header"]["manifest_class"], json!("fast"));
    assert_eq!(out["header"]["declaration_count"], json!(2));
}

#[test]
fn the_digest_is_over_the_canonical_bytes_of_the_manifest() {
    let out = run_recipes(json!({"recipe": "attach_drain", "request": "…",
        "params": {"scope": "/os", "from": "./x", "to": "./y", "route": "error"}}));
    let want = sha256_of_canonical(&out["manifest"]);
    assert_eq!(out["header"]["manifest_sha256"], json!(want));
}

#[test]
fn a_grown_cell_is_wired_in_by_the_same_recipe_that_grew_it() {
    let out = run_recipes(json!({"recipe": "add_node", "request": "…",
        "params": {"scope": "/os", "name": "digest", "template": "summarizer",
                   "target": "./notes", "route": "tick"}}));
    let decls = out["manifest"].as_array().expect("array");
    assert_eq!(
        decls.len(),
        2,
        "a node nobody can reach is not a growth, it is a directory"
    );
    assert_eq!(
        decls[0]["diff"]["add_nodes"][0]["template"],
        json!("summarizer")
    );
    assert_eq!(decls[1]["diff"]["add_edges"][0]["from"], json!("./digest"));
}

#[test]
fn the_fast_lane_answers_in_well_under_a_second() {
    let t = std::time::Instant::now();
    let _ = run_recipes(json!({"recipe": "attach_drain", "request": "…",
        "params": {"scope": "/os", "from": "./x", "to": "./y", "route": "error"}}));
    let ms = t.elapsed().as_millis();
    // The bound is 900 ms and not 50: a bare interpreter start measures 16–17 ms
    // (meta-plan § Messpunkte) and this runs under cargo-parallel load. The
    // SEMANTIC discriminator is "no model, no network" — the clock is the coarse
    // guard that catches an HTTP call somebody built in by accident.
    assert!(
        ms < 900,
        "fast lane took {ms} ms — R6 says sub-second, and this is a python start \
         plus string work, so anything near the bound is a real regression"
    );
}

#[test]
fn a_recipe_this_cell_cannot_render_is_named_rather_than_guessed() {
    // Unreachable behind `classify`, and here anyway: a cell knows no topology
    // and must not build on who stands in front of it.
    let out = run_recipes(json!({"recipe": "teleport", "params": {}}));
    assert_eq!(out["header"]["error_code"], json!("recipe_unknown"));
    assert!(
        out.get("manifest").is_none(),
        "a refusal ships no manifest — an empty one is a failure wearing the face \
         of an honest answer (GH #308)"
    );
}
