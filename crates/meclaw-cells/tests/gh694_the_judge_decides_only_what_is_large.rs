//! D1 -- the judge decides only what is large: the situation it sees carries
//! the dock, and the prompt says what the verdict may and may not do.
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

/// The question the judge is asked, as the cell emits it.
fn ask(views: &[Value], objects: Option<&Value>, now: u64) -> Option<Value> {
    let messages = match objects {
        None => json!([]),
        Some(objs) => json!([{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": objs}).to_string(),
        }]),
    };
    let doc = json!({
        "params": {"judge": "on"},
        "body": {"messages": messages},
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
    let asked = list
        .into_iter()
        .find(|e| e["header"]["route"] == "judge")
        .expect("the judge is asked");
    let text = asked["messages"][0]["text"]
        .as_str()
        .expect("the situation is a turn")
        .to_string();
    Some(meclaw_core::serde_json::from_str(&text).expect("the situation is JSON"))
}

/// The judge sees the dock: every window says whether it has a tile, where it
/// ranks, whether it is on the canvas, and what the floor marked.
#[test]
fn the_situation_carries_the_dock() {
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
            json!({"context": "conversation", "relevance": 0.8, "topic": "chat"}),
        ),
    )];
    let situation = ask(&views, Some(&base), 1000).expect("python3 answers");
    assert_eq!(situation["screen"], "tv", "the exit is named: {situation}");
    assert_eq!(
        situation["dock_overflow"], 0,
        "nothing fell out: {situation}"
    );
    let window = situation["windows"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|w| w["id"] == pane_id("a", "a").as_str())
        .expect("the window is in the situation");
    assert_eq!(window["topic"], "chat", "{window}");
    assert_eq!(window["tile"], true, "{window}");
    assert_eq!(window["topic_dupe"], false, "{window}");
    assert_eq!(
        window["on_canvas"], false,
        "a fresh window is not large yet"
    );
    assert!(
        window["rank"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0)
            > 0.0,
        "and it ranks: {window}"
    );
}

/// The prompt says what the verdict may and may not do.
#[test]
fn the_prompt_says_the_dock_is_not_the_verdicts_business() {
    if !library_ships() {
        return;
    }
    let source = std::fs::read_to_string(repo(COMPOSE)).expect("the script ships");
    for sentence in [
        "The dock on the right shows everything that is present",
        "A window you hide keeps its tile",
        "topic_dupe",
        "below 0.05 is read as 0.05",
    ] {
        assert!(source.contains(sentence), "the judge is told: {sentence:?}");
    }
}

/// A weight of zero does not take a window out of the dock (OR-D10).
#[test]
fn a_weight_of_zero_still_leaves_an_order() {
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
    let first = read_pass(&views, Some(&base), 1000).expect("python3 answers");
    let mut held = base.clone();
    apply(&mut held, &first);
    // Play a verdict that weighs `ambient` to nothing.
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
                    "views": views, "define": [], "now": 2000,
                    "verdict": {"focus": 0.5, "weights": {"ambient": 0.0}, "windows": []}
                }).to_string(),
            },
        }},
    });
    let calls = run(&doc).expect("python3 answers");
    let window = written(&calls, &pane_id("a", "a")).expect("the window is updated");
    assert_eq!(
        window["score"], 0.0,
        "a weight of nothing scores nothing: {window}"
    );
    let tile = written(&calls, &tile_of(&pane_id("a", "a")))
        .or_else(|| {
            held.as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == tile_of(&pane_id("a", "a")).as_str())
                .map(|o| o["props"].clone())
        })
        .expect("the tile stands");
    let rank = tile["rank"]
        .as_str()
        .unwrap_or("0")
        .parse::<f64>()
        .unwrap_or(0.0);
    assert!(rank > 0.0, "the rank keeps a floor of 0.05: {tile}");
    assert_eq!(rank, 0.025, "0.05 x 0.5 x 1: {tile}");
}
