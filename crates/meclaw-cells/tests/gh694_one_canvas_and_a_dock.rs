//! D1 -- one canvas and a dock: `aside` is drawn as the canvas, and an urgent
//! window stands above the focus instead of replacing it.
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

fn pane(pane: &str, props: Value) -> Value {
    let mut props = props;
    props["pane_id"] = json!(pane);
    json!({"component": "display-pane", "props": props, "key": format!("c.{pane}")})
}

fn component_view(view_id: &str, region: &str, tree: Value) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": region, "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    })
}

fn pane_id(view_id: &str, pane: &str) -> String {
    format!("view.alex.{view_id}/c.{pane}")
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

fn prose_view(view_id: &str, region: &str, title: &str, body: &str) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": region, "ord": 0,
        "kind": "prose", "content": json!({"title": title, "body": body}).to_string(),
        "components": "[]", "ttl_ms": 0, "updated_at": 1,
    })
}

fn state_of(calls: &[Value], id: &str) -> Option<String> {
    written(calls, id)?["state"].as_str().map(str::to_string)
}

fn tile_of(window: &str) -> String {
    format!("display.dock/tile.{}", window.replace('/', "~"))
}

/// The props an object holds after `calls`, whether the bundle wrote it or
/// the screen already had it.
fn held_props(held: &Value, calls: &[Value], id: &str) -> Value {
    written(calls, id).unwrap_or_else(|| {
        held.as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == id)
            .map(|o| o["props"].clone())
            .unwrap_or_else(|| panic!("{id} is held"))
    })
}

/// `aside` is accepted and drawn as the canvas (OR-D3): a window that named it
/// may take the focus like any other.
#[test]
fn a_window_in_aside_may_take_the_focus() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![prose_view("p", "aside", "Weather", "Sunny, 21 degrees")];
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let mut held = base.clone();
    apply(&mut held, &first);
    let second = read_pass(&views, Some(&held), 2000).expect("python3");
    let state = state_of(&second, "view.alex.p")
        .or_else(|| {
            held.as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == "view.alex.p")
                .and_then(|o| o["props"]["state"].as_str().map(str::to_string))
        })
        .expect("the prose view carries a state");
    assert_eq!(state, "focus", "aside is canvas now: {second:?}");
}

/// An urgent window does not lock the focus any more (OR-D2): it stands
/// ABOVE the focus, and the focus stays where it was.
#[test]
fn urgent_stands_above_the_focus_instead_of_replacing_it() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let calm = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.9})),
    )];
    let first = read_pass(&calm, Some(&base), 1000).expect("python3 answered once already");
    let mut held = base.clone();
    apply(&mut held, &first);
    let both = vec![
        calm[0].clone(),
        component_view(
            "t",
            "main",
            pane("t", json!({"context": "ambient", "state": "urgent"})),
        ),
    ];
    let second = read_pass(&both, Some(&held), 2000).expect("python3");
    apply(&mut held, &second);
    let list = held.as_array().unwrap();
    let state = |id: &str| {
        list.iter()
            .find(|o| o["id"] == id)
            .map(|o| o["props"]["state"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    assert_eq!(state(&pane_id("t", "t")), "urgent", "the timer rings");
    assert_eq!(
        state(&pane_id("a", "a")),
        "focus",
        "and the answer keeps the canvas: {second:?}"
    );
    let urgent_wrapper = list
        .iter()
        .find(|o| o["id"] == "view.alex.t")
        .expect("the urgent wrapper is held");
    let calm_wrapper = list
        .iter()
        .find(|o| o["id"] == "view.alex.a")
        .expect("the calm wrapper is held");
    assert!(
        urgent_wrapper["ord"].as_i64().unwrap_or(0) < calm_wrapper["ord"].as_i64().unwrap_or(0),
        "urgent sits above the focus on the canvas: {urgent_wrapper} / {calm_wrapper}"
    );
}

/// The empty screen: a clock and a weather window, both pinned, both under
/// the bar. The canvas is EMPTY -- a lowered bar gives them a rung, not the
/// focus -- and the dock holds their two tiles, each wearing its rung.
#[test]
fn the_empty_screen_is_an_empty_canvas_and_a_dock() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![
        component_view(
            "clock",
            "main",
            pane(
                "clock",
                json!({"context": "ambient", "relevance": 0.2, "pinned": true}),
            ),
        ),
        component_view(
            "weather",
            "main",
            pane(
                "weather",
                json!({"context": "ambient", "relevance": 0.25, "pinned": true}),
            ),
        ),
    ];
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);
    let second = read_pass(&views, Some(&held), 2000).expect("python3");
    apply(&mut held, &second);
    for (view, pane) in [("clock", "clock"), ("weather", "weather")] {
        let window = held_props(&held, &second, &pane_id(view, pane));
        assert_eq!(window["state"], "hidden", "not on the canvas: {window}");
        assert_eq!(window["rung"], "ambient", "but on the ladder: {window}");
        let tile = held_props(&held, &second, &tile_of(&pane_id(view, pane)));
        assert_eq!(tile["state"], "ambient", "the tile wears the rung: {tile}");
        assert_eq!(tile["on_canvas"], false, "{tile}");
    }
    let dock = held_props(&held, &second, "display.dock");
    assert_eq!(dock["count"], 2, "two tiles: {dock}");
    let root = held_props(&held, &second, "display.root");
    assert_eq!(
        root["focus"], 0.0,
        "the bar is lowered, nothing is large: {root}"
    );
}

