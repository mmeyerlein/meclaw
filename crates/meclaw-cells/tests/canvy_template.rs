//! GH #159 — the shipped `canvy@0.1.0` template.
//!
//! What is pinned here is what the template PROMISES:
//!
//! 1. **The inventory**, including the two `.py` sources and the client files —
//!    and that each `config.json` carries its `.py` byte-for-byte in
//!    `script_inline`. The `.py` is what a person edits; the config is the
//!    artefact. Without this test the two drift and the tree runs the old one.
//! 2. **The two passes**, driven by running the **shipped bytes** out of
//!    `config.json` through `python3` — the same command the `code` cell builds.
//!    A test that ran the `.py` instead would prove the source, not the product.
//! 3. **The picture and the database cannot disagree**: a drop produces the
//!    layout write AND markup with the box at those coordinates, from one input.
//! 4. **No edge path in the markup.** The routing lives in the client, and this
//!    is the test that keeps it from migrating back to the server.
//! 5. **A cell path is database content**: a name carrying `<script>` comes out
//!    escaped.
//! 6. **The declarations**: the hive is sealed, the store is internal-write and
//!    declares no surface, the render cell does.
//!
//! Free of a provider by construction: this hive holds no model at all.
//!
//! **R2b guard.** Every read is guarded by [`shipped_canvy`]: where the template
//! does not ship, these tests skip rather than fail on a dead reference.

use meclaw_core::serde_json::{Value, json};

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every file the hive is made of. The list is the guard AND the inventory: a
/// file that silently disappears makes these tests skip rather than pass.
const CANVY_FILES: &[&str] = &[
    "config.json",
    "template.json",
    "README.md",
    "store/config.json",
    "render/config.json",
    "render/render.py",
    "render/client/surface.js",
    "render/client/surface.css",
    "render/client/surface.test.js",
    "refresh/config.json",
    "probe/config.json",
    "probe/probe.py",
];

fn shipped_canvy() -> Option<std::path::PathBuf> {
    let root = templates_root().join("canvy");
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

/// Run a cell's shipped script exactly as the `code` cell runs it: the runner
/// from `params.runner`, the script from `params.script_inline` via `-c`, the
/// stdin document from `wire::build_stdin_json`'s three-key shape.
fn run_shipped(root: &std::path::Path, cell: &str, stdin_doc: Value) -> Vec<Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let cfg = read_json(&root.join(cell).join("config.json"));
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = cfg["params"]["script_inline"].as_str().unwrap();

    let mut child = Command::new(runner)
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_doc.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
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
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// The stdin document the substrate hands a `code` cell: exactly three keys.
///
/// `context` is empty, which is what a request injected by the HTTP layer looks
/// like — that absence is how the render cell knows this is pass 1.
fn stdin_doc(body: Value, hop: Value) -> Value {
    stdin_doc_ctx(body, hop, json!({}))
}

/// Pass 2: the store's reply, carrying the `canvy_origin` the hive's own edge
/// stamps on the way in. That marker is the ONLY thing that tells the passes
/// apart, deliberately — deciding it by inspecting the body turned every store
/// error into an infinite render/store loop, found on the first live join.
fn stdin_doc_pass2(body: Value, hop: Value) -> Value {
    // The hive's `./render -> ./store` edge promotes what was asked from `hop`
    // into `context`, because `hop` survives exactly ONE edge and pass 2 is two
    // edges away. This helper does the same promotion, so the test exercises the
    // shape the cell actually receives.
    let mut ctx = json!({ "canvy_origin": "render" });
    for (from, to) in [
        ("moved_id", "canvy_moved_id"),
        ("moved_x", "canvy_moved_x"),
        ("moved_y", "canvy_moved_y"),
        ("event", "canvy_event"),
    ] {
        if let Some(v) = hop.get(from) {
            ctx[to] = v.clone();
        }
    }
    stdin_doc_ctx(body, hop, ctx)
}

fn stdin_doc_ctx(body: Value, hop: Value, context: Value) -> Value {
    json!({
        "envelope": {
            "header": { "context": context, "hop": hop },
            "target": "/canvy/render",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64
        },
        "body": body,
        "params": {}
    })
}

/// A store reply: one `tool_result` turn whose text is the JSON rows.
fn store_reply(rows: Value) -> Value {
    json!({
        "messages": [
            { "origin": "tool", "type": "tool_result", "id": "x", "text": rows.to_string() }
        ]
    })
}

/// A minimal `/colony/graph` answer, as the snapshot row holds it.
fn graph_doc(nodes: &[(&str, &str)], edges: &[(&str, &str, &str)]) -> Value {
    json!({
        "scope": "/",
        "nodes": nodes.iter().map(|(p, t)| json!({"path": p, "cell_type": t})).collect::<Vec<_>>(),
        "edges": edges.iter().map(|(id, f, t)| json!({"id": id, "from": f, "to": t})).collect::<Vec<_>>(),
    })
}

fn snapshot_rows(graph: Value) -> Value {
    json!([{ "kind": "graph", "id": "colony", "doc": graph.to_string() }])
}

/// The HTML out of a surface reply, or a panic naming what came instead.
fn html_of(out: &[Value]) -> String {
    for m in out {
        if let Some(h) = m
            .get("surface")
            .and_then(|s| s.get("html"))
            .and_then(Value::as_str)
        {
            return h.to_string();
        }
        if let Some(e) = m
            .get("surface")
            .and_then(|s| s.get("error"))
            .and_then(Value::as_str)
        {
            panic!("expected html, got an error: {e}");
        }
    }
    panic!("no surface reply in {out:?}");
}

/// The `(x, y)` of a hive box in the markup, by its `data-hive`.
fn hive_box(html: &str, hive: &str) -> (i64, i64) {
    let needle = format!("data-hive=\"{hive}\">");
    let after = html
        .split(&needle)
        .nth(1)
        .unwrap_or_else(|| panic!("no hive box for {hive} in {html}"));
    let rect = after.split("/>").next().unwrap_or("");
    let num = |k: &str| -> i64 {
        rect.split(&format!("{k}=\""))
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("no {k} in {rect}"))
    };
    (num("x"), num("y"))
}

