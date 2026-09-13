//! GH #679 -- the windows wear the attributes the motion reads.
//!
//! The curator (`compose.curate`) writes a state, an age, a since and a score
//! on every window; the sheet's ladder, its enter and leave keyframes and its
//! presence rules read them off the markup. So the templates have to render
//! them: the prose view becomes a window like the three tree windows, the
//! windows carry `data-since`/`data-score`/`data-tone`/`data-pinned`, the
//! overlay its age, and the root the ground the operator chose. No script
//! enters with any of it, and the vocabulary fingerprint moves exactly once.
//!
//! Skips when `python3` is absent or the templates do not ship, like every
//! other interpreter guard in this tree (R2b).

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const SHEET: &str = "templates/display/compose/display-dna.css";

/// The fingerprint `display@2.1.0` shipped with. The wave moves it once.
const VOCAB_BEFORE: &str = "e35405598319";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The shipped script's constants, asked of the script itself: the
/// fingerprint, the five templates, whether the sheet carries a script, and
/// the components. `None` when there is no `python3` on this host.
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
                               'kit_css': m.KIT_CSS, 'components': m.components()}))",
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

/// One read pass over a table of `views` with the display holding nothing
/// (a bootstrap), under `params`. The calls of the bundle it answers.
fn read_pass(views: &[Value], params: Value) -> Option<Vec<Value>> {
    let doc = json!({
        "params": params,
        "body": {"messages": []},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": views, "define": [], "now": 1000}).to_string(),
            },
        }},
    });
    let mut child = Command::new("python3")
        .arg(repo(COMPOSE))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document reaches the script");
    let out = child.wait_with_output().expect("the script ends");
    assert!(
        out.status.success(),
        "compose.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    // Since the due clock (GH #679) a read pass may answer with the patch AND
    // up to two timer orders beside it; the patch is what the display gets.
    let emissions = match answer {
        Value::Array(list) => list,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    };
    let patch = emissions
        .iter()
        .find(|e| e["header"]["route"] == "patch")
        .expect("a bundle for the display");
    Some(
        patch["messages"]
            .as_array()
            .expect("a bundle has messages")
            .iter()
            .map(|turn| {
                meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                    .expect("a call is JSON")
            })
            .collect(),
    )
}

fn prose_view(view_id: &str, title: &str, body: &str) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": "main", "ord": 0,
        "kind": "prose", "content": json!({"title": title, "body": body}).to_string(),
        "components": "[]", "ttl_ms": 0, "updated_at": 1,
    })
}

fn created(calls: &[Value], id: &str) -> Value {
    calls
        .iter()
        .find(|c| c["op"] == "object.create" && c["id"] == id)
        .unwrap_or_else(|| panic!("{id} is created"))
        .clone()
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

/// A prose view is a window: the pass writes state, age, since and score on
/// it, and its template renders them where the sheet reads them -- with one
/// title slot the presence rules scale.
#[test]
fn a_prose_view_is_a_window_now() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let calls = read_pass(&[prose_view("p", "Weather", "Sunny")], json!({}))
        .expect("python3 answered once already");
    let wrapper = created(&calls, "view.alex.p");
    assert_eq!(wrapper["component"], "display-view-prose");
    for key in ["state", "age", "since", "score"] {
        assert!(
            wrapper["props"].get(key).is_some(),
            "the prose wrapper carries `{key}`: {}",
            wrapper["props"]
        );
    }
    let template = probe["prose"].as_str().expect("PROSE_TEMPLATE");
    for attr in [
        "data-state=\"{{state}}\"",
        "data-age=\"{{age}}\"",
        "data-since=\"{{since}}\"",
        "data-score=\"{{score}}\"",
    ] {
        assert!(
            template.contains(attr),
            "PROSE_TEMPLATE renders {attr}: {template}"
        );
    }
    assert!(
        template.contains("display-pane-title"),
        "the title is the one slot presence scales, no fixed kicker: {template}"
    );
    let schema = schema_of(&probe, "display-view-prose");
    for (key, ty) in [
        ("state", "text"),
        ("age", "text"),
        ("since", "text"),
        ("score", "text"),
        ("context", "text"),
        ("relevance", "text"),
        ("class", "text"),
        ("pinned", "boolean"),
        ("relevant_until", "int"),
        ("judged_relevance", "text"),
        ("judged_hidden", "boolean"),
    ] {
        assert_eq!(
            schema[key], ty,
            "display-view-prose declares `{key}`: {schema}"
        );
    }
}

/// The two windows with a title render since, score, tone and pinned; the
/// overlay renders its age beside its state.
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
    for attr in [
        "data-state=\"{{state}}\"",
        "data-age=\"{{age}}\"",
        "data-since=\"{{since}}\"",
        "data-score=\"{{score}}\"",
    ] {
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
    for key in ["ground", "focus", "weights", "judged_at", "due"] {
        assert_eq!(
            schema[key], "text",
            "display-shell declares `{key}`: {schema}"
        );
    }
    let night = read_pass(&[], json!({"ground": "night"})).expect("python3");
    assert_eq!(created(&night, "display.root")["props"]["ground"], "night");
    let dusk = read_pass(&[], json!({"ground": "dusk"})).expect("python3");
    assert_eq!(created(&dusk, "display.root")["props"]["ground"], "day");
    let unset = read_pass(&[], json!({})).expect("python3");
    assert_eq!(created(&unset, "display.root")["props"]["ground"], "day");
}

/// The motion is the sheet's: no script comes with it. Exactly one component
/// of the scope carries `client_js`, and it is the microphone that had it.
#[test]
fn no_script_enters_with_the_motion() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    for name in ["shell", "prose", "pane", "panel", "overlay", "kit_css"] {
        let text = probe[name].as_str().expect("a string");
        assert!(!text.contains("<script"), "{name} carries no script");
        assert!(
            !text.contains("javascript:"),
            "{name} carries no javascript: url"
        );
    }
    let with_js: Vec<&str> = probe["components"]
        .as_array()
        .expect("a list")
        .iter()
        .filter(|c| c["prop_schema"].get("client_js").is_some())
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        with_js,
        vec!["display-mic"],
        "one component carries client_js"
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
        sheet.contains("[data-state=\"hidden\"]:not([data-age=\"leaving\"])"),
        "the hidden rule spares a leaving window"
    );
    for state in ["ambient", "relevant", "focus", "urgent"] {
        let rule =
            format!("[data-state=\"{state}\"] :is(.display-pane-title, .display-panel-title)");
        assert!(sheet.contains(&rule), "presence follows the rung: {rule}");
    }
    assert!(sheet.contains("[data-tone=\"accent\"]"), "tone: accent");
    assert!(sheet.contains("[data-tone=\"muted\"]"), "tone: muted");
}
