//! The two browser gestures the screen answers itself (display-hive.md § 5.6).
//!
//! Which window stands open is display hygiene: it is nobody's turn and nobody's
//! message. A tap on a tile and a hold on the OS mark therefore become a read pass
//! without a write -- the shape of a clock strike -- with the mark on the request, and
//! the mark rides the plan into the curator as the ONE event of § 4.1. A button inside
//! an application's own tree is the application's business and leaves on the `event`
//! lane as it always did.
//!
//! The two names are `tap` and `hold` and nothing else (§ 2, struck words): a tap names
//! the window it hit, a hold names NOTHING -- which window a hold opens is the screen's
//! own knowledge (§ 5.4, § 8.5), so a `topic` a client sends with it is not read (S-088).
//!
//! The script runs the way a `code` cell runs it: as a subprocess, with the pass's
//! document on stdin.

mod support;

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};
use support::{
    COMPOSE, Screen, component_view, library_ships, pane, pane_id, raw, repo, window_id,
};

/// One `event` pass, straight off the browser.
fn event_pass(name: &str, value: Value) -> Vec<Value> {
    raw(&json!({
        "params": {},
        "body": {"event": {"name": name, "value": value}},
        "envelope": {"header": {"hop": {"route": "event"}}},
    }))
}

/// One `views` pass: the store answered the read the absorbed gesture asked for.
fn views_pass(request: &Value, rows: &Value) -> Vec<Value> {
    raw(&json!({
        "params": {},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-select",
            "text": rows.to_string(),
        }]},
        "envelope": {"header": {
            "hop": {"operation": "select"},
            "context": {"display_origin": "views", "display_request": request.to_string()},
        }},
    }))
}

/// The request one emission carries, parsed.
fn request_of(emission: &Value) -> Value {
    meclaw_core::serde_json::from_str(
        emission["header"]["display_request"]
            .as_str()
            .expect("a request"),
    )
    .expect("the request is JSON")
}

/// The plan one emission carries, parsed.
fn plan_of(emission: &Value) -> Value {
    meclaw_core::serde_json::from_str(
        emission["header"]["display_views"]
            .as_str()
            .expect("a plan"),
    )
    .expect("the plan is JSON")
}

/// One `event` pass, keeping what the script said on stderr.
fn event_pass_with_stderr(name: &str, value: Value) -> (Vec<Value>, String) {
    let doc = json!({
        "params": {},
        "body": {"event": {"name": name, "value": value}},
        "envelope": {"header": {"hop": {"route": "event"}}},
    });
    let mut child = Command::new("python3")
        .arg(repo(COMPOSE))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document reaches the script");
    let out = child.wait_with_output().expect("the script ends");
    assert!(out.status.success(), "compose.py failed");
    let answer: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    let emissions = match answer {
        Value::Array(list) => list,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    };
    (emissions, String::from_utf8_lossy(&out.stderr).to_string())
}

