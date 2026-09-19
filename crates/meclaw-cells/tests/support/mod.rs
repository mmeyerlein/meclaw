//! What every display lock needs: the script as a subprocess, one pass at a time, and the
//! two things a pass carries over -- the state row of display-hive.md § 3.1 and the object
//! tree the display holds. Written ONCE here instead of twenty-two times (OR-H6).
//!
//! `compose.py` runs the way a `code` cell runs it: a subprocess, the pass's document on
//! stdin, the emissions as JSON on stdout.
#![allow(dead_code)]

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

pub const COMPOSE: &str = "templates/display/compose/compose.py";

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Whether the template library travels in this tree (it does not in the published one).
pub fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// Every emission the script answered with, unfiltered.
pub fn raw(doc: &Value) -> Vec<Value> {
    let mut child = Command::new("python3")
        .arg(repo(COMPOSE))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs the script");
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
    match answer {
        Value::Array(list) => list,
        one @ Value::Object(_) => vec![one],
        other => panic!("emissions are objects: {other}"),
    }
}

/// The `object.*` calls of the one patch bundle, or none.
pub fn calls(emissions: &[Value]) -> Vec<Value> {
    let patches: Vec<&Value> = emissions
        .iter()
        .filter(|e| e["header"]["route"] == "patch")
        .collect();
    assert!(patches.len() <= 1, "at most one patch: {emissions:?}");
    match patches.first() {
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
    }
}

/// One `display-pane` tree with `props`, keyed so its object id is stable.
pub fn pane(name: &str, props: Value) -> Value {
    let mut props = props;
    props["pane_id"] = json!(name);
    json!({"component": "display-pane", "props": props, "key": format!("c.{name}")})
}

/// A store row of `owner`, carrying a component tree.
pub fn view_of(owner: &str, view_id: &str, region: &str, tree: Value) -> Value {
    json!({
        "owner": owner, "view_id": view_id, "region": region, "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": 1,
    })
}

pub fn component_view(view_id: &str, region: &str, tree: Value) -> Value {
    view_of("alex", view_id, region, tree)
}

/// The same view, put up by SOMEBODY ELSE: a repetition only yields to another app (§ 4.12).
pub fn foreign_view(view_id: &str, region: &str, tree: Value) -> Value {
    view_of("robin", view_id, region, tree)
}

/// The id of a window in the screen state (§ 2 Id): what a tap names and what the state
/// keys its views by. The tree node below it is `<id>/c.<pane>`.
pub fn window_id(owner: &str, view_id: &str) -> String {
    format!("view.{owner}.{view_id}")
}

pub fn pane_id(view_id: &str, name: &str) -> String {
    format!("view.alex.{view_id}/c.{name}")
}

/// The props of the last create or update for `id`, or none.
pub fn written(calls: &[Value], id: &str) -> Option<Value> {
    calls
        .iter()
        .rev()
        .find(|c| (c["op"] == "object.create" || c["op"] == "object.update") && c["id"] == id)
        .map(|c| c["props"].clone())
}

