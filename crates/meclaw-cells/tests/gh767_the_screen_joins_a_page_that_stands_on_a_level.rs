//! A page's stream follows the LEVEL of its window, not its element (GH #767).
//!
//! `display-hive.md` § 7.9: a page joins while its window stands and is given
//! back when it is put away. A window on level 0 is still in the DOM -- the
//! pass computes the level (§ 4.24) and the sheet hides it -- so a hook that
//! joined by existence would hold a screencast open for every page ever shown
//! until the tab closes. The signal is `updated`: LiveView morphs the attribute
//! in place and neither `mounted` nor `destroyed` fires for it.
//!
//! This file reads the hook's SOURCE, which is where those decisions are
//! visible at all; the file `gh767_the_page_canvas_draws_and_sends_browser.rs`
//! runs it in a real engine and measures that it does what it says.
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

/// The sweep reads the LEVEL, and it reads it as a number.
#[test]
fn the_sweep_joins_by_level_and_not_by_existence() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    assert!(
        part.contains("closest(\"[data-level]\")"),
        "the sweep asks the window, not the page: {part:?}"
    );
    // Text, always -- the template language reads an `int 0` as empty, so a
    // string comparison against "0" would be one reading of one spelling.
    assert!(
        part.contains("parseInt(win.getAttribute(\"data-level\"), 10)")
            && part.contains("if (!(level >= 1)) continue;"),
        "the level is parsed once and compared as a number"
    );
    // An empty handle is a frame with no stream behind it and is skipped, not
    // joined under the empty name (§ 6.14).
    assert!(
        part.contains("if (!key) continue;"),
        "a frame with no page is left alone"
    );
    // And the sweep runs where the attribute changes: `updated`.
    assert!(
        js.contains("pageSweep(this.el, this.__scene.st);"),
        "the sweep runs on every patch"
    );
    assert!(
        js.contains("st.pageSweep = function () { pageSweep(el, st); };"),
        "and the handle a proof pulls exists"
    );
}

/// The join payload is FLAT, and its viewport comes out of the profile table.
#[test]
fn the_join_is_flat_and_the_viewport_comes_from_the_profile() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    assert!(
        part.contains("socket.channel(\"page:\" + link.key, {"),
        "the topic is `page:<page>` and its name is in the markup"
    );
    // OR-G32: one level. Nested one deeper it would arrive at the cell as
    // `params.params.viewport`, which nobody reads -- and every page would
    // stand at the shipped default for ever.
    assert!(
        part.contains(
            "viewport: { width: vp.width, height: vp.height, dpr: vp.dpr, mobile: vp.mobile }"
        ),
        "the viewport rides one level under the payload: {part:?}"
    );
    assert!(
        !part.contains("params: {"),
        "nothing wraps the payload a second time"
    );
    // The numbers of § 6.6, and the table is keyed by the exit's own word. A
    // MEASURED box would change with every patch, the cell would write each
    // change into the page as an `Emulation.setDeviceMetricsOverride`, and the
    // streamed page would reflow under the reader's hand.
    for needle in [
        "tv: { width: 1280, height: 800, dpr: 1, mobile: false }",
        "monitor: { width: 1920, height: 1080, dpr: 1, mobile: false }",
        "phone: { width: 393, height: 852, dpr: 3, mobile: true }",
    ] {
        assert!(part.contains(needle), "the profile table carries {needle}");
    }
    assert!(
        part.contains(
            "PAGE_VIEWPORTS[el.getAttribute(\"data-exit\") || \"\"] || PAGE_VIEWPORTS.tv"
        ),
        "an exit nobody knows is a television"
    );
    assert!(
        !part.contains("getBoundingClientRect().width)"),
        "the viewport is read, never measured"
    );
}

/// A refusal is SAID on the frame, in the cell's own words.
#[test]
fn a_refused_join_says_so_on_the_frame() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    assert!(
        part.contains("pageLink(link, \"down\", (e && e.reason) || \"refused\")"),
        "the cell's own reason reaches the frame: {part:?}"
    );
    assert!(
        part.contains("pageLink(link, \"down\", \"the browser never answered\")"),
        "and a join nobody answered is not silence"
    );
    // On the hook's OWN attribute. `data-page-state` belongs to the curator,
    // who maps the cell's seven states (contracts § 4); a hook writing there
    // too overwrote a `suspended` with `ready` and the sheet's grayscale rule
    // never drew (measured 20.09. at the seam, OR-G.g18.1).
    assert!(
        part.contains("fig.setAttribute(\"data-page-link\", word)"),
        "the client's word is written where the sheet reads it"
    );
    assert!(
        !part.contains("setAttribute(\"data-page-state\""),
        "and the curator's word is not touched by the hook: {part:?}"
    );
    // And the attribute is for the SHEET. A person reads WORDS: "no browser
    // mounted" is a different repair from "unknown page", and an attribute
    // nothing renders leaves an empty frame with an address in it and no
    // explanation (contracts § 4: the client says "no browser mounted").
    assert!(
        part.contains("fig.querySelector('[data-role=\"reason\"]')"),
        "the refusal reaches the line a person reads: {part:?}"
    );
    assert!(
        part.contains("line.textContent = why"),
        "and it reaches it as text, not as an attribute: {part:?}"
    );
    // Writing the same words into an `aria-live` region again is an
    // announcement a person hears twice (the same guard the mark's line has).
    assert!(
        part.contains("line.textContent !== why"),
        "the same sentence is not said twice: {part:?}"
    );
}

/// The hook parses in a real engine.
///
/// A hook that does not parse registers nothing, and a screen with no
/// `DisplayScene` looks exactly like a colony with no browser cell: empty
/// frames, and no error anywhere. Skips without `node`.
#[test]
fn the_hook_parses() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let path = td.path().join("scene.js");
    std::fs::write(&path, js).expect("write");
    let out = match Command::new("node").arg("--check").arg(&path).output() {
        Ok(out) => out,
        Err(_) => return,
    };
    assert!(
        out.status.success(),
        "`node --check` on SCENE_CLIENT_JS:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
