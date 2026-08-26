//! W8 Task 15 (GH #383): the 1.x → 2.0.0 position migration.
//!
//! `canvy@2.0.0` is not an in-place upgrade of a 1.x instance — every address
//! the template offered was removed, which is why the first digit moved. An
//! instance is therefore instantiated fresh beside the old one and the one
//! thing worth carrying across is the one thing a person made by hand: where
//! the boxes are.
//!
//! In 1.x that lived in a `store` cell's `cell.db`, in a table called `canvas`,
//! as `(kind, id, x, y, z)` rows. In 2.0.0 there is no store at all: a position
//! is a prop of a `canvy-node` object inside the display's own object tree, and
//! `scripts/canvy_export_positions.py` is the bridge between the two.
//!
//! What is pinned here:
//!
//! 1. **The script reads the shape the recipe promises** — the three kinds the
//!    `SELECT` names, and nothing else in the table.
//! 2. **It emits the props the layout cell actually declares.** The bundle
//!    patches `n/<cell path>` with `x` and `y`, and those are exactly the id
//!    prefix and the two `editable` props `templates/canvy/layout/layout.py`
//!    defines for `canvy-node`. A migration that hit prop names nobody declared
//!    would be refused prop by prop — so the assertion is made against the
//!    bootstrap bundle the shipped layout produces, never against a constant
//!    retyped here.
//! 3. **Two of the three kinds are dropped, loudly.** A hive shift has no
//!    target in 2.0.0 (frames are derived from their members, GH #170) and the
//!    camera never leaves the browser. They are reported as dropped rather than
//!    silently skipped, because "my hive frames moved" is a thing an operator
//!    has to be told before the fact rather than discover after it.
//! 4. **The bundle round-trips into a real `web` cell.** Not "parses as JSON":
//!    the emitted document is put into a live display that the shipped layout
//!    has already bootstrapped, and the coordinates come back out of that
//!    cell's own database and out of the page a browser would be served.
//! 5. **A row for a cell that is gone is refused and the rest still lands.** A
//!    bundle is applied leg by leg; one `unknown_object` is one leg, and the
//!    recipe says to read `bundle_errors` rather than to assume silence.
//!
//! The script is run **the stdin way** — the program on stdin, the database
//! path in argv. Same rule as `canvy2_pipeline.rs`: a single argv string is
//! capped at 128 KiB (`MAX_ARG_STRLEN`), so a harness that pastes a program
//! into `-c` breaks on size rather than on behaviour (GH #349, GH #279).
//!
//! Free of a provider by construction: no model is involved anywhere here.
//!
//! **R-W8-9.** Nothing in this file touches a deployed tree. The fixtures are
//! synthetic, built row by row a few lines below, and every colony here lives
//! in a `TempDir`.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under `dir`, so a claim about the substrate can be checked
/// against the substrate instead of against a memory of it.
fn walk_rs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_rs(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

/// The shipped export script, or `None` when this checkout does not carry it.
///
/// R2b: a file that silently disappears makes these tests skip rather than
/// pass.
fn export_script() -> Option<std::path::PathBuf> {
    let p = core_root().join("scripts/canvy_export_positions.py");
    p.exists().then_some(p)
}

/// The shipped `canvy` template, or `None` — same guard, same reason.
fn shipped_canvy() -> Option<std::path::PathBuf> {
    let root = core_root().join("templates/canvy");
    for rel in ["config.json", "layout/config.json", "MIGRATION.md"] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok()
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ───────────────────────────────────────────── the synthetic 1.x database

/// One row of the 1.x `canvas` table.
struct Row(&'static str, &'static str, i64, i64, i64);

/// The 1.x table, built from scratch.
///
/// The schema is the one the recipe's `SELECT` names — `kind, id, x, y, z` in a
/// table called `canvas`. It is written here rather than copied from a real
/// instance on purpose: a fixture that came off somebody's machine would carry
/// that machine's cell paths, and this is a shape test, not an archaeology
/// test.
fn one_x_store(dir: &std::path::Path, rows: &[Row]) -> std::path::PathBuf {
    let db = dir.join("cell.db");
    let conn = rusqlite::Connection::open(&db).expect("open");
    conn.execute_batch(
        "CREATE TABLE canvas (
             kind TEXT NOT NULL,
             id   TEXT NOT NULL,
             x    INTEGER,
             y    INTEGER,
             z    INTEGER,
             PRIMARY KEY (kind, id)
         );",
    )
    .expect("schema");
    for Row(kind, id, x, y, z) in rows {
        conn.execute(
            "INSERT INTO canvas (kind, id, x, y, z) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![kind, id, x, y, z],
        )
        .expect("insert");
    }
    drop(conn);
    db
}

/// The rows a hand-arranged 1.x canvas holds.
///
/// Four hand-placed cells, one hive that was dragged as a group, one camera —
/// and one row of a kind the `SELECT` does not name, which is what proves the
/// `WHERE` clause is doing something. `a/two` is written **without** the
/// leading slash on purpose: 1.x wrote both forms over its life, and the layout
/// cell's id space has neither (`n/` plus the path stripped of its slashes).
const HAND_ARRANGED: &[Row] = &[
    Row("node", "/a/one", 4321, 1234, 0),
    Row("node", "a/two", 900, 120, 0),
    Row("node", "/b/three", 1500, 640, 0),
    Row("node", "/b/four", 1500, 980, 0),
    Row("hive_shift", "a", 40, -15, 0),
    Row("camera", "", 100, 200, 2),
    Row("schema_version", "canvas", 3, 0, 0),
];

/// The same canvas, plus a box for a cell the colony has since lost.
const WITH_A_GHOST: &[Row] = &[
    Row("node", "/a/one", 4321, 1234, 0),
    Row("node", "a/two", 900, 120, 0),
    Row("node", "/b/three", 1500, 640, 0),
    Row("node", "/b/four", 1500, 980, 0),
    Row("node", "/b/retired", 77, 88, 0),
    Row("hive_shift", "a", 40, -15, 0),
    Row("camera", "", 100, 200, 2),
];

// ───────────────────────────────────────────────────────── the harnesses

/// Run the shipped export script **the stdin way**: the program on stdin, the
/// database path in argv.
///
/// `python3 - <path>` puts `-` in `sys.argv[0]` and the path in `sys.argv[1]`,
/// which is the same argv the documented invocation
/// (`python3 scripts/canvy_export_positions.py <path>`) produces — so this
/// harness proves the shipped file, not a copy of it, and does it without ever
/// putting a program into an argv slot (GH #349).
fn run_export(db: &std::path::Path) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let script = std::fs::read_to_string(export_script().expect("the script")).expect("read");
    let mut child = Command::new("python3")
        .arg("-")
        .arg(db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    // Dropped, not merely borrowed: python reads the program until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(script.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// The export document, with the exit status asserted first.
fn export(db: &std::path::Path) -> Value {
    let out = run_export(db);
    assert!(
        out.status.success(),
        "the export exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "the export's stdout is not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The `op` args of every `tool_call` turn of a body, in call order.
fn calls_of(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|t| t["type"] == "tool_call")
        .map(|t| {
            meclaw_core::serde_json::from_str(t["text"].as_str().expect("text")).expect("args")
        })
        .collect()
}

/// Run a cell's shipped script exactly as the `code` cell runs it — the runner
/// from `params.runner`, the script from `params.script_inline`, both on stdin.
fn run_shipped(root: &std::path::Path, cell: &str, doc: Value) -> Vec<Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let cfg = read_json(&root.join(cell).join("config.json"));
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(&doc.to_string()).unwrap(),
    );
    let mut child = Command::new(runner)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn runner");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{cell} exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice::<Value>(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell} stdout is not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    }) {
        Value::Array(items) => items,
        other => vec![other],
    }
}

