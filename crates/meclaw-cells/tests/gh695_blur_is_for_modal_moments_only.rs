//! Welle D -- blur is for modal moments only (Spec § 2.11, D-12, D-15).
//!
//! A change of focus is not a modal moment: weather in focus does not make
//! the rest of the screen unsharp. What may blur the canvas is a window that
//! SAYS it is modal, and in this wave nobody says it but a test. Pinned stops
//! being invisible: a tile that keeps its place says so with a dot.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).

use std::process::Command;

use meclaw_core::serde_json::Value;

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

fn curated() -> Option<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps({'curated': m.CURATED, 'keys': list(m.CURATOR_KEYS),\n\
                               'hints': list(m.HINT_KEYS)}))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(out.status.success(), "compose.py did not load");
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("JSON"))
}

/// The floor (strand D1) put the two hints in the schema; this strand renders
/// them. A red test here means D1 is not done, and is not this strand's to fix.
#[test]
fn the_floor_declared_the_two_hints_this_sheet_reads() {
    if !library_ships() {
        return;
    }
    let Some(p) = curated() else {
        return;
    };
    assert_eq!(p["curated"]["topic"], "text", "D1 declares `topic`");
    assert_eq!(p["curated"]["modal"], "boolean", "D1 declares `modal`");
    assert!(
        p["keys"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|k| k == "topic_dupe"),
        "D1 declares `topic_dupe` as a curator key"
    );
}

#[test]
fn the_windows_render_the_two_hints() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    for name in ["PANE_TEMPLATE", "PANEL_TEMPLATE", "OVERLAY_TEMPLATE"] {
        let t = src
            .split(&format!("{name} = ("))
            .nth(1)
            .unwrap_or_else(|| panic!("{name}"))
            .split("\n)\n")
            .next()
            .unwrap();
        assert!(t.contains("data-modal=\"{{modal}}\""), "{name}: {t}");
        assert!(t.contains("data-topic=\"{{topic}}\""), "{name}: {t}");
    }
}

#[test]
fn the_neighbour_blur_is_gone_and_only_modal_is_left() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        !sheet.contains(".display-columns:has([data-state=\"focus\"]"),
        "the focus-change blur is struck (D-12)"
    );
    assert!(
        !sheet.contains(".display-scene:has(> [data-state=\"focus\"]"),
        "and so is its twin on the scene"
    );
    assert!(
        sheet.contains(".display-columns:has([data-modal=\"true\"])"),
        "and what is left is the modal one"
    );
    // Counted as a DECLARATION of its own (newline, two spaces), so the
    // material's `backdrop-filter` and the two blurs inside the leave
    // keyframe -- motion, not state -- are not counted with it. Exactly one
    // rule blurs the canvas, and it is the modal one. Never as a way of
    // saying "this is in focus".
    assert_eq!(
        sheet.matches("\n  filter: blur(").count(),
        1,
        "one rule-level `filter: blur(`, and it is the modal one"
    );
}

#[test]
fn a_pinned_tile_says_so() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        sheet.contains(".display-tile[data-pinned=\"true\"]::after"),
        "pinned means the tile stays, and a person can see that it will (D-15)"
    );
    assert!(
        sheet.contains(".display-tile[data-on-canvas=\"true\"]"),
        "the same object, just large right now"
    );
}
