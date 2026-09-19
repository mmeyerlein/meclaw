//! GH #708 -- the client draws the tap before the pass (§ 5.7), and draws only
//! the end state of a series (§ 5.8).
//!
//! "Immediately visible. Not in the model (rendering): the client draws the
//! effect of a tap by sentences 1-3 optimistically (window open or closed, tile
//! pressed, a co-closed modal closed); the curator's pass confirms it. Because
//! the finger stands above the score (§ 4.19), the pass never disagrees; a
//! deviation is a defect -- Q-20 is the pass the client is checked against"
//! (§ 5.7). And § 5.8: "the client draws only the end state of the series, no
//! window flashes".
//!
//! Until this wave the client drew the pressed TILE and nothing else: the
//! window opened when the patch of the next pass arrived, which is one round
//! trip through the colony -- on a phone the finger had long left the glass.
//!
//! What the three sentences mean for the DOM, and what this file pins:
//!
//!   * a tap on the tile of an open window puts it away (§ 5.2): level 0,
//!     rung `ambient`, the tile no longer `open`;
//!   * a tap on the tile of a window that is not open opens it (§ 5.1): level 1
//!     on the canvas ladder, level 2 on the modal one, rung `focus`;
//!   * either way, a tap on a CANVAS window co-closes the open modal (§ 5.3 --
//!     the rule reads the tapped window's `layer`, also on put-away);
//!   * and while the client's own drawing stands, the FLIP animation keeps its
//!     hands off, or ten taps in a second are ten flights (§ 5.8).
//!
//! The browser proofs are B-06 (the DOM changes before the patch, and the end
//! state matches the pass) and B-07 (ten taps, at most one visible change),
//! strand H5.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn block(name: &str) -> String {
    let src = read(COMPOSE);
    let open = format!("{name} = (");
    let start = src.find(&open).unwrap_or_else(|| panic!("{name} is gone"));
    let end = src[start..]
        .find("\n)")
        .unwrap_or_else(|| panic!("{name} is not one parenthesised block"));
    src[start..start + end].to_string()
}

#[test]
fn the_scene_draws_the_window_and_not_only_the_tile() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        scene.contains("function optimistic("),
        "the scene hook has no optimistic drawing at all -- § 5.7: the client \
         draws the effect of a tap, the pass confirms it"
    );
    assert!(
        scene.contains("getAttribute(\\\"data-for\\\")"),
        "the drawing never looks up the WINDOW its tile stands for: the two \
         halves of one object carry the same name (§ 6.10)"
    );
    for written in [
        "setAttribute(\\\"data-level\\\"",
        "setAttribute(\\\"data-rung\\\"",
        "setAttribute(\\\"data-open\\\"",
    ] {
        assert!(
            scene.contains(written),
            "the drawing does not write `{written}` -- the client draws the \
             same words the curator does, or the confirming patch is a second \
             movement (§ 5.7)"
        );
    }
    assert!(
        scene.contains("st.optimistic++"),
        "nothing counts the drawings: B-06 reads the counter to prove the DOM \
         moved BEFORE the patch"
    );
}

#[test]
fn a_tap_on_a_canvas_window_co_closes_the_modal() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        scene.contains("data-layer\\\") === \\\"canvas\\\""),
        "the drawing does not read the tapped window's `layer` -- § 5.3: the \
         modal-closing rule reads the LAYER and not the rung, and it applies on \
         put-away as well (S-077, Decision 16.09.)"
    );
    assert!(
        scene.contains("[data-level=\\\"2\\\"]"),
        "nothing looks for the open modal to close with it (§ 5.3)"
    );
}

#[test]
fn the_flip_keeps_its_hands_off_while_the_client_draws() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    let flip = scene
        .find("function flip(")
        .expect("the FLIP movement is gone");
    let guard = scene
        .find("dataset.optimistic === \\\"1\\\"")
        .expect("nothing tells the movement that the client is drawing (§ 5.8)");
    assert!(
        guard > flip,
        "the guard stands outside `flip()`: § 5.8 asks that a series of taps \
         draws its END state, and an animation per patch is the flashing it \
         forbids"
    );
}
