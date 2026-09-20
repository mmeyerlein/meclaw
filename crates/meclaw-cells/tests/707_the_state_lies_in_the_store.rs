//! display-hive.md § 3.1 (OR-H2): the screen state lies in the store `views`, as ONE row
//! beside the app rows -- not on the display's objects. A read pass emits a second store
//! bundle that takes the row down and puts it up again; a pass that is handed that row back
//! computes the same curator values instead of guessing them anew.
//!
//! The script runs the way a `code` cell runs it: as a subprocess, with the read-pass
//! document on stdin.

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

fn run(doc: &Value) -> Vec<Value> {
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

fn view_row(view_id: &str, tree: Value, now: u64) -> Value {
    json!({
        "owner": "alex", "view_id": view_id, "region": "main", "ord": 0,
        "kind": "component", "content": tree.to_string(), "components": "[]",
        "ttl_ms": 0, "updated_at": now,
    })
}

/// One read pass with the plan the views pass would have built: the app rows, the state row
/// of the pass before (or null), the moment and the event.
fn read_pass(views: &[Value], state: Value, event: Value, now: u64) -> Vec<Value> {
    let plan = json!({
        "views": views, "state": state, "define": [], "now": now, "event": event,
    });
    let doc = json!({
        "params": {"screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
                   "default_screen": "monitor"},
        "body": {"messages": [{
            "origin": "tool", "type": "tool_result", "id": "d-query",
            "text": json!({"objects": []}).to_string(),
        }]},
        "envelope": {"header": {
            "hop": {"operation": "query"},
            "context": {"display_origin": "read", "display_views": plan.to_string()},
        }},
    });
    run(&doc)
}

/// The store calls a read pass emits for the state row.
///
/// Two shapes since GH #744. The first creation is the two-leg bundle it always was --
/// `delete` on `(owner, view_id)` then `insert`, one message, in that order, because the
/// table declares no primary key and that pair IS the identity. Every later pass is ONE
/// `update` that names the version it read (`where updated_at`), so a second pass that
/// computed on the same row cannot overwrite the first one's conclusion.
fn state_writes(emissions: &[Value]) -> Vec<Value> {
    let bundles: Vec<&Value> = emissions
        .iter()
        .filter(|e| {
            e["header"]["route"] == "views"
                && e["header"]["display_request"]
                    .as_str()
                    .is_some_and(|r| r.contains("\"state\": true") || r.contains("\"state\":true"))
        })
        .collect();
    assert_eq!(bundles.len(), 1, "exactly one state bundle: {emissions:?}");
    let legs = bundles[0]["messages"]
        .as_array()
        .expect("a bundle has messages");
    let calls: Vec<Value> = legs
        .iter()
        .map(|t| {
            meclaw_core::serde_json::from_str(t["text"].as_str().expect("a call"))
                .expect("a call is JSON")
        })
        .collect();
    calls
}

/// The one call of a pass whose row already stands: the conditional write.
fn state_write(emissions: &[Value]) -> Value {
    let calls = state_writes(emissions);
    assert_eq!(calls.len(), 1, "one conditional write: {calls:?}");
    calls[0].clone()
}

/// The row a state write puts up, whichever spelling it used.
fn written_row(call: &Value) -> Value {
    match call["operation"].as_str().unwrap_or("") {
        "insert" => call["row"].clone(),
        "update" => {
            let mut row = json!({"owner": "display", "view_id": "screen-state"});
            for (key, value) in call["set"].as_object().expect("an update sets columns") {
                row[key.clone()] = value.clone();
            }
            row
        }
        other => panic!("a state write is an insert or an update, never {other:?}: {call:?}"),
    }
}

#[test]
fn the_state_is_written_as_one_row_and_read_back() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let tree = json!({
        "component": "display-pane",
        "props": {"title": "Note", "context": "system", "relevance": "0.9", "topic": "note:1"},
    });
    let rows = vec![view_row("n1", tree, 100_000)];

    // Pass one: no state row yet. The app's write is the event.
    let first = read_pass(
        &rows,
        Value::Null,
        json!({"kind": "app_write", "oid": "view.alex.n1", "view": {
            "title": "Note", "context": "system", "relevance": "0.9", "topic": "note:1",
        }}),
        100_000,
    );
    let birth = state_writes(&first);
    assert_eq!(
        birth
            .iter()
            .map(|c| c["operation"].clone())
            .collect::<Vec<Value>>(),
        vec![json!("delete"), json!("insert")],
        "the pass found no state row, and the birth holds the identity itself: delete on \
         `(owner, view_id)` then insert, one message, in that order -- the table declares \
         no primary key (GH #744): {birth:?}"
    );
    assert_eq!(birth[0]["table"], "views");
    assert_eq!(birth[0]["where"]["owner"], "display");
    assert_eq!(birth[0]["where"]["view_id"], "screen-state");
    let row = &written_row(&birth[1]);
    assert_eq!(row["owner"], "display");
    assert_eq!(row["view_id"], "screen-state");
    assert_eq!(row["kind"], "state");
    let held: Value = meclaw_core::serde_json::from_str(row["content"].as_str().expect("content"))
        .expect("the state row's content is JSON");
    assert_eq!(
        held["views"]["view.alex.n1"]["curator"]["since"], 100_000,
        "the pass wrote the touch into the state row: {held}"
    );

    // Pass two: a stroke, with that row handed back. The curator value was READ, not guessed.
    let second = read_pass(&rows, row.clone(), json!({"kind": "stroke"}), 101_000);
    let write2 = state_write(&second);
    assert_eq!(
        write2["operation"], "update",
        "a row that stands is replaced under a condition, never blind (GH #744): {write2}"
    );
    assert_eq!(
        write2["where"]["updated_at"], row["updated_at"],
        "and the condition is the version this pass READ: {write2}"
    );
    let row2 = written_row(&write2);
    let held2: Value =
        meclaw_core::serde_json::from_str(row2["content"].as_str().expect("content"))
            .expect("the state row's content is JSON");
    assert_eq!(
        held2["views"]["view.alex.n1"]["curator"]["since"], 100_000,
        "a stroke touches nothing; `since` stands: {held2}"
    );
}

