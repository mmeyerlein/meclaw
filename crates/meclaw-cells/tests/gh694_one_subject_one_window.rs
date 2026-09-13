//! D1 -- one subject, one window (R-D4): a fresh window repeating another
//! application's `topic` yields to the standing one; windows of one owner
//! are never compared (R-D4a).
//!
//! The script runs the way a `code` cell runs it: as a subprocess, with the
//! read-pass document on stdin.

use std::io::Write;
use std::process::{Command, Stdio};

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

fn run(doc: &Value) -> Option<Vec<Value>> {
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
    let emissions = match answer {
        Value::Array(list) => list,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    };
    let patches: Vec<&Value> = emissions
        .iter()
        .filter(|e| e["header"]["route"] == "patch")
        .collect();
    assert!(patches.len() <= 1, "at most one patch: {emissions:?}");
    Some(match patches.first() {
        None => Vec::new(),
        Some(emission) => emission["messages"]
            .as_array()
            .expect("a bundle has messages")
            .iter()
            .map(|turn| {
                meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                    .expect("a call is JSON")
            })
            .collect(),
    })
}

fn read_pass_with(
    views: &[Value],
    objects: Option<&Value>,
    now: u64,
    params: Value,
) -> Option<Vec<Value>> {
    let messages = match objects {
        None => json!([]),
        Some(objs) => json!([{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": objs}).to_string(),
        }]),
    };
    let doc = json!({
        "params": params,
        "body": {"messages": messages},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": views, "define": [], "now": now}).to_string(),
            },
        }},
    });
    run(&doc)
}

fn read_pass(views: &[Value], objects: Option<&Value>, now: u64) -> Option<Vec<Value>> {
    read_pass_with(views, objects, now, json!({}))
}

fn apply(held: &mut Value, calls: &[Value]) {
    let list = held.as_array_mut().expect("the display holds a list");
    for c in calls {
        match c["op"].as_str().unwrap_or("") {
            "object.create" => list.push(json!({
                "id": c["id"], "parent": c["parent"], "ord": c["ord"],
                "component": c["component"], "props": c["props"],
            })),
            "object.update" => {
                let obj = list
                    .iter_mut()
                    .find(|o| o["id"] == c["id"])
                    .unwrap_or_else(|| panic!("an update names a held object: {}", c["id"]));
                for (k, v) in c["props"].as_object().expect("props") {
                    obj["props"][k] = v.clone();
                }
                if !c["parent"].is_null() {
                    obj["parent"] = c["parent"].clone();
                }
            }
            "object.move" => {
                let obj = list
                    .iter_mut()
                    .find(|o| o["id"] == c["id"])
                    .expect("a move names a held object");
                obj["parent"] = c["parent"].clone();
                obj["ord"] = c["ord"].clone();
            }
            "object.delete" => list.retain(|o| o["id"] != c["id"]),
            _ => {}
        }
    }
}

fn held_after(calls: &[Value]) -> Value {
    let mut held = json!([]);
    apply(&mut held, calls);
    held
}

fn bare_screen() -> Option<Value> {
    Some(held_after(&read_pass(&[], None, 1000)?))
}

fn written(calls: &[Value], id: &str) -> Option<Value> {
    calls
        .iter()
        .find(|c| (c["op"] == "object.create" || c["op"] == "object.update") && c["id"] == id)
        .map(|c| c["props"].clone())
}