/// The `(x, y, w, h)` of a hive box.
fn hive_frame(html: &str, hive: &str) -> (i64, i64, i64, i64) {
    let needle = format!("data-hive=\"{hive}\"");
    let after = html
        .split(&needle)
        .nth(1)
        .unwrap_or_else(|| panic!("no hive box for {hive} in {html}"));
    let rect = after.split("/>").next().unwrap_or("");
    let num = |k: &str| -> i64 {
        rect.split(&format!("{k}=\""))
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("no {k} in {rect}"))
    };
    (num(" x"), num(" y"), num("width"), num("height"))
}

/// The `translate(x,y)` of each named node, in the order asked for.
fn node_positions(html: &str, ids: &[&str]) -> Vec<(i64, i64)> {
    ids.iter()
        .map(|id| {
            let needle = format!("data-node=\"{id}\"");
            let after = html
                .split(&needle)
                .nth(1)
                .unwrap_or_else(|| panic!("no node {id} in {html}"));
            let t = after
                .split("translate(")
                .nth(1)
                .and_then(|r| r.split(')').next())
                .unwrap_or_else(|| panic!("no transform for {id}"));
            let mut it = t.split(',');
            (
                it.next().unwrap().trim().parse().unwrap(),
                it.next().unwrap().trim().parse().unwrap(),
            )
        })
        .collect()
}

/// The store operations in an emission list, decoded from their tool_call turns.
fn store_ops(out: &[Value]) -> Vec<Value> {
    let mut ops = Vec::new();
    for m in out {
        if m["header"]["route"] != "canvas" && m["header"]["route"] != "snapshot" {
            continue;
        }
        let text = m["messages"][0]["text"].as_str().unwrap_or("null");
        if let Ok(v) = meclaw_core::serde_json::from_str::<Value>(text) {
            ops.push(v);
        }
    }
    ops
}

// ───────────────────────────────────────────────────────────── 1. the inventory

/// The `.py` a person edits and the `script_inline` the tree runs must be the
/// same bytes. `scripts/canvy_sync.py` writes the copy; this is what makes
/// forgetting to run it a red test rather than a stale surface.
#[test]
fn each_config_carries_its_python_source_byte_for_byte() {
    let Some(root) = shipped_canvy() else { return };
    for (script, cell) in [("render/render.py", "render"), ("probe/probe.py", "probe")] {
        let source = std::fs::read_to_string(root.join(script)).unwrap();
        let cfg = read_json(&root.join(cell).join("config.json"));
        let inline = cfg["params"]["script_inline"].as_str().unwrap();
        assert_eq!(
            inline, source,
            "{cell}/config.json is out of sync with {script} — run: python3 scripts/canvy_sync.py"
        );
        assert!(
            cfg["params"].get("script_path").is_none(),
            "{cell} must not use script_path: it is resolved against the daemon's cwd"
        );
    }
}

/// A `code` cell that carries its own sandbox block declares it fully, and
/// neither of these two needs the network.
#[test]
fn both_code_cells_are_sandboxed_and_offline() {
    let Some(root) = shipped_canvy() else { return };
    for cell in ["render", "probe"] {
        let cfg = read_json(&root.join(cell).join("config.json"));
        let sb = &cfg["params"]["sandbox"];
        assert_eq!(sb["trust"], "restricted", "{cell}");
        assert_eq!(sb["network"], "deny", "{cell} needs no network");
        assert_eq!(sb["filesystem"]["runtime"], true, "{cell}");
        assert!(
            sb["filesystem"].get("read").is_none() && sb["filesystem"].get("write").is_none(),
            "{cell} must declare no host path — an absolute path in a template is GH #20"
        );
    }
}

// ─────────────────────────────────────────────────────── 2. + 3. the two passes

/// Pass 1: a request becomes exactly one `select`, and what was asked rides along
/// in `hop` so pass 2 knows it.
#[test]
fn a_request_becomes_one_select_carrying_what_was_asked() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc(
            json!({ "surface": { "event": "node:moved",
                                 "value": { "id": "a/b", "x": 700, "y": 240 } } }),
            json!({}),
        ),
    );
    assert_eq!(out.len(), 1, "pass 1 emits exactly one message: {out:?}");
    let ops = store_ops(&out);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0]["operation"], "select");
    assert_eq!(ops[0]["table"], "canvas");
    assert_eq!(out[0]["header"]["moved_id"], "a/b");
    assert_eq!(out[0]["header"]["moved_x"], "700");
    assert_eq!(out[0]["header"]["event"], "node:moved");
}

/// Pass 2 on a join: the store's rows become the whole picture, and nothing is
/// written.
#[test]
fn a_join_renders_the_picture_and_writes_nothing() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(
        &[("/talky/brain", "llm"), ("/talky/window", "store")],
        &[("e0", "/talky/brain", "/talky/window")],
    ));
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    );
    assert!(store_ops(&out).is_empty(), "a join must write nothing");
    let html = html_of(&out);
    assert!(html.contains("data-node=\"talky/brain\""), "{html}");
    assert!(html.contains("data-node=\"talky/window\""));
    assert!(
        html.contains("data-hive=\"talky\""),
        "the hive box is derived"
    );
    assert!(html.contains("2 cells, 1 hives, 1 edges"), "{html}");
}

