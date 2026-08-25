//! W8 Task 13 (GH #383): the shipped `canvy@2.0.0` pipeline.
//!
//! What is pinned here is what the template PROMISES:
//!
//! 1. **The inventory and the version.** A file that silently disappears makes
//!    these tests skip rather than pass (R2b), and `template.json` says 2.0.0 —
//!    a removal-shaped change on every address the template offered, which is
//!    the first digit (the `telegram-connector@2.0.0` precedent).
//! 2. **The shipped bytes are the sources.** `layout/config.json` carries
//!    `layout.py` with `canvy.js` and `canvy.css` spliced in, and
//!    `probe/config.json` carries `probe.py`. `scripts/canvy_sync.py --check` is
//!    the gate, asked of the generator itself so that there is exactly one
//!    implementation of the splice.
//! 3. **The three passes**, driven by running the **shipped bytes** out of
//!    `config.json` through `python3` — the same command the `code` cell builds.
//!    A test that ran the `.py` instead would prove the source, not the product.
//! 4. **Query before patch, and a position is kept.** The layout asks what the
//!    display already holds before it writes, and a coordinate the display holds
//!    survives untouched — that is the entire persistence story of a drag, since
//!    a drag never leaves the display cell.
//! 5. **The round terminates.** The acknowledgement of a patch produces nothing.
//!    A cell that cannot recognise the reply to its own write has no way to stop
//!    (GH #161).
//! 6. **It fills a real display.** The emitted bundle is put into an actual
//!    `web` cell and the page is fetched over HTTP: one box per colony cell, one
//!    line per edge, and the hook mounted on markup that carries `phx-hook`.
//!
//! Free of a provider by construction: this hive holds no model at all.

use futures_util::StreamExt;
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

/// Every file the hive is made of. The list is the guard AND the inventory.
const CANVY_FILES: &[&str] = &[
    "config.json",
    "template.json",
    "README.md",
    "refresh/config.json",
    "probe/config.json",
    "probe/probe.py",
    "layout/config.json",
    "layout/layout.py",
    "layout/canvy.js",
    "layout/canvy.css",
    "layout/canvy.test.js",
    "web/config.json",
    // The recipe ships WITH the template (W8 Task 15, R-W8-9): this repository
    // hands over the migration, never the run. A canvy that shipped without it
    // would leave a 1.x operator with a breaking change and no way across.
    "MIGRATION.md",
];

