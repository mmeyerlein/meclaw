//! Welle D -- the OS mark is the button (Spec § 2.6, D-2, D-3, D-17, D-20).
//!
//! A mark with no card behind it, bottom right, and it is what a person holds
//! to speak. What was said does NOT stand beside it any more -- it stands in
//! the chat application, which is the one place a conversation lives. The
//! hook keeps its name (`DisplayMic`): the gesture did not change, only where
//! its words go.
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
fn the_mark_replaced_the_capsule() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    assert!(
        all.iter().all(|c| c["name"] != "display-mic"),
        "display-mic is gone (OR-D7)"
    );
    let os = all
        .iter()
        .find(|c| c["name"] == "display-os")
        .expect("components() defines no display-os");
    assert_eq!(os["prop_schema"]["mount"], "text");
    assert_eq!(os["prop_schema"]["client_js"], "html");
    let t = os["template"].as_str().expect("a template");
    for needle in [
        "class=\"display-os\"",
        "id=\"display-os\"",
        "phx-hook=\"DisplayMic\"",
        "data-mount=\"{{mount}}\"",
        "class=\"display-os-mark\"",
        "aria-label=\"hold to talk\"",
        "<svg",
        "display-os-ring",
        "display-os-s",
        "data-role=\"state\"",
        "aria-live=\"polite\"",
    ] {
        assert!(t.contains(needle), "the mark carries {needle}: {t}");
    }
    assert!(
        !t.contains("data-role=\"transcript\""),
        "the words do not stand beside the mark any more (D-17): {t}"
    );
    assert!(
        !t.contains("display-mic"),
        "and no class of the capsule survived: {t}"
    );
}

#[test]
fn the_hook_no_longer_writes_a_transcript() {
    if !library_ships() {
        return;
    }
    // The client script lives in a Python string, so every JS quote is `\"`
    // on disk; read it the way the browser gets it.
    let src = std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"");
    assert!(
        !src.contains("line.textContent"),
        "the hook still writes a transcript line"
    );
    assert!(
        src.contains("phase(\"listening\")"),
        "the hook says its phase on the element instead"
    );
    assert!(
        src.contains("setPointerCapture") && src.contains("\"lostpointercapture\""),
        "the gesture is unchanged (GH #684)"
    );
    assert!(
        src.contains("\"visibilitychange\""),
        "and a hidden page still releases"
    );
}

#[test]
fn the_mark_is_transparent_and_takes_the_gesture() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    let rule = sheet
        .split(".display-os {")
        .nth(1)
        .expect("the sheet has a rule for the mark")
        .split('}')
        .next()
        .unwrap();
    for needle in [
        "position: fixed",
        "inset-inline-end: calc(var(--dock-pad) + env(safe-area-inset-right, 0px))",
        "inset-block-end: calc(var(--dock-pad) + env(safe-area-inset-bottom, 0px))",
        "inline-size: var(--os)",
        "z-index: calc(var(--plane-os) + 1)",
        "background: transparent",
        "touch-action: none",
    ] {
        assert!(rule.contains(needle), "the mark rule says {needle}: {rule}");
    }
    assert!(
        sheet.contains(".display-os[data-phase=\"listening\"]"),
        "the phase is light on the mark, not a sentence beside it (D-17)"
    );
    assert!(
        !sheet.contains(".display-mic"),
        "and no rule of the capsule survived"
    );
}