fn owned_view(owner: &str, view_id: &str, tree: Value) -> Value {
    json!({
        "owner": owner, "view_id": view_id, "region": "main", "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    })
}

/// Two applications, one subject (R-D4): the standing window is touched and
/// the fresh one yields -- no tile, no score, and the judge is told why.
#[test]
fn a_fresh_window_yields_to_a_standing_one_on_the_same_topic() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let standing = owned_view(
        "ambient",
        "weather",
        json!({"component": "display-pane", "key": "c.w",
               "props": {"pane_id": "w", "context": "ambient",
                         "relevance": 0.3, "topic": "weather:berlin", "pinned": true}}),
    );
    let first =
        read_pass(std::slice::from_ref(&standing), Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);

    let card = owned_view(
        "v2v",
        "card",
        json!({"component": "display-pane", "key": "c.c",
               "props": {"pane_id": "c", "context": "conversation",
                         "relevance": 0.9, "topic": "weather:berlin"}}),
    );
    let second = read_pass(&[standing, card], Some(&held), 60_000).expect("python3");
    let fresh = written(&second, "view.v2v.card/c.c").expect("the card is created");
    assert_eq!(fresh["topic_dupe"], true, "the floor marks it: {fresh}");
    assert_eq!(
        fresh["score"], 0.0,
        "and it does not reach the canvas: {fresh}"
    );
    assert!(
        written(&second, "display.dock/tile.view.v2v.card~c.c").is_none(),
        "a repeated answer gets no tile of its own: {second:?}"
    );
    let older = written(&second, "view.ambient.weather/c.w").expect("the standing window");
    assert_eq!(
        older["since"], 60_000,
        "the standing window is touched instead: {older}"
    );
}

/// Two windows of the SAME owner are never compared (R-D4a): three timers
/// stay three timers.
#[test]
fn one_owner_may_say_the_same_thing_twice() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![
        owned_view(
            "ambient",
            "t1",
            json!({"component": "display-pane", "key": "c.a",
                   "props": {"pane_id": "a", "context": "ambient", "topic": "timer"}}),
        ),
        owned_view(
            "ambient",
            "t2",
            json!({"component": "display-pane", "key": "c.b",
                   "props": {"pane_id": "b", "context": "ambient", "topic": "timer"}}),
        ),
    ];
    let calls = read_pass(&views, Some(&base), 1000).expect("python3 answers");
    for id in ["view.ambient.t1/c.a", "view.ambient.t2/c.b"] {
        let props = written(&calls, id).unwrap_or_else(|| panic!("{id} is created"));
        assert_eq!(
            props["topic_dupe"], false,
            "{id} is not a duplicate: {props}"
        );
    }
}

/// The hints reach a prose window as well: `topic` and `modal` are written
/// on it, and a hint of the wrong shape is refused at the door.
#[test]
fn a_prose_window_carries_topic_and_modal() {
    if !library_ships() {
        return;
    }
    let prose = json!({
        "owner": "alex", "view_id": "p", "region": "main", "ord": 0,
        "kind": "prose",
        "content": json!({"title": "Weather", "body": "Sunny", "topic": "weather:berlin",
                          "modal": true}).to_string(),
        "components": "[]", "ttl_ms": 0, "updated_at": 1,
    });
    let calls = read_pass(&[prose], None, 1000).expect("python3 answers");
    let window = written(&calls, "view.alex.p").expect("the prose window");
    assert_eq!(window["topic"], "weather:berlin", "{window}");
    assert_eq!(window["modal"], true, "{window}");
    assert_eq!(window["topic_dupe"], false, "{window}");
    let source = std::fs::read_to_string(repo(COMPOSE)).expect("the script ships");
    for sentence in [
        "a prose \"topic\" is not a string",
        "a prose \"modal\" is not a boolean",
    ] {
        assert!(source.contains(sentence), "the door says: {sentence}");
    }
}