fn shipped_canvy() -> Option<std::path::PathBuf> {
    let root = core_root().join("templates/canvy");
    for rel in CANVY_FILES {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn read_json(p: &std::path::Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Run a shipped script over a real stdin document, handing the script to the
/// runner **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the layout
/// cell's script carries the whole browser half of canvy, so `<runner> -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #349,
/// GH #279). stdin carries the program, so the document rides inside it and is
/// put under `sys.stdin` before the script runs.
fn run_script_on_stdin(runner: &str, script: &str, stdin_doc: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new(runner)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run a cell's shipped script exactly as the `code` cell runs it: the runner
/// from `params.runner`, the script from `params.script_inline`, the stdin
/// document from `wire::build_stdin_json`'s three-key shape.
fn run_shipped(root: &std::path::Path, cell: &str, stdin_doc: Value) -> Vec<Value> {
    let cfg = read_json(&root.join(cell).join("config.json"));
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap();

    let out = run_script_on_stdin(runner, script, &stdin_doc.to_string());
    assert!(
        out.status.success(),
        "{cell} exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell} stdout is not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
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

/// A minimal `/colony/graph` answer.
fn graph_doc(nodes: &[(&str, &str)], edges: &[(&str, &str, &str)]) -> Value {
    json!({
        "scope": "/",
        "nodes": nodes.iter().map(|(p, t)| json!({"path": p, "cell_type": t})).collect::<Vec<_>>(),
        "edges": edges.iter().map(|(id, f, t)| json!({"id": id, "from": f, "to": t})).collect::<Vec<_>>(),
    })
}

/// The four cells and three edges every pipeline test draws.
fn fixture_graph() -> Value {
    graph_doc(
        &[
            ("/a/one", "code"),
            ("/a/two", "store"),
            ("/b/three", "llm"),
            ("/b/four", "timer"),
        ],
        &[
            ("e1", "/a/one", "/a/two"),
            ("e2", "/a/two", "/b/three"),
            ("e3", "/b/three", "/b/four"),
        ],
    )
}

/// A `web` cell reply to a `query`, as the hive's edge delivers it to `layout`.
fn query_answer(objects: &[Value]) -> Value {
    json!({
        "messages": [{
            "origin": "tool", "type": "tool_result", "id": "q",
            "text": json!({"route": "/", "root": "canvy", "objects": objects}).to_string(),
        }]
    })
}

/// A `web` cell REFUSAL of a `query`: the only signal there is that a display
/// has never had its page set.
fn query_refusal() -> Value {
    json!({
        "messages": [{
            "origin": "tool", "type": "tool_result", "id": "q",
            "text": "no page declares the route \"/\"",
        }]
    })
}

/// The context the hive's `./layout -> ./web` edge stamps on the way in and the
/// display's reply carries back.
fn layout_context(graph_hop: &str) -> Value {
    json!({ "canvy_origin": "layout", "canvy_graph": graph_hop })
}

/// Drive pass 1 and return the hop the graph travelled on.
fn ask_pass(root: &std::path::Path, graph: Value) -> Value {
    let out = run_shipped(
        root,
        "layout",
        stdin_doc(
            json!({ "messages": [], "graph": graph }),
            json!({ "route": "snapshot" }),
            json!({}),
        ),
    );
    assert_eq!(out.len(), 1, "pass 1 emits exactly one read: {out:#?}");
    out.into_iter().next().unwrap()
}

/// The `op` args of every tool call in an emission, in order.
fn calls_of(emission: &Value) -> Vec<Value> {
    emission["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|t| {
            meclaw_core::serde_json::from_str(t["text"].as_str().expect("text")).expect("args")
        })
        .collect()
}

/// The objects a `create` bundle would leave behind, in the shape `query`
/// answers with.
fn objects_from(calls: &[Value]) -> Vec<Value> {
    calls
        .iter()
        .filter(|c| c["op"] == "object.create")
        .map(|c| {
            json!({
                "id": c["id"],
                "parent": c.get("parent").cloned().unwrap_or(Value::Null),
                "component": c["component"],
                "ord": c.get("ord").cloned().unwrap_or(json!(0)),
                "props": c["props"],
            })
        })
        .collect()
}

// ─────────────────────────────────────────────── what the template declares

#[test]
fn the_template_ships_its_whole_inventory_at_two_zero() {
    let Some(root) = shipped_canvy() else { return };
    let t = read_json(&root.join("template.json"));
    assert_eq!(t["name"], "canvy");
    assert_eq!(
        t["version"], "2.0.0",
        "a removal-shaped change on every address the template offered is the first digit"
    );
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(cfg["cell"]["type"], "hive");

    // The two cells the re-cut retired must be gone, not merely unused: a
    // directory left behind is a second answer to "what draws this picture".
    for retired in ["render", "store"] {
        assert!(
            !root.join(retired).exists(),
            "{retired}/ was retired with its subject and must not be in the tree"
        );
    }
}

/// The generator is the gate, asked of itself.
///
/// `layout/config.json` carries `layout.py` with `canvy.js` and `canvy.css`
/// spliced into it, because a `web` cell has no path that serves a file off
/// disk — what a page shows comes out of its object tree, so the client is a
/// PROP of the root object. Reproducing that splice here would be a second
/// implementation of it, and two implementations of one substitution is exactly
/// the drift this gate exists to catch. So the script is run in `--check` mode.
#[test]
fn the_shipped_bytes_match_their_sources() {
    if shipped_canvy().is_none() {
        return;
    }
    let script = core_root().join("scripts/canvy_sync.py");
    if !script.exists() {
        return;
    }
    let out = match std::process::Command::new("python3")
        .arg(&script)
        .arg("--check")
        .output()
    {
        Ok(o) => o,
        Err(_) => return, // no python3 on this host
    };
    assert!(
        out.status.success(),
        "a config.json is out of sync with its source — run: python3 scripts/canvy_sync.py\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A shipped `script_inline` carries no environment token it did not mean.
///
/// This one is a trap the re-cut walked straight into and has to stay closed.
/// Every string value of a `config.json` goes through the substrate's
/// environment substitution on **every read** (spec § Variable substitution),
/// and the layout cell's script now carries the whole browser half of canvy.
/// A JavaScript template literal — `` `M${fmt(p.x)}` `` — is a dollar-brace form,
/// so the colony reads it as an env token with no value and refuses the cell at
/// boot with `env_var_missing`. The client is written with concatenation
/// instead, and this is what keeps it that way.
///
/// The `refresh` timer is the counter-example that proves the check is not a
/// blanket ban: it carries two tokens ON PURPOSE (`CANVY_REFRESH_CRON` and the
/// instance-class `uuid7:`), and they belong to `params` keys rather than to a
/// script.
#[test]
fn a_shipped_script_carries_no_environment_token() {
    let Some(root) = shipped_canvy() else { return };
    for cell in ["layout", "probe"] {
        let cfg = read_json(&root.join(cell).join("config.json"));
        let script = cfg["params"]["script_inline"].as_str().expect("script");
        assert!(
            !script.contains("${"),
            "{cell}'s script carries a dollar-brace form; the colony reads it as \
             an env token and refuses the cell at boot"
        );
    }
    let refresh = read_json(&root.join("refresh/config.json"));
    let cron = refresh["params"]["schedules"][0]["cron"]
        .as_str()
        .expect("cron");
    assert!(
        cron.starts_with("${CANVY_REFRESH_CRON:-"),
        "the one knob this template has is still a knob: {cron}"
    );
}

/// The hive is sealed, states two lanes, and both of them have a door.
#[test]
fn the_hive_is_sealed_and_states_two_lanes() {
    let Some(root) = shipped_canvy() else { return };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(
        cfg["params"]["ports"],
        json!([]),
        "the empty list is the statement 'the hive path is the only address'"
    );

    let accepts: Vec<&str> = cfg["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .map(|l| l["route"].as_str().unwrap())
        .collect();
    let emits: Vec<&str> = cfg["params"]["contract"]["emits"]
        .as_array()
        .expect("emits")
        .iter()
        .map(|l| l["route"].as_str().unwrap())
        .collect();
    assert_eq!(accepts, ["in_refresh"]);
    assert_eq!(emits, ["event"]);

    let edges = cfg["params"]["graph"]["edges"].as_array().expect("edges");
    // Every lane the contract names has a door, and no door names a lane the
    // contract does not. The colony-side gate asks the real router; this one
    // keeps the file itself honest at the two ends that touch `.`.
    for (dir, want) in [("from", "in_refresh"), ("to", "event")] {
        assert!(
            edges.iter().any(|e| e[dir] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("'{want}'")))),
            "no door for {want}"
        );
    }

    // The one absolute lane, and the condition that keeps it from swallowing
    // every other emission of the probe (GH #161: unconditional, a snapshot
    // becomes two graph answers becomes four, and the routing loop wedges on a
    // full mailbox inside twenty seconds).
    let colony = edges
        .iter()
        .find(|e| e["to"] == "/colony/graph")
        .expect("the topology lane travels with the template (GH #163)");
    assert_eq!(colony["from"], "./probe");
    assert!(
        colony["condition"]
            .as_str()
            .is_some_and(|c| c.contains("ask_colony")),
        "the absolute lane must be conditional: {colony}"
    );
}

/// The display is a REFERENCE, and the one default it overrides is the port.
#[test]
fn the_display_is_a_reference_with_its_port_overridden() {
    let Some(root) = shipped_canvy() else { return };
    let web = read_json(&root.join("web/config.json"));
    assert_eq!(web["cell"]["type"], "ref");
    assert_eq!(web["cell"]["template"], "web@1.0.0");
    // `override_params` sits top-level beside `cell`, keyed by the cells of the
    // REFERENCED template — `""` is its root (docs/config.md § Template
    // reference). Not inside `cell` (that key list is closed) and not inside
    // `params` (a ref has none).
    assert!(
        web["override_params"][""]["port"].as_u64().is_some(),
        "the display's port is the one default an instance almost always sets: {web}"
    );
    assert!(
        web.get("params").is_none(),
        "a ref carries no params of its own"
    );

    // A ref directory holds its config.json and nothing else — one address, two
    // sources otherwise, and the subtree parser refuses it
    // (`reject_stray_ref_entries`).
    let stray: Vec<String> = std::fs::read_dir(root.join("web"))
        .expect("web/")
        .filter_map(|e| {
            let n = e.ok()?.file_name().to_string_lossy().into_owned();
            (n != "config.json").then_some(n)
        })
        .collect();
    assert!(stray.is_empty(), "a ref directory must be alone: {stray:?}");
}

// ─────────────────────────────────────────────────────── the three passes

/// Pass 1 of the probe asks, and asks once. Pass 2 hands the answer on.
#[test]
fn the_probe_asks_once_and_hands_the_answer_on() {
    let Some(root) = shipped_canvy() else { return };

    let tick = run_shipped(&root, "probe", stdin_doc(json!({}), json!({}), json!({})));
    assert_eq!(tick.len(), 1);
    assert_eq!(tick[0]["header"]["route"], "ask_colony");
    assert_eq!(tick[0]["query"]["scope"], "/");

    let answered = run_shipped(
        &root,
        "probe",
        stdin_doc(json!({ "graph": fixture_graph() }), json!({}), json!({})),
    );
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0]["header"]["route"], "snapshot");
    assert_eq!(
        answered[0]["graph"]["nodes"].as_array().map(Vec::len),
        Some(4),
        "the snapshot travels whole and unread: {}",
        answered[0]
    );
    assert!(
        answered[0]["messages"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "every body crossing the substrate carries a messages slot, or it is \
         refused as invalid_ubf_body before it reaches an edge"
    );
}

/// The layout reads before it writes, and the graph travels on the hop because
/// `hop` survives exactly one edge and pass 2 is two edges away.
#[test]
fn the_layout_asks_before_it_patches() {
    let Some(root) = shipped_canvy() else { return };
    let ask = ask_pass(&root, fixture_graph());
    assert_eq!(ask["header"]["route"], "read");
    let calls = calls_of(&ask);
    assert_eq!(calls.len(), 1, "one read and nothing else: {calls:#?}");
    assert_eq!(calls[0]["op"], "query");
    assert_eq!(calls[0]["route"], "/");
    let carried = ask["header"]["canvy_graph"]
        .as_str()
        .expect("the graph rides on the hop, for the edge to promote");
    let snap: Value = meclaw_core::serde_json::from_str(carried).expect("json");
    assert_eq!(snap["nodes"].as_array().map(Vec::len), Some(4));
    assert_eq!(snap["edges"].as_array().map(Vec::len), Some(3));
}

/// A display that has never had its page set answers `query` with a refusal,
/// and that refusal is the bootstrap signal — the same pass then defines the
/// components, creates the root and sets the page.
#[test]
fn an_empty_display_is_bootstrapped_by_the_same_pass() {
    let Some(root) = shipped_canvy() else { return };
    let ask = ask_pass(&root, fixture_graph());
    let ctx = layout_context(ask["header"]["canvy_graph"].as_str().unwrap());

    let out = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_refusal(),
            json!({ "operation": "query", "error_code": "invalid_input" }),
            ctx,
        ),
    );
    assert_eq!(out.len(), 1, "one bundle: {out:#?}");
    assert_eq!(out[0]["header"]["route"], "patch");
    let calls = calls_of(&out[0]);

    let defines: Vec<&str> = calls
        .iter()
        .filter(|c| c["op"] == "component.define")
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        defines,
        ["canvy-shell", "canvy-hive", "canvy-edge", "canvy-node"],
        "the vocabulary is defined before anything is written in it"
    );
    // The declaration a drag rests on. The display checks it against the
    // COMPONENT, never against the message.
    let node = calls
        .iter()
        .find(|c| c["name"] == "canvy-node")
        .expect("canvy-node");
    assert_eq!(node["editable"], json!(["x", "y"]));

    // Components, then the root, then the page: one bundle applied in call
    // order, so a fresh display and a running one take the same path.
    let root_at = calls
        .iter()
        .position(|c| c["op"] == "object.create" && c["id"] == "canvy")
        .expect("the root is created");
    let page_at = calls
        .iter()
        .position(|c| c["op"] == "page.set")
        .expect("the page is set");
    assert!(root_at > 3 && page_at > root_at, "{calls:#?}");

    // The browser half rides in on the bootstrap and only there: it is by far
    // the largest thing this cell says and it does not change between ticks.
    let props = &calls[root_at]["props"];
    assert!(
        props["client_js"]
            .as_str()
            .is_some_and(|s| s.contains("SurfaceHooks")),
        "the hook reaches the browser as a prop of the root object"
    );
    assert!(
        props["client_css"]
            .as_str()
            .is_some_and(|s| s.contains(".canvy")),
        "and so does the stylesheet"
    );

    // One object per cell, per edge, and per hive.
    let created: Vec<&str> = calls
        .iter()
        .filter(|c| c["op"] == "object.create")
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    for want in [
        "canvy",
        "n/a/one",
        "n/a/two",
        "n/b/three",
        "n/b/four",
        "e/e1",
        "e/e2",
        "e/e3",
        "h/a",
        "h/b",
    ] {
        assert!(created.contains(&want), "{want} missing from {created:?}");
    }
}

/// The second tick patches instead of creating, and the coordinates the display
/// already holds are left exactly as they are.
///
/// That is the entire persistence story of a drag: the browser wrote `x` and `y`
/// into the object without a single message entering the colony, and this pass
/// reads them back and does not touch them.
#[test]
fn a_position_the_display_already_holds_is_left_alone() {
    let Some(root) = shipped_canvy() else { return };
    let ask = ask_pass(&root, fixture_graph());
    let hop = ask["header"]["canvy_graph"].as_str().unwrap().to_string();

    let boot = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_refusal(),
            json!({ "operation": "query", "error_code": "invalid_input" }),
            layout_context(&hop),
        ),
    );
    let mut objects = objects_from(&calls_of(&boot[0]));
    // Somebody drags one box a long way.
    for o in objects.iter_mut() {
        if o["id"] == "n/a/one" {
            o["props"]["x"] = json!(4321);
            o["props"]["y"] = json!(1234);
        }
    }

    let out = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_answer(&objects),
            json!({ "operation": "query" }),
            layout_context(&hop),
        ),
    );
    let calls = calls_of(&out[0]);
    assert!(
        calls.iter().all(|c| c["op"] == "object.update"),
        "a display that already holds the picture is patched, not rebuilt: {:#?}",
        calls.iter().map(|c| c["op"].clone()).collect::<Vec<_>>()
    );
    let moved = calls
        .iter()
        .find(|c| c["id"] == "n/a/one")
        .expect("n/a/one");
    assert_eq!(moved["props"]["x"], json!(4321));
    assert_eq!(moved["props"]["y"], json!(1234));

    // …and a cell the colony no longer has loses its box. 1.x could not do this
    // — a row naming a vanished cell was indistinguishable from a rename, so the
    // deletion had to be an operator's button (GH #184). Here the objects ARE
    // the picture rather than a side table beside it.
    let smaller = ask_pass(
        &root,
        graph_doc(
            &[("/a/one", "code"), ("/a/two", "store")],
            &[("e1", "/a/one", "/a/two")],
        ),
    );
    let shrunk = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_answer(&objects),
            json!({ "operation": "query" }),
            layout_context(smaller["header"]["canvy_graph"].as_str().unwrap()),
        ),
    );
    let gone: Vec<String> = calls_of(&shrunk[0])
        .iter()
        .filter(|c| c["op"] == "object.delete")
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    for want in ["n/b/three", "n/b/four", "e/e2", "e/e3", "h/b"] {
        assert!(
            gone.iter().any(|g| g == want),
            "{want} should have been deleted: {gone:?}"
        );
    }
}

