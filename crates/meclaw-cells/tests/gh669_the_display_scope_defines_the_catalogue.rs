//! GH #669 -- the display scope defines the catalogue.
//!
//! A screen that ships a design language ships the components the language is
//! written against, or the language styles nothing. The compose cell's
//! `components()` is where the display's own components are defined, and this
//! file pins what that list says: the four windows and the twenty-two content
//! components of the kit, under the screen's own prefix, on the layer the
//! `web` cell would demand of them, and none of them editable.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! library is skipped, never judged; a host without `python3` is skipped too,
//! since the catalogue is asked of the script itself.

use meclaw_cells::web::ops::{check_glass_layer, writes_glass};
use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

/// The five the screen is made of, before the catalogue begins.
const OWN: [&str; 5] = [
    "display-shell",
    "display-region",
    "display-view-prose",
    "display-view-custom",
    "display-mic",
];

/// The four windows: the only glass in the catalogue, all navigation layer.
const WINDOWS: [&str; 4] = [
    "display-pane",
    "display-panel",
    "display-overlay",
    "display-ornament",
];

/// The twenty-two content components, in the kit's own order.
const CONTENT: [&str; 22] = [
    "display-value",
    "display-text",
    "display-voice",
    "display-kicker",
    "display-list",
    "display-item",
    "display-table",
    "display-weather",
    "display-clock",
    "display-timer",
    "display-chat",
    "display-chat-line",
    "display-notification",
    "display-media",
    "display-document",
    "display-status",
    "display-action",
    "display-choice",
    "display-option",
    "display-chart",
    "display-stack",
    "display-progress",
];

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// `components()` as the shipped script defines them, asked of the script
/// itself -- the same door `gh669_the_screen_ships_its_design_language_in_three_places`
/// uses. `None` when there is no `python3` on this host.
fn components() -> Option<Vec<Value>> {
    let out = std::process::Command::new("python3")
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

fn name(c: &Value) -> &str {
    c["name"].as_str().expect("a component has a name")
}

fn template(c: &Value) -> &str {
    c["template"].as_str().expect("a component has a template")
}

fn layer(c: &Value) -> &str {
    c["layer"].as_str().expect("a component has a layer")
}

/// The four windows stand in the catalogue, right after the five the screen
/// always had (shell, region, two view wrappers, microphone).
#[test]
fn the_scope_defines_the_four_windows() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let names: Vec<&str> = all.iter().map(name).collect();
    for window in WINDOWS {
        assert!(
            names.contains(&window),
            "`{window}` is not defined: {names:?}"
        );
    }
    assert_eq!(&names[..5], &OWN, "the screen's own five come first");
    assert_eq!(&names[5..9], &WINDOWS, "then the four windows");
}

/// Five of the screen's own, four windows, twenty-two content components.
#[test]
fn the_scope_defines_thirty_one_components() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let names: Vec<&str> = all.iter().map(name).collect();
    assert_eq!(all.len(), 31, "{names:?}");
}

/// The catalogue is these twenty-six names and no others, held as a sorted
/// constant: a typo in one name is a red test here and not a component an
/// application names in vain.
#[test]
fn the_catalogue_names_are_exactly_the_twenty_six() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let mut expected: Vec<&str> = WINDOWS.iter().chain(CONTENT.iter()).copied().collect();
    expected.sort_unstable();
    let mut catalogue: Vec<&str> = all.iter().map(name).filter(|n| !OWN.contains(n)).collect();
    catalogue.sort_unstable();
    assert_eq!(catalogue, expected);
    // And the twenty-two stand in the kit's order, after the windows.
    let names: Vec<&str> = all.iter().map(name).collect();
    assert_eq!(&names[9..], &CONTENT, "the content components, in order");
}

/// The rule the `web` cell enforces at `component.define`, asked of every
/// component here and now: whatever writes one of the three glass words is
/// navigation layer, and nothing on the content layer writes one. The cell's
/// own scanner does the reading, so the answer here is the answer a running
/// colony would give.
#[test]
fn every_window_is_navigation_and_every_content_component_is_not() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    for c in &all {
        let (n, t, l) = (name(c), template(c), layer(c));
        check_glass_layer(n, t, l).unwrap_or_else(|e| panic!("{e}"));
        if WINDOWS.contains(&n) {
            assert!(writes_glass(t), "`{n}` is a window and writes no glass");
            assert_eq!(l, "navigation", "`{n}` is a window on the wrong layer");
        }
        if l == "content" {
            assert!(!writes_glass(t), "`{n}` is content and writes glass");
        }
    }
}