/// A window that loses the focus to a better one leaves the canvas with one
/// `leaving` frame -- like a window that fell under the bar -- while its tile
/// stands and says `relevant`.
#[test]
fn a_window_that_loses_the_focus_leaves_the_canvas_and_keeps_its_tile() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let a = component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.9})),
    );
    let b = component_view(
        "b",
        "main",
        pane("b", json!({"context": "conversation", "relevance": 0.95})),
    );
    let first = read_pass(std::slice::from_ref(&a), Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);
    let settled = read_pass(std::slice::from_ref(&a), Some(&held), 2000).expect("python3");
    apply(&mut held, &settled);
    assert_eq!(
        held_props(&held, &settled, &pane_id("a", "a"))["state"],
        "focus",
        "a stands large first"
    );
    let arrives = read_pass(&[a.clone(), b.clone()], Some(&held), 3000).expect("python3");
    apply(&mut held, &arrives);
    let takes = read_pass(&[a, b], Some(&held), 4000).expect("python3");
    let a_now = written(&takes, &pane_id("a", "a")).expect("a is updated");
    assert_eq!(a_now["state"], "hidden", "a leaves the canvas: {a_now}");
    assert_eq!(
        a_now["age"], "leaving",
        "with the one leaving frame: {a_now}"
    );
    assert_eq!(
        a_now["rung"], "relevant",
        "and is still on the ladder: {a_now}"
    );
    let b_now = written(&takes, &pane_id("b", "b")).expect("b is updated");
    assert_eq!(b_now["state"], "focus", "b takes the canvas: {b_now}");
    apply(&mut held, &takes);
    let tile = held_props(&held, &takes, &tile_of(&pane_id("a", "a")));
    assert_eq!(tile["state"], "relevant", "the tile wears the rung: {tile}");
    assert_eq!(tile["on_canvas"], false, "{tile}");
    let drawn: Vec<&Value> = held
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| {
            o["id"].as_str().is_some_and(|id| id.starts_with("view."))
                && o["props"]["state"].as_str().is_some_and(|s| !s.is_empty())
        })
        .collect();
    assert!(
        drawn.iter().all(|o| {
            let s = o["props"]["state"].as_str().unwrap_or("");
            s == "focus" || s == "urgent" || s == "hidden"
        }),
        "the canvas knows three words only: {drawn:?}"
    );
}

/// A pinned window's own content change is no touch (D-15: pinned means the
/// tile stays, not that it asks for attention). The clock rewrites its time,
/// the weather its degrees -- and the canvas stays empty, because neither
/// woke the screen: the floor's weight goes to the last window that was
/// really touched, and a pinned window that only arrived was not.
#[test]
fn a_pinned_window_that_rewrites_itself_does_not_wake_the_screen() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let scene = |time: &str, degrees: &str| {
        vec![
            component_view(
                "clock",
                "main",
                pane(
                    "clock",
                    json!({"context": "ambient", "relevance": 0.3, "pinned": true, "title": time}),
                ),
            ),
            component_view(
                "weather",
                "main",
                pane(
                    "weather",
                    json!({"context": "ambient", "relevance": 0.3, "pinned": true, "title": degrees}),
                ),
            ),
        ]
    };
    let first = read_pass(&scene("12:00", "21"), Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);
    let second = read_pass(&scene("12:00", "21"), Some(&held), 2000).expect("python3");
    apply(&mut held, &second);
    // Thirty seconds on: the clock ticked, the weather changed.
    let third = read_pass(&scene("12:01", "22"), Some(&held), 32_000).expect("python3");
    apply(&mut held, &third);
    for (view, pane) in [("clock", "clock"), ("weather", "weather")] {
        let window = held_props(&held, &third, &pane_id(view, pane));
        assert_eq!(window["state"], "hidden", "not on the canvas: {window}");
        assert_eq!(window["rung"], "ambient", "{window}");
        let tile = held_props(&held, &third, &tile_of(&pane_id(view, pane)));
        assert_eq!(tile["state"], "ambient", "the tile wears the rung: {tile}");
        assert_eq!(tile["on_canvas"], false, "{tile}");
    }
    let root = held_props(&held, &third, "display.root");
    assert_eq!(
        root["focus"], 0.0,
        "nothing reaches the bar, nothing is large: {root}"
    );
}

/// The `touched` hint still wakes a pinned window: an application that
/// says so is asking for attention, and the floor gives its context the
/// weight of the last touch.
#[test]
fn the_touched_hint_still_wakes_a_pinned_window() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let scene = |touched: &str| {
        vec![component_view(
            "weather",
            "main",
            pane(
                "weather",
                json!({"context": "ambient", "relevance": 0.3, "pinned": true,
                       "title": "21", "touched": touched}),
            ),
        )]
    };
    let first = read_pass(&scene(""), Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);
    let second = read_pass(&scene(""), Some(&held), 2000).expect("python3");
    apply(&mut held, &second);
    assert_eq!(
        held_props(&held, &second, &pane_id("weather", "weather"))["state"],
        "hidden",
        "arrival alone does not make a pinned window large"
    );
    let woken = read_pass(&scene("3000"), Some(&held), 3000).expect("python3");
    apply(&mut held, &woken);
    let settled = read_pass(&scene("3000"), Some(&held), 4000).expect("python3");
    apply(&mut held, &settled);
    let window = held_props(&held, &settled, &pane_id("weather", "weather"));
    assert_eq!(window["since"], 3000, "the hint is the touch: {window}");
    assert_eq!(
        window["state"], "focus",
        "and the window takes the canvas: {window}"
    );
}
