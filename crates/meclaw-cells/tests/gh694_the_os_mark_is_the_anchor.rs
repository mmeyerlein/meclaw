//! D1 -- the OS mark is the anchor: a child of the root beside the dock,
//! written on every screen, and the old microphone object is written no more.
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

/// The anchor at the bottom right is the OS mark, and it is a child of the
/// ROOT beside the dock -- never inside it (OR-D4): it carries its own hook,
/// and a dock that re-renders must not take the microphone with it.
#[test]
fn the_os_mark_stands_on_the_root_below_the_dock() {
    if !library_ships() {
        return;
    }
    let calls = read_pass(&[], None, 1000).expect("python3 answers");
    let os = calls
        .iter()
        .find(|c| c["id"] == "display.os")
        .expect("the OS mark is created");
    assert_eq!(os["component"], "display-os", "the new component: {os}");
    assert_eq!(os["parent"], "display.root", "a child of the root: {os}");
    assert_eq!(os["props"]["mount"], "voice", "it knows its voice cell");
    assert!(
        os["props"]["client_js"]
            .as_str()
            .is_some_and(|js| js.contains("DisplayMic")),
        "and it carries the browser half"
    );
    let dock = calls
        .iter()
        .find(|c| c["id"] == "display.dock")
        .expect("the dock is created");
    assert!(
        dock["ord"].as_i64().unwrap_or(0) < os["ord"].as_i64().unwrap_or(0),
        "the mark sits below the dock: {dock} / {os}"
    );
    assert!(
        !calls.iter().any(|c| c["id"] == "display.mic"),
        "and nothing writes the old microphone any more: {calls:?}"
    );
    assert!(written(&calls, "display.os").is_some());
}

/// The mark is structural: it is written on a bare screen and it stands on
/// every later pass.
#[test]
fn the_mark_is_written_on_every_screen() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let held = base.as_array().expect("a list");
    assert!(
        held.iter().any(|o| o["id"] == "display.os"),
        "the bare screen holds the mark: {base}"
    );
    assert!(
        !held.iter().any(|o| o["id"] == "display.mic"),
        "and not the old one: {base}"
    );
}

/// A screen that still holds the old microphone object is cleaned up on the
/// first pass after the lift (OR-D7).
#[test]
fn the_old_microphone_is_swept_as_an_orphan() {
    if !library_ships() {
        return;
    }
    let Some(base) = bare_screen() else {
        return;
    };
    let mut held = base.clone();
    held.as_array_mut().expect("a list").push(json!({
        "id": "display.mic", "parent": "display.root", "ord": 20,
        "component": "display-os", "props": {"mount": "voice", "client_js": ""},
    }));
    let calls = read_pass(&[], Some(&held), 2000).expect("python3");
    assert!(
        calls
            .iter()
            .any(|c| c["op"] == "object.delete" && c["id"] == "display.mic"),
        "the old object is deleted: {calls:?}"
    );
}