/// A prop a browser may write is an authorisation an application grants over
/// its OWN component. Nothing in the screen's catalogue is that.
#[test]
fn nothing_in_the_catalogue_is_editable() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    for c in &all {
        assert_eq!(
            c["editable"],
            Value::Array(vec![]),
            "`{}` grants a browser a prop",
            name(c)
        );
    }
}

/// The prose view is the screen's own use of its catalogue: a `display-pane`
/// outside, a `display-kicker` and a `display-text` inside, and nothing of the
/// base template's `card` left. It still writes glass, so it stays on the
/// navigation layer; its name and its props do not move.
#[test]
fn the_prose_view_wears_the_catalogue() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let prose = all
        .iter()
        .find(|c| name(c) == "display-view-prose")
        .expect("the prose view is defined");
    let t = template(prose);
    for class in ["display-pane", "display-kicker", "display-text"] {
        assert!(t.contains(class), "the prose view writes no `{class}`: {t}");
    }
    assert!(
        !t.contains("card") && !t.contains("title-3"),
        "the prose view still wears the base template's clothes: {t}"
    );
    assert!(
        writes_glass(t),
        "the prose view is a window and writes glass"
    );
    assert_eq!(layer(prose), "navigation");
    for prop in ["view_id", "owner", "title", "body"] {
        assert_eq!(prose["prop_schema"][prop], "text", "the prop `{prop}`");
    }
    // The custom wrapper is untouched: a bare content shell, so an application
    // may hang its own pane inside it.
    let custom = all
        .iter()
        .find(|c| name(c) == "display-view-custom")
        .expect("the custom view is defined");
    assert!(!writes_glass(template(custom)));
    assert_eq!(layer(custom), "content");
}

/// The README and the contract say what the scope does (§ 2d drift lock): the
/// sentence about the catalogue names every component `components()` defines
/// beyond the screen's own five, and no other; the sentence about `font_base`
/// describes the setting `contract.settings` carries, with the same default and
/// the same two file names.
#[test]
fn the_readme_and_the_catalogue_agree_with_the_scope() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let readme = std::fs::read_to_string(repo("templates/display/README.md")).expect("README");

    // The catalogue: the section that lists it, gripped by its heading, and
    // the sentence that gives the number.
    let start = readme
        .find("## The components this scope defines")
        .expect("the README has a section on the components");
    let end = readme[start + 3..]
        .find("\n## ")
        .map(|i| start + 3 + i)
        .unwrap_or(readme.len());
    let section = &readme[start..end];
    assert!(
        section.contains("twenty-six"),
        "the README says how many components the catalogue has"
    );
    let mut named: Vec<&str> = section
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| t.starts_with("display-"))
        .collect();
    named.sort_unstable();
    named.dedup();
    let mut defined: Vec<&str> = all.iter().map(name).collect();
    defined.sort_unstable();
    assert_eq!(
        named, defined,
        "the README names exactly the components the scope defines"
    );
    let catalogue = all.iter().map(name).filter(|n| !OWN.contains(n)).count();
    assert_eq!(catalogue, 26, "twenty-six, as the README says");

    // The faces: the README's `font_base` paragraph against the contract.
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/display/compose/config.json")).expect("config"),
    )
    .expect("config.json parses");
    let spec = &cfg["contract"]["settings"]["font_base"];
    assert_eq!(spec["type"], "string", "font_base is a string setting");
    assert_eq!(spec["default"], "", "font_base defaults to empty");
    let start = readme
        .find("## `params.font_base`")
        .expect("the README has a section on font_base");
    let end = readme[start + 3..]
        .find("\n## ")
        .map(|i| start + 3 + i)
        .unwrap_or(readme.len());
    // Wrapped prose, read as one line.
    let section = readme[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        section.contains("Empty, the shipped default, means no `@font-face` at all"),
        "the README says what the empty default means"
    );
    let description = spec["description"].as_str().expect("described");
    for file in ["inter.woff2", "fraunces.woff2"] {
        assert!(section.contains(file), "the README names {file}");
        assert!(description.contains(file), "the contract names {file}");
    }
    assert!(
        section.contains("No font file ships in this repository"),
        "the README says the faces are not in the tree"
    );
    assert!(
        description.contains("No font file ships in this repository"),
        "the contract says the same"
    );
}
