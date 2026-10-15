//! GH #1199 (second class search) -- a write that fails keeps the row it would
//! replace.
//!
//! The display's `compose` and the presenter's `stage` kept one row per key
//! by hand: a `delete` of the key and an `insert` of the new row, two legs of
//! a store bundle that is no transaction. Both cells read from memory, so no
//! reader falls into the gap -- but an insert leg that fails (a full disk, a
//! locked file) left the key with NO row, and after a restart the view, the
//! turn in flight, the journal line, the topic or the work row was gone. The
//! store's `insert` with `replace` removes and writes in one step, and a
//! failure takes the removal back with it (pinned generically in
//! `store::ops::tests::gh1199_a_replace_whose_insert_fails_keeps_the_old_rows`).
//!
//! This file pins the class on the shipped writers: each helper's legs are run
//! twice against a store built from the template's own schema; the second run
//! fails every insert into the table (injected where a loaded store fails it),
//! and the rows the first run wrote still stand. Unit class: the shipped
//! scripts are imported, nothing runs as a cell.

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Imports the two shipped scripts (their `script_inline`, not the source
/// beside them) and prints, per writer: the table, the key the writer keeps
/// its rows under, and the legs it writes.
const DRIVER: &str = r#"
import json, sys

def load(path):
    ns = {"__name__": "gh1199_b"}
    exec(compile(json.load(open(path))["params"]["script_inline"], path, "exec"), ns)
    return ns

def legs(msgs):
    return [json.loads(m["text"]) for m in msgs]

c, s = load(sys.argv[1]), load(sys.argv[2])
view = {"owner": "/apps/a", "view_id": "v1", "region": "main", "ord": 0, "kind": "card",
        "content": "{}", "components": "[]", "ttl_ms": 0, "updated_at": 1}
rec = {"turn_id": "t1", "topic": "sample", "p": 0.9, "lead": "", "also": "",
       "fallback": "", "late": False, "t_verdict": None, "t_window": None,
       "t_content": None, "model": "m", "missing": [], "audience_set": None}
known = {"a": {"owner": "/apps/a", "manifest": "{}", "at": 1},
         "b": {"owner": "/apps/a", "manifest": "{}", "at": 1},
         "z": {"owner": "/apps/other", "manifest": "{}", "at": 1}}
work = {"seq": 1, "session": "s1", "turn_id": "t1", "tool": "bash", "call_id": "c1",
        "state": "done", "summary": "ls", "path": "", "exit_code": 0,
        "audience_set": "", "at": 1}
print(json.dumps([
    ["compose write_row", "views", {"owner": "/apps/a", "view_id": "v1"},
     legs(c["write_row"]("/apps/a", view)[0]["messages"])],
    ["compose rest_legs", "views", {"owner": c["REST_OWNER"], "view_id": c["REST_VIEW_ID"]},
     legs(c["rest_legs"]("{}", 1))],
    ["stage w_pending", "pending", {"turn_id": "t1"}, legs(s["w_pending"](rec))],
    ["stage w_journal", "journal", {"turn_id": "t1"}, legs(s["w_journal"](rec))],
    ["stage w_shows", "shows", {"owner_app": "/apps/a"}, legs(s["w_shows"]("/apps/a", known))],
    ["stage w_work_row", "work", {"call_id": "c1"}, legs(s["w_work_row"](work))],
]))
"#;

fn writers() -> Option<Vec<Value>> {
    let compose = repo("templates/display/compose/config.json");
    let stage = repo("templates/presenter/stage/config.json");
    if !compose.exists() || !stage.exists() {
        return None; // a trimmed tree without these templates has nothing to pin
    }
    let out = std::process::Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(DRIVER)
        .arg(&compose)
        .arg(&stage)
        .output()
        .expect("python3 runs");
    assert!(
        out.status.success(),
        "the driver failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the driver prints json"))
}

/// A store as the template declares it, empty.
fn store(schema_of: &str) -> rusqlite::Connection {
    let raw: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo(schema_of)).expect("the store config"),
    )
    .expect("json");
    let params =
        meclaw_cells::store::StoreParams::parse(&raw["params"]).expect("the store params parse");
    let conn = rusqlite::Connection::open_in_memory().expect("an in-memory store");
    meclaw_cells::store::ddl::apply_declared_schema_ddl(&conn, &params)
        .expect("the declared schema applies");
    conn
}

/// Every leg in order, as the store runs a bundle; the error codes it answered.
fn run(conn: &rusqlite::Connection, legs: &Value) -> Vec<&'static str> {
    legs.as_array()
        .expect("legs")
        .iter()
        .filter_map(|leg| {
            meclaw_cells::store::ops::dispatch(conn, leg)
                .unwrap_or_else(|e| panic!("the store refused {leg} as input: {e}"))
                .error_code
        })
        .collect()
}

/// The rows standing under `key`, as the store reads them.
fn rows_under(conn: &rusqlite::Connection, table: &str, key: &Value) -> Vec<Value> {
    let columns: Vec<&String> = key.as_object().expect("key").keys().collect();
    let o = meclaw_cells::store::ops::dispatch(
        conn,
        &json!({"operation": "select", "table": table, "columns": columns, "where": key}),
    )
    .expect("the select runs");
    o.payload.as_array().cloned().unwrap_or_default()
}

#[test]
fn gh1199_b_a_failed_write_keeps_the_row_it_would_replace() {
    let Some(writers) = writers() else { return };
    let mut lost = Vec::new();
    for w in &writers {
        let (name, table, key, legs) = (
            w[0].as_str().expect("name"),
            w[1].as_str().expect("table"),
            &w[2],
            &w[3],
        );
        let schema = if name.starts_with("compose") {
            "templates/display/views/config.json"
        } else {
            "templates/presenter/store/config.json"
        };
        let conn = store(schema);
        assert!(run(&conn, legs).is_empty(), "{name}: the first write runs");
        let before = rows_under(&conn, table, key);
        assert!(!before.is_empty(), "{name}: the first write left its row");
        conn.execute_batch(&format!(
            "CREATE TRIGGER gh1199_b_fail BEFORE INSERT ON \"{table}\" \
             BEGIN SELECT RAISE(ABORT, 'injected insert failure'); END;"
        ))
        .expect("the trigger");
        assert!(
            !run(&conn, legs).is_empty(),
            "{name}: the injected failure reached the write"
        );
        let after = rows_under(&conn, table, key);
        if after != before {
            lost.push(format!(
                "{name}: a failed write left {} of {} rows under {key}",
                after.len(),
                before.len()
            ));
        }
    }
    assert_eq!(writers.len(), 6, "every writer of the class ran");
    assert!(lost.is_empty(), "GH #1199:\n{}", lost.join("\n"));
}
