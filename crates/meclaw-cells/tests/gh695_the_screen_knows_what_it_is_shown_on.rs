//! Welle D -- the renderer knows its display (Spec § 2.8, D-21..D-24).
//!
//! The root carries a profile, the inputs it has and a scale; the sheet turns
//! that scale into the sizes the dock and the type are made of. Nothing here
//! reads a pixel width: a television is far away, not small.
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

/// `components()` as the shipped script defines them.
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
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    Some(v.as_array().expect("a list").clone())
}

fn find<'a>(all: &'a [Value], name: &str) -> &'a Value {
    all.iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("components() defines no {name}"))
}

#[test]
fn the_root_wears_the_profile_the_inputs_and_the_scale() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let shell = find(&all, "display-shell");
    // The whole contract surface at once: a schema the floor writes into is
    // not something to grow one prop at a time.
    for key in ["screen", "profile", "inputs", "scale", "screens"] {
        assert_eq!(
            shell["prop_schema"][key], "text",
            "display-shell declares `{key}`: {}",
            shell["prop_schema"]
        );
    }
    assert_eq!(shell["prop_schema"]["dock_overflow"], "int");
    assert_eq!(
        shell["prop_schema"]["client_js"], "html",
        "raw, or the screen's own motion would ship escaped"
    );
    let page = render_pieces_plain(
        shell["template"].as_str().expect("a template"),
        &json!({"stylesheet": true, "faces": "", "ground": "day",
                "screen": "tv", "profile": "tv", "inputs": "audio",
                "scale": "1.6", "dock_overflow": 0, "screens": "{}",
                "client_js": ""}),
        &shell["prop_schema"],
    )
    .expect("the web cell renders the shell");
    assert!(
        page.contains("data-profile=\"tv\""),
        "the root says which screen it is: {page}"
    );
    assert!(
        page.contains("data-inputs=\"audio\""),
        "and what that screen can take: {page}"
    );
    assert!(
        page.contains("data-screen=\"tv\""),
        "and which exit of the one state this page is: {page}"
    );
    assert!(
        page.contains("--scale: 1.6"),
        "and the one number the sizes are derived from: {page}"
    );
}

#[test]
fn the_sheet_derives_every_size_from_the_scale() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    for token in [
        "--scale",
        "--type-scale",
        "--tile",
        "--os",
        "--dock-pad",
        "--dock-gap",
    ] {
        assert!(
            sheet.contains(&format!("{token}:")),
            "the sheet declares {token}"
        );
    }
    let block = sheet
        .split(".display-columns {\n  --type-scale:")
        .nth(1)
        .expect("the profile block restates the tokens where the inline scale can reach them")
        .split('}')
        .next()
        .unwrap();
    for size in [
        "--t-caption",
        "--t-small",
        "--t-body",
        "--t-title-2",
        "--t-value",
    ] {
        assert!(
            block.contains(&format!("{size}: calc(")),
            "the type is multiplied by the scale: {block}"
        );
    }
    assert!(
        sheet.contains(".display-columns[data-profile=\"tv\"] {"),
        "a television gets its own contrast (D-24)"
    );
    // No pixel media query undoes the profile: both width blocks name it.
    for query in ["@media (max-width: 80rem)", "@media (max-width: 48rem)"] {
        let body = sheet
            .split(query)
            .nth(1)
            .unwrap_or_else(|| panic!("the sheet has {query}"));
        let body = &body[..body
            .find("\n}\n")
            .map(|i| i + 3)
            .unwrap_or(body.len().min(1200))];
        assert!(
            body.contains("[data-profile=\"tv\"]"),
            "{query} does not shrink a television: {body}"
        );
    }
}

#[test]
fn the_inputs_only_steer_what_is_visible() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet");
    assert!(
        sheet.contains("[data-inputs~=\"audio\"]"),
        "without audio the mark stays and only its glow goes"
    );
}
