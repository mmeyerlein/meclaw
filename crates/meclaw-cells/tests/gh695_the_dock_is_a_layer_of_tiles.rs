//! The dock is a layer of tiles of ONE size (display-hive.md § 6.10, § 6.11).
//!
//! One canvas, one dock. The dock is fixed at the right edge, above the canvas, a
//! column of tiles of one size ordered by rank, and it ends above the OS mark. The
//! canvas keeps a gutter so nothing lies under it. The two regions are both canvas now:
//! `aside` is accepted and drawn as `main`.
//!
//! What a tile carries is § 6.10: glyph, line, value, unit, `unread`, the rung, whether
//! its window is open, whether it is pinned, `end_at`, and the OBJECT ID of its window
//! for the tap. The attribute names are the contract of `ATTRS` in `compose.py`
//! (`data-rung`, `data-open`, `data-seat`, `data-unread`, ...), and the tap is
//! `phx-click="tap"` with `phx-value-for=<window id>` (§ 5).
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
fn a_tile_carries_what_the_contract_names_and_is_one_size() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let tile = find(&all, "display-tile");
    let schema = &tile["prop_schema"];
    // § 6.10, and nothing that was struck with the old vocabulary: no `state`, no
    // `on_canvas`. `oid` is the window a tap carries, `for` the name the client matches
    // the two halves of one object on.
    for (key, kind) in [
        ("glyph", "text"),
        ("line", "text"),
        ("value", "text"),
        ("unit", "text"),
        ("unread", "text"),
        ("topic", "text"),
        ("for", "text"),
        ("oid", "text"),
        ("rung", "text"),
        ("open", "text"),
        ("pinned", "text"),
        ("seat", "text"),
        ("rank", "text"),
        ("tap", "boolean"),
        ("end_at", "int"),
    ] {
        assert_eq!(schema[key], kind, "display-tile declares `{key}`: {schema}");
    }
    for gone in ["state", "on_canvas"] {
        assert!(
            schema[gone].is_null(),
            "`{gone}` left with the old vocabulary: {schema}"
        );
    }
    let html = render_pieces_plain(
        tile["template"].as_str().expect("a template"),
        &json!({"glyph": "\u{23f1}", "line": "pasta", "value": "04:12", "unit": "min",
                "unread": "1", "topic": "timer:x", "for": "view.alex.timer",
                "oid": "view.alex.timer", "rung": "relevant", "open": "1",
                "pinned": "1", "seat": "", "rank": "0.44", "tap": true, "end_at": 1}),
        schema,
    )
    .expect("the web cell renders a tile");
    for needle in [
        "class=\"display-tile\"",
        "data-for=\"view.alex.timer\"",
        "data-rung=\"relevant\"",
        "data-rank=\"0.44\"",
        "data-pinned=\"1\"",
        "data-open=\"1\"",
        "data-unread=\"1\"",
        "data-end-at=\"1\"",
        // § 5: the tap names the WINDOW, and the value is the object id.
        "phx-click=\"tap\"",
        "phx-value-for=\"view.alex.timer\"",
        "display-tile-glyph",
        "display-tile-value",
        "display-tile-unit",
        "display-tile-line",
    ] {
        assert!(html.contains(needle), "a tile carries {needle}: {html}");
    }
    // § 6.4: an output with no finger binds no tap, and then the tile has no click.
    let dead = render_pieces_plain(
        tile["template"].as_str().expect("a template"),
        &json!({"glyph": "\u{23f1}", "line": "pasta", "oid": "view.alex.timer",
                "tap": false}),
        schema,
    )
    .expect("the web cell renders a tile");
    assert!(
        !dead.contains("phx-click"),
        "without a finger the tile is not clickable: {dead}"
    );

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
        "every tile is ONE size (§ 6.10): {rule}"
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
    // And nothing else: `profile` was a copy of the root's own word in an
    // attribute no selector has read since 2.5.0 -- the root says which output
    // this is (`data-exit`, § 6.1) and the sheet asks there.
    assert_eq!(dock["prop_schema"]["profile"], Value::Null);
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
        "z-index: var(--plane-os)",
        "pointer-events: none",
        "padding-block-end: calc(var(--os)",
    ] {
        assert!(rule.contains(needle), "the dock rule says {needle}: {rule}");
    }
    assert!(
        sheet.contains(
            "padding-inline-end: calc(var(--tile) + 3 * var(--dock-pad) - var(--gutter))"
        ),
        "the canvas keeps a lane so nothing lies under the dock -- minus the \
         gutter it carries inside its own scroller since GH #739"
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
    // Where a region draws is the sheet's § 6.3 since 2.5.0, and it is there
    // alone: this constant said `display: contents` for every region while the
    // sheet said `display: grid` for the canvas, and only the order inside one
    // `<style>` decided which won (development-rules § 2d).
    assert!(
        !layout.contains("[data-region]"),
        "the layout rules lay a region out again: {layout}"
    );
    let sheet_src = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        sheet_src.contains(".display-columns > [data-region=\"aside\"] { display: contents; }"),
        "`aside` is accepted and drawn as canvas (OR-D3): its box generates \
         nothing, so its windows stand in the column beside the canvas"
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
