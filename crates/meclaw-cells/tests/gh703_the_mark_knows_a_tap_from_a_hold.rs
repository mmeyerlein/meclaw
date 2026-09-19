//! Welle F -- the mark tells a tap from a hold (R-23-4, R-23-5, E-18).
//!
//! One button, two gestures. Under the threshold the press is a PRESS and
//! means the dock; over it, it is a hold and means speech -- and a hold says
//! so twice, once to the `voice` cell as a frame and once to the curator as
//! the `hold` event of display-hive.md § 5.6, because holding the mark is
//! where a dialogue starts (§ 5.4, § 5.5, R-23-5).
//!
//! Since 2.5.0 the threshold is a token of the sheet (§ 5.5: "a token,
//! 250 ms"; § 2: numbers stand in the sheet) -- `708_the_threshold_is_a_token`
//! pins that -- and the event carries no field (§ 5.6, S-088).
//!
//! Two fallen ways in, both from the device: Safari sends a `click` after
//! `touchend`, and under `touch-action: none` with a captured pointer it is
//! the `pointerup` that can go missing. Both are wired, and the second way
//! asks whether this gesture already ended by pointer -- a span of time was
//! a number outside the sheet (§ 2) and a browser state that decides (§ 3.2).
//!
//! The client script lives in a Python string, so every JS quote is `\"` on
//! disk; it is read here the way the browser gets it. Skips when the
//! templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// `compose.py` with the escaping of the Python string undone.
fn script() -> String {
    std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"")
}

#[test]
fn the_threshold_is_read_and_the_press_opens_a_clock() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        src.contains("var HOLD_MS = dur(cols || el, \"--hold-ms\", 250);"),
        "the threshold comes out of the sheet (§ 5.5, § 2)"
    );
    for needle in [
        "holdTimer = setTimeout(begin, wait)",
        "function begin()",
        ", gesture = ",
    ] {
        assert!(
            src.contains(needle),
            "the press opens a clock instead of a take: {needle}"
        );
    }
}

#[test]
fn a_tap_toggles_the_dock_on_the_document_and_tells_nobody() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        src.contains("document.documentElement") && src.contains("data-dock-open"),
        "the opening lives on <html>, outside the LiveView container (OR-F5)"
    );
    assert!(
        !src.contains("pushEvent(\"dock\""),
        "a toggle with no semantics has nothing to tell the colony (OR-F5)"
    );
    assert!(
        !src.contains("dock_seen"),
        "and there is no seen-marker anywhere (OR-F4)"
    );
    assert!(
        !src.contains("__displayDock"),
        "no client-side dock state survives a reload: the profile decides"
    );
    // The second way in, and the check that made it work on the device.
    assert!(
        src.contains("function onClick()") && src.contains("if (gesture !== \"\") return;"),
        "a click that follows a gesture which already ended by pointer is not a \
         second press -- and a press that spoke also ends in a click (§ 5.5)"
    );
    assert!(
        src.contains("btn.addEventListener(\"click\", onClick)"),
        "and it is actually wired"
    );
    // A click is the END of a gesture, and under `touch-action: none` with a
    // captured pointer it can be the only end there is: the `pointerup` goes
    // missing. If the click only switched the dock, the threshold timer would
    // still be running and would open a microphone nothing ever releases.
    let click = src
        .split("function onClick() {")
        .nth(1)
        .expect("the hook has an `onClick`")
        .split("tapDock();")
        .next()
        .unwrap();
    for needle in [
        "clearTimeout(holdTimer)",
        "pressed = false;",
        "ready = false;",
    ] {
        assert!(
            click.contains(needle),
            "the click closes the press it belongs to before it switches: {needle} in {click}"
        );
    }
}

#[test]
fn a_hold_says_so_to_both_halves_of_the_system() {
    if !library_ships() {
        return;
    }
    let src = script();
    let begin = src
        .split("function begin() {")
        .nth(1)
        .expect("the hook has a `begin`")
        .split("function up()")
        .next()
        .unwrap();
    assert!(
        begin.contains("frame({ type: \"hold\" })"),
        "the voice cell hears the hold: {begin}"
    );
    // § 5.6: the event is `hold` and it carries NOTHING. Which window a hold
    // reaches is the SCREEN's knowledge (§ 5.4, § 8.5), not the browser's --
    // the hook used to name the chat by `topic`, and a `topic` a client sends
    // with a hold is no longer read at all (S-088).
    assert!(
        begin.contains("pushEvent(\"hold\", {})"),
        "and the curator hears the event `hold`, with no field on it: which \
         window it lands on is step 3's business, by TOPIC and not by owner \
         path (§ 5.4, § 5.6, S-088): {begin}"
    );
    assert!(
        !src.contains("pushEvent(\"touch\""),
        "the struck `touch` event is gone from the hook for good: it is not one of the six triggers of § 4.1"
    );
}

#[test]
fn losing_the_page_switches_nothing() {
    if !library_ships() {
        return;
    }
    let src = script();
    // The end marker is measured, not assumed: the first `holding = false;`
    // is where the tap branch ends. An earlier marker (`"\\n      }"`) never
    // occurred in the Python source at all, and the "slice" was the whole
    // rest of the file -- 131355 characters, in which every needle holds.
    let up = src
        .split("function up() {")
        .nth(1)
        .expect("the hook has an `up`")
        .split("holding = false;")
        .next()
        .unwrap();
    assert!(
        up.len() < 3000,
        "the slice is the tap branch, not the rest of the file: {} characters",
        up.len()
    );
    assert!(
        up.contains("byPointer"),
        "only a finger or a mouse toggles a dock -- the space key has none: {up}"
    );
    assert!(
        src.contains("function onBlur() { up(); }") && src.contains("\"visibilitychange\""),
        "blur and a hidden page still release, and land in `up` with no press behind them"
    );
}