/// The acknowledgement of a patch ends the round.
///
/// Getting this wrong is not a cosmetic bug and it is worth its own test: before
/// the equivalent check existed in 1.x, every store acknowledgement fell through
/// to "ask again" — one tick became two, two became four, and the routing loop
/// wedged on a full mailbox inside twenty seconds with an EMPTY dead-letter
/// queue (GH #161).
#[test]
fn the_acknowledgement_of_a_patch_ends_the_round() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "layout",
        stdin_doc(
            json!({ "messages": [], "results": [] }),
            json!({ "operation": "bundle", "bundle_errors": 0 }),
            json!({ "canvy_origin": "patch" }),
        ),
    );
    assert!(
        out.is_empty(),
        "a cell that cannot recognise the reply to its own write has no way to \
         stop: {out:#?}"
    );
}

// ─────────────────────────────────────────── and it fills a real display

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

/// A `web` cell with an empty database — exactly what an instance of
/// `canvy@2.0.0` starts with, since a ref directory carries no seed.
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

    // An empty display has no page, so it answers 404 — which is still the
    // listener answering. Waiting for a 200 here would wait for the bootstrap
    // this test has not sent yet.
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

/// Send one bundle of shipped calls and read the reply.
async fn apply(live: &mut Live, calls: &[Value]) -> Value {
    let turns: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(i, args)| {
            json!({"origin": "assistant", "type": "tool_call",
                   "text": args.to_string(), "id": format!("c{i}")})
        })
        .collect();
    let msg = MessageBuilder::new(Path::new("/canvy/web"))
        .body(Body::Inline(json!({ "messages": turns })))
        .reply_to(Path::new("/canvy/layout"))
        .build();
    live.mailbox.send(msg).await.expect("mailbox");
    tokio::time::timeout(Duration::from_secs(60), live.out_rx.recv())
        .await
        .expect("the display must answer a bundle")
        .expect("an emission")
        .content
}

