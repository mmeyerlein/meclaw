//! GH #679 -- the windows wear the attributes the motion reads.
//!
//! The curator writes a rung, a level, an age, a since and a score on every window
//! (`display-hive.md` § 3.1); the sheet's ladder, its enter and leave keyframes and its
//! presence rules read them off the markup. So the templates have to render them: the
//! prose view is a window like the three tree windows, the windows carry
//! `data-since`/`data-score`/`data-tone`/`data-pinned`, the overlay its age, and the
//! root the ground the operator chose. No script enters with any of it beyond the
//! components `SCRIPTED` names (GH #696), and the vocabulary fingerprint moves exactly
//! once.
//!
//! The word on a window used to be `data-state` and used to be the curator's; since the
//! contract of § 4.17 it is `data-rung`, and `state` is only what an application may say
//! about itself (`urgent`/`hidden`). The names live in `ATTRS` (§ 6), and this lock
//! reads them from there rather than spelling them a second time.
//!
//! Skips when `python3` is absent or the templates do not ship, like every
//! other interpreter guard in this tree (R2b).

mod support;

use std::process::Command;

use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, library_ships, repo, window_id};

const SHEET: &str = "templates/display/compose/display-dna.css";

/// The fingerprint `display@2.1.0` shipped with. The wave moves it once.
const VOCAB_BEFORE: &str = "e35405598319";

/// One output, named and complete: `display_type` is mandatory and `default_screen` has
/// to name an entry of `screens` (§ 4.7), so there is no such thing as a pass without a
/// profile any more.
fn params(ground: Option<&str>) -> Value {
    let mut p = json!({"screens": {"tv": {"display_type": "tv", "viewing_distance_m": 3.0}},
                       "default_screen": "tv"});
    if let Some(word) = ground {
        p["ground"] = json!(word);
    }
    p
}

/// The shipped script's constants, asked of the script itself: the
/// fingerprint, the five templates, whether the sheet carries a script, the
/// components, the names that may carry a script (GH #696) and the attribute
/// names of § 6. `None` when there is no `python3` on this host.
fn probe() -> Option<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps({'vocab': m.VOCAB, 'shell': m.SHELL_TEMPLATE,\n\
                               'prose': m.PROSE_TEMPLATE, 'pane': m.PANE_TEMPLATE,\n\
                               'panel': m.PANEL_TEMPLATE, 'overlay': m.OVERLAY_TEMPLATE,\n\
                               'kit_css': m.KIT_CSS, 'components': m.components(),\n\
                               'attrs': m.ATTRS, 'scripted': list(m.SCRIPTED)}))",
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

fn prose_view(view_id: &str, title: &str, body: &str) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": "main", "ord": 0,
        "kind": "prose", "content": json!({"title": title, "body": body}).to_string(),
        "components": "[]", "ttl_ms": 0, "updated_at": 1,
    })
}

fn schema_of(probe: &Value, name: &str) -> Value {
    probe["components"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("components() defines {name}"))["prop_schema"]
        .clone()
}

/// A prose view is a window: the pass writes the curator's values on the wrapper
/// ITSELF -- a prose row has no tree to hang them on -- and the template renders them
/// under the names `ATTRS` declares.
#[test]
fn a_prose_view_is_a_window_now() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let mut screen = Screen::new(params(None));
    screen.write(prose_view("p", "Weather", "Sunny"), 100_000);
    let oid = window_id("alex", "p");
    let props = screen
        .props(&oid)
        .unwrap_or_else(|| panic!("the prose wrapper stands: {:?}", screen.held));
    assert_eq!(props["title"], "Weather");
    assert_eq!(props["body"], "Sunny");
    for key in ["rung", "level", "age", "since", "score"] {
        assert!(
            props[key].as_str().is_some(),
            "the prose wrapper carries `{key}`: {props}"
        );
    }
    assert!(
        ["hidden", "ambient", "relevant", "focus", "urgent"]
            .contains(&props["rung"].as_str().unwrap_or("")),
        "and the rung is one of the five (§ 4.17): {props}"
    );
    assert_eq!(props["age"], "fresh", "in the pass it arrived in (§ 4.35)");
    assert_eq!(
        props["since"], "100000",
        "and `since` is the touch that put it there"
    );
    assert!(
        props["state"].is_null(),
        "the curator's word is `rung` now; `state` stays the app's: {props}"
    );
    let template = probe["prose"].as_str().expect("PROSE_TEMPLATE");
    for key in ["rung", "level", "age"] {
        let attr = probe["attrs"][key].as_str().expect("an attribute name");
        assert!(
            template.contains(&format!("{attr}=\"{{{{{key}}}}}\"")),
            "PROSE_TEMPLATE renders {attr}: {template}"
        );
    }
    for attr in ["data-since=\"{{since}}\"", "data-score=\"{{score}}\""] {
        assert!(
            template.contains(attr),
            "PROSE_TEMPLATE renders {attr}: {template}"
        );
    }
    assert!(
        !template.contains("data-state=") && !template.contains("data-plane="),
        "and neither struck name: {template}"
    );
    assert!(
        template.contains("display-pane-title"),
        "the title is the one slot presence scales, no fixed kicker: {template}"
    );
}

