//! A finger in a page reaches the CELL, and the curator hears nothing
//! (GH #767).
//!
//! `display-hive.md` § 5.6 and § 5.11: the pass knows `app_write`,
//! `app_withdraw`, `verdict`, `tap`, `hold` and `stroke`, and typing into a
//! page is none of them. That a page in use does not expire is the CELL's
//! business (a throttled `active_at`) together with the APP's (`touched`) --
//! the client sends compose nothing at all.
//!
//! And the page must not swallow the tap of the tile beside it (§ 5.1,
//! § 5.2, § 5.7): on a telephone the dock is the only way to a window.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).
mod support;

use std::process::Command;

use support::{COMPOSE, library_ships, repo};

/// The scene hook as the browser gets it, asked of the script itself.
fn scene() -> Option<String> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             sys.stdout.write(m.SCENE_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The part of the hook that belongs to pages: everything the wave added,
/// before the lifecycle object. The listener rules below are statements about
/// THIS part, and measuring them over the whole file would read the scene's own
/// tile handler as a breach.
fn pages_part(js: &str) -> &str {
    let from = js
        .find("// \u{2500}\u{2500} Pages")
        .expect("the hook carries a page section");
    let to = js[from..]
        .find("var hook = {")
        .expect("and it ends before the hook");
    &js[from..from + to]
}

/// The same part with its comment lines taken out.
///
/// The prohibitions below are about what the hook CALLS, and the comments say
/// in words what it must not do -- a `contains` over both would read the
/// explanation as the breach.
fn code_only(part: &str) -> String {
    part.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The five frame forms of the wire, and no sixth.
#[test]
fn five_frame_forms_travel_and_a_cancel_is_an_up() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    for needle in [
        "{ type: \"pointer\", kind: kind, x: at.x, y: at.y,",
        "{ type: \"touch\", kind: kind, points: points }",
        "{ type: \"wheel\", x: at.x, y: at.y, dx: e.deltaX, dy: e.deltaY }",
        "{ type: \"key\", kind: \"down\", key: e.key, code: e.code,",
        "{ type: \"text\", text: text }",
    ] {
        assert!(part.contains(needle), "the wire carries {needle}: {part:?}");
    }
    // A cancel is not a sixth type: the system takes the pointer away, and a
    // streamed page that never heard the release keeps the button down.
    assert!(
        part.contains("on(canvas, \"pointercancel\", up);"),
        "a cancel travels as an up"
    );
    // And a right click is swallowed, nothing more: it would otherwise open
    // the VIEWER's menu over a page rendered somewhere else.
    assert!(
        part.contains("on(canvas, \"contextmenu\", function (e) { e.preventDefault(); });"),
        "the viewer's own menu stays shut"
    );
}

/// Coordinates are scaled into page pixels, x and y on their own.
#[test]
fn the_coordinate_is_the_pages_own() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    assert!(
        part.contains("x: Math.round((e.clientX - box.left) * (w / (box.width || 1))),")
            && part.contains("y: Math.round((e.clientY - box.top) * (h / (box.height || 1)))"),
        "event coordinates times viewport over client rect: {part:?}"
    );
}

/// The keyboard types into a hidden field, and the field is RENDERED.
#[test]
fn the_keyboard_goes_into_a_hidden_field_on_the_body() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // A canvas is focusable but not editable: the engine sends it no
    // `beforeinput`, no `input` and no `composition*`, and CDP's
    // `Input.insertText` reaches nothing on it -- so every dead key, every
    // emoji out of a picker and every on-screen keyboard would be lost.
    assert!(
        part.contains("field.className = \"display-browser-keys\";")
            && part.contains("document.body.appendChild(field);"),
        "the field hangs outside what LiveView patches: {part:?}"
    );
    assert!(
        part.contains("var text = link.field.value;") && part.contains("link.field.value = \"\";"),
        "what the field ended up holding is what was typed, and it starts over"
    );
    assert!(
        part.contains("fig.setAttribute(\"data-keys\", \"true\")"),
        "and the frame says where the keyboard is"
    );
}

/// Nothing of this reaches the curator, and nothing of it eats the tile's tap.
#[test]
fn the_page_sends_compose_nothing_and_swallows_nothing() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    let code = code_only(part);
    // `touch` as an event is struck (§ 5.6): there is no lane for it, and a
    // client that invented one would have its whole emission discarded.
    assert!(
        !code.contains("pushEvent"),
        "the page part sends the curator nothing: {code}"
    );
    assert!(
        !code.contains("stopPropagation"),
        "and stops nothing on its way to the tile: {code}"
    );
    // Every listener hangs on the canvas or on the field. None on the root,
    // none on `document`, and none in the capture phase -- any of the three
    // would take the dock away from a telephone.
    assert!(
        !code.contains("el.addEventListener") && !code.contains("document.addEventListener"),
        "no listener on the root and none on the document: {code}"
    );
    assert_eq!(
        code.matches("addEventListener").count(),
        1,
        "every listener goes through the one place that also gives it back: {code}"
    );
    assert!(
        code.contains("node.addEventListener(type, fn);"),
        "and that place hangs it in the BUBBLE phase -- a capture listener \
         would be ahead of the tile's own handler: {code}"
    );
    // What an output cannot do is not wired: a television declares `["audio"]`
    // and gets no listener, no field and no work.
    for word in ["pointer", "touch", "keyboard"] {
        assert!(
            part.contains(&format!("pageWord(el, \"{word}\")")),
            "`{word}` decides whether it is wired at all"
        );
    }
}