/// The standing window TAKES the answer (R-D4): with a third window on the
/// screen it is the standing weather, not the fresh card, that has the focus
/// -- the touch reaches the weights, not only `since`.
#[test]
fn the_standing_window_takes_the_answer() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let weather = owned_view(
        "ambient",
        "weather",
        json!({"component": "display-pane", "key": "c.w",
               "props": {"pane_id": "w", "context": "ambient",
                         "relevance": 0.5, "topic": "weather:berlin", "pinned": true}}),
    );
    let chat = owned_view(
        "chat",
        "chat",
        json!({"component": "display-pane", "key": "c.t",
               "props": {"pane_id": "t", "context": "conversation", "relevance": 0.6}}),
    );
    let first =
        read_pass(&[weather.clone(), chat.clone()], Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(&[weather.clone(), chat.clone()], Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);

    let card = owned_view(
        "v2v",
        "card",
        json!({"component": "display-pane", "key": "c.c",
               "props": {"pane_id": "c", "context": "conversation",
                         "relevance": 0.9, "topic": "weather:berlin"}}),
    );
    let third = read_pass(&[weather, chat, card], Some(&held), 3000).expect("python3");
    let standing = written(&third, "view.ambient.weather/c.w").expect("the standing window");
    assert_eq!(
        standing["state"], "focus",
        "the standing window takes the answer: {standing}"
    );
    let fresh = written(&third, "view.v2v.card/c.c").expect("the card");
    assert_eq!(fresh["topic_dupe"], true, "{fresh}");
    assert_eq!(
        fresh["state"], "hidden",
        "the card is not rendered: {fresh}"
    );
    assert!(
        written(&third, "display.dock/tile.view.v2v.card~c.c").is_none(),
        "and has no tile: {third:?}"
    );
}

/// The scene of spec 7.2: the member asks about the weather. The chat is
/// touched by the answer, and in the SAME pass a fresh card on the weather's
/// topic arrives. The standing weather takes the answer: it borrows the
/// card's relevance while the answer stands (OR-D-Bau-6a), its context is
/// the last touched one of that pass (6b) -- so the weather is large, the
/// chat is not, and the card is not drawn.
fn weather_scene(chat_touched: &str, with_card: bool) -> Vec<Value> {
    let mut views = vec![
        owned_view(
            "ambient",
            "weather",
            json!({"component": "display-pane", "key": "c.w",
                   "props": {"pane_id": "w", "context": "ambient",
                             "relevance": 0.3, "topic": "weather:berlin", "pinned": true}}),
        ),
        owned_view(
            "chat",
            "chat",
            json!({"component": "display-pane", "key": "c.t",
                   "props": {"pane_id": "t", "context": "conversation",
                             "relevance": 0.6, "touched": chat_touched}}),
        ),
    ];
    if with_card {
        views.push(owned_view(
            "v2v",
            "card",
            json!({"component": "display-pane", "key": "c.c",
                   "props": {"pane_id": "c", "context": "conversation",
                             "relevance": 0.7, "topic": "weather:berlin"}}),
        ));
    }
    views
}

