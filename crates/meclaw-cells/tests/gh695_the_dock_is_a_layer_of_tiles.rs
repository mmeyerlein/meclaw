//! Welle D -- the dock is a layer of tiles of one size (Spec § 2.4, D-1..D-10).
//!
//! One canvas, one dock. The dock is fixed at the right edge, above the
//! canvas, a column of tiles of ONE size ordered by rank, and it ends above
//! the OS mark. The canvas keeps a gutter so nothing lies under it. The two
//! regions are both canvas now: `aside` is accepted and drawn as `main`.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).

use std::process::Command;

use meclaw_cells::web::render::render_pieces_plain;
use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const SHEET: &str = "templates/display/compose/display-dna.css";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn components() -> Option<Vec<Value>> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps(m.components()))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(out.status.success(), "compose.py did not load");
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("JSON");
    Some(v.as_array().expect("a list").clone())
}

fn find<'a>(all: &'a [Value], name: &str) -> &'a Value {
    all.iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("components() defines no {name}"))
}

#[test]
fn a_tile_says_ten_things_and_is_one_size() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let tile = find(&all, "display-tile");
    let schema = &tile["prop_schema"];
    for (key, kind) in [
        ("glyph", "text"),
        ("line", "text"),
        ("value", "text"),
        ("topic", "text"),
        ("for", "text"),
        ("state", "text"),
        ("on_canvas", "boolean"),
        ("pinned", "boolean"),
        ("rank", "text"),
        ("end_at", "int"),
    ] {
        assert_eq!(schema[key], kind, "display-tile declares `{key}`: {schema}");
    }
    let html = render_pieces_plain(
        tile["template"].as_str().expect("a template"),
        &json!({"glyph": "\u{23f1}", "line": "pasta", "value": "04:12",
                "topic": "timer:x", "for": "t-timer", "state": "relevant",
                "on_canvas": false, "pinned": true, "rank": "0.44", "end_at": 1}),
        schema,
    )
    .expect("the web cell renders a tile");
    for needle in [
        "class=\"display-tile\"",
        "data-for=\"t-timer\"",
        "data-state=\"relevant\"",
        "data-rank=\"0.44\"",
        "data-pinned=\"true\"",
        "data-on-canvas=\"false\"",
        "display-tile-glyph",
        "display-tile-value",
        "display-tile-line",
    ] {
        assert!(html.contains(needle), "a tile carries {needle}: {html}");
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    let rule = sheet
        .split(".display-tile {")
        .nth(1)
        .expect("the sheet has a rule for a tile")
        .split('}')
        .next()
        .unwrap();
    assert!(
        rule.contains("inline-size: var(--tile)") && rule.contains("aspect-ratio: 1"),
        "every tile is ONE size (D-1): {rule}"
    );
}

#[test]
fn the_dock_is_a_layer_above_the_canvas_that_ends_above_the_mark() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let dock = find(&all, "display-dock");
    assert_eq!(dock["prop_schema"]["count"], "int");
    assert_eq!(dock["prop_schema"]["profile"], "text");
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    let rule = sheet
        .split(".display-dock {")
        .nth(1)
        .expect("the sheet has a rule for the dock")
        .split('}')
        .next()
        .unwrap();
    for needle in [
        "position: fixed",
        "inset-inline-end: 0",
        "flex-direction: column",
        "justify-content: flex-end",
        "z-index: 20",
        "pointer-events: none",
        "padding-block-end: calc(var(--os)",
    ] {
        assert!(rule.contains(needle), "the dock rule says {needle}: {rule}");
    }
    assert!(
        sheet.contains("padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad))"),
        "the canvas keeps a gutter so nothing lies under the dock"
    );
}

#[test]
fn both_regions_are_canvas_now() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    let layout = src
        .split("LAYOUT_RULES = (")
        .nth(1)
        .expect("LAYOUT_RULES")
        .split("\n)\n")
        .next()
        .unwrap();
    assert!(
        layout.contains(".display-columns {"),
        "the layout rule the three-places lock reads is still there: {layout}"
    );
    assert!(
        layout.contains("[data-region] { display: contents; }"),
        "`aside` is accepted and drawn as canvas (OR-D3): {layout}"
    );
    assert!(
        !layout.contains("clamp(15rem, 22%, 24rem)"),
        "the narrow column is gone: {layout}"
    );
    assert!(
        !layout.contains(".display-mic"),
        "and the microphone's placement left with it: {layout}"
    );
    // An application's window hangs in a `<div data-view>` wrapper, a
    // grandchild of the region: the wrapper generates no box, and the
    // compact rule reaches the window through it (D-11).
    let sheet = std::fs::read_to_string(repo("templates/display/compose/display-dna.css"))
        .expect("the sheet ships");
    // ... and ONLY the wrapper: a prose window carries `data-view` itself,
    // as a direct child of the region, and must keep its box -- or a
    // 0-3-0 `display: contents` would beat the 0-2-0 hidden rule and draw a
    // hidden prose window's title and body on the canvas.
    assert!(
        sheet.contains(
            ".display-columns > [data-region] > [data-view]:not(.display-pane, .display-panel, .display-overlay) { display: contents; }"
        ),
        "the wrapper of an application's window generates no box, a prose window keeps its own"
    );
    assert!(
        !sheet.contains("[data-view] { display: contents; }"),
        "no bare `[data-view]` rule reaches the prose window"
    );
    // The compact rule reaches EVERY window on the canvas that is not inside
    // another window -- under the wrapper, or under a stack the application
    // put around its card -- and that stack generates no box either.
    assert!(
        sheet.contains(
            ".display-columns > [data-region] :where(.display-pane, .display-panel, .display-overlay):not(:where(.display-pane, .display-panel, .display-overlay) *)"
        ),
        "the compact rule reaches every top-level window on the canvas"
    );
    assert!(
        sheet.contains(
            ".display-columns > [data-region] > [data-view] > .display-stack { display: contents; }"
        ),
        "a stack directly under the wrapper generates no box"
    );
    // One size, always (D-1): a tile's value and line stay on one line.
    for class in [".display-tile-value {", ".display-tile-line {"] {
        let block = sheet
            .split(class)
            .nth(1)
            .expect(class)
            .split('}')
            .next()
            .unwrap();
        for decl in [
            "white-space: nowrap",
            "overflow: hidden",
            "text-overflow: ellipsis",
            "max-inline-size: 100%",
        ] {
            assert!(block.contains(decl), "{class} keeps one line: {decl}");
        }
    }
    assert!(
        !sheet.contains(
            ":where(.display-pane, .display-panel, .display-overlay)[data-state=\"ambient\"] {"
        ) && !sheet.contains(
            ":where(.display-pane, .display-panel, .display-overlay)[data-state=\"relevant\"] {"
        ),
        "nothing ambient or relevant is drawn on the canvas (spec 2.2)"
    );
}