/// `components()` as the shipped script defines them, asked of the script.
fn probe() -> Vec<Value> {
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
        .expect("python3 runs");
    assert!(
        out.status.success(),
        "compose.py did not load:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("components() is JSON");
    v.as_array().expect("a list").clone()
}

fn template_of(all: &[Value], name: &str) -> String {
    all.iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` is not defined"))["template"]
        .as_str()
        .expect("a template")
        .to_string()
}

/// A tap and a hold do not leave the hive: they become a read pass with the mark on it.
/// Everything else still leaves as an `event`.
#[test]
fn a_tap_and_a_hold_are_absorbed_and_a_button_is_not() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let node = pane_id("chat", "chat");
    let window = window_id("alex", "chat");
    let tap = event_pass("tap", json!({"for": node}));
    assert_eq!(tap.len(), 1, "one emission and no more: {tap:?}");
    assert_eq!(tap[0]["header"]["route"], "views", "{tap:?}");
    let request = request_of(&tap[0]);
    assert_eq!(request["tick"], true, "it is the tick's pass: {request}");
    assert_eq!(
        request["tap"],
        window.as_str(),
        "and it names the WINDOW, not the node the finger landed on (§ 2 Id): {request}"
    );

    // The hold carries nothing at all (§ 5.4): which window it opens is the screen's
    // knowledge, not the client's.
    let hold = event_pass("hold", json!({}));
    assert_eq!(hold.len(), 1, "one emission and no more: {hold:?}");
    assert_eq!(hold[0]["header"]["route"], "views", "{hold:?}");
    let bare = request_of(&hold[0]);
    assert_eq!(bare["hold"], true, "{bare}");
    assert_eq!(bare["tick"], true, "{bare}");

    // S-088: a client that sends a topic with the hold anyway is not read. Compared
    // against the bare request, so a topic that leaked through in ANY shape shows.
    let chatty = request_of(&event_pass("hold", json!({"topic": "chat"}))[0]);
    assert_eq!(
        chatty, bare,
        "a topic sent with the hold is not read (S-088): {chatty}"
    );

    // A button on an application's own tree is not the screen's business and leaves as
    // it always did.
    let button = event_pass("action", json!({"for": node}));
    assert_eq!(button[0]["header"]["route"], "event", "{button:?}");
    assert_eq!(button[0]["owner"], "alex", "with its addressee: {button:?}");
}

/// The mark rides the whole way: the views pass turns it into the ONE event of § 4.1,
/// and that is what the read pass reads.
#[test]
fn the_mark_becomes_the_one_event_of_the_pass() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let out = views_pass(
        &json!({"tick": true, "tap": pane_id("chat", "chat")}),
        &json!([]),
    );
    assert_eq!(out[0]["header"]["route"], "read", "{out:?}");
    let plan = plan_of(&out[0]);
    assert_eq!(
        plan["event"],
        json!({"kind": "tap", "for": window_id("alex", "chat")}),
        "a tap names its window: {plan}"
    );

    let out = views_pass(&json!({"tick": true, "hold": true}), &json!([]));
    let plan = plan_of(&out[0]);
    assert_eq!(
        plan["event"],
        json!({"kind": "hold"}),
        "a hold names nothing: {plan}"
    );

    // And a pass that nothing marked is a stroke -- the clock's own minute (§ 4.1).
    let plan = plan_of(&views_pass(&json!({"tick": true}), &json!([]))[0]);
    assert_eq!(plan["event"], json!({"kind": "stroke"}), "{plan}");
}

/// An absorbed gesture never asks the judge, even with the judge switched on: the hand
/// is not a content change, so it is no app touch (§ 4.3, § 4.8 d/e).
///
/// The counter-probe in the middle is what makes the rest mean anything. The judge is
/// braked for `judge_min_interval_ms` after every question, and the first pass here asks
/// one -- so a second pass taken too soon is silent whatever the finger does, and a test
/// taken there would measure the brake instead of the property. This one steps past the
/// brake and shows both answers on the same scene.
#[test]
fn an_absorbed_gesture_never_asks_the_judge() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let scene = |relevance: &str| {
        component_view(
            "a",
            "main",
            pane(
                "a",
                json!({"context": "work", "relevance": relevance, "topic": "chat"}),
            ),
        )
    };
    let mut screen = Screen::new(json!({"judge": "on"}));
    screen.write(scene("0.9"), 1000);
    let asks = |screen: &Screen| !screen.lane("judge").is_empty();
    assert!(asks(&screen), "the window's arrival asks the judge");

    // `judge_min_interval_ms` is 3000 and the pass above asked at 1000, so everything
    // below runs well past the brake.
    let window = window_id("alex", "a");
    screen.write(scene("0.4"), 9000);
    assert!(
        asks(&screen),
        "the counter-probe: a content change asks the judge at 9000"
    );

    screen.pass(json!({"kind": "tap", "for": window}), 9500);
    assert!(!asks(&screen), "a tap is no question");
    screen.pass(json!({"kind": "hold"}), 9600);
    assert!(!asks(&screen), "and neither is a hold");
    // Both did reach the curator all the same: the hold found the window with
    // `topic: chat` (§ 5.4, § 8.5) and touched it.
    assert_eq!(
        screen.curator(&window, "since"),
        json!(9600),
        "the hold touched the chat: {}",
        screen.screen_state()
    );
}

/// The names the client sends and the names the absorber listens for are ONE pair of
/// names (`TAP_EVENT`, `HOLD_EVENT`): the tile's template interpolates the first, so
/// renaming it can never leave the tile shouting at a door that no longer opens.
///
/// Read out of the rendered markup and put straight into an `event` pass -- a spelling
/// mismatch would send the tap out of the hive as an ordinary application event instead
/// of absorbing it, silently. The mark's half is read out of the shipped client script,
/// where the hold is pushed with an EMPTY payload (§ 5.4, S-088).
#[test]
fn the_client_sends_the_names_the_absorber_listens_for() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The client script lives in a Python string, so every JS quote is `\"` on disk;
    // read it the way the browser gets it.
    let script = std::fs::read_to_string(repo(COMPOSE))
        .expect("the script ships")
        .replace("\\\"", "\"");
    assert!(
        script.contains("pushEvent(\"hold\", {})"),
        "the mark pushes the hold under its own name and with nothing in it"
    );
    let hold = event_pass("hold", json!({}));
    assert_eq!(
        hold[0]["header"]["route"], "views",
        "and the screen answers it itself: {hold:?}"
    );

    let tile = template_of(&probe(), "display-tile");
    let name = tile
        .split("phx-click=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the tile binds a click");
    assert!(
        !name.contains("{{"),
        "the binding is a name, not a slot: {name}"
    );
    assert!(
        tile.contains("phx-value-for=\"{{oid}}\""),
        "and the click carries the window's id: {tile}"
    );
    let out = event_pass(name, json!({"for": pane_id("chat", "chat")}));
    assert_eq!(
        out[0]["header"]["route"], "views",
        "the screen answers its own tile's event: {out:?}"
    );
}

/// A tap whose payload the screen cannot read leaves a line on stderr.
///
/// The gesture is absorbed, so a malformed one has no receipt, no dead letter and no
/// reply -- swallowing it silently would make a client defect invisible. One line, the
/// way the judge writes one when a verdict carries no JSON.
///
/// The hold has no counterpart here on purpose: it carries nothing to misread (S-088).
#[test]
fn a_tap_the_screen_cannot_read_says_so_on_stderr() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let (emissions, err) = event_pass_with_stderr("tap", json!({"for": "not an object id"}));
    assert!(
        emissions.is_empty(),
        "a payload the screen cannot read is absorbed: {emissions:?}"
    );
    assert!(
        err.contains("tap"),
        "and it leaves a line naming the gesture: {err:?}"
    );
}