/// The stdin document the substrate hands a `code` cell: exactly three keys.
fn stdin_doc(body: Value, hop: Value, context: Value) -> Value {
    json!({
        "envelope": {
            "header": { "context": context, "hop": hop },
            "target": "/canvy/layout",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64
        },
        "body": body,
        "params": {}
    })
}

/// The colony the hand-arranged canvas was drawn against.
fn fixture_graph() -> Value {
    json!({
        "scope": "/",
        "nodes": [
            {"path": "/a/one", "cell_type": "code"},
            {"path": "/a/two", "cell_type": "store"},
            {"path": "/b/three", "cell_type": "llm"},
            {"path": "/b/four", "cell_type": "timer"},
        ],
        "edges": [
            {"id": "e1", "from": "/a/one", "to": "/a/two"},
            {"id": "e2", "from": "/a/two", "to": "/b/three"},
        ],
    })
}

/// The bootstrap bundle the shipped layout emits for an empty display: pass 1
/// asks, the refusal answers, and the same pass defines and creates everything.
fn bootstrap_bundle(root: &std::path::Path) -> Value {
    let ask = run_shipped(
        root,
        "layout",
        stdin_doc(
            json!({ "messages": [], "graph": fixture_graph() }),
            json!({ "route": "snapshot" }),
            json!({}),
        ),
    );
    let hop = ask[0]["header"]["canvy_graph"]
        .as_str()
        .expect("the graph rides on the hop")
        .to_string();
    let boot = run_shipped(
        root,
        "layout",
        stdin_doc(
            json!({"messages": [{
                "origin": "tool", "type": "tool_result", "id": "q",
                "text": "no page declares the route \"/\"",
            }]}),
            json!({ "operation": "query", "error_code": "invalid_input" }),
            json!({ "canvy_origin": "layout", "canvy_graph": hop }),
        ),
    );
    boot.into_iter().next().expect("a bootstrap bundle")
}