/// **The property that keeps the picture and the database from disagreeing.** One
/// input produces the write and the markup, and they name the same coordinates.
#[test]
fn a_drop_writes_the_position_it_also_renders() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(&[("/talky/brain", "llm")], &[]));
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(rows),
            json!({ "moved_id": "talky/brain", "moved_x": "900", "moved_y": "400" }),
        ),
    );
    let ops = store_ops(&out);
    assert_eq!(ops.len(), 2, "delete then insert, in one emission: {ops:?}");
    assert_eq!(ops[0]["operation"], "delete");
    assert_eq!(ops[0]["where"]["id"], "talky/brain");
    assert_eq!(ops[1]["operation"], "insert");
    assert_eq!(ops[1]["row"]["x"], 900);
    assert_eq!(ops[1]["row"]["y"], 400);
    let html = html_of(&out);
    assert!(
        html.contains("translate(900,400)"),
        "the box must be rendered where it is written: {html}"
    );
}

/// The same box moved twice must leave ONE row. That is the delete-then-insert
/// order, and it is why a position is not appended.
#[test]
fn a_position_is_replaced_and_not_appended() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(&[("/a/b", "code")], &[]));
    for (x, y) in [(10, 20), (30, 40)] {
        let out = run_shipped(
            &root,
            "render",
            stdin_doc_pass2(
                store_reply(rows.clone()),
                json!({ "moved_id": "a/b", "moved_x": x.to_string(), "moved_y": y.to_string() }),
            ),
        );
        let ops = store_ops(&out);
        assert_eq!(ops[0]["operation"], "delete", "the delete must come first");
        assert_eq!(ops[1]["row"]["x"], x);
    }
}

/// A refused write produces one corrective answer and no picture: a page claiming
/// a position the database does not hold is worse than a visible failure.
#[test]
fn a_refused_write_produces_an_error_and_no_html() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(json!([])),
            json!({ "error_code": "write_surface_violation" }),
        ),
    );
    assert_eq!(out.len(), 1);
    let err = out[0]["surface"]["error"].as_str().unwrap();
    assert!(err.contains("write_surface_violation"), "{err}");
    assert!(out[0]["surface"].get("html").is_none());
}

/// No snapshot yet is a sentence, not an empty canvas: an operator must be able
/// to tell "the timer has not run" from "the colony is empty".
#[test]
fn a_missing_snapshot_says_so() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(json!([])), json!({})),
    );
    let err = out[0]["surface"]["error"].as_str().unwrap();
    assert!(err.contains("snapshot"), "{err}");
}

/// A malformed snapshot is refused rather than half-rendered.
#[test]
fn a_malformed_snapshot_is_refused() {
    let Some(root) = shipped_canvy() else { return };
    let rows = json!([{ "kind": "graph", "id": "colony",
                        "doc": "{\"nodes\": \"not a list\", \"edges\": []}" }]);
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    );
    assert!(out[0]["surface"].get("error").is_some(), "{out:?}");
}

// ───────────────────────────────────────── 4. + 5. what the markup may not carry

/// **The line between the server and the client, as a test.** The server sends
/// endpoints and a lane; the client routes. A `d` attribute here would be the same
/// algorithm in two languages, and the first thing to drift.
#[test]
fn the_markup_carries_no_edge_path() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(
        &[("/a/one", "llm"), ("/a/two", "store")],
        &[("e0", "/a/one", "/a/two"), ("e1", "/a/two", "/a/one")],
    ));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(
        !html.contains(" d=\""),
        "an edge path leaked into the server's markup: {html}"
    );
    assert!(html.contains("data-lane="), "but the lane must be there");
    assert!(html.contains("data-from=\"a/one\""));
}

/// **The defect a browser found and no test did** (2026-08-17): the canvas offered
/// the client nothing to attach to.
///
/// A LiveView hook mounts on an element that carries `phx-hook="<Name>"` AND an
/// `id`. The markup carried neither, so `surface.js` — the file that fills in every
/// edge path and owns the whole drag — never ran. What reached the browser was a
/// picture with no lines that could not be moved, on every join since the surface
/// existed. Every server-side test passed, because every one of them asserted about
/// the markup and none about the contract between the markup and the client.
///
/// The hook NAME is read out of `surface.js` rather than written here twice, so a
/// rename on either side turns this red instead of silently detaching the client
/// again.
#[test]
fn the_canvas_offers_the_hook_that_the_client_registers() {
    let Some(root) = shipped_canvy() else { return };
    let js = std::fs::read_to_string(root.join("render/client/surface.js")).unwrap();
    // `root.SurfaceHooks = Object.assign(root.SurfaceHooks || {}, {Canvy: Canvy});`
    let registered = js
        .split("SurfaceHooks || {}, {")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .map(str::trim)
        .expect("surface.js must register exactly one hook by name");

    let rows = snapshot_rows(graph_doc(&[("/a/one", "llm")], &[]));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(
        html.contains(&format!("phx-hook=\"{registered}\"")),
        "the markup must offer the hook `{registered}` that surface.js registers, \
         otherwise no edge is ever drawn and nothing can be dragged: {html}"
    );
    // LiveView refuses to mount a hook on an element without an id.
    let head = &html[..html.find('>').unwrap_or(html.len())];
    assert!(
        head.contains("id=\""),
        "a hook element needs an id or LiveView will not mount it: {head}"
    );
}

