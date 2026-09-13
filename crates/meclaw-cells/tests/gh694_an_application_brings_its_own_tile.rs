//! D1 -- an application brings its own tile, and the floor takes it out of
//! the window (R-D1); a window without one falls back; the dock has a
//! capacity.
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

fn tile_of(window: &str) -> String {
    format!("display.dock/tile.{}", window.replace('/', "~"))
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

/// A window brings its tile as a child keyed `tile` (R-D1). The dock gets it,
/// and the canvas window does NOT: the tile is taken out of the tree.
#[test]
fn the_tile_is_taken_out_of_the_window() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let tree = json!({
        "component": "display-pane",
        "props": {"pane_id": "w", "context": "ambient", "relevance": 0.8},
        "key": "c.w",
        "children": [
            {"component": "display-tile", "key": "tile",
             "props": {"glyph": "\u{2600}", "line": "Berlin", "value": "21\u{00b0}"}},
            {"component": "display-value", "key": "v", "props": {"value": "21"}}
        ],
    });
    let views = vec![component_view("w", "main", tree)];
    let calls = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let tile = written(&calls, &tile_of(&pane_id("w", "w"))).expect("the dock has the tile");
    assert_eq!(tile["glyph"], "\u{2600}", "the application's glyph: {tile}");
    assert_eq!(tile["line"], "Berlin");
    assert_eq!(tile["value"], "21\u{00b0}");
    // The other child keeps its place in the tree: the tile was taken out,
    // nothing else was.
    assert!(
        written(&calls, &format!("{}/v", pane_id("w", "w")))
            .is_some_and(|p| p["value"] == json!("21")),
        "the sibling of the tile is drawn inside the window: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|c| c["component"] == "display-tile" && c["parent"] != json!("display.dock")),
        "no tile stands anywhere but in the dock: {calls:?}"
    );
}

/// A long line is cut with an ellipsis; a window without a tile falls back to
/// a glyph from its context and to its title.
#[test]
fn a_window_without_a_tile_falls_back() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![component_view(
        "c",
        "main",
        pane(
            "c",
            json!({"context": "conversation", "relevance": 0.8,
                   "title": "A title that is very much longer than one tile line"}),
        ),
    )];
    let calls = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let tile = written(&calls, &tile_of(&pane_id("c", "c"))).expect("a tile");
    assert_eq!(tile["glyph"], "\u{1f4ac}", "the conversation glyph: {tile}");
    let line = tile["line"].as_str().expect("a line");
    assert_eq!(line.chars().count(), 24, "one line, cut: {line:?}");
    assert!(
        line.ends_with('\u{2026}'),
        "and the cut is visible: {line:?}"
    );
}

/// The dock has a capacity. The lowest ranks fall out; a pinned window never
/// does.
#[test]
fn the_dock_has_a_capacity() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let mut views = vec![component_view(
        "pin",
        "main",
        pane(
            "pin",
            json!({"context": "ambient", "relevance": 0.05, "pinned": true}),
        ),
    )];
    for i in 0..5 {
        views.push(component_view(
            &format!("w{i}"),
            "main",
            pane(
                &format!("w{i}"),
                json!({"context": "conversation", "relevance": 0.5 + f64::from(i) / 100.0}),
            ),
        ));
    }
    let calls = read_pass_with(&views, Some(&base), 1000, json!({"dock_max": 3}))
        .expect("python3 answered once already");
    let dock = written(&calls, "display.dock").expect("the dock is created");
    assert_eq!(dock["count"], 3, "the dock keeps three: {dock}");
    assert!(
        written(&calls, &tile_of(&pane_id("pin", "pin"))).is_some(),
        "a pinned window never falls out: {calls:?}"
    );
    assert!(
        written(&calls, &tile_of(&pane_id("w0", "w0"))).is_none(),
        "the lowest of the rest does: {calls:?}"
    );
    let root = written(&calls, "display.root").expect("the root is updated");
    assert_eq!(
        root["dock_overflow"], 3,
        "and the root counts what fell out: {root}"
    );
}
