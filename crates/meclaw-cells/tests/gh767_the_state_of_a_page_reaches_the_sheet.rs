//! The state of a page survives the curator and reaches the sheet (GH #767).
//!
//! A page carries a `state`, and it is the one prop of `display-browser` that
//! says whether a person is looking at a picture, at a picture on its way, or
//! at a page that is not there at all (wave G contracts § 4: the application
//! maps the cell's seven words onto `loading|ready|error|suspended`). The
//! sheet has a rule for three of the four, so the word is the whole of the
//! difference between a frame that fills and a frame that failed.
//!
//! Measured on the twin with a real page on the screen (report `g15.md`, the
//! second finding at the edge; sharpened by `g16.md`, which made `error` the
//! first state with anything visible behind it): the figure came out as
//! `<figure class="display-browser" data-page-state="">` while the cell had
//! said `opening` and the application had written `loading`. The word was
//! dropped in between -- in `add_tree`, by the guard that keeps an
//! application's word about a WINDOW down to `urgent`/`hidden` (GH #679).
//! That guard ran on every node of the tree, so the four words a page may
//! wear were dropped along with the `focus` it was built against.
//!
//! This lock reads the MARKUP the web cell renders out of the object the
//! curator wrote -- not the prop the application handed in, which would be a
//! round trip against the sender.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).

mod support;

use std::process::Command;

use meclaw_cells::web::render::render_pieces_plain;
use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, component_view, library_ships, pane_id, repo, window_id};

/// One output, named and complete (§ 4.7).
fn params() -> Value {
    json!({"screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0}},
           "default_screen": "tv"})
}

/// The shipped script's components and its sheet, asked of the script itself.
fn probe() -> Option<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps({'components': m.components(), 'kit_css': m.KIT_CSS}))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the probe is JSON"))
}

fn component<'a>(probe: &'a Value, name: &str) -> &'a Value {
    probe["components"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("components() defines no {name}"))
}

/// A window with a page in it, as the application sends one (`stage.py`
/// `card_node`): five props of its own, `mount` left to the screen.
fn page_view(view_id: &str, state: &str) -> Value {
    component_view(
        view_id,
        "main",
        json!({
            "component": "display-pane",
            "key": "c.p",
            "props": {"pane_id": "c.p", "title": "A page"},
            "children": [{"component": "display-browser",
                          "props": {"page": "card-7", "url": "https://example.org/a",
                                    "title": "A page", "viewport": "1280x800@1",
                                    "state": state}}],
        }),
    )
}

/// The markup of the page in a view the curator has just passed.
fn page_markup(probe: &Value, state: &str) -> String {
    let mut screen = Screen::new(params());
    screen.write(page_view("v", state), 100_000);
    let props = screen
        .props(&format!("{}/0", pane_id("v", "p")))
        .unwrap_or_else(|| panic!("the page object stands: {:?}", screen.held));
    let page = component(probe, "display-browser");
    render_pieces_plain(
        page["template"].as_str().expect("a template"),
        &props,
        &page["prop_schema"],
    )
    .expect("the web cell renders a page")
}

/// Every one of the four words comes out of the curator and into the figure.
#[test]
fn the_four_words_of_a_page_reach_the_markup() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    for word in ["loading", "ready", "error", "suspended"] {
        let html = page_markup(&probe, word);
        assert!(
            html.contains(&format!("data-page-state=\"{word}\"")),
            "a page the application called `{word}` wears that word: {html}"
        );
    }
}

/// And the words are worth something: three of the four have a rule, so a
/// page that failed does not look like a page that is there.
#[test]
fn a_page_that_failed_does_not_look_like_a_page_that_is_there() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let ready = page_markup(&probe, "ready");
    for word in ["error", "loading", "suspended"] {
        let html = page_markup(&probe, word);
        assert_ne!(
            html, ready,
            "a `{word}` page differs from a `ready` one in the markup"
        );
        let rule = format!("[data-page-state=\"{word}\"]");
        assert!(
            probe["kit_css"].as_str().unwrap_or("").contains(&rule),
            "and the sheet has something to say about it: {rule}"
        );
    }
}

/// A word that is not one of the four does not reach the figure.
///
/// Fail-closed, the same movement the window guard makes: the cell's own
/// seven words are the application's to map (contracts § 4), and `urgent` is
/// a word about a window and means nothing on a canvas. A rule nobody wrote
/// cannot fire, so an attribute wearing an unknown word is only a lie in the
/// markup.
#[test]
fn a_word_the_page_does_not_know_is_dropped() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    for word in ["opening", "urgent", "focus", "closed"] {
        let html = page_markup(&probe, word);
        assert!(
            html.contains("data-page-state=\"\""),
            "a page that said `{word}` wears nothing: {html}"
        );
    }
}

/// And a window keeps only its own two words -- GH #679 is not undone.
///
/// The second half is read on a window INSIDE the tree: a bad word on the
/// first window is refused at the door of the pass (`state` is a hint of the
/// view there), and what this case is about is the walk, which sees every
/// node.
#[test]
fn a_window_still_keeps_only_its_own_two_words() {
    if !library_ships() {
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(
        component_view(
            "w",
            "main",
            json!({"component": "display-pane", "key": "c.p",
                   "props": {"pane_id": "c.p", "title": "A pane", "state": "urgent"},
                   "children": [{"component": "display-pane", "key": "c.q",
                                 "props": {"pane_id": "c.q", "title": "Inside",
                                           "state": "focus"}}]}),
        ),
        100_000,
    );
    let props = screen
        .props(&pane_id("w", "p"))
        .unwrap_or_else(|| panic!("the pane stands: {:?}", screen.held));
    assert_eq!(props["state"], "urgent", "an app may say `urgent`: {props}");
    let inner = screen
        .props(&format!("{}/c.q", pane_id("w", "p")))
        .unwrap_or_else(|| panic!("the inner pane stands: {:?}", screen.held));
    assert!(
        inner.get("state").is_none(),
        "a `focus` an app claims never reaches the screen (GH #679): {inner}"
    );
    // The guard drops a word, not a view.
    assert!(screen.holds(&window_id("alex", "w")));
}