/// The whole picture has to be reachable. Without a `viewBox` the SVG shows the
/// top-left corner of a canvas that is thousands of pixels tall and there is no way
/// to scroll it — which is what "unusable" meant in the first report.
#[test]
fn the_frame_shows_the_whole_picture() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(
        &[
            ("/a/one", "llm"),
            ("/a/two", "store"),
            ("/b/three", "code"),
            ("/c/four", "code"),
        ],
        &[("e0", "/a/one", "/b/three")],
    ));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    let vb = html
        .split("viewBox=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("the stage must carry a viewBox");
    let nums: Vec<f64> = vb
        .split_whitespace()
        .map(|n| n.parse().expect("viewBox numbers"))
        .collect();
    assert_eq!(nums.len(), 4, "viewBox is min-x min-y width height: {vb}");
    // Every box the markup places must lie inside the frame.
    let mut worst_x: f64 = 0.0;
    let mut worst_y: f64 = 0.0;
    for chunk in html.split("translate(").skip(1) {
        let inner = chunk.split(')').next().unwrap_or("");
        let mut it = inner.split(',');
        let x: f64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0.0);
        let y: f64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0.0);
        worst_x = worst_x.max(x);
        worst_y = worst_y.max(y);
    }
    assert!(
        worst_x <= nums[0] + nums[2] && worst_y <= nums[1] + nums[3],
        "a box at ({worst_x},{worst_y}) sits outside the frame {vb}"
    );
}

/// **The arrangement.** Hives used to be stacked in ONE column, so a 14-hive colony
/// was a 3672-pixel-tall strip: correct, deterministic, and unreadable. Hives are
/// packed into rows now, and this is the discriminator — a layout that regresses to
/// a single column fails here even though every other layout test stays green.
#[test]
fn hives_are_packed_into_rows_and_not_a_single_column() {
    let Some(root) = shipped_canvy() else { return };
    // Six hives, two cells each: enough that a column arrangement is unmistakable.
    let mut nodes: Vec<(String, &str)> = Vec::new();
    for h in ["a", "b", "c", "d", "e", "f"] {
        nodes.push((format!("/{h}/one"), "llm"));
        nodes.push((format!("/{h}/two"), "store"));
    }
    let as_refs: Vec<(&str, &str)> = nodes.iter().map(|(p, t)| (p.as_str(), *t)).collect();
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(&as_refs, &[]))),
            json!({}),
        ),
    ));

    // The hive rectangles carry their own x/y, which is the arrangement itself.
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for chunk in html.split("class=\"hive depth-").skip(1) {
        let rect = chunk.split("/>").next().unwrap_or("");
        let get = |k: &str| -> f64 {
            rect.split(&format!("{k}=\""))
                .nth(1)
                .and_then(|r| r.split('"').next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0)
        };
        xs.push(get(" x"));
        ys.push(get(" y"));
    }
    assert_eq!(xs.len(), 6, "six hives must be drawn: {html}");
    let distinct_x = {
        let mut v = xs.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup();
        v.len()
    };
    assert!(
        distinct_x > 1,
        "every hive sits at the same x — that is the single column: {xs:?}"
    );
    let width = xs.iter().cloned().fold(0.0_f64, f64::max);
    let height = ys.iter().cloned().fold(0.0_f64, f64::max);
    assert!(
        width >= height,
        "six equal hives must spread sideways at least as far as downwards, \
         got width {width} height {height}"
    );
}

/// **The client's own test suite, run.** It was in the tree and in the inventory
/// list above, and nothing ever executed it — not CI, not `cargo test`, not the
/// release routine. A test file that only exists is a comment.
///
/// That is the process half of the two defects a browser found on 2026-08-17: the
/// geometry had 19 green property tests, the hook had none, and nobody would have
/// noticed either way. Now `cargo test` runs it, and a client-side regression is a
/// red Rust test.
///
/// Skips when `node` is absent, like every other guard in this file — a missing
/// interpreter is not a failing canvas.
#[test]
fn the_clients_own_tests_pass() {
    let Some(root) = shipped_canvy() else { return };
    let script = root.join("render/client/surface.test.js");
    let out = match std::process::Command::new("node").arg(&script).output() {
        Ok(o) => o,
        Err(_) => return, // no node on this host
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("all green"),
        "the canvas client's tests must pass:\n{stdout}\n{stderr}"
    );
    // The suite must actually have run something — an empty file "passes" too.
    assert!(
        stdout.matches("  ok ").count() >= 20,
        "too few client assertions ran; did the suite lose its cases?\n{stdout}"
    );
}

