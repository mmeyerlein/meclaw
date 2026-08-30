//! GH #455 -- the structure of the re-cut: a screen and an app, and the bytes
//! that make them.
//!
//! `canvy` fused a surface and a view. `display` is the surface half and
//! `colony-view` the view half, and what this file pins is the part that is
//! true before anything runs:
//!
//! 1. **The inventory.** A file that silently disappears makes these tests skip
//!    rather than pass (R2b), the same rule `canvy2_pipeline` follows.
//! 2. **The seal.** Both are hives, so `params.ports` is the empty list and the
//!    lanes they name are the lanes their README and `template.json` describe.
//!    The question is asked of the substrate's own boundary validator, never of
//!    a second opinion.
//! 3. **`app` is a tag the scanner already reads.** `colony-view` declares
//!    `tags: ["app"]`, and `parse_template_json` carries it into the registry
//!    row. No new field was invented for this, and this test is what says so.
//! 4. **The shipped bytes are the sources.** Every `code` cell in both hives
//!    carries its `.py` verbatim in `params.script_inline`, and the app's
//!    browser half is byte-identical in three places -- the `.js`/`.css` files,
//!    the constants in `layout.py`, and the copy inside `config.json`. There is
//!    no sync script here (there is one for `canvy`), so this test IS the
//!    splice gate.
//! 5. **The drift locks** (`docs/development-rules.md` § 2d): a countable or
//!    behavioural promise on a public template surface is asserted against the
//!    mechanism, not merely grepped.

use meclaw_colony::templates::{parse_simple_version, parse_template_json};
use meclaw_core::serde_json::Value;

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repo(rel: &str) -> std::path::PathBuf {
    core_root().join(rel)
}

/// Every file `display` is made of. The list is the guard AND the inventory.
const DISPLAY_FILES: &[&str] = &[
    "config.json",
    "template.json",
    "README.md",
    "web/config.json",
    "views/config.json",
    "compose/config.json",
    "compose/compose.py",
];

/// Every file `colony-view` is made of.
const COLONY_VIEW_FILES: &[&str] = &[
    "config.json",
    "template.json",
    "README.md",
    "refresh/config.json",
    "probe/config.json",
    "probe/probe.py",
    "layout/config.json",
    "layout/layout.py",
    "layout/colony-view.js",
    "layout/colony-view.css",
];

