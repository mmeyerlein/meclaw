//! D1 -- one screen state, many exits (R-D3): every entry of the `screens`
//! setting is a page of the same tree, the root wears the profile of the
//! exit it is rendered for, and the routes are written only when the exits
//! change.
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

fn written(calls: &[Value], id: &str) -> Option<Value> {
    calls
        .iter()
        .find(|c| (c["op"] == "object.create" || c["op"] == "object.update") && c["id"] == id)
        .map(|c| c["props"].clone())
}

fn pages(calls: &[Value]) -> Vec<(String, String)> {
    calls
        .iter()
        .filter(|c| c["op"] == "page.set")
        .map(|c| {
            (
                c["route"].as_str().unwrap_or("").to_string(),
                c["root"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// One screen state, N exits (R-D3). The default exit shares the root with
/// `/`; every further one is a prefixed copy of the same tree with its own
/// profile at the root.
#[test]
fn every_exit_is_a_page_of_the_same_state() {
    if !library_ships() {
        return;
    }
    let params = json!({"screens": {
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0,
               "physical_size_in": 55, "inputs": ["audio"]},
        "desk": {"display_type": "monitor", "viewing_distance_m": 0.7,
                 "physical_size_in": 27, "inputs": ["pointer", "keyboard"]}
    }, "default_screen": "tv"});
    let views = vec![component_view(
        "a",
        "main",
        pane("a", json!({"context": "conversation", "relevance": 0.8})),
    )];
    let calls = read_pass_with(&views, None, 1000, params).expect("python3 answers");
    let set = pages(&calls);
    assert!(
        set.contains(&("/".to_string(), "display.root".to_string())),
        "the root route stands: {set:?}"
    );
    assert!(
        set.contains(&("/tv".to_string(), "display.root".to_string())),
        "the default exit is the same tree: {set:?}"
    );
    assert!(
        set.contains(&("/desk".to_string(), "desk.display.root".to_string())),
        "a further exit gets its own root: {set:?}"
    );
    let tv = written(&calls, "display.root").expect("the root");
    let desk = written(&calls, "desk.display.root").expect("the second root");
    assert_eq!(tv["profile"], "tv", "the profile at the root: {tv}");
    assert_eq!(tv["scale"], "1.6", "a television is scaled up: {tv}");
    assert_eq!(tv["inputs"], "audio");
    assert_eq!(tv["screen"], "tv");
    assert!(
        tv["client_js"]
            .as_str()
            .is_some_and(|js| js.contains("DisplayScene: hook")),
        "the shell script is the scene hook: {}",
        tv["client_js"]
    );
    assert_eq!(desk["profile"], "monitor", "{desk}");
    assert_eq!(desk["scale"], "1.0", "{desk}");
    assert_eq!(
        desk["inputs"], "keyboard pointer",
        "sorted, space separated"
    );
    assert_eq!(desk["screen"], "desk");
    assert_eq!(
        desk["client_js"], tv["client_js"],
        "the second exit carries the same motion"
    );
    assert!(
        written(&calls, "desk.view.alex.a").is_some(),
        "the same windows stand on the second exit: {calls:?}"
    );
    assert!(
        written(&calls, "desk.display.dock").is_some(),
        "with the same dock: {calls:?}"
    );
    let dock = written(&calls, "display.dock").expect("the dock");
    assert_eq!(
        dock["profile"], "tv",
        "the dock copies the root's profile: {dock}"
    );
    let desk_dock = written(&calls, "desk.display.dock").expect("the mirrored dock");
    assert_eq!(desk_dock["profile"], "monitor", "{desk_dock}");
    // The routes go LAST: `page.set` refuses a root that does not exist yet.
    let last = calls.last().expect("calls");
    assert_eq!(
        last["op"], "page.set",
        "the routes are set after the objects: {last}"
    );
}

/// The routes are written once, and again only when the exits change.
#[test]
fn the_routes_are_not_rewritten_every_tick() {
    if !library_ships() {
        return;
    }
    let params = json!({"screens": {
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0, "inputs": ["audio"]}
    }, "default_screen": "tv"});
    let first = read_pass_with(&[], None, 1000, params.clone()).expect("python3 answers");
    let mut held = json!([]);
    apply(&mut held, &first);
    let second = read_pass_with(&[], Some(&held), 2000, params).expect("python3");
    assert!(
        pages(&second).is_empty(),
        "a quiet tick sets no page: {second:?}"
    );
    let more = json!({"screens": {
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0, "inputs": ["audio"]},
        "hand": {"display_type": "phone", "viewing_distance_m": 0.35, "inputs": ["touch"]}
    }, "default_screen": "tv"});
    let third = read_pass_with(&[], Some(&held), 3000, more).expect("python3");
    let set = pages(&third);
    assert!(
        set.contains(&("/hand".to_string(), "hand.display.root".to_string())),
        "a new exit is routed when the setting changes: {set:?}"
    );
}

/// Without a setting the screen is one television at three metres; a
/// `default_screen` that names no entry falls to the first one.
#[test]
fn the_shipped_screen_is_a_television() {
    if !library_ships() {
        return;
    }
    let calls = read_pass(&[], None, 1000).expect("python3 answers");
    let root = written(&calls, "display.root").expect("the root");
    assert_eq!(root["screen"], "tv");
    assert_eq!(root["profile"], "tv");
    assert_eq!(root["scale"], "1.6");
    assert_eq!(root["inputs"], "audio");
    assert_eq!(root["dock_overflow"], 0);
    let screens: Value =
        meclaw_core::serde_json::from_str(root["screens"].as_str().expect("JSON text"))
            .expect("the screens parse");
    assert_eq!(screens["tv"]["display_type"], "tv");
    assert_eq!(
        pages(&calls),
        vec![
            ("/".to_string(), "display.root".to_string()),
            ("/tv".to_string(), "display.root".to_string())
        ]
    );
    let odd = read_pass_with(
        &[],
        None,
        1000,
        json!({"screens": {"z": {"display_type": "phone"}, "b": {}}, "default_screen": "nope"}),
    )
    .expect("python3");
    let root = written(&odd, "display.root").expect("the root");
    assert_eq!(root["screen"], "b", "the first entry by name: {root}");
    assert_eq!(
        root["profile"], "tv",
        "an entry that says nothing is a television"
    );
}

/// A change of `default_screen` alone -- the exits unchanged -- moves the
/// routes: `/` and the new default share the root, the old default gets a
/// prefixed root of its own.
#[test]
fn a_new_default_screen_moves_the_routes() {
    if !library_ships() {
        return;
    }
    let screens = json!({
        "tv": {"display_type": "tv", "viewing_distance_m": 3.0, "inputs": ["audio"]},
        "desk": {"display_type": "monitor", "viewing_distance_m": 0.7, "inputs": ["pointer"]}
    });
    let first = read_pass_with(
        &[],
        None,
        1000,
        json!({"screens": screens, "default_screen": "tv"}),
    )
    .expect("python3 answers");
    let mut held = json!([]);
    apply(&mut held, &first);
    let second = read_pass_with(
        &[],
        Some(&held),
        2000,
        json!({"screens": screens, "default_screen": "desk"}),
    )
    .expect("python3");
    let set = pages(&second);
    assert!(
        set.contains(&("/".to_string(), "display.root".to_string()))
            && set.contains(&("/desk".to_string(), "display.root".to_string()))
            && set.contains(&("/tv".to_string(), "tv.display.root".to_string())),
        "every route is set again, on the right root: {set:?}"
    );
    assert!(
        written(&second, "tv.display.root").is_some_and(|r| r["profile"] == "tv"),
        "the old default gets its own root: {second:?}"
    );
    assert!(
        written(&second, "display.root").is_some_and(|r| r["screen"] == "desk"),
        "and the shared root wears the new default: {second:?}"
    );
}