/// The whole point of the re-cut, end to end: the shipped scripts produce a
/// bundle, a real `web` cell takes it, and the page a browser would get carries
/// the picture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pipeline_fills_a_real_display() {
    let Some(root) = shipped_canvy() else { return };
    // Python is the runner the `code` cells declare; without it there is nothing
    // to drive, and a missing interpreter is not a failing canvas.
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let ask = ask_pass(&root, fixture_graph());
    let boot = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_refusal(),
            json!({ "operation": "query", "error_code": "invalid_input" }),
            layout_context(ask["header"]["canvy_graph"].as_str().unwrap()),
        ),
    );
    let calls = calls_of(&boot[0]);

    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;

    let reply = apply(&mut live, &calls).await;
    assert_eq!(
        reply["header"]["bundle_errors"],
        json!(0),
        "every leg of the bootstrap must land: {reply}"
    );

    // One `canvy-node` object per colony cell, straight out of the display's own
    // database.
    let conn = rusqlite::Connection::open(live.cell_dir.join("cell.db")).expect("open");
    let nodes: i64 = conn
        .query_row(
            "SELECT count(*) FROM objects WHERE component = 'canvy-node'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(nodes, 4, "one box per cell");
    let edges: i64 = conn
        .query_row(
            "SELECT count(*) FROM objects WHERE component = 'canvy-edge'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(edges, 3, "one line per edge");
    drop(conn);

    // …and the page a browser gets.
    let body = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");

    // The 0.12.1 lesson, on the server side: a LiveView hook mounts only on an
    // element that carries BOTH an id and `phx-hook`. Without them the client
    // never runs — no edge gets a path and nothing can be dragged — and every
    // server-side test stays green.
    assert!(
        body.contains("phx-hook=\"Canvy\"") && body.contains("id=\"canvy\""),
        "the hook has to have something to mount on"
    );
    assert!(body.contains("<svg class=\"stage\"") && body.contains("viewBox="));
    for cell in ["a/one", "a/two", "b/three", "b/four"] {
        assert!(
            body.contains(&format!("data-node=\"{cell}\"")),
            "{cell} is not in the picture"
        );
    }
    assert_eq!(
        body.matches("class=\"edge-hit\"").count(),
        3,
        "an SVG path per edge, plus the fat twin a mouse can hit"
    );
    for hive in ["a", "b"] {
        assert!(body.contains(&format!("data-hive=\"{hive}\"")));
    }
    // The browser half arrived raw, not escaped: a stylesheet rendered escaped
    // is a page with no style at all.
    assert!(body.contains("<style>") && body.contains("<script>"));
    assert!(body.contains("SurfaceHooks"), "the hook is in the page");

    live.join.abort();
}

/// A second bundle over a display that already holds the picture changes what
/// the page says without rebuilding it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_tick_patches_the_page_it_already_serves() {
    let Some(root) = shipped_canvy() else { return };
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let ask = ask_pass(&root, fixture_graph());
    let hop = ask["header"]["canvy_graph"].as_str().unwrap().to_string();
    let boot = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_refusal(),
            json!({ "operation": "query", "error_code": "invalid_input" }),
            layout_context(&hop),
        ),
    );
    let boot_calls = calls_of(&boot[0]);

    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;
    apply(&mut live, &boot_calls).await;

    // The colony grew a cell. The layout sees the display's own objects, keeps
    // every position it finds and gives the newcomer one.
    let grown = ask_pass(
        &root,
        graph_doc(
            &[
                ("/a/one", "code"),
                ("/a/two", "store"),
                ("/b/three", "llm"),
                ("/b/four", "timer"),
                ("/b/five", "proxy"),
            ],
            &[
                ("e1", "/a/one", "/a/two"),
                ("e2", "/a/two", "/b/three"),
                ("e3", "/b/three", "/b/four"),
                ("e4", "/b/four", "/b/five"),
            ],
        ),
    );
    let objects = objects_from(&boot_calls);
    let next = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_answer(&objects),
            json!({ "operation": "query" }),
            layout_context(grown["header"]["canvy_graph"].as_str().unwrap()),
        ),
    );
    let reply = apply(&mut live, &calls_of(&next[0])).await;
    assert_eq!(reply["header"]["bundle_errors"], json!(0), "{reply}");

    let body = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    assert!(
        body.contains("data-node=\"b/five\""),
        "the new cell is in the picture"
    );
    assert_eq!(body.matches("class=\"edge-hit\"").count(), 4);

    live.join.abort();
}