/// **A hive is draggable too, and it costs ONE row.** Reported right after the
/// canvas became usable: the boxes moved, the groups did not.
///
/// A hive box is derived from where its members ended up (`hive_boxes`) and is
/// never stored — that is what makes dragging a cell out of a crowd grow its hive
/// instead of stranding it outside a stale rectangle. So moving a hive cannot mean
/// "store the rectangle": it means storing one OFFSET for the group, which the
/// layout applies to its members before their own saved positions win. Two store
/// ops per hive drag, exactly like a cell — not two per member.
#[test]
fn dragging_a_hive_writes_one_row_and_moves_its_members() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(
        &[("/a/one", "llm"), ("/a/two", "store"), ("/b/three", "code")],
        &[],
    );
    let before = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let a_before = hive_box(&before, "a");
    let b_before = hive_box(&before, "b");

    // The drop: the hive `a` box is let go 500 right and 300 down of where it sat.
    let target = (a_before.0 + 500, a_before.1 + 300);
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(snapshot_rows(graph)),
            json!({}),
            json!({
                "canvy_origin": "render",
                "canvy_event": "hive:moved",
                "canvy_moved_id": "a",
                "canvy_moved_x": target.0.to_string(),
                "canvy_moved_y": target.1.to_string(),
            }),
        ),
    );

    let ops = store_ops(&out);
    assert_eq!(
        ops.len(),
        2,
        "one delete and one insert for the GROUP, not per member: {ops:?}"
    );
    assert_eq!(ops[0]["operation"], "delete");
    assert_eq!(ops[0]["where"]["kind"], "hive");
    assert_eq!(ops[0]["where"]["id"], "a");
    assert_eq!(ops[1]["row"]["kind"], "hive");
    assert_eq!(ops[1]["row"]["x"], target.0);
    assert_eq!(ops[1]["row"]["y"], target.1);

    // And the picture it answers with has the group there, with its members.
    let after = html_of(&out);
    assert_eq!(
        hive_box(&after, "a"),
        target,
        "the hive box must be rendered where it was dropped"
    );
    assert_eq!(
        hive_box(&after, "b"),
        b_before,
        "and a hive nobody touched must not move"
    );
    let moved: Vec<(i64, i64)> = node_positions(&after, &["a/one", "a/two"]);
    let stayed: Vec<(i64, i64)> = node_positions(&before, &["a/one", "a/two"]);
    for (m, s) in moved.iter().zip(stayed.iter()) {
        assert_eq!(
            (m.0 - s.0, m.1 - s.1),
            (500, 300),
            "every member moves with its hive, by the same delta"
        );
    }
}

/// A saved cell position still wins over the offset of its hive: the precedence is
/// cell, then hive, then automatic. Without this, moving a hive would silently
/// undo every hand-placed cell inside it.
#[test]
fn a_saved_cell_position_survives_a_hive_move() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let mut rows = snapshot_rows(graph);
    let arr = rows.as_array_mut().unwrap();
    arr.push(json!({"kind": "node", "id": "a/one", "x": 4000, "y": 4000}));
    arr.push(json!({"kind": "hive", "id": "a", "x": 900, "y": 900}));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert_eq!(
        node_positions(&html, &["a/one"])[0],
        (4000, 4000),
        "the cell keeps the position somebody gave it"
    );
    let two = node_positions(&html, &["a/two"])[0];
    assert!(
        two.0 >= 900 && two.1 >= 900,
        "and its neighbour follows the hive offset, got {two:?}"
    );
}

/// **A hive inside a hive has to LOOK inside it.** Reported as "hive in hive does
/// not work properly yet", and it did not: a box was derived from a cell's DIRECT
/// parent only, so `/a` and `/a/b` were two unrelated rectangles packed side by
/// side, and a hive holding nothing but sub-hives got no box at all. The picture
/// then said nothing about the tree it is a picture of.
///
/// Two properties, and both have to hold at once: every ancestor hive is drawn, and
/// an ancestor's box strictly CONTAINS its descendant's. Strictly, not merely
/// overlapping — a shared edge reads as two boxes bumping into each other rather
/// than one being inside the other.
#[test]
fn an_ancestor_hive_contains_its_children() {
    let Some(root) = shipped_canvy() else { return };
    // `/a` holds one cell and two sub-hives; `/a/b` holds a cell and a sub-hive of
    // its own; `/c` is an unrelated neighbour. Three depths, so "contains" is
    // asserted transitively and not just for one pair.
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(
                &[
                    ("/a/own", "llm"),
                    ("/a/b/two", "store"),
                    ("/a/b/deep/three", "code"),
                    ("/a/other/four", "code"),
                    ("/c/five", "code"),
                ],
                &[],
            ))),
            json!({}),
        ),
    ));

    for h in ["a", "a/b", "a/b/deep", "a/other", "c"] {
        assert!(
            html.contains(&format!("data-hive=\"{h}\"")),
            "every hive on the path must be drawn, {h} is missing: {html}"
        );
    }
    let contains = |outer: &str, inner: &str| {
        let (ox, oy, ow, oh) = hive_frame(&html, outer);
        let (ix, iy, iw, ih) = hive_frame(&html, inner);
        assert!(
            ox < ix && oy < iy && ox + ow > ix + iw && oy + oh > iy + ih,
            "{outer} ({ox},{oy},{ow},{oh}) must strictly contain {inner} ({ix},{iy},{iw},{ih})"
        );
    };
    contains("a", "a/b");
    contains("a", "a/other");
    contains("a/b", "a/b/deep");
    contains("a", "a/b/deep");

    // And a neighbour is beside it, not inside it.
    let (ax, _, aw, _) = hive_frame(&html, "a");
    let (cx, _, _, _) = hive_frame(&html, "c");
    assert!(
        cx >= ax + aw || cx < ax,
        "an unrelated hive must not sit inside another one"
    );
}