/// The routes this pass wrote, as `(route, root)`.
pub fn pages(calls: &[Value]) -> Vec<(String, String)> {
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

/// One call of a patch bundle on the tree a display holds.
pub fn apply(held: &mut Value, calls: &[Value]) {
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

/// One screen over several passes: the store's rows, the ONE state row (§ 3.1) and the
/// object tree the display holds -- exactly what the store and the display carry between
/// two passes of the cell.
pub struct Screen {
    pub rows: Vec<Value>,
    pub params: Value,
    pub state: Value,
    pub held: Value,
    pub last: Vec<Value>,
}

/// The request a pass started from, the way `pass_views` hands it on in the plan
/// (`mark`, GH #744): what a repeat of this very pass would be emitted with. Derived
/// from the event here, because a `Screen` is handed the event and not the request.
fn mark_of(event: &Value) -> Value {
    match event["kind"].as_str().unwrap_or("") {
        "tap" => json!({"tick": true, "tap": event["for"]}),
        "hold" => json!({"tick": true, "hold": true}),
        "app_write" => json!({"withdraw": false, "oid": event["oid"]}),
        "app_withdraw" => json!({"withdraw": true, "oid": event["oid"]}),
        _ => json!({"tick": true}),
    }
}

impl Screen {
    pub fn new(params: Value) -> Self {
        Screen {
            rows: Vec::new(),
            params,
            state: Value::Null,
            held: json!([]),
            last: Vec::new(),
        }
    }

    /// The rows of the store, replacing what stood under the same `(owner, view_id)`.
    pub fn put(&mut self, row: Value) {
        self.rows
            .retain(|r| !(r["owner"] == row["owner"] && r["view_id"] == row["view_id"]));
        self.rows.push(row);
    }

    pub fn withdraw(&mut self, owner: &str, view_id: &str) {
        self.rows
            .retain(|r| !(r["owner"] == owner && r["view_id"] == view_id));
    }

    /// One read pass with the event the views pass would have built. `event` is the one
    /// event of § 4.1, e.g. `json!({"kind": "stroke"})`.
    pub fn pass(&mut self, event: Value, now: u64) -> Vec<Value> {
        let objects = if self.held.as_array().map(|l| l.is_empty()).unwrap_or(true) {
            json!([])
        } else {
            self.held.clone()
        };
        let plan = json!({
            "views": self.rows, "state": self.state, "define": [], "now": now,
            "event": event, "mark": mark_of(&event),
        });
        let doc = json!({
            "params": self.params,
            "body": {"messages": [{
                "origin": "tool", "type": "tool_result", "id": "d-query",
                "text": json!({"objects": objects}).to_string(),
            }]},
            "envelope": {"header": {
                "hop": {"operation": "query"},
                "context": {"display_origin": "read", "display_views": plan.to_string()},
            }},
        });
        let emissions = raw(&doc);
        let patch = calls(&emissions);
        apply(&mut self.held, &patch);
        // The state row of this pass: the store would hold it, so this screen does.
        for em in &emissions {
            let request = em["header"]["display_request"].as_str().unwrap_or("");
            if em["header"]["route"] == "views" && request.contains("\"state\"") {
                for leg in em["messages"].as_array().unwrap_or(&Vec::new()) {
                    let call: Value =
                        meclaw_core::serde_json::from_str(leg["text"].as_str().unwrap_or("{}"))
                            .expect("a call is JSON");
                    put_state(&mut self.state, &call);
                }
            }
        }
        self.last = emissions;
        patch
    }

    /// A write of one app, as one pass: the row goes into the store, the event says so.
    pub fn write(&mut self, row: Value, now: u64) -> Vec<Value> {
        let oid = window_id(
            row["owner"].as_str().unwrap_or(""),
            row["view_id"].as_str().unwrap_or(""),
        );
        let hints = hints_of(&row);
        self.put(row);
        self.pass(json!({"kind": "app_write", "oid": oid, "view": hints}), now)
    }

    /// The emissions of the last pass on one lane.
    pub fn lane(&self, route: &str) -> Vec<&Value> {
        self.last
            .iter()
            .filter(|e| e["header"]["route"] == route)
            .collect()
    }

    /// The props of one object of the tree the display holds.
    pub fn props(&self, id: &str) -> Option<Value> {
        self.held
            .as_array()?
            .iter()
            .find(|o| o["id"] == id)
            .map(|o| o["props"].clone())
    }

    /// Whether the display holds an object of that id.
    pub fn holds(&self, id: &str) -> bool {
        self.props(id).is_some()
    }

    /// The state row's content, parsed: the screen state of § 3.
    pub fn screen_state(&self) -> Value {
        let text = self.state["content"].as_str().unwrap_or("{}");
        meclaw_core::serde_json::from_str(text).expect("the state row's content is JSON")
    }

    /// One curator value of one window, out of the state row.
    pub fn curator(&self, oid: &str, key: &str) -> Value {
        self.screen_state()["views"][oid]["curator"][key].clone()
    }
}

/// One leg of a state write, applied to the row this screen holds.
///
/// Two spellings since GH #744: the first creation is an `insert` of the whole row,
/// every later pass an `update` under a condition on the version it read. The update
/// sets every column but the identity, so merging its `set` into the row this screen
/// holds is what the store does.
pub fn put_state(state: &mut Value, call: &Value) {
    match call["operation"].as_str().unwrap_or("") {
        "insert" => *state = call["row"].clone(),
        "update" => {
            if state.is_null() {
                *state = json!({"owner": "display", "view_id": "screen-state"});
            }
            for (key, value) in call["set"].as_object().expect("an update sets columns") {
                state[key.clone()] = value.clone();
            }
        }
        _ => {}
    }
}

/// The hints of a store row, the way `hints_of_row` reads them: the window node's props
/// plus its `ttl_ms`. What a test hands the pass as the event's `view`.
pub fn hints_of(row: &Value) -> Value {
    let content: Value = meclaw_core::serde_json::from_str(row["content"].as_str().unwrap_or("{}"))
        .expect("the row's content is JSON");
    // A `prose` row is FLAT: its content IS the hints, so there is no window node to
    // unwrap (`hints_of_row` in compose.py reads the row's `kind` the same way). Without
    // this branch a prose view reaches the pass empty, and a lock that measures its hints
    // would go green while nothing arrived.
    if row["kind"] == "prose" {
        let mut props = content;
        props["ttl_ms"] = row["ttl_ms"].clone();
        return props;
    }
    let win = window_node(&content).unwrap_or(content.clone());
    let mut props = win["props"].clone();
    if props.is_null() {
        props = json!({});
    }
    props["ttl_ms"] = row["ttl_ms"].clone();
    props
}

fn window_node(node: &Value) -> Option<Value> {
    const WINDOWS: [&str; 4] = [
        "display-pane",
        "display-panel",
        "display-overlay",
        "display-view-prose",
    ];
    if WINDOWS.contains(&node["component"].as_str().unwrap_or("")) {
        return Some(node.clone());
    }
    for kid in node["children"].as_array()? {
        if let Some(found) = window_node(kid) {
            return Some(found);
        }
    }
    None
}
