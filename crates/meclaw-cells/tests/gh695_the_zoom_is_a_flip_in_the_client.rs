//! Welle D -- the zoom between a tile and a window is a FLIP in the client
//! (Spec § 2.5, D-8, OR-D6).
//!
//! LiveView patches with morphdom and does not let itself be wrapped in
//! `document.startViewTransition`, and `::view-transition-group(*)` would move
//! every window at once. So the shell carries a hook: it measures before the
//! patch, measures after it, and animates the one object that changed level of
//! attention. Under `prefers-reduced-motion` it measures nothing.
//!
//! The identifiers are asserted statically here; that the thing RUNS is
//! `gh695_the_scene_ticks_and_chimes_browser.rs`.
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

#[test]
fn the_shell_carries_the_scene_hook() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let shell = all
        .iter()
        .find(|c| c["name"] == "display-shell")
        .expect("display-shell");
    assert_eq!(shell["prop_schema"]["client_js"], "html");
    let t = shell["template"].as_str().expect("a template");
    assert!(t.contains("phx-hook=\"DisplayScene\""), "the hook: {t}");
    assert!(
        t.contains("id=\"display-shell\""),
        "a hook needs an id: {t}"
    );
    assert!(
        t.contains("<script>{{&client_js}}</script>"),
        "and the script rides in the dead render, before the socket boot: {t}"
    );
}

#[test]
fn the_scene_measures_before_and_after_and_gives_up_under_reduced_motion() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"");
    for needle in [
        "beforeUpdate: function",
        "updated: function",
        "getBoundingClientRect",
        "prefers-reduced-motion: reduce",
        ".animate(",
        "data-zoomed",
        "__displayScene",
        "DisplayScene: hook",
    ] {
        assert!(src.contains(needle), "the scene hook says {needle}");
    }
    assert!(
        !src.contains("startViewTransition"),
        "the zoom is a FLIP, not a view transition (OR-D6)"
    );
}

/// The floor left the root's `client_js` empty because the constant did not
/// exist yet; the one curator line this strand touches fills it (D2/D1
/// interface). A shell that ships its hook with no script behind it would be
/// a hook LiveView cannot find.
#[test]
fn the_root_ships_the_scene_script() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    assert!(
        src.contains("\"client_js\": SCENE_CLIENT_JS"),
        "the root's client_js is the scene script"
    );
}