/// Each depth gets its own tint, so the nesting is readable without counting
/// dashes. The class is what the stylesheet keys on, so a picture that lost the
/// class would look flat again even with every box in the right place.
#[test]
fn every_hive_carries_its_depth() {
    let Some(root) = shipped_canvy() else { return };
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(
                &[
                    ("/a/own", "llm"),
                    ("/a/b/two", "store"),
                    ("/a/b/deep/three", "code"),
                ],
                &[],
            ))),
            json!({}),
        ),
    ));
    for (h, depth) in [("a", 1), ("a/b", 2), ("a/b/deep", 3)] {
        assert!(
            html.contains(&format!("class=\"hive depth-{depth}\" data-hive=\"{h}\"")),
            "hive {h} must be drawn as depth-{depth}: {html}"
        );
    }
    // The stylesheet has to actually distinguish them, or the class is decoration —
    // and it has to cover every depth the renderer can emit, or the deepest hives
    // fall back to a fill they share with depth 1. The two numbers are read out of
    // the two files rather than written here, because that is the drift that would
    // otherwise be invisible.
    let css = std::fs::read_to_string(root.join("render/client/surface.css")).unwrap();
    let py = std::fs::read_to_string(root.join("render/render.py")).unwrap();
    let tints: usize = py
        .split("HIVE_DEPTH_TINTS = ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("render.py must declare HIVE_DEPTH_TINTS");
    assert!(tints >= 8, "a real colony reaches depth 8, got {tints}");
    let mut fills = std::collections::HashSet::new();
    for depth in 1..=tints {
        let rule = format!(".canvy .hive.depth-{depth} rect{{ fill:var(--hive{depth}); }}");
        assert!(
            css.contains(&rule),
            "surface.css must give depth-{depth} its own fill: `{rule}` missing"
        );
        let value = css
            .split(&format!("--hive{depth}:"))
            .nth(1)
            .and_then(|r| r.split(';').next())
            .expect("every tint needs a value")
            .trim()
            .to_string();
        assert!(
            fills.insert(value.clone()),
            "depth-{depth} reuses the tint {value} — every depth gets its own"
        );
    }
}

/// Dragging a nested hive moves its own subtree and lets its parent follow: the
/// parent's box is derived, so it grows rather than being left behind.
#[test]
fn dragging_a_nested_hive_takes_its_subtree_and_grows_its_parent() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(
        &[
            ("/a/own", "llm"),
            ("/a/b/two", "store"),
            ("/a/b/deep/three", "code"),
        ],
        &[],
    );
    let before = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let inner = hive_frame(&before, "a/b");
    let target = (inner.0 + 600, inner.1 + 400);
    let after = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(snapshot_rows(graph)),
            json!({}),
            json!({
                "canvy_origin": "render",
                "canvy_event": "hive:moved",
                "canvy_moved_id": "a/b",
                "canvy_moved_x": target.0.to_string(),
                "canvy_moved_y": target.1.to_string(),
            }),
        ),
    ));
    assert_eq!(
        (hive_frame(&after, "a/b").0, hive_frame(&after, "a/b").1),
        target,
        "the nested hive goes where it was dropped"
    );
    // Its own sub-hive travelled with it.
    let deep_before = hive_frame(&before, "a/b/deep");
    let deep_after = hive_frame(&after, "a/b/deep");
    assert_eq!(
        (deep_after.0 - deep_before.0, deep_after.1 - deep_before.1),
        (600, 400),
        "a hive moves its whole subtree, not just its direct cells"
    );
    // And the parent still contains it.
    let (ox, oy, ow, oh) = hive_frame(&after, "a");
    let (ix, iy, iw, ih) = hive_frame(&after, "a/b");
    assert!(
        ox < ix && oy < iy && ox + ow > ix + iw && oy + oh > iy + ih,
        "the parent must grow to keep holding what moved inside it"
    );
}

/// A cell path is database content. A name that could close a tag must not.
#[test]
fn a_name_that_looks_like_markup_comes_out_escaped() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(&[("/a/<script>alert(1)</script>", "code")], &[]));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(!html.contains("<script>"), "unescaped markup: {html}");
    assert!(html.contains("&lt;script&gt;"), "{html}");
}

/// The same input twice must draw the same picture: a changed payload then means a
/// changed colony, not a reshuffled layout.
#[test]
fn the_layout_is_deterministic() {
    let Some(root) = shipped_canvy() else { return };
    let rows = snapshot_rows(graph_doc(
        &[("/a/one", "llm"), ("/a/two", "store"), ("/b/three", "code")],
        &[("e0", "/a/one", "/a/two"), ("e1", "/a/two", "/b/three")],
    ));
    let first = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows.clone()), json!({})),
    ));
    let second = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert_eq!(first, second);
}

/// A saved position wins over the automatic one, and an untouched neighbour does
/// not move because of it.
#[test]
fn a_saved_position_wins_without_moving_its_neighbours() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let plain = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let mut rows = snapshot_rows(graph);
    rows.as_array_mut().unwrap().push(json!({
        "kind": "node", "id": "a/one", "x": 4000, "y": 3000
    }));
    let moved = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(moved.contains("translate(4000,3000)"), "{moved}");

    // The neighbour's transform must be identical in both pictures.
    let neighbour = |h: &str| {
        let at = h.find("data-node=\"a/two\"").expect("a/two");
        let tail = &h[at..];
        let t = tail.find("translate(").expect("transform");
        tail[t..tail[t..].find(')').unwrap() + t + 1].to_string()
    };
    assert_eq!(
        neighbour(&plain),
        neighbour(&moved),
        "one saved box must not move an untouched neighbour"
    );
}

/// A box dragged far out must make its hive GROW. The alternative — a stale
/// rectangle with a cell stranded outside it — is what deriving the box prevents.
#[test]
fn dragging_a_cell_out_grows_its_hive() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let width = |html: &str| -> i64 {
        let at = html.find("data-hive=\"a\"").expect("hive a");
        let tail = &html[at..];
        let w = tail.find("width=\"").expect("width") + 7;
        tail[w..w + tail[w..].find('"').unwrap()].parse().unwrap()
    };
    let base = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let far = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph)),
            json!({ "moved_id": "a/one", "moved_x": "4000", "moved_y": "3000" }),
        ),
    ));
    assert!(
        width(&far) > width(&base),
        "{} vs {}",
        width(&far),
        width(&base)
    );
}