#[test]
fn the_standing_window_borrows_the_answers_relevance() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let first = read_pass(&weather_scene("", false), Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(&weather_scene("", false), Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);
    // The answer: the chat is touched, and the card arrives, in one pass.
    let answered = read_pass(&weather_scene("3000", true), Some(&held), 3000).expect("python3");
    apply(&mut held, &answered);
    let weather = written(&answered, "view.ambient.weather/c.w").expect("the weather is updated");
    assert_eq!(
        weather["topic_relevance"], "0.7",
        "borrowed from the card: {weather}"
    );
    assert!(
        weather["score"].as_f64().unwrap_or(0.0) >= 0.6,
        "the weather scores with the borrowed relevance: {weather}"
    );
    assert_eq!(weather["state"], "focus", "and takes the canvas: {weather}");
    let chat = written(&answered, "view.chat.chat/c.t").expect("the chat is updated");
    assert_ne!(chat["state"], "focus", "the chat is not large: {chat}");
    let card = written(&answered, "view.v2v.card/c.c").expect("the card is created");
    assert_eq!(card["topic_dupe"], true, "{card}");
    assert_eq!(card["state"], "hidden", "{card}");
    assert!(
        written(&answered, "display.dock/tile.view.v2v.card~c.c").is_none(),
        "no tile for the card: {answered:?}"
    );
    // The next pass keeps it: the answer stands, nothing new was touched.
    let next = read_pass(&weather_scene("3000", true), Some(&held), 4000).expect("python3");
    apply(&mut held, &next);
    let list = held.as_array().unwrap();
    let state = |id: &str| {
        list.iter()
            .find(|o| o["id"] == id)
            .map(|o| o["props"]["state"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    assert_eq!(
        state("view.ambient.weather/c.w"),
        "focus",
        "the answer stands: {next:?}"
    );
    assert_ne!(state("view.chat.chat/c.t"), "focus");
}

/// The borrowed relevance fades like a verdict: `linger_ms + fade_ms` after
/// the rival touch it is gone, and the weather is its own 0.3 again.
#[test]
fn the_borrowed_relevance_fades_like_a_verdict() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let first = read_pass(&weather_scene("", false), Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(&weather_scene("", false), Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);
    let answered = read_pass(&weather_scene("3000", true), Some(&held), 3000).expect("python3");
    apply(&mut held, &answered);
    // linger 20 s + fade 120 s + one second after the rival touch.
    let later =
        read_pass(&weather_scene("3000", true), Some(&held), 3000 + 141_000).expect("python3");
    let weather = written(&later, "view.ambient.weather/c.w").expect("the weather is updated");
    assert_eq!(
        weather["topic_relevance"], "",
        "the borrowed relevance is gone: {weather}"
    );
    assert_ne!(
        weather["state"], "focus",
        "and the weather is no longer large: {weather}"
    );
}

/// The floor's weight of 1 for the last touched context fades like a verdict
/// (OR-D-Bau-8): `linger_ms + fade_ms` after that touch every context weighs
/// the default again, so a pinned weather that took an answer goes back to
/// its tile by itself -- and the clock is ordered for that moment, so it
/// goes without a tick.
#[test]
fn the_floors_weight_fades_like_a_verdict() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let weather_only = || vec![weather_scene("", false).remove(0)];
    let first = read_pass(&weather_scene("", false), Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(&weather_scene("", false), Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);
    let answered = read_pass(&weather_scene("3000", true), Some(&held), 3000).expect("python3");
    apply(&mut held, &answered);
    // The chat and the card go; the weather stands alone with the weight it
    // was given at 3000. Two passes: one lays the two back as leaving, the
    // next deletes them.
    let leave = read_pass(&weather_only(), Some(&held), 100_000).expect("python3");
    apply(&mut held, &leave);
    let gone = read_pass(&weather_only(), Some(&held), 101_000).expect("python3");
    apply(&mut held, &gone);
    let alone = read_pass(&weather_only(), Some(&held), 102_000).expect("python3");
    apply(&mut held, &alone);
    fn props(held: &Value, id: &str) -> Value {
        held.as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == id)
            .map(|o| o["props"].clone())
            .unwrap_or_else(|| panic!("{id} is held"))
    }
    assert_eq!(
        props(&held, "view.ambient.weather/c.w")["state"],
        "focus",
        "the weight of the touch still stands: {}",
        props(&held, "view.ambient.weather/c.w")
    );
    // A lone pinned window has no fade of its own to strike for; the one
    // moment left is the fade of its context's weight, and it is ordered.
    assert!(
        props(&held, "display.root")["due"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the clock is ordered for the weight's fade: {}",
        props(&held, "display.root")
    );
    // linger 20 s + fade 120 s after the touch at 3000, plus one.
    let faded = read_pass(&weather_only(), Some(&held), 3000 + 140_000 + 1).expect("python3");
    apply(&mut held, &faded);
    let weather = props(&held, "view.ambient.weather/c.w");
    assert_eq!(weather["state"], "hidden", "off the canvas: {weather}");
    assert_eq!(weather["rung"], "ambient", "{weather}");
    assert_eq!(
        weather["score"], 0.15,
        "0.5 x 0.3: the default weight again: {weather}"
    );
    let tile = props(&held, "display.dock/tile.view.ambient.weather~c.w");
    assert_eq!(tile["state"], "ambient", "{tile}");
    assert_eq!(tile["rank"], "0.15", "{tile}");
    assert_eq!(tile["on_canvas"], false, "{tile}");
}

/// The `at` of the one due order a read pass emits, or None.
fn due_at(views: &[Value], objects: &Value, now: u64) -> Option<String> {
    let doc = json!({
        "params": {},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": objects}).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({"views": views, "define": [], "now": now}).to_string(),
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
    let answer: Value =
        meclaw_core::serde_json::from_slice(&out.stdout).expect("the answer is JSON");
    let list = match answer {
        Value::Array(l) => l,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    };
    list.iter()
        .find(|e| e["header"]["route"] == "due" && e["op"] == "add")
        .and_then(|e| e["at"].as_str().map(str::to_string))
}

/// An epoch in milliseconds as the clock's `at`: RFC 3339, UTC, rounded up.
fn iso_z(ms: u64) -> String {
    let secs = ms.div_ceil(1000);
    format!(
        "1970-01-01T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// The clock reads the borrowed relevance too: a standing window that took
/// an answer crosses the midpoint and the bar on the borrowed number, and
/// the due order says so.
#[test]
fn the_clock_reads_the_borrowed_relevance() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let weather = |with_card: bool| {
        let mut views = vec![owned_view(
            "ambient",
            "weather",
            json!({"component": "display-pane", "key": "c.w",
                   "props": {"pane_id": "w", "context": "ambient",
                             "relevance": 0.3, "topic": "weather:berlin"}}),
        )];
        if with_card {
            views.push(owned_view(
                "v2v",
                "card",
                json!({"component": "display-pane", "key": "c.c",
                       "props": {"pane_id": "c", "context": "conversation",
                                 "relevance": 0.7, "topic": "weather:berlin"}}),
            ));
        }
        views
    };
    let first = read_pass(&weather(false), Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(&weather(false), Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);
    let answered = read_pass(&weather(true), Some(&held), 3000).expect("python3");
    apply(&mut held, &answered);
    for t in [4000, 5000] {
        let calls = read_pass(&weather(false), Some(&held), t).expect("python3");
        apply(&mut held, &calls);
    }
    // Alone with the borrowed 0.7 at weight 1: the midpoint 0.65 is crossed
    // at 3000 + 20 000 + 120 000 x (1 - 0.65 / 0.7) + 1 = 31 572 ms; on its
    // own 0.3 the score never crosses anything before the zero at 143 001.
    let at = due_at(&weather(false), &held, 6000).expect("a due order");
    assert_eq!(at, iso_z(31_572), "the crossing on the borrowed number");
}

/// The judge may overrule the borrowed relevance: a verdict's `relevance`
/// on the window wins against the mark, and the judge sees the mark.
#[test]
fn a_verdict_overrules_the_borrowed_relevance() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let first = read_pass(&weather_scene("", false), Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(&weather_scene("", false), Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);
    let answered = read_pass(&weather_scene("3000", true), Some(&held), 3000).expect("python3");
    apply(&mut held, &answered);
    let doc = json!({
        "params": {"judge": "on"},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": held}).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {
                "display_origin": "read",
                "display_views": json!({
                    "views": weather_scene("3000", true), "define": [], "now": 4000,
                    "verdict": {"focus": 0.3, "weights": {"ambient": 1.0},
                                "windows": [{"id": "view.ambient.weather/c.w", "relevance": 0.1}]}
                }).to_string(),
            },
        }},
    });
    let calls = run(&doc).expect("python3 answers");
    let weather = written(&calls, "view.ambient.weather/c.w").expect("the weather is updated");
    assert_eq!(weather["judged_relevance"], "0.1", "{weather}");
    assert_eq!(
        weather["score"], 0.1,
        "the verdict wins against the borrowed 0.7: {weather}"
    );
    assert_eq!(weather["state"], "hidden", "{weather}");
}

/// The judge is told what was borrowed.
#[test]
fn the_situation_carries_the_borrowed_relevance() {
    if !library_ships() {
        return;
    }
    let source = std::fs::read_to_string(repo(COMPOSE)).expect("the script ships");
    let situation = source
        .split("def situation(")
        .nth(1)
        .expect("situation()")
        .split("\ndef ")
        .next()
        .unwrap();
    assert!(
        situation.contains("\"topic_relevance\""),
        "the judge sees the borrowed relevance"
    );
}

/// The four lanes of one turn as the relay delivers them (partial, turn,
/// answer, sidecar), with the throwaway knobs (linger 3 s, fade 6 s), and
/// the chat's answer view reaching the screen one pass AFTER the card: the
/// answer's `touched` hint carries the answer's own epoch, which is before
/// the card's arrival, so the standing weather stays the last touched one.
/// Then, linger + fade after the rival touch, the canvas is empty.
#[test]
fn one_turn_over_four_lanes_lets_the_weather_answer_and_go() {
    if !library_ships() {
        return;
    }
    let knobs = json!({"linger_ms": 3000, "fade_ms": 6000});
    let weather = || {
        owned_view(
            "ambient",
            "weather",
            json!({"component": "display-pane", "key": "c.w",
                   "props": {"pane_id": "w", "context": "ambient",
                             "relevance": 0.3, "topic": "weather:berlin", "pinned": true}}),
        )
    };
    let chat = |relevance: f64, touched: &str| {
        owned_view(
            "chat",
            "chat",
            json!({"component": "display-pane", "key": "c.t",
                   "props": {"pane_id": "t", "context": "conversation", "topic": "chat",
                             "relevance": relevance, "touched": touched}}),
        )
    };
    let card = || {
        owned_view(
            "v2v",
            "card",
            json!({"component": "display-pane", "key": "c.c",
                   "props": {"pane_id": "c", "context": "conversation",
                             "relevance": 0.7, "topic": "weather:berlin"}}),
        )
    };
    let mut held = json!([]);
    let pass = |views: Vec<Value>, now: u64, held: &mut Value| {
        let calls = read_pass_with(&views, Some(held), now, knobs.clone()).expect("python3");
        apply(held, &calls);
    };
    pass(vec![], 100, &mut held);
    pass(vec![weather()], 1000, &mut held);
    pass(vec![weather()], 2000, &mut held);
    pass(vec![weather(), chat(0.8, "10000")], 10_000, &mut held); // partial
    pass(vec![weather(), chat(0.8, "11000")], 11_000, &mut held); // turn
    pass(
        vec![weather(), chat(0.8, "11000"), card()],
        12_500,
        &mut held,
    ); // sidecar first
    pass(
        vec![weather(), chat(0.6, "12000"), card()],
        13_000,
        &mut held,
    ); // the answer view, late
    pass(
        vec![weather(), chat(0.6, "12000"), card()],
        14_000,
        &mut held,
    );
    let props = |held: &Value, id: &str| {
        held.as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == id)
            .map(|o| o["props"].clone())
            .unwrap_or_else(|| panic!("{id} is held"))
    };
    let w = props(&held, "view.ambient.weather/c.w");
    assert_eq!(w["state"], "focus", "the weather answers: {w}");
    assert!(w["score"].as_f64().unwrap_or(0.0) >= 0.6, "{w}");
    let c = props(&held, "view.chat.chat/c.t");
    assert_ne!(c["state"], "focus", "the chat is not large: {c}");
    assert_eq!(
        c["since"], 12_000,
        "the answer's own epoch is the touch: {c}"
    );
    assert_eq!(props(&held, "view.v2v.card/c.c")["state"], "hidden");

    // linger + fade + 1 after the rival touch at 12 500: everything back in
    // the dock, the canvas empty.
    pass(
        vec![weather(), chat(0.6, "12000"), card()],
        12_500 + 9_001,
        &mut held,
    );
    for id in [
        "view.ambient.weather/c.w",
        "view.chat.chat/c.t",
        "view.v2v.card/c.c",
    ] {
        let p = props(&held, id);
        assert_eq!(p["state"], "hidden", "{id} is off the canvas: {p}");
    }
    assert_eq!(
        props(&held, "display.root")["focus"],
        0.0,
        "nothing is large"
    );
}