// ─────────────────────────────────────────── what the export document says

/// The three kinds the recipe names are the three the script reads, and only
/// the first of them survives the crossing.
#[test]
fn the_export_carries_the_node_rows_and_names_what_it_drops() {
    if export_script().is_none() || !have_python() {
        return;
    }
    let td = TempDir::new().expect("td");
    let db = one_x_store(td.path(), HAND_ARRANGED);
    let doc = export(&db);

    let calls = calls_of(&doc);
    assert_eq!(calls.len(), 4, "one patch per hand-placed cell: {calls:#?}");
    for c in &calls {
        assert_eq!(
            c["op"], "object.update",
            "a migration patches, it never creates: {c}"
        );
    }

    // The id space, and the normalisation that goes with it. 1.x wrote both
    // `/a/one` and `a/two`; the layout cell's id is `n/` plus the path with its
    // slashes stripped, and there is exactly one of those per cell.
    let ids: Vec<&str> = calls.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["n/a/one", "n/a/two", "n/b/four", "n/b/three"]);

    // The coordinates travel whole, as integers — the prop schema types both
    // `int`, and a string there would render as a broken transform.
    let one = calls.iter().find(|c| c["id"] == "n/a/one").unwrap();
    assert_eq!(
        one["props"],
        json!({"x": 4321, "y": 1234, "pinned": "1"}),
        "a replayed position is a HAND-placed one and has to say so (GH #415): \
         without the marker the very next layout tick lays the cell out again \
         and the migration's one promise is quietly broken"
    );
    assert!(
        one["props"]["x"].is_i64() && one["props"]["y"].is_i64(),
        "positions are numbers, not strings: {one}"
    );

    // The two kinds that have no target in 2.0.0 are REPORTED, not skipped.
    let dropped = &doc["canvy_migration"]["dropped"];
    assert_eq!(
        dropped["hive_shift"]["rows"],
        json!(1),
        "a hive shift has no target: frames are derived from their members \
         (GH #170), so this is a thing the operator has to be told: {dropped}"
    );
    assert_eq!(dropped["camera"]["rows"], json!(1));
    for kind in ["hive_shift", "camera"] {
        assert!(
            dropped[kind]["because"]
                .as_str()
                .is_some_and(|s| s.len() > 20),
            "a drop without a reason is a silence with a number on it: {dropped}"
        );
    }

    // A kind the SELECT does not name is not in the document at all — neither
    // carried nor counted as dropped.
    assert!(
        !doc.to_string().contains("schema_version"),
        "the WHERE clause reads three kinds and leaves the rest of the table \
         alone: {doc}"
    );
    assert_eq!(doc["canvy_migration"]["carried"], json!(4));
}

/// The same database twice is the same bytes twice.
///
/// A migration bundle is a thing an operator diffs, re-runs and keeps beside a
/// receipt. A random tool-call id would make two runs of one export look like
/// two different migrations.
#[test]
fn the_export_is_reproducible() {
    if export_script().is_none() || !have_python() {
        return;
    }
    let td = TempDir::new().expect("td");
    let db = one_x_store(td.path(), HAND_ARRANGED);
    let first = run_export(&db);
    let second = run_export(&db);
    assert_eq!(
        first.stdout, second.stdout,
        "two exports of one database must be byte-identical"
    );
}