/// A sentence a person typed names no view (OR-F46g).
///
/// An ordinary event leaves the hive addressed to whoever owns the window it came from,
/// and the address is read out of `event.value`. The catalogue writes exactly one
/// `phx-value-*` there -- `for` -- and a window's own button may name `id` beside it;
/// everything else in the payload is what the person did. Reading EVERY string of it made
/// the typed line an address: with an empty `for`,
/// `{"key": "Enter", "value": "view.mallory.evil/0"}` came back owned by `mallory`, view
/// `evil`. A filled `for` won that race by the alphabet, so the hole was open to anybody
/// who typed an id into a line whose own window had not been rendered yet.
///
/// Empty and not wrong is the right answer: `pass_event` puts the owner on the hop,
/// always, and an empty one fails every owner guard by construction, so the event
/// dead-letters (GH #459).
#[test]
fn a_typed_sentence_names_no_view() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let typed = json!({"for": "", "key": "Enter", "value": "view.mallory.evil/0"});
    let out = event_pass("say", typed);
    assert_eq!(out.len(), 1, "one event leaves the hive: {out:?}");
    assert_eq!(
        out[0]["header"]["owner"], "",
        "the sentence is not an address: {out:?}"
    );
    assert_eq!(out[0]["header"]["view_id"], "", "{out:?}");
    assert!(
        out[0].get("owner").is_none(),
        "and the body names nobody either: {out:?}"
    );

    // And the line that the screen DID write still addresses its window, with the very
    // same sentence beside it.
    let out = event_pass(
        "say",
        json!({"for": pane_id("chat", "a"), "key": "Enter",
               "value": "view.mallory.evil/0"}),
    );
    assert_eq!(out[0]["header"]["owner"], "alex", "{out:?}");
    assert_eq!(out[0]["header"]["view_id"], "chat", "{out:?}");
}