// ────────────────────────────────────────────────────── the probe's two passes

/// A tick asks, and asks nothing else. No target and no `reply_to`: the lane is
/// the edge's decision and the substrate stamps the sender.
#[test]
fn a_tick_becomes_one_read_of_the_colony_graph() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "probe",
        stdin_doc(json!({ "messages": [] }), json!({})),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "ask_colony");
    assert_eq!(out[0]["query"]["scope"], "/");
    assert!(
        out[0].get("target").is_none() && out[0].get("reply_to").is_none(),
        "a code cell chooses neither: {:?}",
        out[0]
    );
}

/// The colony's answer becomes exactly one snapshot row, replacing the previous.
#[test]
fn a_graph_reply_replaces_the_snapshot_row() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "probe",
        stdin_doc(
            // The shape a LIVE /colony/graph reply has: the answer nests under a
            // `graph` slot. Verified against a running colony, because the first
            // version of the probe looked at the top level and silently re-asked
            // for ever without writing a snapshot.
            json!({ "graph": { "scope": "/",
                               "nodes": [{"path": "/a/b", "cell_type": "code"}],
                               "edges": [] } }),
            json!({}),
        ),
    );
    let ops = store_ops(&out);
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[0]["operation"], "delete");
    assert_eq!(ops[0]["where"]["kind"], "graph");
    assert_eq!(ops[1]["operation"], "insert");
    let doc: Value =
        meclaw_core::serde_json::from_str(ops[1]["row"]["doc"].as_str().unwrap()).unwrap();
    assert_eq!(doc["nodes"][0]["path"], "/a/b");
}

// ─────────────────────────────────────────────────────────── 6. the declarations

/// The hive seals itself, and the two ports it names are the two it has.
#[test]
fn the_hive_is_sealed_to_its_two_ports() {
    let Some(root) = shipped_canvy() else { return };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(cfg["cell"]["type"], "hive");
    let ports: Vec<&str> = cfg["params"]["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ports, vec!["render", "refresh"], "the store is NOT a port");
    // GH #163: both of the hive's outward lanes stay INSIDE what a mutation may
    // draw, which is what makes the whole template installable into a running
    // colony. The answer goes to the hive itself and leaves through the egress
    // door from there (the door is opened by the marker, not by the root hive);
    // the topology lane addresses the colony's read-only endpoint, the one
    // absolute endpoint a mutation is allowed. A regression to `-> /` would make
    // the mutation `scope_out_of_bounds` again.
    let edges = cfg["params"]["graph"]["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == "./render" && e["to"] == "."),
        "the surface answer needs a lane to this hive: {edges:?}"
    );
    assert!(
        !edges.iter().any(|e| e["to"] == "/"),
        "no lane may address the root hive — that is the edge no mutation can \
         draw: {edges:?}"
    );
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == "./probe" && e["to"] == "/colony/graph"),
        "the hive carries its own topology lane: {edges:?}"
    );
}

/// The store is the hive's own and nobody else's: one writer, and no surface, so
/// the HTTP layer cannot reach the data behind the picture.
#[test]
fn the_store_is_internal_and_declares_no_surface() {
    let Some(root) = shipped_canvy() else { return };
    let cfg = read_json(&root.join("store/config.json"));
    assert_eq!(cfg["params"]["write_surface"], "internal");
    assert!(
        cfg["cell"].get("surface").is_none(),
        "the store must NOT be servable — the renderer is the surface"
    );
    let cols = &cfg["params"]["schema"]["canvas"];
    for (col, ty) in [
        ("kind", "text"),
        ("id", "text"),
        ("x", "int"),
        ("y", "int"),
        ("z", "int"),
        ("doc", "json"),
    ] {
        assert_eq!(cols[col], ty, "column {col}");
    }
}

/// The render cell is the surface, and its declaration names the asset directory
/// that actually ships.
#[test]
fn the_render_cell_declares_the_surface_and_its_assets() {
    let Some(root) = shipped_canvy() else { return };
    let cfg = read_json(&root.join("render/config.json"));
    let decl = &cfg["cell"]["surface"];
    assert_eq!(decl["assets"], "client");
    assert!(decl["title"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(
        root.join("render/client/surface.js").exists()
            && root.join("render/client/surface.css").exists(),
        "the declared asset directory must hold the files the page asks for"
    );
    // The names the dead render hard-codes.
    let js = std::fs::read_to_string(root.join("render/client/surface.js")).unwrap();
    assert!(
        js.contains("SurfaceHooks"),
        "the hook script must fill the slot the binary offers"
    );
    assert!(
        js.contains("node:moved"),
        "and it must be the one that pushes the drop"
    );
}

// ---------------------------------------------------------------------------
// GH #161 — the four defects one live join found, each as its own property.
//
// All four were invisible to the tests that existed: every one of them lives at
// the boundary between an emission and an edge, and the tests above check what a
// script produces, not what the substrate will accept from it.
// ---------------------------------------------------------------------------

/// **Every emission carries a `messages` slot, because the substrate validates
/// every body as a UBF document.**
///
/// The surface reply used to carry only its `surface` slot. It never reached an
/// edge: it was refused as `invalid_ubf_body`, which surfaced as one dead letter
/// from the render cell and a join that timed out with nothing else to see.
#[test]
fn every_emission_carries_a_messages_slot() {
    let Some(root) = shipped_canvy() else { return };

    let join = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(&[("/a/b", "code")], &[]))),
            json!({"operation": "select", "rows_affected": 1}),
        ),
    );
    // A drop, which emits writes AND a reply — every one of the three is a body.
    let drop = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(snapshot_rows(graph_doc(&[("/a/b", "code")], &[]))),
            json!({"operation": "select", "rows_affected": 1}),
            json!({"canvy_origin": "render", "canvy_moved_id": "a/b",
                   "canvy_moved_x": "700", "canvy_moved_y": "300"}),
        ),
    );
    let tick = run_shipped(
        &root,
        "probe",
        stdin_doc(json!({"messages": []}), json!({})),
    );
    let snap = run_shipped(
        &root,
        "probe",
        stdin_doc(
            json!({"graph": {"scope": "/", "nodes": [], "edges": []}}),
            json!({}),
        ),
    );

    for (what, out) in [
        ("render join", &join),
        ("render drop", &drop),
        ("probe tick", &tick),
        ("probe snapshot", &snap),
    ] {
        assert!(!out.is_empty(), "{what}: emitted nothing");
        for (i, em) in out.iter().enumerate() {
            assert!(
                em.get("messages").map(|m| m.is_array()).unwrap_or(false),
                "{what} emission {i} has no `messages` array — the substrate \
                 refuses it as invalid_ubf_body before any edge sees it: {em}"
            );
        }
    }
}

