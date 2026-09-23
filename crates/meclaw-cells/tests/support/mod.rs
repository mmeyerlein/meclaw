//! What every display lock needs: a living curator, one message at a time, and what it
//! leaves behind -- the rows of the `views` store (the apps' rows and the rest row of
//! display-hive.md § 3) and the object tree the display holds. Written ONCE here instead of
//! twenty-two times (OR-H6).
//!
//! `compose.py` runs the way a `resident` code cell runs it (GH #809): `Screen` speaks
//! line JSON to `curator_driver.py --serve`, which compiles the script once and executes
//! it into ONE globals dict per message, so the state lives in the cell's memory between
//! messages. `raw` still runs one stateless document through a fresh subprocess.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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

/// The curator driver: `compose.py` the way a `resident` code cell runs it (GH #809).
pub const DRIVER: &str = "templates/display/compose/scenarios/curator_driver.py";

/// One display hive over many messages: `compose.py` resident, the `views` store and the
/// `web` cell's tree, all three played by `curator_driver.py --serve` (OR-D9).
///
/// Since display 2.7.0 the curator keeps the screen state in MEMORY between messages
/// (GH #809): no state row, no plan in any header, one patch per pass. A test can no
/// longer hand one pass its prior state -- it talks to ONE living cell, the way the hive
/// does, and reads what came out at the seams: the patch calls (`pass`), the store rows
/// (`table`), the outer lanes (`lane`), the internal hops (`hops`). The model state
/// (`screen_state`) comes from the driver, which reads the cell's memory; nothing a
/// colony writes carries it any more.
pub struct Screen {
    /// The member's params, handed to the cell with every message. A test may change
    /// them between two passes; the next message carries the new ones.
    pub params: Value,
    /// The tree the display holds (`web`), as a list of `{id, parent, ord, component, props}`.
    pub held: Value,
    /// The `views` store as it stands: the apps' rows and the curator's rest row.
    pub rows: Vec<Value>,
    /// The curator's state in memory (the model state of § 3), `Null` before its boot.
    pub state: Value,
    /// The outer emissions of the last message (`event`, `receipt`, `due`, `judge`).
    pub last: Vec<Value>,
    said: Value,
    hops: Vec<Value>,
    sent: Value,
    child: Child,
    input: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
}

