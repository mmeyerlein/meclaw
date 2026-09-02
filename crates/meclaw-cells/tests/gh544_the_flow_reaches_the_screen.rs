//! GH #544 -- the picture a viewer gets is the picture the flow computed, and
//! an arrangement can no longer take it apart.
//!
//! `colony-view@1.0.0` declared `keep: ["x", "y"]` on every box and called what
//! it found there "hand placed". Measured on a running colony of 104 cells:
//! 1 box stood where the current flow put it, 208 of 215 unrelated hive pairs
//! had overlapping frames, one frame ran 299x the area of the three cells
//! inside it, and 1133 (hive, foreign cell) pairs intersected. No hand had been
//! anywhere near it -- and none could have been, because `data-oid` named an
//! object one tree level away from the one the display mints, so every drag
//! wrote to an id that does not exist.
//!
//! Two things were wrong under that, and both are identity:
//!
//! 1. **`keep` was attached to a slot.** An object id is the child index chain,
//!    and the picture writes hives, then edges, then cells -- so one edge more
//!    shifts every cell's index by one and the kept props are handed to the new
//!    occupant. `display@1.0.1` lets a tree node name its own `key`; a box's key
//!    is its cell path.
//! 2. **A coordinate was read as a pin.** `canvy@2.1.8` had already replaced
//!    that with a marker of its own; the re-cut of GH #455 lost it. The flow
//!    now owns `x`/`y` on every tick and a hand owns `hand` (`"dx,dy"`, ONE
//!    prop) plus `pinned` beside them -- an offset against the cell's own spot,
//!    so it travels with its hive instead of being left behind by the next
//!    re-rank (which is what GH #170 removed, and why this is not that).
//!
//! The measurement itself lives in
//! `tests/fixtures/gh544_geometry_check.py`, because it has to load the two
//! SHIPPED sources -- `layout.py` and the display's `compose.py` -- and mint
//! ids with the display's own `add_tree` rather than with a Rust copy of it.
//! Two copies of the arithmetic is the defect class this issue is an instance
//! of. What this file adds is that `cargo test` runs it at all.
//!
//! Skips when `python3` is absent or the templates do not ship, like every
//! other interpreter guard in this tree (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const LAYOUT: &str = "templates/colony-view/layout/layout.py";
const COMPOSE: &str = "templates/display/compose/compose.py";
const CHECK: &str = "crates/meclaw-cells/tests/fixtures/gh544_geometry_check.py";

#[test]
fn the_picture_holds_the_five_counts_arranged_and_untouched() {
    for rel in [LAYOUT, COMPOSE, CHECK] {
        if !repo(rel).exists() {
            return;
        }
    }
    let out = match std::process::Command::new("python3")
        .arg(repo(CHECK))
        .arg(repo(LAYOUT))
        .arg(repo(COMPOSE))
        .output()
    {
        Ok(o) => o,
        Err(_) => return, // no python3 on this host
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("all green"),
        "the colony view's geometry must hold:\n{stdout}\n{stderr}"
    );
    // The suite must actually have run something -- an empty file "passes" too.
    assert!(
        stdout.matches("  ok  ").count() >= 20,
        "too few checks ran; did the suite lose its cases?\n{stdout}"
    );
}

/// The two halves of the pin are declared, and the two that are not are gone.
///
/// A drift lock (`docs/development-rules.md` § 2d) rather than a second
/// measurement: what the checker proves is the geometry, and what this proves is
/// that the surface a browser writes to still says what the geometry assumed.
#[test]
fn the_node_component_declares_the_hand_and_not_the_flow() {
    if !repo(LAYOUT).exists() {
        return;
    }
    let src = std::fs::read_to_string(repo(LAYOUT)).expect("layout.py");
    assert!(
        src.contains(r#""editable": ["hand", "pinned"],"#),
        "a box's editable props are the hand's two, never the flow's"
    );
    assert!(
        src.contains(r#""keep": ["hand", "pinned"],"#),
        "`keep` covers the hand's two props and nothing the flow owns"
    );
    assert!(
        !src.contains(r#""keep": ["x", "y"],"#),
        "`x`/`y` kept is GH #544 itself: a position frozen at the tick its \
         object happened to be created"
    );
    assert!(
        src.contains("data-pinned=\"{{pinned}}\""),
        "the marker has to reach the markup, or the panel cannot offer to \
         release a box"
    );
    assert!(
        src.contains(r#"transform="translate({{x}},{{y}}) translate({{hand}})""#),
        "the flow's translate and the hand's translate are two, composed by \
         SVG, because the component language has no arithmetic -- and the \
         hand's is ONE prop, because a prop at a time is a picture at a time"
    );
    assert!(
        !src.contains("def clamp_offset("),
        "the bound that trimmed a hand's offset to its own hive is gone: it \
         made every count hold and measured as 85 x 15 pixels of travel on a \
         real screen, which reads as a broken gesture"
    );
    assert!(
        src.contains(r#""editable": ["x", "y", "w", "h"],"#),
        "the browser corrects the hive rectangle, because it is the only half \
         that can see where a hand put the boxes"
    );
    assert!(
        src.contains("CLIENT_ID = hashlib.sha1(") && src.contains(r#"data-client="{{client}}""#),
        "the picture names the browser half that wrote it, so a tab left open \
         across a template change can see that it is old"
    );
}

/// The browser half parses, and it parses as the file the layout ships.
///
/// The 0.12.1 lesson, kept: a client path is never proven over the websocket
/// alone. A syntax error in this file is a canvas that does nothing at all, and
/// every server-side test stays green through it. Skips without `node`.
#[test]
fn the_browser_half_is_syntactically_a_program() {
    let js = repo("templates/colony-view/layout/colony-view.js");
    if !js.exists() {
        return;
    }
    let out = match std::process::Command::new("node")
        .arg("--check")
        .arg(&js)
        .output()
    {
        Ok(o) => o,
        Err(_) => return, // no node on this host
    };
    assert!(
        out.status.success(),
        "colony-view.js must parse:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The display can be told what a tree node IS, and says so in its own source.
#[test]
fn a_tree_node_may_name_its_own_identity() {
    if !repo(COMPOSE).exists() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    assert!(
        src.contains("def is_node_key(value):"),
        "the key is validated at the door like every other id in this cell"
    );
    assert!(
        src.contains(r#"oid = "%s/%s" % (parent, key if is_node_key(key) else index)"#),
        "a keyed node is named by its key and an unkeyed one by its index -- \
         the index path is what every other view on the screen still uses"
    );
}
