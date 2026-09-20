//! A page is CONTENT in a window, not a fifth window object (GH #767).
//!
//! `display-hive.md` § 2 names four window objects -- `display-pane`,
//! `display-panel`, `display-overlay`, `display-view-prose` -- and a page is
//! none of them: § 7.9 says a page is a `display-pane` with a
//! `display-browser` child. So `display-browser` joins catalogue B on the
//! content layer, beside `display-media`, which is the same piece of
//! furniture: a frame with a caption under it.
//!
//! What it carries is six props and no seventh (wave G contracts § 4):
//! `page` (the topic suffix the client joins on), `mount` (the browser cell
//! the screen names, filled by the curator), `url`, `title` (the PAGE's title
//! -- the model title is a hint of the WINDOW, OR-G48), `viewport` and
//! `state`. `layer`, `relevance` and `linger` are the window's hints and have
//! no business here.
//!
//! And it carries no `client_js`: what is drawn on the canvas is drawn by the
//! scene hook at the root (R-G8), so `SCRIPTED` does not grow and no second
//! script enters with a content component.
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

/// The catalogue entry: content layer, not editable, six props and no seventh.
#[test]
fn the_page_is_a_content_component_with_six_props() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let page = find(&all, "display-browser");
    assert_eq!(
        page["layer"], "content",
        "a page sits on a window's fill, it is not the window: {page}"
    );
    assert_eq!(
        page["editable"].as_array().map(Vec::len),
        Some(0),
        "nothing on this screen is dragged: {page}"
    );
    let schema = &page["prop_schema"];
    for key in ["page", "mount", "url", "title", "viewport", "state"] {
        assert_eq!(
            schema[key], "text",
            "`{key}` is one of the six, and every one of them travels as text: {schema}"
        );
    }
    assert_eq!(
        schema.as_object().map(|o| o.len()),
        Some(6),
        "six props and no seventh -- `layer`, `relevance` and `linger` \
         are hints of the WINDOW: {schema}"
    );
    // R-G8: the drawing happens in the scene hook at the root. A content
    // component with its own script would be a third `client_js` on the page.
    assert!(
        page["client_js"].is_null(),
        "no script rides in with a page: {page}"
    );
}

/// The template, needle by needle: the canvas carries what the hook reads, the
/// caption carries what a person reads, and nothing in it is raw.
#[test]
fn the_template_hands_the_hook_its_handles_and_nothing_raw() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let page = find(&all, "display-browser");
    let tpl = page["template"].as_str().expect("a template");
    for needle in [
        "class=\"display-browser\"",
        "data-page-state=\"{{state}}\"",
        "class=\"display-browser-view\"",
        "data-page=\"{{page}}\"",
        "data-mount=\"{{mount}}\"",
        "data-viewport=\"{{viewport}}\"",
        "tabindex=\"0\"",
        "role=\"img\"",
        "aria-label=\"{{title}}\"",
        "class=\"display-browser-bar display-line\"",
        "class=\"display-browser-title\"",
        "class=\"display-browser-url\"",
        // The line the client's own refusal goes into: an attribute is for
        // the sheet, and a person needs the words (contracts § 4). It is
        // rendered EMPTY -- no application writes it, the hook does -- and it
        // is polite, because a refusal is not an alarm.
        "class=\"display-browser-reason\"",
        "data-role=\"reason\"",
        "aria-live=\"polite\"",
    ] {
        assert!(
            tpl.contains(needle),
            "the page template carries {needle}: {tpl}"
        );
    }
    // A screencast frame is BYTES on a canvas; nothing here is markup an
    // application wrote, so nothing here is raw. And no `href`: an address a
    // viewer could follow would leave the screen for the page's own site --
    // the page is looked at through the cell, or not at all.
    assert!(!tpl.contains("{{&"), "no raw prop rides into a page: {tpl}");
    assert!(
        !tpl.contains("href"),
        "the address is text, not a link: {tpl}"
    );
}

/// The markup the `web` cell renders, with every prop filled.
#[test]
fn a_filled_page_renders_what_the_hook_looks_for() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let page = find(&all, "display-browser");
    let html = render_pieces_plain(
        page["template"].as_str().expect("a template"),
        &json!({"page": "card-7", "mount": "browser", "url": "https://example.org/a",
                "title": "A page", "viewport": "1280x800", "state": "ready"}),
        &page["prop_schema"],
    )
    .expect("the web cell renders a page");
    for needle in [
        "data-page=\"card-7\"",
        "data-mount=\"browser\"",
        "data-viewport=\"1280x800\"",
        "data-page-state=\"ready\"",
        "aria-label=\"A page\"",
        ">A page<",
        ">https://example.org/a<",
    ] {
        assert!(
            html.contains(needle),
            "a drawn page carries {needle}: {html}"
        );
    }
}

/// And the empty case: a frame with no stream behind it is still a frame
/// (§ 6.14 -- a window without a stream stands all the same).
#[test]
fn a_page_with_nothing_in_it_is_still_a_frame() {
    if !library_ships() {
        return;
    }
    let Some(all) = components() else {
        return;
    };
    let page = find(&all, "display-browser");
    let html = render_pieces_plain(
        page["template"].as_str().expect("a template"),
        &json!({"page": "", "mount": "", "url": "", "title": "", "viewport": "",
                "state": "error"}),
        &page["prop_schema"],
    )
    .expect("the web cell renders a page");
    // The attribute STANDS and is empty: the sweep reads `data-page` off every
    // canvas and skips the empty ones, and an absent attribute would make the
    // canvas invisible to it instead of skipped by it.
    assert!(
        html.contains("data-page=\"\""),
        "an empty page keeps its handle: {html}"
    );
    assert!(
        html.contains("data-page-state=\"error\""),
        "the state is written even when nothing else is: {html}"
    );
    assert!(
        !html.contains("display-browser-title"),
        "no title element without a title: {html}"
    );
}