/// **Every store-bound emission carries every hop field the hive's edge modifier
/// reads.**
///
/// The `./render -> ./store` edge promotes four hop fields into `context`. An
/// edge modifier that reads a field the emission does not have does not merely
/// skip the promotion — the edge stops matching, and the emission dead-letters as
/// `no_route`. That is how a drop's two position writes died while the picture
/// still showed the box in its new place: the render left through a different
/// edge.
#[test]
fn store_bound_emissions_satisfy_the_hive_edge_modifier() {
    let Some(root) = shipped_canvy() else { return };

    // The fields the modifier reads, taken from the shipped hive config so this
    // test cannot drift away from the edge it is about.
    let cfg = read_json(&root.join("config.json"));
    let edge = cfg["params"]["graph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["from"] == "./render" && e["to"] == "./store")
        .expect("the render -> store edge")
        .clone();
    let mut needed: Vec<String> = vec![];
    for expr in edge["modifier"]["set_context"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|v| v.as_str())
    {
        if let Some(field) = expr.strip_prefix("hop.") {
            needed.push(field.to_string());
        }
    }
    assert!(
        !needed.is_empty(),
        "this test is pointless if the modifier reads no hop field"
    );

    // A drop emits the two writes; a join emits none. Both must satisfy the edge.
    let drop = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(snapshot_rows(graph_doc(&[("/a/b", "code")], &[]))),
            json!({"operation": "select", "rows_affected": 1}),
            json!({"canvy_origin": "render", "canvy_moved_id": "a/b",
                   "canvy_moved_x": "700", "canvy_moved_y": "300"}),
        ),
    );
    let writes: Vec<&Value> = drop
        .iter()
        .filter(|em| em["header"]["route"] == "canvas")
        .collect();
    assert_eq!(writes.len(), 2, "a drop writes delete + insert: {drop:?}");
    for em in writes {
        for field in &needed {
            assert!(
                em["header"].get(field).is_some(),
                "a store-bound emission is missing hop field `{field}`, so the \
                 hive's edge modifier cannot evaluate and the edge stops matching \
                 — the write dead-letters as no_route: {em}"
            );
        }
    }
}

/// **The store's reply to the probe's own write ends the chain.**
///
/// Pass 2 emits two store ops, the store answers each, and each answer used to
/// fall through to "the timer ticked, ask again" — so one tick became two asks,
/// two became four. The colony's routing loop wedged on a full mailbox inside 20
/// seconds with an empty dead-letter queue.
#[test]
fn the_probe_stops_when_its_own_write_is_answered() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "probe",
        stdin_doc_ctx(
            store_reply(json!([])),
            json!({"operation": "insert", "rows_affected": 1}),
            json!({"canvy_origin": "probe"}),
        ),
    );
    assert!(
        out.is_empty(),
        "the probe must emit NOTHING when the store answers its own write, \
         otherwise every write answer starts a fresh topology read: {out:?}"
    );
}

/// **The privileged lane is conditional, so only the read leaves through it.**
///
/// The lane `./canvy/probe -> /colony/graph` is granted by the parent's
/// `config.json`. Granted unconditionally it matches EVERY emission the probe
/// makes, including the two store writes — so each write also asked for the graph
/// again, each answer produced two more writes, and the growth was exponential.
/// The README is where an operator copies this from, so the README is what this
/// test reads.
#[test]
fn the_documented_colony_lane_is_conditional() {
    let Some(root) = shipped_canvy() else { return };
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    // Per fenced block, not per line: the lane is a JSON object and an operator
    // copies the whole block, so the block is the unit that has to be correct.
    let lanes: Vec<&str> = readme
        .split("```")
        .filter(|b| b.contains("/colony/graph") && b.contains("\"from\""))
        .collect();
    assert!(
        !lanes.is_empty(),
        "the README must show the lane an operator has to write by hand"
    );
    for block in lanes {
        assert!(
            block.contains("condition"),
            "the documented lane must be conditional — unconditional it matches \
             the probe's store writes too, and the growth is exponential: {block}"
        );
    }
    // And the route marker the condition tests is the one the script emits.
    let tick = run_shipped(
        &root,
        "probe",
        stdin_doc(json!({"messages": []}), json!({})),
    );
    assert_eq!(tick.len(), 1, "a tick asks exactly once: {tick:?}");
    assert_eq!(
        tick[0]["header"]["route"], "ask_colony",
        "the condition in the README names this marker"
    );
}
