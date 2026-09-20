//! The mount of the browser cell is the SCREEN's word, never the app's (GH #767).
//!
//! A `display-browser` carries a `mount`: the name under which the `web` cell
//! finds the browser cell when the client joins `page:<page>`. An application
//! cannot know that name -- it is an operator's arrangement of one member's
//! colony, the same kind of thing as `voice_mount` -- so the screen writes it,
//! in `add_tree`, out of `params.browser_mount`. Exactly the movement with
//! which the same walk writes `for` on every `display-input`.
//!
//! And it is written ALWAYS, not only into an empty slot: `object.update`
//! merges per key, so a mount an application once said would stand on that
//! object for ever after, and the screen's own value would never reach it.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{Screen, component_view, library_ships, pane_id, repo};

/// One output, named and complete (§ 4.7), plus whatever knobs a case sets.
fn params(extra: &[(&str, Value)]) -> Value {
    let mut p = json!({"screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0}},
                       "default_screen": "tv"});
    for (key, value) in extra {
        p[*key] = value.clone();
    }
    p
}

/// A window with a page in it, as an application would send one: the app fills
/// five of the six props and leaves `mount` to the screen.
fn page_view(view_id: &str, said: Value) -> Value {
    component_view(
        view_id,
        "main",
        json!({
            "component": "display-pane",
            "key": "c.p",
            "props": {"pane_id": "c.p", "title": "A page"},
            "children": [{"component": "display-browser", "props": said}],
        }),
    )
}

fn said(mount: Option<&str>) -> Value {
    let mut props = json!({"page": "card-7", "url": "https://example.org/a",
                           "title": "A page", "viewport": "", "state": "loading"});
    if let Some(name) = mount {
        props["mount"] = json!(name);
    }
    props
}

/// The shipped default reaches the object without anybody saying it.
#[test]
fn the_shipped_mount_is_written_on_every_page() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params(&[]));
    screen.write(page_view("v", said(None)), 100_000);
    let props = screen
        .props(&format!("{}/0", pane_id("v", "p")))
        .unwrap_or_else(|| panic!("the page object stands: {:?}", screen.held));
    assert_eq!(
        props["mount"], "browser",
        "the screen names the cell, with its shipped default: {props}"
    );
    // And it wrote only that: what the application said about its own page is
    // untouched.
    assert_eq!(props["page"], "card-7");
    assert_eq!(props["url"], "https://example.org/a");
}

/// An operator who mounted the cell under another name is followed.
#[test]
fn another_mount_reaches_the_page() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params(&[("browser_mount", json!("pages"))]));
    screen.write(page_view("v", said(None)), 100_000);
    let props = screen
        .props(&format!("{}/0", pane_id("v", "p")))
        .expect("the page object stands");
    assert_eq!(props["mount"], "pages", "{props}");
}

/// And an application that names a mount itself is OVERWRITTEN.
///
/// Not "filled where empty": `object.update` merges per key, so a value said
/// once would outlive every later pass, and a page would go on asking a cell
/// that is not there.
#[test]
fn a_mount_an_application_invented_is_overwritten() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params(&[]));
    screen.write(page_view("v", said(Some("somewhere-else"))), 100_000);
    let props = screen
        .props(&format!("{}/0", pane_id("v", "p")))
        .expect("the page object stands");
    assert_eq!(
        props["mount"], "browser",
        "the screen's word stands over the application's: {props}"
    );
}

/// The knob is declared where an operator reads it, and the two halves agree.
///
/// `params.<k>` and `contract.settings.<k>.default` are one statement in two
/// places (the lock `gh204_declared_defaults_match_the_inline` holds it for
/// every template); this case says the knob EXISTS at both and says what it is
/// for, because a setting nobody documents is a setting nobody sets.
#[test]
fn the_knob_stands_in_the_contract_and_in_the_params() {
    if !library_ships() {
        return;
    }
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/display/compose/config.json")).expect("config"),
    )
    .expect("config.json parses");
    assert_eq!(
        cfg["params"]["browser_mount"], "browser",
        "the shipped default is the cell type's own name"
    );
    let spec = &cfg["contract"]["settings"]["browser_mount"];
    assert_eq!(spec["type"], "string", "{spec}");
    assert_eq!(spec["secret"], false, "a mount is a name, not a secret");
    assert_eq!(
        spec["default"], cfg["params"]["browser_mount"],
        "the declared default is the inline one: {spec}"
    );
    let description = spec["description"].as_str().unwrap_or("");
    assert!(
        description.contains("page:"),
        "the description names the topic the mount answers: {description:?}"
    );
}