/// A path that is not a database, and a database with no `canvas` table, are
/// both refused — with nothing on stdout.
///
/// The failure mode this closes is the one that costs the most: an empty
/// bundle written into a file, applied without a second thought, and read
/// afterwards as "there was nothing to migrate".
#[test]
fn an_unreadable_source_is_refused_with_an_empty_stdout() {
    if export_script().is_none() || !have_python() {
        return;
    }
    let td = TempDir::new().expect("td");

    let missing = td.path().join("nowhere/cell.db");
    let out = run_export(&missing);
    assert!(
        !out.status.success(),
        "a database that is not there is not an export"
    );
    assert!(
        out.stdout.is_empty(),
        "nothing may reach stdout on a refusal"
    );

    let empty = td.path().join("empty.db");
    rusqlite::Connection::open(&empty)
        .expect("open")
        .execute_batch("CREATE TABLE something_else (a INTEGER);")
        .expect("schema");
    let out = run_export(&empty);
    assert!(
        !out.status.success(),
        "a database with no `canvas` table is not a canvy 1.x store"
    );
    assert!(out.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("canvas"),
        "the refusal has to name what it looked for: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The props the migration writes are the props the shipped layout declares.
///
/// This is the assertion that keeps the two halves of the recipe from drifting
/// apart. Both sides are read off the shipped tree: the component definition
/// comes out of the layout cell's own bootstrap bundle, the prop names out of
/// the export. A migration that hit prop names nobody declared would be refused
/// key by key with `invalid_input`, and one that hit an id space nobody uses
/// would be refused with `unknown_object` — both silently, one leg at a time.
#[test]
fn the_migration_writes_only_props_the_component_declares() {
    let (Some(canvy), Some(_)) = (shipped_canvy(), export_script()) else {
        return;
    };
    if !have_python() {
        return;
    }
    let boot = calls_of(&bootstrap_bundle(&canvy));
    let node = boot
        .iter()
        .find(|c| c["op"] == "component.define" && c["name"] == "canvy-node")
        .expect("the layout defines canvy-node");
    let editable: Vec<&str> = node["editable"]
        .as_array()
        .expect("editable")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    let td = TempDir::new().expect("td");
    let db = one_x_store(td.path(), HAND_ARRANGED);
    for c in calls_of(&export(&db)) {
        for key in c["props"].as_object().expect("props").keys() {
            assert!(
                editable.contains(&key.as_str()),
                "the migration writes {key:?}, which `canvy-node` does not \
                 declare editable ({editable:?})"
            );
            // Declared, and declared as the kind of value the migration
            // actually writes: the coordinates are integers, and the pin
            // marker is the `"text"` flag this template language spells a
            // boolean with (GH #415).
            let want = if key == "pinned" { "text" } else { "int" };
            assert_eq!(
                node["prop_schema"][key.as_str()],
                json!(want),
                "{key:?} is not a {want} prop of canvy-node: {}",
                node["prop_schema"]
            );
        }
        // And the id prefix the layout gives a cell's box.
        let id = c["id"].as_str().unwrap();
        assert!(
            boot.iter()
                .any(|b| b["op"] == "object.create" && b["id"] == id),
            "{id} is not an object the layout creates: the id spaces have drifted"
        );
    }
}

// ────────────────────────────────────── and it lands in a real display

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

struct Live {
    port: u16,
    cell_dir: std::path::PathBuf,
    mailbox: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
}

/// A `web` cell with an empty database — what a fresh `canvy@2.0.0` starts
/// with, since a ref directory carries no seed.
async fn start(cell_dir: &std::path::Path) -> Live {
    let port = free_port();
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(64);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/canvy/web"),
            json!({ "port": port }),
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("Active");
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if reqwest::get(format!("http://127.0.0.1:{port}/"))
            .await
            .is_ok()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the cell never bound its port");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Live {
        port,
        cell_dir: cell_dir.to_path_buf(),
        mailbox: sender,
        out_rx,
        _stop: stop_tx,
        join,
    }
}

/// Send one body of `tool_call` turns and read the display's reply.
async fn send(live: &mut Live, body: Value) -> Value {
    let msg = MessageBuilder::new(Path::new("/canvy/web"))
        .body(Body::Inline(body))
        .reply_to(Path::new("/canvy/layout"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");
    tokio::time::timeout(Duration::from_secs(60), live.out_rx.recv())
        .await
        .expect("the display must answer a bundle")
        .expect("an emission")
        .content
}

/// The bundle the layout's bootstrap produces, as a body.
fn as_body(calls: &[Value]) -> Value {
    json!({
        "messages": calls.iter().enumerate().map(|(i, args)| json!({
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": format!("c{i}")
        })).collect::<Vec<_>>()
    })
}

/// The `x`/`y` a display holds for one node object, straight out of its own
/// database.
fn held(live: &Live, id: &str) -> (i64, i64) {
    let conn = rusqlite::Connection::open(live.cell_dir.join("cell.db")).expect("open");
    let props: String = conn
        .query_row("SELECT props FROM objects WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&props).expect("props json");
    (
        v["x"].as_i64().unwrap_or_else(|| panic!("{id}: x {props}")),
        v["y"].as_i64().unwrap_or_else(|| panic!("{id}: y {props}")),
    )
}

/// The whole recipe, end to end: a 1.x store, a fresh 2.0.0 display the shipped
/// layout has drawn once, and the hand-made positions replayed on top of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_exported_bundle_round_trips_into_a_web_cell() {
    let (Some(canvy), Some(_)) = (shipped_canvy(), export_script()) else {
        return;
    };
    if !have_python() {
        return;
    }

    let td = TempDir::new().expect("td");
    let db = one_x_store(td.path(), HAND_ARRANGED);
    let migration = export(&db);
    let boot = calls_of(&bootstrap_bundle(&canvy));

    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;

    // Step one of the recipe: the display draws itself once. Until it has, the
    // objects a migration patches do not exist.
    let reply = send(&mut live, as_body(&boot)).await;
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(0),
        "the bootstrap must land before anything is replayed onto it: {reply}"
    );
    let computed = held(&live, "n/a/one");

    // Step two: the migration, verbatim, as it came off the script.
    let reply = send(&mut live, migration.clone()).await;
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(0),
        "every leg of a clean migration lands: {reply}"
    );

    assert_eq!(held(&live, "n/a/one"), (4321, 1234));
    assert_eq!(held(&live, "n/a/two"), (900, 120));
    assert_eq!(held(&live, "n/b/three"), (1500, 640));
    assert_ne!(
        held(&live, "n/a/one"),
        computed,
        "the point of the exercise is that the hand-made position wins over the \
         computed one"
    );

    // The other props of the object are untouched: `object.update` merges per
    // key, so a migration that named only `x` and `y` did not quietly blank the
    // colour, the type or the name.
    let conn = rusqlite::Connection::open(live.cell_dir.join("cell.db")).expect("open");
    let props: String = conn
        .query_row("SELECT props FROM objects WHERE id = 'n/a/one'", [], |r| {
            r.get(0)
        })
        .expect("row");
    drop(conn);
    let v: Value = meclaw_core::serde_json::from_str(&props).expect("json");
    assert_eq!(v["path"], json!("a/one"));
    assert_eq!(v["type"], json!("code"));

    // …and the page a browser is served carries the moved box.
    let body = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    assert!(
        body.contains("translate(4321,1234)"),
        "the migrated position is not in the rendered page"
    );

    live.join.abort();
}

/// A row naming a cell the colony no longer has is refused as its own leg, and
/// the rest of the migration still lands.
///
/// This is why the recipe tells an operator to read `bundle_errors` instead of
/// assuming a silent success: a 1.x canvas outlived cells, and 1.x had no way
/// to tell a vanished cell from a renamed one (GH #184).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_for_a_cell_that_is_gone_is_one_refused_leg() {
    let (Some(canvy), Some(_)) = (shipped_canvy(), export_script()) else {
        return;
    };
    if !have_python() {
        return;
    }

    let td = TempDir::new().expect("td");
    let db = one_x_store(td.path(), WITH_A_GHOST);
    let migration = export(&db);
    assert_eq!(
        calls_of(&migration).len(),
        5,
        "the script carries every node row — it has no way to know which cells \
         still exist, and guessing would be worse"
    );

    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;
    let reply = send(&mut live, as_body(&calls_of(&bootstrap_bundle(&canvy)))).await;
    assert_eq!(reply["header"]["bundle_errors"], json!(0));

    let reply = send(&mut live, migration).await;
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(1),
        "one row, one refused leg — not a refused bundle: {reply}"
    );
    // `results[]` is a BODY slot, never a turn: `$defs/TurnObject` is
    // `additionalProperties: false`, so per-op metadata on a turn would
    // dead-letter the whole reply (the store's GH #295 rule).
    let refused: Vec<&Value> = reply["results"]
        .as_array()
        .expect("a bundle answers with a results slot")
        .iter()
        .filter(|r| r["error_code"].is_string())
        .collect();
    assert_eq!(refused.len(), 1, "{reply}");
    assert_eq!(refused[0]["error_code"], json!("unknown_object"));

    // …and the four that name cells the colony still has are in place.
    assert_eq!(held(&live, "n/a/one"), (4321, 1234));
    assert_eq!(held(&live, "n/b/four"), (1500, 980));

    live.join.abort();
}