impl Screen {
    pub fn new(params: Value) -> Self {
        let mut child = Command::new("python3")
            .arg(repo(DRIVER))
            .arg("--serve")
            .arg("--params")
            .arg(params.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("python3 runs the curator driver");
        let input = child.stdin.take().expect("stdin");
        let out = BufReader::new(child.stdout.take().expect("stdout"));
        Screen {
            sent: params.clone(),
            params,
            held: json!([]),
            rows: Vec::new(),
            state: Value::Null,
            last: Vec::new(),
            said: json!([]),
            hops: Vec::new(),
            child,
            input: Some(input),
            out,
        }
    }

    /// One line to the driver, its one answer back; the mirrors follow the answer.
    fn ask(&mut self, req: Value) -> Value {
        if self.params != self.sent {
            self.sent = self.params.clone();
            let params = self.params.clone();
            self.line(json!({"op": "params", "params": params}));
        }
        self.line(req)
    }

    fn line(&mut self, req: Value) -> Value {
        let input = self.input.as_mut().expect("the driver is running");
        writeln!(input, "{req}").expect("the driver takes a line");
        input.flush().expect("the line reaches the driver");
        let mut text = String::new();
        self.out.read_line(&mut text).expect("the driver answers");
        assert!(!text.is_empty(), "the curator driver ended on {req}");
        let answer: Value =
            meclaw_core::serde_json::from_str(&text).expect("the driver answers JSON");
        assert!(
            answer["error"].is_null(),
            "the cell failed on {req}: {}\n{}",
            answer["error"],
            answer["stderr"].as_str().unwrap_or("")
        );
        self.held = answer["objects"].clone();
        self.rows = answer["table"].as_array().cloned().unwrap_or_default();
        self.state = answer["state"].clone();
        self.said = answer["said"].clone();
        self.hops = answer["hops"].as_array().cloned().unwrap_or_default();
        self.last = answer["out"].as_array().cloned().unwrap_or_default();
        answer
    }

    /// A row in the store WITHOUT a pass: what the apps wrote while the cell was down.
    /// The cell reads the store only at its boot, so this is a prior state -- before the
    /// first message, or before a `kill`.
    pub fn put(&mut self, row: Value) {
        self.ask(json!({"op": "table_put", "row": row}));
    }

    /// Take a row out of the store WITHOUT a pass (see `put`).
    pub fn withdraw(&mut self, owner: &str, view_id: &str) {
        self.ask(json!({"op": "table_del", "owner": owner, "view_id": view_id}));
    }

    /// One event of § 4.1 (`json!({"kind": "stroke"})`, a tap, a verdict …) as the message
    /// that carries it. The patch calls this message sent to the display come back.
    pub fn pass(&mut self, event: Value, now: u64) -> Vec<Value> {
        self.ask(json!({"op": "event", "event": event, "now": now}));
        self.patch()
    }

    /// Any document, as the hive would hand it to the cell (a notice, a raw event).
    pub fn send(&mut self, doc: Value, now: u64) -> Vec<Value> {
        self.ask(json!({"op": "now", "ms": now}));
        self.ask(json!({"op": "send", "doc": doc}));
        self.patch()
    }

    /// A write of one app, through the door: the row as `in_view` from its owner.
    pub fn write(&mut self, row: Value, now: u64) -> Vec<Value> {
        let content: Value =
            meclaw_core::serde_json::from_str(row["content"].as_str().unwrap_or("{}"))
                .expect("the row's content is JSON");
        let components: Value =
            meclaw_core::serde_json::from_str(row["components"].as_str().unwrap_or("[]"))
                .expect("the row's components are JSON");
        let doc = json!({
            "body": {
                "messages": [], "view_id": row["view_id"], "region": row["region"],
                "ord": row["ord"], "kind": row["kind"], "content": content,
                "components": components, "ttl_ms": row["ttl_ms"],
            },
            "envelope": {
                "reply_to": row["owner"],
                "header": {"hop": {"route": "in_view"}, "context": {}},
            },
        });
        self.send(doc, now)
    }

    /// An app takes its view down, through the door.
    pub fn take_down(&mut self, owner: &str, view_id: &str, now: u64) -> Vec<Value> {
        let doc = json!({
            "body": {"messages": [], "view_id": view_id},
            "envelope": {
                "reply_to": owner,
                "header": {"hop": {"route": "in_withdraw"}, "context": {}},
            },
        });
        self.send(doc, now)
    }

    /// The child dies; the store and the tree stay (a restart of the cell).
    pub fn kill(&mut self) {
        self.ask(json!({"op": "kill"}));
    }

    /// A fresh `web`: no objects, no pages.
    pub fn web_reset(&mut self) {
        self.ask(json!({"op": "web_reset"}));
    }

    /// An object in the display's tree WITHOUT a patch: what `web` held before the cell
    /// woke. Seen by the next boot's `read` -- before the first message, or before `kill`.
    pub fn web_put(&mut self, object: Value) {
        self.ask(json!({"op": "web_put", "object": object}));
    }

    /// Props of an object `web` holds, changed WITHOUT a patch (a tree an older version of
    /// the cell left behind). Seen by the next boot's `read`.
    pub fn web_update(&mut self, id: &str, props: Value) {
        self.ask(json!({"op": "web_update", "id": id, "props": props}));
    }

    /// `web` refuses the next patch whole (its first leg fails, no leg is applied).
    pub fn refuse_next_patch(&mut self) {
        self.ask(json!({"op": "refuse_next_patch"}));
    }

    /// Hold the replies of the store and of `web` back (`true`) until `flush`: the answers
    /// still in the air while the next message arrives.
    pub fn hold(&mut self, on: bool) {
        self.ask(json!({"op": "hold", "on": on}));
    }

    /// Deliver the replies held back; the patch calls they caused come back.
    pub fn flush(&mut self) -> Vec<Value> {
        self.ask(json!({"op": "flush"}));
        self.patch()
    }

    /// The patch calls of the last message, in order (every `patch` hop to `web`).
    pub fn patch(&self) -> Vec<Value> {
        self.hops
            .iter()
            .filter(|h| h["route"] == "patch")
            .flat_map(|h| h["calls"].as_array().cloned().unwrap_or_default())
            .collect()
    }

    /// The outer emissions of the last message on one lane.
    pub fn lane(&self, route: &str) -> Vec<&Value> {
        self.last
            .iter()
            .filter(|e| e["header"]["route"] == route)
            .collect()
    }

    /// The internal hops of the last message: `{route, request, header, ops, calls}` for
    /// every emission to the store (`views`), the tree (`read`) and the display (`patch`).
    pub fn hops(&self) -> &[Value] {
        &self.hops
    }

    /// The `views` store as it stands.
    pub fn table(&self) -> Vec<Value> {
        self.rows.clone()
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

    /// The screen state of § 3: the curator's memory, as the driver reads it.
    pub fn screen_state(&self) -> Value {
        if self.state.is_null() {
            return json!({});
        }
        self.state.clone()
    }

    /// What the last pass said out loud (§ 4.7): every refusal and every error of it, as
    /// `["refused" | "error", ...]` rows -- the list the cell itself keeps in memory to decide
    /// what to say once (`ram()["said"]`, filled by `spoken_of` in compose.py), read out of
    /// the driver and not computed again here. Until display 2.7.0 it stood as `said` in
    /// the state row.
    pub fn said(&self) -> Value {
        self.said.clone()
    }

    /// One curator value of one window.
    pub fn curator(&self, oid: &str, key: &str) -> Value {
        self.screen_state()["views"][oid]["curator"][key].clone()
    }
}

impl Drop for Screen {
    /// Close the driver's stdin (it ends at EOF) and reap it. Only this child, never a
    /// signal to anything else.
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.wait();
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
