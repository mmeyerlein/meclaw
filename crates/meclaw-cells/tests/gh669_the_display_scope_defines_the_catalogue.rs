//! GH #669 -- the display scope defines the catalogue.
//!
//! A screen that ships a design language ships the components the language is
//! written against, or the language styles nothing. The compose cell's
//! `components()` is where the display's own components are defined, and this
//! file pins what that list says: the four GLASS components and the twenty-six
//! content components of the kit, under the screen's own prefix, on the layer the
//! `web` cell would demand of them, and none of them editable.
//!
//! Glass is not the same set as the windows. A WINDOW is what a view's root may
//! be, and the curator writes its rendering values -- `data-level` among them --
//! on those and on nothing else; `compose.py` names them in `WINDOWS`, and they
//! are `display-pane`, `display-panel`, `display-overlay` and `display-view-prose`.
//! `display-ornament` is glass and is NOT one of them: it is furniture an
//! application hangs into its view, which the sheet fixes to the bottom edge. The
//! two sets overlap in three, and this file measures the glass one.
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
    "display-os",
];

/// The four glass components of the catalogue: the only glass in the catalogue, all on
/// the navigation layer. Three of them are windows; the fourth, the ornament, is
/// furniture. What the curator levels is `WINDOWS` in `compose.py`, which is a
/// different list -- see `the_curator_writes_a_level_on_exactly_the_four_windows`.
const GLASS: [&str; 4] = [
    "display-pane",
    "display-panel",
    "display-overlay",
    "display-ornament",
];

/// The twenty-six content components, in the kit's own order.
const CONTENT: [&str; 26] = [
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
    "display-input",
    "display-chart",
    "display-stack",
    "display-progress",
    "display-dock",
    // The empty seat in the dock (display-hive.md § 4.29): empty SPACE and not a
    // placeholder, so it is an object of its own and therefore a component of its own.
    // It joined the catalogue with display 2.4.0 and is why every count here moved by one.
    "display-seat",
    "display-tile",
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

/// The components the curator writes its rendering values on, asked of the
/// script itself: `WINDOWS` in `compose.py`.
fn curator_windows() -> Option<Vec<String>> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps(list(m.WINDOWS)))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(out.status.success(), "compose.py did not load");
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("WINDOWS is JSON");
    Some(
        v.as_array()
            .expect("a list")
            .iter()
            .map(|n| n.as_str().expect("a name").to_string())
            .collect(),
    )
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

/// The four glass components stand in the catalogue, right after the five the
/// screen always had (shell, region, two view wrappers, the OS mark).
#[test]
fn the_scope_defines_the_four_glass_components() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let names: Vec<&str> = all.iter().map(name).collect();
    for glass in GLASS {
        assert!(
            names.contains(&glass),
            "`{glass}` is not defined: {names:?}"
        );
    }
    assert_eq!(&names[..5], &OWN, "the screen's own five come first");
    assert_eq!(&names[5..9], &GLASS, "then the four glass components");
}

/// Five of the screen's own, four glass components, twenty-six content components.
#[test]
fn the_scope_defines_thirty_five_components() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let names: Vec<&str> = all.iter().map(name).collect();
    assert_eq!(all.len(), 35, "{names:?}");
}

/// The catalogue is these thirty names and no others, held as a sorted
/// constant: a typo in one name is a red test here and not a component an
/// application names in vain.
#[test]
fn the_catalogue_names_are_exactly_the_thirty() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let mut expected: Vec<&str> = GLASS.iter().chain(CONTENT.iter()).copied().collect();
    expected.sort_unstable();
    let mut catalogue: Vec<&str> = all.iter().map(name).filter(|n| !OWN.contains(n)).collect();
    catalogue.sort_unstable();
    assert_eq!(catalogue, expected);
    // And the twenty-six stand in the kit's order, after the glass.
    let names: Vec<&str> = all.iter().map(name).collect();
    assert_eq!(&names[9..], &CONTENT, "the content components, in order");
}