/// The two windows with a title render since, score, tone and pinned; the
/// overlay renders its age beside its rung.
#[test]
fn a_window_renders_since_score_tone_and_pinned_and_an_overlay_its_age() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    for name in ["pane", "panel"] {
        let template = probe[name].as_str().expect("a template");
        for attr in [
            "data-since=\"{{since}}\"",
            "data-score=\"{{score}}\"",
            "data-tone=\"{{tone}}\"",
            "data-pinned=\"{{pinned}}\"",
        ] {
            assert!(template.contains(attr), "{name} renders {attr}: {template}");
        }
    }
    let overlay = probe["overlay"].as_str().expect("OVERLAY_TEMPLATE");
    for key in ["rung", "age"] {
        let attr = probe["attrs"][key].as_str().expect("an attribute name");
        assert!(
            overlay.contains(&format!("{attr}=\"{{{{{key}}}}}\"")),
            "the overlay renders {attr}: {overlay}"
        );
    }
    for attr in ["data-since=\"{{since}}\"", "data-score=\"{{score}}\""] {
        assert!(
            overlay.contains(attr),
            "the overlay renders {attr}: {overlay}"
        );
    }
    assert_eq!(schema_of(&probe, "display-overlay")["age"], "text");
}

/// `params.ground` is the operator's word for the sheet's night variant: the
/// root renders it, `night` is night and anything else is day.
#[test]
fn the_ground_is_a_knob_of_the_screen() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let shell = probe["shell"].as_str().expect("SHELL_TEMPLATE");
    assert!(
        shell.contains("data-ground=\"{{ground}}\""),
        "the root element wears the ground: {shell}"
    );
    let schema = schema_of(&probe, "display-shell");
    // What the root actually carries since § 6.1. The bar and the weights left it with
    // the screen state (§ 3.1): one state per member, in the store, not on an output.
    for key in [
        "ground",
        "due",
        "exit",
        "screen_name",
        "default_screen",
        "inputs",
        "scale",
    ] {
        assert_eq!(
            schema[key], "text",
            "display-shell declares `{key}`: {schema}"
        );
    }
    for struck in ["focus", "weights", "judged_at", "screen", "profile"] {
        assert!(
            schema[struck].is_null(),
            "`{struck}` left the root: {schema}"
        );
    }
    let ground_of = |word: Option<&str>| {
        let mut screen = Screen::new(params(word));
        screen.pass(json!({"kind": "stroke"}), 1000);
        screen
            .props("display.root")
            .expect("the root stands")
            .get("ground")
            .cloned()
            .expect("the root wears a ground")
    };
    assert_eq!(ground_of(Some("night")), "night");
    assert_eq!(ground_of(Some("dusk")), "day", "an unknown word is day");
    assert_eq!(ground_of(None), "day");
}

/// The motion is the sheet's, and what is not the sheet's is NAMED. Until
/// 2.3.0 the lock counted to one; a number is not a decision. Two components
/// carry a script now -- the OS mark, whose hook is the gesture, and the
/// shell, whose hook is the screen's own motion -- and nothing else may
/// (GH #696).
#[test]
fn no_script_enters_with_the_motion() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    for name in ["prose", "pane", "panel", "overlay", "kit_css"] {
        let text = probe[name].as_str().expect("a string");
        assert!(!text.contains("<script"), "{name} carries no script");
        assert!(
            !text.contains("javascript:"),
            "{name} carries no javascript: url"
        );
    }
    let allowed = ["display-os", "display-shell"];
    let with_js: Vec<&str> = probe["components"]
        .as_array()
        .expect("a list")
        .iter()
        .filter(|c| c["prop_schema"].get("client_js").is_some())
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    assert!(!with_js.is_empty(), "the screen brings its own motion");
    for name in &with_js {
        assert!(
            allowed.contains(name),
            "{name} brought a script nobody named; the list is {allowed:?}"
        );
    }
    // The same set, not the same order: `SCRIPTED` is a list of names and
    // `components()` is a list of definitions, and neither owes the other its
    // sequence.
    let mut named: Vec<&str> = probe["scripted"]
        .as_array()
        .expect("SCRIPTED")
        .iter()
        .map(|v| v.as_str().unwrap_or(""))
        .collect();
    let mut carries = with_js.clone();
    named.sort_unstable();
    carries.sort_unstable();
    assert_eq!(
        named, carries,
        "SCRIPTED names exactly what carries a script"
    );
}

/// The wave moves the vocabulary fingerprint once: away from what 2.1.0
/// shipped, and to one value that two probes agree on.
#[test]
fn the_vocabulary_moved_once_for_the_curator() {
    if !library_ships() {
        return;
    }
    let Some(first) = probe() else {
        return;
    };
    let second = probe().expect("python3 answered once already");
    let vocab = first["vocab"].as_str().expect("VOCAB");
    assert_ne!(
        vocab, VOCAB_BEFORE,
        "the templates changed, so the fingerprint did"
    );
    assert_eq!(first["vocab"], second["vocab"], "and it is constant");
}

/// `hidden` is `display: none` -- unless the window is on its way out, in
/// which case the leave keyframe plays first.
#[test]
fn hidden_lets_a_leaving_window_play_out() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet ships");
    assert!(
        sheet.contains("[data-rung=\"hidden\"]:not([data-age=\"leaving\"])"),
        "the hidden rule spares a leaving window"
    );
    // The two rungs the canvas draws (spec 2.2, since 2.3.0): ambient and
    // relevant are tiles, and a tile has no title.
    for state in ["focus", "urgent"] {
        let rule =
            format!("[data-rung=\"{state}\"] :is(.display-pane-title, .display-panel-title)");
        assert!(sheet.contains(&rule), "presence follows the rung: {rule}");
    }
    for state in ["ambient", "relevant"] {
        let rule =
            format!("[data-rung=\"{state}\"] :is(.display-pane-title, .display-panel-title)");
        assert!(
            !sheet.contains(&rule),
            "nothing {state} is drawn on the canvas: {rule}"
        );
    }
    assert!(sheet.contains("[data-tone=\"accent\"]"), "tone: accent");
    assert!(sheet.contains("[data-tone=\"muted\"]"), "tone: muted");
}