fn shipped(dir: &str, files: &[&str]) -> Option<std::path::PathBuf> {
    let root = repo(&format!("templates/{dir}"));
    for rel in files {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn shipped_display() -> Option<std::path::PathBuf> {
    shipped("display", DISPLAY_FILES)
}

fn shipped_colony_view() -> Option<std::path::PathBuf> {
    shipped("colony-view", COLONY_VIEW_FILES)
}

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&read(p)).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ───────────────────────────────────────────────────────── 1. the inventory

#[test]
fn both_templates_ship_their_whole_inventory_at_one_zero() {
    let Some(display) = shipped_display() else {
        return;
    };
    let Some(app) = shipped_colony_view() else {
        return;
    };

    for (root, name) in [(&display, "display"), (&app, "colony-view")] {
        let t = parse_template_json(&root.join("template.json"))
            .unwrap_or_else(|e| panic!("{name}/template.json: {e:?}"));
        assert_eq!(t.name, name, "the directory name is the template name");
        let version = t
            .version
            .unwrap_or_else(|| panic!("{name} declares no version"));
        // The exact shipped version is gated by `gh235_readme_library_table`
        // against `templates/README.md`; what is pinned here is the major, and
        // that the string is one a reference can actually name.
        let parsed = parse_simple_version(&version)
            .unwrap_or_else(|e| panic!("{name}@{version} is not a resolvable version: {e}"));
        assert_eq!(
            parsed.0, 1,
            "{name} is on major 1: a first cut, and nothing has been removed from it yet"
        );
    }
}

// ─────────────────────────────────────────────────── 2. the seal and the lanes

/// The lanes a hive names, read out of its own `params.contract`.
fn lanes(cfg: &Value) -> (Vec<String>, Vec<String>) {
    let read_side = |key: &str| -> Vec<String> {
        cfg["params"]["contract"][key]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|e| e["route"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    (read_side("accepts"), read_side("emits"))
}

#[test]
fn the_screen_is_sealed_and_states_four_lanes() {
    let Some(root) = shipped_display() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(cfg["cell"]["type"], "hive");
    assert_eq!(
        cfg["params"]["ports"],
        meclaw_core::serde_json::json!([]),
        "the hive path is the only address (#228)"
    );

    let (accepts, emits) = lanes(&cfg);
    assert_eq!(accepts, vec!["in_view", "in_withdraw"]);
    assert_eq!(emits, vec!["event", "receipt"]);

    // A lane name says what the caller wants, never where it lands inside.
    for cell in ["compose", "views", "web"] {
        for lane in accepts.iter().chain(emits.iter()) {
            assert_ne!(
                lane, cell,
                "a lane renamed after an inner cell satisfies the letter of the seal and misses it"
            );
        }
    }
}

#[test]
fn the_app_is_sealed_and_states_two_lanes() {
    let Some(root) = shipped_colony_view() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(cfg["cell"]["type"], "hive");
    assert_eq!(cfg["params"]["ports"], meclaw_core::serde_json::json!([]));

    let (accepts, emits) = lanes(&cfg);
    assert_eq!(accepts, vec!["in_refresh"]);
    assert_eq!(emits, vec!["view"]);
}

/// The one absolute lane, and the condition that is not optional.
///
/// An edge matches EVERY emission of the cell it starts at, and the probe emits
/// on two lanes. Granted unconditionally, this one would send the snapshot to
/// the graph endpoint as well; each answer produces another snapshot, and the
/// growth is exponential — a colony that stops routing with an EMPTY dead-letter
/// queue, which is GH #161 and cost most of a day to find.
#[test]
fn the_graph_lane_is_declared_here_and_carries_its_condition() {
    let Some(root) = shipped_colony_view() else {
        return;
    };
    let cfg = read_json(&root.join("config.json"));
    let edges = cfg["params"]["graph"]["edges"].as_array().expect("edges");
    let absolute: Vec<&Value> = edges
        .iter()
        .filter(|e| e["to"].as_str().unwrap_or_default().starts_with('/'))
        .collect();
    assert_eq!(
        absolute.len(),
        1,
        "exactly one absolute endpoint, and it is bootstrap-only (#163): {absolute:#?}"
    );
    assert_eq!(absolute[0]["to"], "/colony/graph");
    let cond = absolute[0]["condition"]
        .as_str()
        .expect("the absolute lane carries a condition (#161)");
    assert!(
        cond.contains("hop.route") && cond.contains("ask_colony"),
        "the condition names the lane it is for: {cond}"
    );
}

// ────────────────────────────────────────────────────────── 3. `app` is a tag

#[test]
fn the_app_declares_the_tag_and_the_screen_does_not() {
    let Some(app) = shipped_colony_view() else {
        return;
    };
    let Some(display) = shipped_display() else {
        return;
    };

    // Asked of the substrate's own reader: `tags` is a field
    // `parse_template_json` already carries into the registry row, so declaring
    // an app costs no substrate change. That is the whole claim.
    let scanned = parse_template_json(&app.join("template.json")).expect("colony-view");
    let tags: Value =
        meclaw_core::serde_json::from_str(&scanned.tags_json).expect("tags_json is JSON");
    assert_eq!(
        tags,
        meclaw_core::serde_json::json!(["app"]),
        "the catalogue marker is a tag, not an invented `kind`"
    );

    let screen = parse_template_json(&display.join("template.json")).expect("display");
    let screen_tags: Value =
        meclaw_core::serde_json::from_str(&screen.tags_json).expect("tags_json is JSON");
    assert_eq!(
        screen_tags,
        meclaw_core::serde_json::json!([]),
        "a screen is a channel, not an app"
    );
}

// ──────────────────────────────────────────── 4. the shipped bytes are sources

#[test]
fn every_script_is_the_file_beside_it() {
    let pairs = [
        ("display", "compose", "compose.py"),
        ("colony-view", "probe", "probe.py"),
        ("colony-view", "layout", "layout.py"),
    ];
    if shipped_display().is_none() || shipped_colony_view().is_none() {
        return;
    }
    for (tpl, cell, file) in pairs {
        let dir = repo(&format!("templates/{tpl}/{cell}"));
        let cfg = read_json(&dir.join("config.json"));
        let inline = cfg["params"]["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{tpl}/{cell} declares no script_inline"));
        let source = read(&dir.join(file));
        assert_eq!(
            inline, source,
            "{tpl}/{cell}/config.json has drifted from {file}"
        );
    }
}

/// The browser half is a file a person reads, greps and diffs -- and it is the
/// same bytes in three places. `canvy` keeps them in step with
/// `scripts/canvy_sync.py`; this template has no generator, so the equality is
/// asserted here instead of being produced somewhere and hoped for.
#[test]
fn the_browser_half_is_the_same_bytes_in_three_places() {
    let Some(root) = shipped_colony_view() else {
        return;
    };
    let layout = read(&root.join("layout/layout.py"));
    for (constant, file) in [
        ("CLIENT_JS", "colony-view.js"),
        ("CLIENT_CSS", "colony-view.css"),
    ] {
        let on_disk = read(&root.join("layout").join(file));
        let extracted = extract_constant(&layout, constant)
            .unwrap_or_else(|| panic!("layout.py carries no extractable {constant}"));
        assert_eq!(
            extracted, on_disk,
            "layout.py's {constant} has drifted from layout/{file}"
        );
    }
}

/// Pull one `NAME = r\"\"\"…\"\"\"` literal out of the layout source.
///
/// The extraction is deliberately dumb: one assignment, one raw triple-quoted
/// string, no escapes to interpret. If the browser half ever needs a `"""` in
/// it, the answer is to change the browser half, not to teach this function
/// about quoting -- a splice gate that can be argued with is not a gate.
fn extract_constant(src: &str, name: &str) -> Option<String> {
    let open = format!("{name} = r\"\"\"");
    let start = src.find(&open)? + open.len();
    let end = src[start..].find("\"\"\"")? + start;
    Some(src[start..end].to_string())
}

/// The example's own producer is the same arrangement: a `.py` a person reads
/// and a copy the runner gets. It travels with the export like everything under
/// `examples/`, so it needs the same lock as a template's.
#[test]
fn the_examples_scribe_is_the_file_beside_it() {
    let dir = repo("examples/display-colony-view/seed/main/scribe");
    if !dir.join("scribe.py").exists() {
        return;
    }
    let cfg = read_json(&dir.join("config.json"));
    assert_eq!(
        cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        read(&dir.join("scribe.py")),
        "the example's config.json has drifted from scribe.py"
    );
}

/// A shipped script names no environment variable and no absolute local path.
/// Same guard `canvy2_pipeline` puts on its own bytes: what travels here is a
/// topology, never a deployment.
#[test]
fn a_shipped_script_carries_no_environment_token() {
    if shipped_display().is_none() || shipped_colony_view().is_none() {
        return;
    }
    for (tpl, cell, file) in [
        ("display", "compose", "compose.py"),
        ("colony-view", "probe", "probe.py"),
        ("colony-view", "layout", "layout.py"),
    ] {
        let src = read(&repo(&format!("templates/{tpl}/{cell}/{file}")));
        for forbidden in ["os.environ", "getenv", "/home/", "127.0.0.1:78"] {
            assert!(
                !src.contains(forbidden),
                "{tpl}/{cell}/{file} names {forbidden}"
            );
        }
    }
}
