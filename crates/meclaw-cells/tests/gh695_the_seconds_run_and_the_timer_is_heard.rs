//! Welle D -- the seconds run and the timer is heard (Spec § 2.7, D-14, D-27).
//!
//! The server's resolution is twenty seconds; a countdown that ticks in real
//! seconds therefore ticks in the BROWSER. The scene hook holds one interval,
//! writes the remainder into every `[data-end-at]`, and stamps `--now` once so
//! the CSS bar is not off by however long the page had been open. When a
//! window arrives carrying `urgent`, it plays a two-tone chime made of
//! oscillators -- no asset, no speech, no `<audio>`.
//!
//! What RUNS is `gh695_the_scene_ticks_and_chimes_browser.rs`, and beside it
//! `710_the_colony_holds_in_both_engines_browser.rs` (B-21 counts the seconds down in
//! Chromium AND WebKit); this file is the cheap half. Skips when `python3` is absent
//! or the templates do not ship.

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
fn the_timer_hands_the_browser_its_epoch() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let timer = all
        .iter()
        .find(|c| c["name"] == "display-timer")
        .expect("display-timer");
    assert_eq!(timer["prop_schema"]["now"], "int", "the bar gets a --now");
    let t = timer["template"].as_str().expect("a template");
    assert!(t.contains("data-end-at=\"{{end_at}}\""), "{t}");
    assert!(t.contains("--now: {{now}}"), "{t}");
    assert!(
        t.contains("data-role=\"remaining\""),
        "the client knows where to write the seconds: {t}"
    );
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        sheet.contains("--elapsed: calc(var(--now, var(--start-at)) - var(--start-at))"),
        "the bar reads --now, and the honest degradation stays"
    );
}

#[test]
fn the_scene_ticks_and_chimes() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"");
    for needle in [
        "setInterval",
        "[data-end-at]",
        "function mmss(",
        "createOscillator",
        "st.chimes++",
        "CHIME_EVERY_MS",
        "CHIME_MAX_MS",
        "ctx.resume()",
        "nowStamped",
    ] {
        assert!(src.contains(needle), "the scene hook says {needle}");
    }
    assert!(
        !src.contains("new Audio("),
        "the chime is synthetic: no asset ships for it"
    );
    assert!(
        src.contains("clearInterval"),
        "and a re-mount gives the interval back -- a wall screen reconnects all day"
    );
}
