//! D1 -- the dock shows what is present, the canvas shows what is in focus.
//!
//! Two axes instead of one (spec 2.2): a window is PRESENT while it stands in
//! the table and has not faded, and presence alone puts a tile in the dock.
//! The judge decides only what is LARGE. A window the judge hides keeps its
//! tile.
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

/// Every tile the bundle writes, in the order its `ord` puts it.
fn tiles(calls: &[Value]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64, i64)> = calls
        .iter()
        .filter(|c| c["component"] == "display-tile")
        .map(|c| {
            (
                c["props"]["for"].as_str().unwrap_or("").to_string(),
                c["props"]["rank"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0),
                c["ord"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    out.sort_by_key(|t| t.2);
    out.into_iter().map(|t| (t.0, t.1)).collect()
}

/// The dock stands on the root, beside the regions, with one tile per present
/// window, the highest rank first.
#[test]
fn the_dock_carries_one_tile_per_present_window() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![
        component_view(
            "a",
            "main",
            pane("a", json!({"context": "conversation", "relevance": 0.9})),
        ),
        component_view(
            "b",
            "main",
            pane("b", json!({"context": "ambient", "relevance": 0.3})),
        ),
    ];
    let calls = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let dock = written(&calls, "display.dock").expect("the dock is created");
    assert_eq!(dock["count"], 2, "the dock counts its tiles: {dock}");
    let seen = tiles(&calls);
    // `for` is the window's `pane_id`: the name the window's element carries
    // in the DOM, so the client can match the two halves of one object.
    assert_eq!(
        seen.iter().map(|t| t.0.clone()).collect::<Vec<_>>(),
        vec!["a".to_string(), "b".to_string()],
        "the highest rank stands on top: {seen:?}"
    );
    assert!(seen[0].1 > seen[1].1, "rank falls down the dock: {seen:?}");
    let tile = written(&calls, &tile_of(&pane_id("a", "a"))).expect("a tile");
    assert_eq!(tile["for"], json!("a"), "the tile names its window");
    assert!(tile["glyph"].as_str().is_some_and(|g| !g.is_empty()));
}

/// The judge hides a window: it leaves the canvas and KEEPS its tile. That is
/// the whole point of two axes (D-6, revised S4/S7).
#[test]
fn a_hidden_window_keeps_its_tile() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "ambient", "relevance": 0.9, "state": "hidden"}),
        ),
    )];
    let calls = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let window = written(&calls, &pane_id("a", "a")).expect("the window is created");
    assert_eq!(window["state"], "hidden", "hidden stays hidden: {window}");
    assert_eq!(window["score"], 0.0, "a hidden window scores nothing");
    let tile = written(&calls, &tile_of(&pane_id("a", "a")))
        .expect("the tile stands whatever the canvas says");
    assert_eq!(tile["on_canvas"], false, "it is not large: {tile}");
    assert!(
        tile["rank"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0)
            > 0.0,
        "and it still has an order in the dock: {tile}"
    );
}

/// A faded window is not present any more: the tile goes with the score.
#[test]
fn a_faded_window_loses_its_tile() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "ambient", "relevance": 0.5})),
    )];
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let mut held = base.clone();
    apply(&mut held, &first);
    // linger 20 s + fade 120 s + one second.
    let later = read_pass(&views, Some(&held), 1000 + 141_000).expect("python3");
    assert!(
        written(&later, &tile_of(&pane_id("a", "a"))).is_none(),
        "a faded window has no tile: {later:?}"
    );
    assert!(
        later
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == tile_of(&pane_id("a", "a"))),
        "and the one it had is swept: {later:?}"
    );
}

/// Pinned freezes the decay, so the tile stands through any fade (D-15).
#[test]
fn a_pinned_window_stays_in_the_dock() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let views = vec![component_view(
        "a",
        "main",
        pane(
            "a",
            json!({"context": "ambient", "relevance": 0.5, "pinned": true}),
        ),
    )];
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answered once already");
    let mut held = base.clone();
    apply(&mut held, &first);
    let later = read_pass(&views, Some(&held), 1000 + 600_000).expect("python3");
    let tile = written(&later, &tile_of(&pane_id("a", "a")))
        .or_else(|| {
            held.as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == tile_of(&pane_id("a", "a")).as_str())
                .map(|o| o["props"].clone())
        })
        .expect("a pinned window keeps its tile");
    assert_eq!(tile["pinned"], true, "and says so: {tile}");
}

/// A window that names its own deadline (`relevant_until`) is present until
/// then, faded or not (OR-D-Bau-7): a timer that rang stands as a quiet tile
/// for the minutes its application said, and goes when the deadline strikes.
#[test]
fn a_window_with_a_deadline_is_present_until_it() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let faded = 1000 + 141_000; // linger 20 s + fade 120 s + one second.
    let with_deadline = |until: u64| {
        vec![component_view(
            "t",
            "main",
            pane(
                "t",
                json!({"context": "ambient", "relevance": 0.5, "relevant_until": until}),
            ),
        )]
    };
    let first = read_pass(&with_deadline(faded + 60_000), Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let later = read_pass(&with_deadline(faded + 60_000), Some(&held), faded).expect("python3");
    apply(&mut held, &later);
    let tile = held
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == tile_of(&pane_id("t", "t")).as_str())
        .map(|o| o["props"].clone())
        .expect("the tile stands until the deadline");
    assert!(
        tile["rank"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0)
            > 0.0,
        "and keeps a readable place in the dock: {tile}"
    );
    assert_eq!(tile["on_canvas"], false, "quiet, not large: {tile}");
    let window = written(&later, &pane_id("t", "t")).expect("the window is updated");
    assert_eq!(window["state"], "hidden", "faded off the canvas: {window}");
    // The deadline is a moment of the screen: the clock is ordered for it.
    // (The due order travels on the root: `next_due` reads `relevant_until`.)
    let root = written(&later, "display.root").expect("the root is updated");
    assert!(
        root["due"].as_str().is_some_and(|d| !d.is_empty()),
        "a strike is ordered for the deadline: {root}"
    );

    // The same window without a deadline: faded means gone from the dock.
    let plain = vec![component_view(
        "t",
        "main",
        pane("t", json!({"context": "ambient", "relevance": 0.5})),
    )];
    let first = read_pass(&plain, Some(&base), 1000).expect("python3");
    let mut held = base.clone();
    apply(&mut held, &first);
    let later = read_pass(&plain, Some(&held), faded).expect("python3");
    assert!(
        later
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == tile_of(&pane_id("t", "t"))),
        "without a deadline the faded tile goes: {later:?}"
    );
}