// ──────────────────────────────────────────────── the recipe is shipped

/// The recipe is the artifact this task ships, so its absence is a failure and
/// not a skip once the template is here at all.
///
/// R-W8-9: this repository ships the recipe; running it against a live tree
/// stays the operator's act, and the document has to say so in its own words.
#[test]
fn the_recipe_ships_beside_the_template() {
    let Some(canvy) = shipped_canvy() else { return };
    let doc = std::fs::read_to_string(canvy.join("MIGRATION.md")).expect("MIGRATION.md");

    // The version the recipe tells an operator to instantiate is DERIVED from
    // the template, never written twice. A recipe naming a version the library
    // no longer ships does not resolve, and pinning the literal here would mean
    // every third-digit repair is a test edit that proves nothing.
    let shipped: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(canvy.join("template.json")).expect("template.json"),
    )
    .expect("template.json parses");
    let target = format!("canvy@{}", shipped["version"].as_str().expect("a version"));
    assert!(
        doc.contains(&target),
        "the recipe must instantiate the version the library ships ({target}) — \
         a migration that names an older one sends the operator to a template \
         that is not there"
    );

    // GH #403 (§ 2d drift lock). Step 5 can deactivate a whole subtree and the
    // § 6 undo does not cover the edge that causes it. Both halves:
    //
    //   prose      — the recipe warns before the mutation and retracts the
    //                unqualified undo, naming the refusal an operator will see;
    //   mechanism  — those refusals are real `error_code` strings the substrate
    //                emits, asserted against the source rather than quoted from
    //                a report. A recipe that names a code the substrate cannot
    //                produce sends the reader looking for the wrong thing.
    for phrase in [
        "Pre-flight",
        "only boundary-crossing edge",
        "Zero means stop",
        "__never__",
        "cannot be re-drawn",
        "one-shot",
    ] {
        assert!(
            doc.contains(phrase),
            "the recipe must still carry {phrase:?} — step 5 can take a colony's \
             activity to zero and step 6 does not cover the edge that does it \
             (GH #403). A migration that drops the warning is the version that \
             was measured at 47 active cells to 0."
        );
    }
    let colony_src = core_root().join("crates/meclaw-colony/src");
    for code in ["hive_port_boundary", "stop_wiring_unavailable"] {
        assert!(
            doc.contains(code),
            "the recipe names the refusal an operator hits, not a paraphrase: \
             {code} is missing"
        );
        let found = walk_rs(&colony_src)
            .iter()
            .any(|f| std::fs::read_to_string(f).is_ok_and(|s| s.contains(code)));
        assert!(
            found,
            "the recipe promises the refusal `{code}`, and no source file under \
             crates/meclaw-colony/src/ produces it — the prose outlived its \
             mechanism, which is the whole failure mode § 2d exists for"
        );
    }

    for token in [
        "scripts/canvy_export_positions.py",
        "object.update",
        "bundle_errors",
        "remove_nodes",
        "unknown_object",
    ] {
        assert!(
            doc.contains(token),
            "the recipe never mentions {token:?} — a step is missing"
        );
    }
    assert!(
        doc.contains("operator"),
        "the recipe has to say whose act running it is (R-W8-9)"
    );
    // The `SELECT` is the contract between the recipe and the script: an
    // operator reading one has to be able to check the other.
    let script = export_script().map(|p| std::fs::read_to_string(p).expect("read"));
    if let Some(script) = script {
        assert!(
            script.contains("FROM canvas"),
            "the script no longer reads the 1.x table the recipe names"
        );
        for kind in ["node", "hive_shift", "camera"] {
            assert!(
                doc.contains(kind),
                "the recipe does not account for {kind:?} rows"
            );
            assert!(
                script.contains(kind),
                "the script does not read {kind:?} rows"
            );
        }
    }
}