/// The rule the `web` cell enforces at `component.define`, asked of every
/// component here and now: whatever writes one of the three glass words is
/// navigation layer, and nothing on the content layer writes one. The cell's
/// own scanner does the reading, so the answer here is the answer a running
/// colony would give.
#[test]
fn every_glass_component_is_navigation_and_every_content_component_is_not() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    for c in &all {
        let (n, t, l) = (name(c), template(c), layer(c));
        check_glass_layer(n, t, l).unwrap_or_else(|e| panic!("{e}"));
        if GLASS.contains(&n) {
            assert!(
                writes_glass(t),
                "`{n}` is in the glass four and writes none"
            );
            assert_eq!(l, "navigation", "`{n}` writes glass on the wrong layer");
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
/// outside, a `display-pane-title` and a `display-text` inside, and nothing of
/// the base template's `card` left. It still writes glass, so it stays on the
/// navigation layer; its name and its props do not move. It is a WINDOW, and the
/// test below is where that is measured.
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
    // The title is a `display-pane-title` since GH #679: one title slot the
    // sheet scales by the rung, no fixed kicker form.
    for class in ["display-pane", "display-pane-title", "display-text"] {
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

/// The catalogue and the faces against the template's own declarations
/// (`docs/development-rules.md` § 2d drift lock).
///
/// The README is NOT the other half any more. Since display@2.5.0 it is the
/// rendering of the display-hive description (display-hive.md § 0.3), and that
/// description carries no component list: which components a template defines is
/// build detail of the template (§ 0.1). So the public contract surface for both
/// halves is the template itself -- `components()` for the catalogue, and
/// `contract.settings` for the setting an operator sets.
#[test]
fn the_catalogue_and_the_faces_agree_with_the_contract() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };

    // The catalogue: every name `components()` defines beyond the screen's own
    // five, counted, so a component that appears or vanishes is a red test here.
    let catalogue: Vec<&str> = all.iter().map(name).filter(|n| !OWN.contains(n)).collect();
    assert_eq!(
        catalogue.len(),
        30,
        "thirty beyond the screen's own: {catalogue:?}"
    );
    let mut sorted = catalogue.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        catalogue.len(),
        "no name twice: {catalogue:?}"
    );

    // The faces: `params.font_base` is a setting of the cell, so the contract is
    // what an operator reads. A string, empty as shipped, and the description
    // names the two files and says that neither of them is in this tree.
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/display/compose/config.json")).expect("config"),
    )
    .expect("config.json parses");
    let spec = &cfg["contract"]["settings"]["font_base"];
    assert_eq!(spec["type"], "string", "font_base is a string setting");
    assert_eq!(spec["default"], "", "font_base defaults to empty");
    let description = spec["description"].as_str().expect("described");
    for file in ["inter.woff2", "fraunces.woff2"] {
        assert!(description.contains(file), "the contract names {file}");
    }
    assert!(
        description.contains("No font file ships in this repository"),
        "the contract says the faces are not in the tree"
    );
    assert!(
        description.contains("Empty means no @font-face at all"),
        "the contract says what the empty default means"
    );
}

/// The windows, measured rather than named: the components the curator writes a
/// drawing LEVEL on.
///
/// `data-level` is what § 7d of the description is spent on -- which window stands
/// large, which is put away, which is hidden altogether -- and the curator writes it
/// on a view's root and on nothing else. `compose.py` holds that list in `WINDOWS`,
/// the rendering reads it in `add_tree`, `ghost` and `unwrap_window`, and the four
/// templates it names each carry a `data-level` slot.
///
/// `display-ornament` is not one of them. It writes glass, so it stands in `GLASS`
/// above, but it declares none of the curator's props and its template has no slot
/// to put them in: rendered as the root of a view it comes out as a bare
/// `<nav class="display-ornament glass">`, and no level rule of the sheet reaches
/// it. The word "window" therefore names four components here and four different
/// ones in `GLASS`; this test is the one that pins the curator's four.
#[test]
fn the_curator_writes_a_level_on_exactly_the_four_windows() {
    if !library_ships() {
        return;
    }
    let (Some(listed), Some(all)) = (curator_windows(), components()) else {
        return;
    };
    assert_eq!(
        listed,
        [
            "display-pane",
            "display-panel",
            "display-overlay",
            "display-view-prose"
        ],
        "the curator's windows"
    );
    let of = |n: &str| {
        all.iter()
            .find(|c| name(c) == n)
            .unwrap_or_else(|| panic!("`{n}` is not defined"))
            .clone()
    };
    // Each of the four declares the curator's `level` and has a slot to render it in.
    for w in &listed {
        let c = of(w);
        assert_eq!(c["prop_schema"]["level"], "text", "`{w}` declares `level`");
        assert!(
            template(&c).contains("data-level=\"{{level}}\""),
            "`{w}` renders no `data-level`: {}",
            template(&c)
        );
    }
    // The ornament does neither, and is glass all the same.
    let ornament = of("display-ornament");
    assert!(
        !listed.iter().any(|w| w == "display-ornament"),
        "the ornament is not one of the curator's windows"
    );
    assert!(
        ornament["prop_schema"]["level"].is_null(),
        "the ornament declares a `level` it cannot render"
    );
    assert!(
        !template(&ornament).contains("data-level"),
        "the ornament renders a `data-level`: {}",
        template(&ornament)
    );
    assert!(
        writes_glass(template(&ornament)),
        "the ornament is glass none the less"
    );
}