/// The reply to a state write, and what it may say.
///
/// It starts no new pass -- a pass out of it would be an endless round, and the row it
/// wrote is the one the cell just computed. Since GH #744 that holds for the write that
/// LANDED: a write the store refused repeats its own pass, and that case is locked in
/// `744_two_taps_in_one_round_trip.rs`. Since GH #765 (way A) the landed reply has one
/// thing to say after all, and exactly one: the drawing the pass handed to this very
/// write. A pass that rendered nothing still answers with silence.
#[test]
fn the_state_row_is_no_view_and_its_write_answers_with_the_drawing_and_nothing_else() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The request carries its `retry` mark and the reply says the row moved: this has to
    // be the answer to a write that WORKED, not to a request with no mark on it.
    let landed = |request: Value| {
        json!({
            "params": {},
            "body": {"messages": [{"origin": "tool", "type": "tool_result", "id": "s-update",
                                   "text": "null"}],
                     "results": [{"tool_call_id": "s-update", "operation": "update",
                                  "rows_affected": 1, "duration_ms": 1}]},
            "envelope": {"header": {
                "hop": {"operation": "update", "rows_affected": 1},
                "context": {"display_origin": "views",
                            "display_request": request.to_string()},
            }},
        })
    };

    assert!(
        run(&landed(json!({"state": true, "retry": {"tick": true}}))).is_empty(),
        "a pass that drew nothing answers its own write with silence"
    );

    // And the one thing a landed write does say: the calls the pass computed, which
    // travelled on the request because this cell has no memory between two messages.
    let call = json!({"op": "object.update", "id": "display.root", "props": {"dock": "shown"}});
    let out = run(&landed(
        json!({"state": true, "retry": {"tick": true}, "patch": [call.clone()]}),
    ));
    assert_eq!(out.len(), 1, "one emission, and it is the drawing: {out:?}");
    assert_eq!(out[0]["header"]["route"], "patch", "{out:?}");
    let sent: Vec<Value> = out[0]["messages"]
        .as_array()
        .expect("a bundle has messages")
        .iter()
        .map(|turn| {
            meclaw_core::serde_json::from_str(turn["text"].as_str().expect("a call"))
                .expect("a call is JSON")
        })
        .collect();
    assert_eq!(
        sent,
        vec![call],
        "call for call what the pass handed to its write -- nothing is computed on a \
         reply that carries no state to compute from: {out:?}"
    );
}