/// A drag is a LOCAL write, and the tick after it does not undo it.
///
/// The two halves of the promise meet here: the browser's `object:set` never
/// enters the colony, and the layout's next patch reads the new coordinate back
/// out of the display and leaves it alone. Either half alone would look correct
/// and lose the arrangement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drag_survives_the_next_tick() {
    let Some(root) = shipped_canvy() else { return };
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let ask = ask_pass(&root, fixture_graph());
    let hop = ask["header"]["canvy_graph"].as_str().unwrap().to_string();
    let boot = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_refusal(),
            json!({ "operation": "query", "error_code": "invalid_input" }),
            layout_context(&hop),
        ),
    );
    let boot_calls = calls_of(&boot[0]);

    let td = TempDir::new().expect("td");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("dir");
    let mut live = start(&cell_dir).await;
    apply(&mut live, &boot_calls).await;

    // Drag `a/one` to 4321,1234 — the two events the hook sends on release.
    let page = reqwest::get(format!("http://127.0.0.1:{}/", live.port))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start_at = page.find(marker).expect("token") + marker.len();
    let end = start_at + page[start_at..].find('"').expect("quote");
    let token = page[start_at..end].to_string();
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/canvy/web"));
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/live/websocket", live.port))
            .await
            .expect("connect");
    ws.send(WsMessage::Text(
        json!(["1", "1", topic, "phx_join", {"session": token, "url": "/"}])
            .to_string()
            .into(),
    ))
    .await
    .expect("join");
    let _ = ws.next().await.expect("open").expect("frame");

    for (i, (prop, value)) in [("x", 4321), ("y", 1234)].into_iter().enumerate() {
        ws.send(WsMessage::Text(
            json!(["1", format!("{}", 9 + i), topic, "event",
                   {"event": "object:set",
                    "value": {"id": "n/a/one", "prop": prop, "value": value}}])
            .to_string()
            .into(),
        ))
        .await
        .expect("set");
        let reply = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .ok()
            .flatten()
            .and_then(Result::ok)
            .expect("a reply");
        let WsMessage::Text(t) = reply else {
            panic!("text frame")
        };
        let f: Value = meclaw_core::serde_json::from_str(&t).expect("json");
        assert_eq!(f[4]["status"], json!("ok"), "the drag was refused: {f}");
        // Drain whatever diff follows, so the next reply is not read as this one.
        let _ = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
    }

    // Nothing entered the colony. This is the assertion a browser-looking-correct
    // test would miss.
    assert!(
        live.out_rx.try_recv().is_err(),
        "a drag must emit NO message — zero topology round trip"
    );

    // Now the next tick, computed against exactly what the display holds.
    let conn = rusqlite::Connection::open(live.cell_dir.join("cell.db")).expect("open");
    let mut stmt = conn
        .prepare("SELECT id, parent, component, ord, props FROM objects")
        .expect("prepare");
    let objects: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "parent": r.get::<_, Option<String>>(1)?,
                "component": r.get::<_, String>(2)?,
                "ord": r.get::<_, i64>(3)?,
                "props": meclaw_core::serde_json::from_str::<Value>(&r.get::<_, String>(4)?)
                    .unwrap_or_else(|_| json!({})),
            }))
        })
        .expect("rows")
        .map(Result::unwrap)
        .collect();
    drop(stmt);
    drop(conn);

    let next = run_shipped(
        &root,
        "layout",
        stdin_doc(
            query_answer(&objects),
            json!({ "operation": "query" }),
            layout_context(&hop),
        ),
    );
    let moved = calls_of(&next[0])
        .into_iter()
        .find(|c| c["id"] == "n/a/one")
        .expect("n/a/one");
    assert_eq!(
        (moved["props"]["x"].as_i64(), moved["props"]["y"].as_i64()),
        (Some(4321), Some(1234)),
        "the tick after a drag must not move the box back: {moved}"
    );

    live.join.abort();
}
