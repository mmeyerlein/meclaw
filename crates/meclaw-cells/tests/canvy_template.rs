//! GH #159 — the shipped `canvy` template.
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

/// Run a shipped script over a real stdin document, handing the script to the
/// runner **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `<runner> -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `-c` ran it: same `__main__` globals, same stdout, same
/// exit status.
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

/// A store reply whose text is NOT a result set: an error string, or the `null`
/// payload every write answers with.
fn store_reply_text(text: &str) -> Value {
    json!({
        "messages": [
            { "origin": "tool", "type": "tool_result", "id": "x", "text": text }
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
    let needle = format!("data-hive=\"{hive}\"");
    let after = html
        .split(&needle)
        .nth(1)
        .unwrap_or_else(|| panic!("no hive box for {hive} in {html}"));
    // Split at the `<rect` first: the group now carries `data-ox`/`data-oy`, and
    // a naive search for `x="` would find the tail of `data-ox="`.
    let rect = after
        .split("<rect")
        .nth(1)
        .unwrap_or("")
        .split("/>")
        .next()
        .unwrap_or("");
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
    // Split at the `<rect` first: the group now carries `data-ox`/`data-oy`, and
    // a naive search for `x="` would find the tail of `data-ox="`.
    let rect = after
        .split("<rect")
        .nth(1)
        .unwrap_or("")
        .split("/>")
        .next()
        .unwrap_or("");
    let num = |k: &str| -> i64 {
        rect.split(&format!("{k}=\""))
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("no {k} in {rect}"))
    };
    (num(" x"), num(" y"), num("width"), num("height"))
}

/// Does a cell box at `at` overlap the rectangle `r` at all?
fn inside(at: (i64, i64), r: (i64, i64, i64, i64)) -> bool {
    let (x, y) = at;
    let (rx, ry, rw, rh) = r;
    x < rx + rw && rx < x + 150 && y < ry + rh && ry < y + 38
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

/// **The store's answer to a write this cell asked for is not a failure.**
///
/// The `./store -> ./render` edge fires on EVERY store reply, acknowledgements
/// included. An ack is not a result set, so `rows_of` answers `None` for it —
/// correctly — and render turned that `None` into a visible error on the surface.
/// So every drag reported its own two position writes to the browser as "the
/// canvas store did not return rows", and #170's one-time conversion produced 32
/// such reports on a real colony's first render (GH #183).
///
/// What tells the two apart is the store's own reply header: `build_tool_result`
/// stamps the `operation` it ran on every answer, and that header is this cell's
/// `hop` — one edge back, so it survives.
#[test]
fn the_answer_to_renders_own_write_is_not_a_failure_report() {
    let Some(root) = shipped_canvy() else { return };
    for op in ["insert", "delete"] {
        let out = run_shipped(
            &root,
            "render",
            stdin_doc_ctx(
                // What a write really answers with: the payload of a non-select op
                // is `null`, which is precisely why `rows_of` cannot classify it.
                store_reply(Value::Null),
                json!({ "operation": op, "rows_affected": 1 }),
                json!({ "canvy_origin": "render" }),
            ),
        );
        assert!(
            out.is_empty(),
            "the answer to render's own {op} must end the chain silently, not be \
             reported to the surface as a failure: {out:?}"
        );
    }
}

/// **A store that really did fail still reaches the surface.**
///
/// The guard above must not become a blanket silence. A read that came back as
/// something other than rows is a genuine defect an operator has to see — it is
/// how the missing `columns` argument was found in the first place.
#[test]
fn a_store_error_on_the_read_still_reaches_the_surface() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply_text("no such table: canvas"),
            json!({ "operation": "select", "rows_affected": 0 }),
            json!({ "canvy_origin": "render" }),
        ),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    let err = out[0]["surface"]["error"].as_str().unwrap();
    assert!(err.contains("no such table"), "{err}");
}

/// **A write that FAILED is not an acknowledgement.**
///
/// A SQL error is a normal `tool_result` and not an error message (brainstorm
/// E5), so the store stamps BOTH the operation it ran and an `error_code` on the
/// same reply. The two guards therefore have an order: the failure is read first,
/// and only an answer with no `error_code` counts as an echo of our own write.
#[test]
fn a_write_that_failed_is_not_mistaken_for_an_acknowledgement() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply_text("UNIQUE constraint failed: canvas.id"),
            json!({ "operation": "insert", "rows_affected": 0, "error_code": "sql_error" }),
            json!({ "canvy_origin": "render" }),
        ),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    let err = out[0]["surface"]["error"].as_str().unwrap();
    assert!(err.contains("sql_error"), "{err}");
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
    // Sharpened when the arrowhead markers arrived: a `<marker>` glyph is a `d`
    // too, and it is not an edge path. What must not exist is a `d` on an element
    // that IS an edge — the routing is the client's, in one language.
    for chunk in html.split("class=\"edge").skip(1) {
        let element = chunk.split("/>").next().unwrap_or("");
        assert!(
            !element.contains(" d=\""),
            "an edge path leaked into the server's markup: {element}"
        );
    }
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

/// **A disconnected canvas says so.** A picture drawn before a colony restart and
/// a live one are pixel-identical, and that is the half of GH #172 no server fix
/// reaches: a join renders instead of taking the cache now, but the transport
/// rejoins on its own schedule, and until it does the browser is holding whatever
/// was last drawn. Minutes of it, on the report this came from.
///
/// The vendored LiveView client already publishes the state on the `data-phx-main`
/// container; the stylesheet simply was not reading it. The class names are read
/// back OUT of the shipped bundle rather than typed here, because a rule for a
/// class nobody sets is silent — and a client-side path that only looks wired is
/// the exact defect this template shipped once already.
#[test]
fn the_stylesheet_marks_the_disconnected_states_the_client_publishes() {
    let Some(root) = shipped_canvy() else { return };
    let css = std::fs::read_to_string(root.join("render/client/surface.css")).unwrap();
    let bundle = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../meclaw-api/src/surface/client/phoenix_live_view.min.js");
    let Ok(js) = std::fs::read_to_string(&bundle) else {
        return; // the binary's vendored copy is not in this tree
    };
    for class in [
        "phx-loading",
        "phx-error",
        "phx-client-error",
        "phx-server-error",
    ] {
        assert!(
            js.contains(&format!("\"{class}\"")),
            "the vendored client no longer publishes `{class}`, so the rule for it is dead"
        );
        assert!(
            css.contains(&format!(".{class}")),
            "surface.css says nothing about `{class}` — a stale canvas then looks live"
        );
    }
    assert!(
        css.contains("out of date"),
        "the marking has to be readable as words, not only as a shade of grey"
    );
}

/// **A hive is draggable too, and it costs ONE row.** Reported right after the
/// canvas became usable: the boxes moved, the groups did not.
///
/// A hive box is derived from where its members ended up (`hive_boxes`) and is
/// never stored — that is what makes dragging a cell out of a crowd grow its hive
/// instead of stranding it outside a stale rectangle. So moving a hive cannot mean
/// "store the rectangle": it means storing one SHIFT for the group, which the
/// layout applies to its members before their own saved positions win. Two store
/// ops per hive drag, exactly like a cell — not two per member.
///
/// The client names the origin it was handed plus the drag; what is written is the
/// difference, because only a shift survives a colony that grows (GH #170).
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
    assert_eq!(ops[0]["where"]["kind"], "hive_shift");
    assert_eq!(ops[0]["where"]["id"], "a");
    assert_eq!(ops[1]["row"]["kind"], "hive_shift");
    assert_eq!(ops[1]["row"]["x"], 500);
    assert_eq!(ops[1]["row"]["y"], 300);

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

/// **Moving a group moves the group.** A hand-placed cell travels with its hive,
/// keeping its position RELATIVE to the group — because the alternative, which this
/// view shipped with, is that the two gestures fight: the hive offset was applied
/// only to cells nobody had touched, so a hive with one hand-placed cell in it had
/// no single position at all, and the cell was left behind every time the group
/// moved.
///
/// The mechanism is what makes it hold: a stored cell position lives in the
/// layout's own space, BEFORE the shifts of the hives above it, so the two add up
/// instead of overriding each other.
#[test]
fn a_hand_placed_cell_travels_with_its_hive() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let plain = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let untouched = node_positions(&plain, &["a/two"])[0];

    let mut rows = snapshot_rows(graph);
    let arr = rows.as_array_mut().unwrap();
    arr.push(json!({"kind": "node", "id": "a/one", "x": 4000, "y": 4000}));
    arr.push(json!({"kind": "hive_shift", "id": "a", "x": 900, "y": 900}));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));

    assert_eq!(
        node_positions(&html, &["a/one"])[0],
        (4900, 4900),
        "the hand-placed cell keeps its offset from the group it belongs to"
    );
    assert_eq!(
        node_positions(&html, &["a/two"])[0],
        (untouched.0 + 900, untouched.1 + 900),
        "and its neighbour follows the same shift"
    );
}

/// **GH #170 — a stored arrangement survives the colony growing.** Instantiate a
/// cell in a colony somebody arranged by hand and the hand-placed hives moved,
/// every one of them: 12 of 19 frames on the colony this was reported from, and
/// 17 of 53 hand-placed cells with them. Nothing about the new cell said where
/// they should go; it was the reference point that moved underneath them.
///
/// A hive row used to hold a POINT in the flow layout's space, re-read on every
/// render as a delta against that layout's own idea of the same corner — and the
/// flow layout is a function of the WHOLE node set, so one arrival redefined the
/// point every stored hive was measured against. Here `a/new` arrives beside
/// `a/b`, which pushes `a/c`'s computed corner 72 pixels down, and the anchored
/// `a/c` and both of its hand-placed cells answered by walking 72 pixels UP.
///
/// The arrangement is made the way an operator makes one — a drag — and whatever
/// row that drag writes IS the arrangement. This test never spells that row out,
/// so what it pins is the property and not the shape the property is stored in.
#[test]
fn an_arriving_cell_leaves_a_stored_arrangement_where_it_was() {
    let Some(root) = shipped_canvy() else { return };
    let before_nodes: &[(&str, &str)] = &[
        ("/a/own", "llm"),
        ("/a/b/one", "llm"),
        ("/a/b/two", "store"),
        ("/a/c/three", "code"),
        ("/a/c/four", "code"),
    ];
    let before_edges: &[(&str, &str, &str)] = &[
        ("e1", "/a/own", "/a/b/one"),
        ("e2", "/a/b/one", "/a/b/two"),
        ("e3", "/a/b/two", "/a/c/three"),
        ("e4", "/a/c/three", "/a/c/four"),
    ];
    // Both cells of `a/c` are placed by hand. That is what makes the hive an
    // arrangement rather than a layout, and what the arrival must not disturb.
    let pinned = |graph: Value| {
        let mut rows = snapshot_rows(graph);
        let arr = rows.as_array_mut().unwrap();
        arr.push(json!({"kind": "node", "id": "a/c/three", "x": 240, "y": 400}));
        arr.push(json!({"kind": "node", "id": "a/c/four", "x": 480, "y": 400}));
        rows
    };
    let graph = graph_doc(before_nodes, before_edges);

    // …and the hive itself is dragged 300 right and 200 down.
    let corner = hive_box(
        &html_of(&run_shipped(
            &root,
            "render",
            stdin_doc_pass2(store_reply(pinned(graph.clone())), json!({})),
        )),
        "a/c",
    );
    let drag = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(pinned(graph.clone())),
            json!({}),
            json!({
                "canvy_origin": "render",
                "canvy_event": "hive:moved",
                "canvy_moved_id": "a/c",
                "canvy_moved_x": (corner.0 + 300).to_string(),
                "canvy_moved_y": (corner.1 + 200).to_string(),
            }),
        ),
    );
    let written = store_ops(&drag)
        .into_iter()
        .find(|o| o["operation"] == "insert")
        .expect("a hive drag writes a row")["row"]
        .clone();

    let arrangement = |graph: Value| {
        let mut rows = pinned(graph);
        rows.as_array_mut().unwrap().push(written.clone());
        rows
    };
    let before = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(arrangement(graph)), json!({})),
    ));

    // One cell is instantiated, wired into a hive the arrangement never touched.
    // Nothing else changes.
    let mut nodes = before_nodes.to_vec();
    nodes.push(("/a/new", "code"));
    let mut edges = before_edges.to_vec();
    edges.push(("e5", "/a/own", "/a/new"));
    let after = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(arrangement(graph_doc(&nodes, &edges))),
            json!({}),
        ),
    ));

    assert_eq!(
        node_positions(&after, &["a/c/three", "a/c/four"]),
        node_positions(&before, &["a/c/three", "a/c/four"]),
        "a cell somebody placed by hand may only move when a hand moves it"
    );
    assert_eq!(
        hive_frame(&after, "a/c"),
        hive_frame(&before, "a/c"),
        "and the frame those cells make must not move either"
    );
}

/// A canvas whose table has outlived part of its colony: `a/one` and the hive `a`
/// are still there, `a/gone` and the hive `b` are not.
fn rows_naming_nothing() -> Value {
    let mut rows = snapshot_rows(graph_doc(&[("/a/one", "code"), ("/a/two", "code")], &[]));
    let list = rows.as_array_mut().unwrap();
    list.push(json!({"kind": "node", "id": "a/one", "x": 700, "y": 300}));
    list.push(json!({"kind": "node", "id": "a/gone", "x": 11, "y": 22}));
    list.push(json!({"kind": "hive_shift", "id": "a", "x": 40, "y": 50}));
    list.push(json!({"kind": "hive_shift", "id": "b", "x": 60, "y": 70}));
    rows
}

/// **A row that names nothing is never swept behind the operator's back.**
///
/// From the table's side a rename and a removal are the SAME event: the colony has
/// no rename operation at all (`add_nodes` / `remove_nodes` — see
/// `mutation/validate.rs`), so a rename IS a removal plus an arrival under a name
/// the row cannot know. On the colony this was reported from, all four hive rows
/// naming nothing were renames — `talky/keeper` -> `talky/session-keeper`,
/// `talky/summary` -> `talky/summarizer`, `archive` -> `day-archive`,
/// `memdrain` -> `memory-drain` — so an eager sweep would have thrown away four
/// hand-placed group positions and nothing else. A stale snapshot reads the same
/// way again: the topology arrives on a timer, so "not in the picture" also means
/// "the timer has not run since this cell was added".
///
/// So the render reports and never deletes. This also keeps #183 shut: a join that
/// finds stale rows must stay a read, or every render becomes a write.
#[test]
fn a_row_that_names_nothing_is_reported_and_not_swept() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows_naming_nothing()), json!({})),
    );
    assert!(
        store_ops(&out).is_empty(),
        "a join must not shed rows on its own: {:?}",
        store_ops(&out)
    );
    let html = html_of(&out);
    assert!(
        html.contains("2 rows name nothing"),
        "the picture has to say what the table is carrying: {html}"
    );
    assert!(
        html.contains("data-sweep"),
        "and offer the one gesture that can decide it: {html}"
    );
}

/// **A colony with nothing to shed offers nothing to press.**
#[test]
fn the_sweep_is_offered_only_when_there_is_something_to_shed() {
    let Some(root) = shipped_canvy() else { return };
    let mut rows = snapshot_rows(graph_doc(&[("/a/one", "code")], &[]));
    rows.as_array_mut()
        .unwrap()
        .push(json!({"kind": "node", "id": "a/one", "x": 700, "y": 300}));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(!html.contains("data-sweep"), "{html}");
    assert!(!html.contains("name nothing"), "{html}");
}

/// **The sweep sheds exactly the rows that name nothing, and only when asked.**
///
/// The operator is the only party that knows whether a name went away or moved,
/// so the deletion is their gesture and the render is only ever the reporter.
#[test]
fn a_sweep_sheds_exactly_the_rows_that_name_nothing() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(rows_naming_nothing()),
            json!({ "operation": "select", "rows_affected": 5 }),
            json!({ "canvy_origin": "render", "canvy_event": "canvas:sweep" }),
        ),
    );
    let ops = store_ops(&out);
    assert_eq!(
        ops.len(),
        2,
        "one delete per orphan row and no more: {ops:?}"
    );
    for op in &ops {
        assert_eq!(op["operation"], "delete", "{op}");
    }
    let shed: Vec<(String, String)> = ops
        .iter()
        .map(|o| {
            (
                o["where"]["kind"].as_str().unwrap_or("").to_string(),
                o["where"]["id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert!(
        shed.contains(&("node".to_string(), "a/gone".to_string())),
        "{shed:?}"
    );
    assert!(
        shed.contains(&("hive_shift".to_string(), "b".to_string())),
        "{shed:?}"
    );
    for (_, id) in &shed {
        assert!(
            id != "a/one" && id != "a",
            "the sweep touched something the colony still has: {shed:?}"
        );
    }
    // And the picture still comes back: a sweep is a render that also wrote.
    assert!(html_of(&out).contains("data-node=\"a/one\""));
}

/// **A second sweep writes nothing.** Idempotent by the same rule the #170
/// conversion converges by: a pass that finds nothing to do must do nothing, or
/// every press becomes a write and every write an answer (GH #183).
#[test]
fn a_sweep_with_nothing_to_shed_writes_nothing() {
    let Some(root) = shipped_canvy() else { return };
    let mut rows = snapshot_rows(graph_doc(&[("/a/one", "code"), ("/a/two", "code")], &[]));
    let list = rows.as_array_mut().unwrap();
    list.push(json!({"kind": "node", "id": "a/one", "x": 700, "y": 300}));
    list.push(json!({"kind": "hive_shift", "id": "a", "x": 40, "y": 50}));
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(rows),
            json!({ "operation": "select", "rows_affected": 4 }),
            json!({ "canvy_origin": "render", "canvy_event": "canvas:sweep" }),
        ),
    );
    assert!(
        store_ops(&out).is_empty(),
        "a sweep with nothing to shed must write nothing: {:?}",
        store_ops(&out)
    );
}

/// **The conversion, once.** The colonies this shipped to hold hive rows in the old
/// shape — a point in the flow layout's space — and re-arranging a 50-cell canvas
/// by hand is not a migration path. So the point is read once, through the very
/// layout it was written against, and rewritten as the shift it always meant.
///
/// Two properties, and the fix is worthless without either: the picture after the
/// conversion is the picture before it, and the conversion CONVERGES — a render
/// that finds no old row writes nothing, or the surface writes to its own store on
/// every join for ever.
#[test]
fn an_old_hive_point_is_converted_to_a_shift_exactly_once() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let plain = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let corner = hive_box(&plain, "a");

    let mut rows = snapshot_rows(graph.clone());
    rows.as_array_mut()
        .unwrap()
        .push(json!({"kind": "hive", "id": "a", "x": corner.0 + 500, "y": corner.1 + 300}));
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    );

    let ops = store_ops(&out);
    assert_eq!(ops.len(), 2, "one row in, one row out: {ops:?}");
    assert_eq!(ops[0]["operation"], "delete");
    assert_eq!(ops[0]["where"]["kind"], "hive");
    assert_eq!(ops[1]["row"]["kind"], "hive_shift");
    assert_eq!(ops[1]["row"]["x"], 500);
    assert_eq!(ops[1]["row"]["y"], 300);

    // The same colony, with the converted row in place of the old one.
    let mut converted = snapshot_rows(graph);
    converted
        .as_array_mut()
        .unwrap()
        .push(json!({"kind": "hive_shift", "id": "a", "x": 500, "y": 300}));
    let second = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(converted), json!({})),
    );
    assert!(
        store_ops(&second).is_empty(),
        "a converted colony writes nothing on a join: {:?}",
        store_ops(&second)
    );
    assert_eq!(
        hive_frame(&html_of(&second), "a"),
        hive_frame(&html_of(&out), "a"),
        "and it is the same picture on both sides of the conversion"
    );
}

/// **A drop lands where it was let go, and stays there.** Reported as "after a
/// move, the moved elements jump back again": the client names
/// an absolute point, and inside a hive somebody had moved, that point is only
/// where it is BECAUSE of the hive's offset. Storing it verbatim re-applied the
/// offset on the next render, so every drop inside a moved hive landed one shift
/// away from the cursor.
#[test]
fn a_drop_inside_a_moved_hive_does_not_jump() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let mut rows = snapshot_rows(graph.clone());
    rows.as_array_mut()
        .unwrap()
        .push(json!({"kind": "hive_shift", "id": "a", "x": 900, "y": 900}));

    // Drop `a/one` at an absolute point, exactly as the browser reports it.
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(rows.clone()),
            json!({}),
            json!({
                "canvy_origin": "render",
                "canvy_event": "node:moved",
                "canvy_moved_id": "a/one",
                "canvy_moved_x": "1500",
                "canvy_moved_y": "1200",
            }),
        ),
    );
    assert_eq!(
        node_positions(&html_of(&out), &["a/one"])[0],
        (1500, 1200),
        "the box is drawn where it was dropped"
    );

    // Now replay what the store holds afterwards: the SAME picture has to come back.
    let ops = store_ops(&out);
    let row = &ops[1]["row"];
    let mut again = rows;
    again.as_array_mut().unwrap().push(json!({
        "kind": "node", "id": "a/one", "x": row["x"], "y": row["y"],
    }));
    let replay = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(again), json!({})),
    ));
    assert_eq!(
        node_positions(&replay, &["a/one"])[0],
        (1500, 1200),
        "and it is still there on the next render — no jump"
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

/// **Direction.** 123 edges and no arrowhead: the stylesheet says
/// `marker-end: url(#ar)` and the markup never defined `#ar`, so every edge in
/// every picture was an undirected line. A graph you cannot read the direction of
/// is not a graph, it is a doodle — and it was like that from the first render,
/// because the stylesheet was copied from a working tool and the markup was not.
#[test]
fn every_edge_points_somewhere() {
    let Some(root) = shipped_canvy() else { return };
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(
                &[("/a/one", "llm"), ("/a/two", "store")],
                &[("e0", "/a/one", "/a/two")],
            ))),
            json!({}),
        ),
    ));
    // Every marker the stylesheet references has to exist in the document that
    // uses it — read out of the CSS, so a renamed marker is red on both sides.
    let css = std::fs::read_to_string(root.join("render/client/surface.css")).unwrap();
    let referenced: std::collections::HashSet<&str> = css
        .split("marker-end:url(#")
        .skip(1)
        .filter_map(|r| r.split(')').next())
        .collect();
    assert!(
        !referenced.is_empty(),
        "the stylesheet must ask for arrowheads, or this test proves nothing"
    );
    for id in referenced {
        assert!(
            html.contains(&format!("<marker id=\"{id}\"")),
            "the markup must define the marker `{id}` its own stylesheet uses: {html}"
        );
    }
    assert!(
        html.contains("<defs>"),
        "markers live in a defs block: {html}"
    );
}

/// A conditional edge has to LOOK conditional — the stylesheet dashes `.cond`, and
/// the markup never set it. Half of a colony's edges carry a condition; a picture
/// that draws them like the rest hides the thing an operator is looking for.
#[test]
fn a_conditional_edge_is_drawn_as_one() {
    let Some(root) = shipped_canvy() else { return };
    let mut graph = graph_doc(
        &[("/a/one", "llm"), ("/a/two", "store")],
        &[("e0", "/a/one", "/a/two"), ("e1", "/a/two", "/a/one")],
    );
    graph["edges"][0]["condition"] = json!("has(hop.route) && hop.route == 'x'");
    graph["edges"][0]["modifier"] = json!({"set_context": {"k": "'v'"}});
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph)), json!({})),
    ));
    assert!(
        html.contains("class=\"edge cond\""),
        "the conditional edge must carry the class the stylesheet dashes: {html}"
    );
    assert_eq!(
        html.matches("class=\"edge cond\"").count(),
        1,
        "and the unconditional one must NOT: {html}"
    );
    // The condition and the modifier travel with the edge, so clicking it can show
    // them without another round trip.
    assert!(
        html.contains("hop.route == &#39;x&#39;"),
        "condition text: {html}"
    );
    assert!(
        html.contains("data-mod="),
        "the modifier rides along too: {html}"
    );
}

/// An edge is 1.4 pixels wide. Clicking one has to be possible with a mouse, so
/// every edge carries a fat invisible twin — the same construction the standalone
/// renderer uses, and the reason its edges are clickable at all.
#[test]
fn every_edge_has_something_to_click() {
    let Some(root) = shipped_canvy() else { return };
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(
                &[("/a/one", "llm"), ("/a/two", "store")],
                &[("e0", "/a/one", "/a/two"), ("e1", "/a/two", "/a/one")],
            ))),
            json!({}),
        ),
    ));
    assert_eq!(
        html.matches("class=\"edge-hit\"").count(),
        2,
        "one hit path per edge: {html}"
    );
}

/// The page needs somewhere to say what was clicked. Without a panel the whole
/// selection idea is invisible, and the condition of an edge stays a string in an
/// attribute nobody can read.
#[test]
fn the_page_carries_a_detail_panel() {
    let Some(root) = shipped_canvy() else { return };
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(&[("/a/one", "llm")], &[]))),
            json!({}),
        ),
    ));
    assert!(html.contains("id=\"detail\""), "a detail panel: {html}");
    assert!(
        html.contains("class=\"legend\""),
        "and the legend stays: {html}"
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

/// GH #197 — the hive is sealed to its own path, and says what it does in lanes.
///
/// `canvy@0.2` declared `ports: ["render", "refresh"]`, which are the names of
/// two cells in here. That is the boundary moved one step rather than drawn: a
/// caller had to know the inside in order to address it (overview § Die
/// Hive-Grenze, requirement 2).
#[test]
fn the_hive_is_sealed_to_its_own_path_and_states_its_lanes() {
    let Some(root) = shipped_canvy() else { return };
    let cfg = read_json(&root.join("config.json"));
    assert_eq!(cfg["cell"]["type"], "hive");
    let ports = cfg["params"]["ports"]
        .as_array()
        .expect("params.ports is declared");
    assert!(
        ports.is_empty(),
        "the hive path is the only address, got {ports:?}"
    );

    let lanes = |key: &str| -> Vec<String> {
        cfg["params"]["contract"][key]
            .as_array()
            .unwrap_or_else(|| panic!("params.contract.{key}"))
            .iter()
            .map(|l| {
                assert!(
                    !l["because"].as_str().expect("a lane says why").is_empty(),
                    "a lane without a sentence is a lane nobody can wire"
                );
                l["route"].as_str().expect("a lane is a route").to_string()
            })
            .collect()
    };
    assert_eq!(lanes("accepts"), vec!["in_refresh".to_string()]);
    assert_eq!(lanes("emits"), vec!["surface".to_string()]);
    // And no lane carries the name of a cell in here.
    for lane in lanes("accepts").iter().chain(lanes("emits").iter()) {
        for cell in ["render", "refresh", "probe", "store"] {
            assert_ne!(lane, cell, "'{lane}' is a cell of this hive, not a lane");
        }
    }

    // The knowledge that `in_refresh` is served by the probe lives on exactly
    // one edge, and it is this hive's own (requirement 3).
    let edges = cfg["params"]["graph"]["edges"].as_array().unwrap();
    let doors: Vec<&Value> = edges.iter().filter(|e| e["from"] == ".").collect();
    assert_eq!(doors.len(), 1, "one door in: {edges:?}");
    assert_eq!(doors[0]["to"], "./probe");
    assert!(
        doors[0]["condition"]
            .as_str()
            .unwrap()
            .contains("'in_refresh'")
    );

    // GH #163: both of the hive's outward lanes stay INSIDE what a mutation may
    // draw, which is what makes the whole template installable into a running
    // colony. The answer goes to the hive itself and leaves through the egress
    // door from there (the door is opened by the marker, not by the root hive);
    // the topology lane addresses the colony's read-only endpoint, the one
    // absolute endpoint a mutation is allowed. A regression to `-> /` would make
    // the mutation `scope_out_of_bounds` again.
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

/// **The flow reads left to right.** A turn travels through a colony, and the
/// picture is only legible if it travels across the page the way a reader's eye
/// does. The first arrangement stacked flow rank downwards, so a chain of four
/// cells was a column and the horizontal axis carried nothing at all.
///
/// A chain also must not be split by the wrapping: wrapping exists so a block that
/// outgrows a screen folds into rows, and a four-cell chain does not.
#[test]
fn a_chain_runs_across_the_page() {
    let Some(root) = shipped_canvy() else { return };
    let cells = [
        ("/h/one", "proxy"),
        ("/h/two", "llm"),
        ("/h/three", "code"),
        ("/h/four", "store"),
    ];
    let edges = [
        ("e1", "/h/one", "/h/two"),
        ("e2", "/h/two", "/h/three"),
        ("e3", "/h/three", "/h/four"),
    ];
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(&cells, &edges))),
            json!({}),
        ),
    ));
    let at = node_positions(&html, &["h/one", "h/two", "h/three", "h/four"]);
    for pair in at.windows(2) {
        assert!(
            pair[1].0 > pair[0].0,
            "each step of the chain sits to the RIGHT of the one that feeds it: {at:?}"
        );
        assert_eq!(
            pair[1].1, pair[0].1,
            "and on the same line, so the chain is one straight run: {at:?}"
        );
    }
}

/// **A hive is anchored by its block, not by its rectangle.** The rectangle is
/// derived from the cells inside it, so it moves whenever they do — anchoring a
/// group to it meant that dragging one cell leftwards out of a hive shoved the
/// whole group to the right on the next render, and that a dropped hive never
/// landed twice in the same place.
#[test]
fn a_hive_carries_an_anchor_that_its_contents_cannot_move() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);
    let anchor_of = |html: &str| -> (i64, i64) {
        let after = html.split("data-hive=\"a\"").nth(1).unwrap_or("");
        let head = after.split('>').next().unwrap_or("");
        let num = |k: &str| -> i64 {
            head.split(&format!("{k}=\""))
                .nth(1)
                .and_then(|r| r.split('"').next())
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("no {k} in {head}"))
        };
        (num("data-ox"), num("data-oy"))
    };

    let plain = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let before = anchor_of(&plain);
    let frame_before = hive_box(&plain, "a");

    // Drag one cell far to the upper left. The frame HAS to grow to hold it...
    let mut rows = snapshot_rows(graph);
    rows.as_array_mut()
        .unwrap()
        .push(json!({"kind": "node", "id": "a/one", "x": -900, "y": -700}));
    let after = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(
        hive_box(&after, "a").0 < frame_before.0,
        "the frame must grow to hold a cell dragged out of it"
    );
    // ...and the anchor must not have moved with it.
    assert_eq!(
        anchor_of(&after),
        before,
        "the anchor is the block's, so what is inside the block cannot shift it"
    );
}

/// **Where the operator is looking is part of the arrangement.** The camera was
/// read back on every render and never once written, so a reload threw away the
/// zoom and the corner of a 2000-pixel picture somebody had just navigated to —
/// while every box position survived, which made it read like a fluke rather than
/// a missing write.
#[test]
fn the_camera_is_remembered() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm")], &[]);
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_ctx(
            store_reply(snapshot_rows(graph.clone())),
            json!({}),
            json!({
                "canvy_origin": "render",
                "canvy_event": "camera:moved",
                "canvy_moved_x": "-320",
                "canvy_moved_y": "180",
                "canvy_moved_z": "1750",
            }),
        ),
    );
    let ops = store_ops(&out);
    assert_eq!(
        ops.len(),
        2,
        "one delete and one insert for the view: {ops:?}"
    );
    assert_eq!(ops[0]["where"]["kind"], "camera");
    assert_eq!(ops[1]["row"]["kind"], "camera");
    assert_eq!(ops[1]["row"]["x"], -320);
    assert_eq!(ops[1]["row"]["z"], 1750);

    // And it comes back on the next render, as the transform on the viewport.
    let mut rows = snapshot_rows(graph);
    rows.as_array_mut()
        .unwrap()
        .push(json!({"kind": "camera", "id": "view", "x": -320, "y": 180, "z": 1750}));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));
    assert!(
        html.contains("translate(-320,180) scale(1.75)"),
        "the stored view has to be the one rendered: {html}"
    );
}

/// A cell move must not disturb the view, and the camera write must not disturb a
/// position: they are separate rows, and the emission carries every slot the hive's
/// edge promotes — an emission missing one does not merely skip the promotion, the
/// edge stops matching and the write dead-letters as `no_route`.
#[test]
fn every_store_emission_carries_the_zoom_slot() {
    let Some(root) = shipped_canvy() else { return };
    let out = run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(&[("/a/one", "llm")], &[]))),
            json!({}),
        ),
    );
    for m in &out {
        let canvas = m
            .get("header")
            .filter(|h| h.get("route").and_then(Value::as_str) == Some("canvas"));
        if let Some(h) = canvas {
            assert!(
                h.get("moved_z").is_some(),
                "every canvas emission needs the slot the edge reads: {h:?}"
            );
        }
    }
}

/// **Moving an outer hive moves everything under it — sub-hives included.**
///
/// Reported as "when I move an outer hive, the one code cell inside it is not
/// carried along properly": what actually happened was the reverse, and worse.
/// EVERY cell inside a sub-hive stayed put and the one cell that sat in no
/// sub-hive was the only thing that travelled — so the report read as "one cell
/// misbehaves" when it was the only one behaving.
///
/// The cause: an inner hive's stored anchor was compared against the origin as it
/// stood AFTER its parent had been shifted, so each inner anchor said "put me back
/// exactly where I was" and cancelled the parent's move. Anchors are measured in
/// the untouched layout; the shifts of the hives above simply add on top.
#[test]
fn moving_an_outer_hive_takes_the_inner_ones_with_it() {
    let Some(root) = shipped_canvy() else { return };
    // An outer hive with a loose cell of its own AND two sub-hives, one nested two
    // deep — the shape that produced the report.
    let cells = [
        ("/top/loose", "code"),
        ("/top/inner/one", "llm"),
        ("/top/inner/deep/two", "store"),
        ("/top/other/three", "code"),
    ];
    let plain = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(snapshot_rows(graph_doc(&cells, &[]))),
            json!({}),
        ),
    ));
    let anchor = hive_box(&plain, "top");
    let ids = [
        "top/loose",
        "top/inner/one",
        "top/inner/deep/two",
        "top/other/three",
    ];
    let before = node_positions(&plain, &ids);

    // The sub-hives have anchors of their own — somebody arranged them first,
    // which is the state the defect needed.
    let inner = hive_box(&plain, "top/inner");
    let other = hive_box(&plain, "top/other");
    let arrange = |extra: Vec<Value>| -> Value {
        let mut rows = snapshot_rows(graph_doc(&cells, &[]));
        let arr = rows.as_array_mut().unwrap();
        arr.push(json!({"kind": "hive_shift", "id": "top/inner", "x": 40, "y": 60}));
        arr.push(json!({"kind": "hive_shift", "id": "top/other", "x": -30, "y": 10}));
        arr.extend(extra);
        rows
    };
    let arranged = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(arrange(vec![])), json!({})),
    ));
    let placed = node_positions(&arranged, &ids);

    // Now drag the OUTER hive. Everything under it must move by the same delta.
    let moved = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(
            store_reply(arrange(vec![
                json!({"kind": "hive_shift", "id": "top", "x": 500, "y": 300}),
            ])),
            json!({}),
        ),
    ));
    let after = node_positions(&moved, &ids);

    for (k, id) in ids.iter().enumerate() {
        assert_eq!(
            (after[k].0 - placed[k].0, after[k].1 - placed[k].1),
            (500, 300),
            "{id} must travel with the hive that contains it"
        );
    }
    // And the inner arrangement is preserved, not flattened back to automatic.
    assert_ne!(
        placed[1], before[1],
        "the sub-hive's own anchor still has to do something"
    );

    // The FRAMES have to travel too, and they are the half a cell check cannot
    // see: a hive box is drawn from the cells it contains, so a frame could in
    // principle be recomputed to the same place while its contents moved away
    // underneath it. Then the box and its contents have come apart, every cell
    // assertion above still passes, and the picture is wrong.
    for hive in ["top", "top/inner", "top/other"] {
        let (px, py) = hive_box(&arranged, hive);
        let (mx, my) = hive_box(&moved, hive);
        assert_eq!(
            (mx - px, my - py),
            (500, 300),
            "the {hive} frame must travel with the hive that contains it"
        );
    }

    // ...and the sub-hives really were arranged away from where the automatic
    // layout put them, or the frame check above proves nothing: it would be
    // comparing two computed layouts that agree because nobody touched either.
    assert_ne!(
        hive_box(&arranged, "top/inner"),
        inner,
        "the stored shift has to move the inner hive's frame"
    );
    assert_ne!(
        hive_box(&arranged, "top/other"),
        other,
        "the stored shift has to move the other hive's frame"
    );
    // The outer hive was never shifted by hand — it moves only because its
    // children did, which is what makes it the interesting one to drag later.
    assert_ne!(
        hive_box(&arranged, "top"),
        anchor,
        "an outer frame follows the cells it contains"
    );
}

/// **A cell nobody placed does not land on one somebody did.** In an arrangement
/// made by hand every cell carries a stored position, so a cell instantiated
/// later is the only one the automatic layout still places — into a picture that
/// layout no longer describes. Adding six tool cells to a fifty-cell colony put
/// three of them on top of hand-placed ones (GH #167), which reads as a
/// rendering fault and costs the operator the drag the layout was there to save.
///
/// The rule this pins: a stored position never moves, and a cell without one is
/// offered its computed spot first and the nearest free one after that.
#[test]
fn a_new_cell_gives_way_to_the_hand_placed_ones() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(&[("/a/one", "llm"), ("/a/two", "store")], &[]);

    // `two` is pinned exactly where the layout would otherwise put `one`, so the
    // collision is certain rather than a happy accident of this graph's shape.
    let computed = node_positions(
        &html_of(&run_shipped(
            &root,
            "render",
            stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
        )),
        &["a/one"],
    )[0];

    let mut rows = snapshot_rows(graph);
    rows.as_array_mut()
        .unwrap()
        .push(json!({"kind": "node", "id": "a/two", "x": computed.0, "y": computed.1}));
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));

    let placed = node_positions(&html, &["a/one", "a/two"]);
    assert_eq!(
        placed[1], computed,
        "the hand-placed cell is the one that must not move"
    );
    assert_ne!(
        placed[0], placed[1],
        "and the new cell must not be left sitting on top of it"
    );
    let (dx, dy) = (
        (placed[0].0 - placed[1].0).abs(),
        (placed[0].1 - placed[1].1).abs(),
    );
    assert!(
        dx >= 150 || dy >= 38,
        "the two boxes still overlap: {placed:?}"
    );
}

/// **A colony nobody has arranged is untouched by that rule.** Without a single
/// stored position there is nothing to give way to, and the flow layout does not
/// overlap itself — so the picture has to come out byte-identical to the one
/// before the rule existed. This is the guard against a collision pass that
/// quietly starts rearranging a layout it was only supposed to repair.
#[test]
fn the_settling_pass_does_nothing_to_an_untouched_colony() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(
        &[
            ("/a/one", "llm"),
            ("/a/two", "store"),
            ("/a/three", "code"),
            ("/b/four", "timer"),
        ],
        &[("e1", "/a/one", "/a/two"), ("e2", "/a/two", "/a/three")],
    );
    let once = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let twice = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph)), json!({})),
    ));
    assert_eq!(once, twice, "the render is deterministic");
    let p = node_positions(&once, &["a/one", "a/two", "a/three", "b/four"]);
    for i in 0..p.len() {
        for j in (i + 1)..p.len() {
            assert!(
                (p[i].0 - p[j].0).abs() >= 150 || (p[i].1 - p[j].1).abs() >= 38,
                "the untouched layout overlaps on its own: {:?} vs {:?}",
                p[i],
                p[j]
            );
        }
    }
}

/// **A cell that has to give way gives way INTO its own hive.** The ring search
/// that finds it a free spot ranked candidates by distance alone, so the nearest
/// one was as likely to be outside the group it belongs to as inside it — and a
/// box that leaves its hive drags the frame out with it, because the frame is
/// derived from its members. "Nearest free spot, preferring inside its own hive"
/// is the rule GH #167 asks for, and the preference is the half that was missing.
///
/// The arrangement here leaves room on both sides: the spot below the crowd is
/// free and outside the hand-placed group's rectangle, the spot above it is free
/// and inside. Distance alone picks the one below (it did, at 42,120); the
/// preference picks the one above.
#[test]
fn a_cell_that_gives_way_gives_way_into_its_own_hive() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(
        &[
            ("/a/one", "llm"),
            ("/a/two", "store"),
            ("/a/three", "code"),
            ("/a/new", "code"),
        ],
        &[],
    );
    // Where the flow layout puts the cell nobody has placed.
    let computed = node_positions(
        &html_of(&run_shipped(
            &root,
            "render",
            stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
        )),
        &["a/new"],
    )[0];

    // `one` is pinned exactly on that spot, so the collision is certain; `two` and
    // `three` pull the group's rectangle left and up, so "inside the group" and
    // "nearest by distance" point in different directions.
    let hand = [
        ("a/one", computed.0, computed.1),
        ("a/two", computed.0 - 480, computed.1),
        ("a/three", computed.0 - 480, computed.1 - 144),
    ];
    let mut rows = snapshot_rows(graph);
    for (id, x, y) in hand {
        rows.as_array_mut()
            .unwrap()
            .push(json!({"kind": "node", "id": id, "x": x, "y": y}));
    }
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));

    let ids: Vec<&str> = hand.iter().map(|(i, _, _)| *i).collect();
    let pinned = node_positions(&html, &ids);
    assert_eq!(
        pinned,
        hand.iter().map(|(_, x, y)| (*x, *y)).collect::<Vec<_>>(),
        "a hand-placed cell is the one that must not move"
    );

    // The rectangle those three give their hive: `box_of`'s padding, 24 at the
    // sides, 30 above, 24 below.
    let (x0, x1) = (
        hand.iter().map(|h| h.1).min().unwrap() - 24,
        hand.iter().map(|h| h.1).max().unwrap() + 150 + 24,
    );
    let (y0, y1) = (
        hand.iter().map(|h| h.2).min().unwrap() - 30,
        hand.iter().map(|h| h.2).max().unwrap() + 38 + 24,
    );
    let at = node_positions(&html, &["a/new"])[0];
    assert_ne!(
        at, computed,
        "the new cell is still sitting on the pinned one"
    );
    assert!(
        at.0 >= x0 && at.0 + 150 <= x1 && at.1 >= y0 && at.1 + 38 <= y1,
        "the new cell left its own hive to give way: {at:?} not inside \
         ({x0},{y0})-({x1},{y1})"
    );
}

/// **And a cell nobody placed is never left inside a foreign hive's frame.** The
/// shift above needs a hand-placed cell in the hive to measure; a hive that has
/// none keeps its computed block, and a neighbour dragged across it then draws its
/// rectangle around cells that do not belong to it. That is the same defect read
/// from the other side (GH #167) — the operator's first move is to drag the box
/// out, which is the work the layout exists to save — so a spot inside a foreign
/// frame counts as occupied for a cell with no stored position.
#[test]
fn a_new_cell_is_not_left_inside_a_foreign_hives_frame() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(
        &[
            ("/a/one", "llm"),
            ("/a/two", "store"),
            ("/b/x", "timer"),
            ("/b/y", "timer"),
        ],
        &[],
    );
    let flow = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph.clone())), json!({})),
    ));
    let a = node_positions(&flow, &["a/one", "a/two"]);

    // Hive `b` is dragged wide open around hive `a`'s untouched block: neither of
    // its two cells touches one of `a`'s, but its frame swallows both.
    let mut rows = snapshot_rows(graph);
    for (id, x, y) in [("b/x", a[0].0 - 500, a[0].1), ("b/y", a[1].0 + 500, a[1].1)] {
        rows.as_array_mut()
            .unwrap()
            .push(json!({"kind": "node", "id": id, "x": x, "y": y}));
    }
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(rows), json!({})),
    ));

    let b = hive_frame(&html, "b");
    for (id, at) in ["a/one", "a/two"]
        .iter()
        .zip(node_positions(&html, &["a/one", "a/two"]))
    {
        assert!(
            !inside(at, b),
            "{id} is drawn inside hive `b`'s frame: {at:?} in {b:?}"
        );
    }
}

/// **A cell that hangs on nothing says so.** `remove_nodes` drops every edge and
/// keeps the node — no-delete — so rewiring a lane leaves the cell it replaced
/// standing in the picture, drawn exactly like a live one. Three of those
/// appeared after one tool move, and nothing in the markup told them apart from
/// the cells doing the work.
///
/// A heuristic, deliberately: the graph endpoint reports no activity, and a cell
/// instantiated one second ago and not yet wired looks the same. So it dims
/// rather than hides — both states are worth seeing.
#[test]
fn a_cell_with_no_edges_is_drawn_as_unwired() {
    let Some(root) = shipped_canvy() else { return };
    let graph = graph_doc(
        &[
            ("/a/one", "llm"),
            ("/a/two", "store"),
            ("/a/lonely", "bash"),
        ],
        &[("e1", "/a/one", "/a/two")],
    );
    let html = html_of(&run_shipped(
        &root,
        "render",
        stdin_doc_pass2(store_reply(snapshot_rows(graph)), json!({})),
    ));

    for id in ["a/one", "a/two"] {
        let tag = html
            .split(&format!("data-node=\"{id}\""))
            .next()
            .and_then(|h| h.rsplit("<g class=\"").next())
            .unwrap_or_default()
            .to_string();
        assert!(
            !tag.contains("unwired"),
            "{id} takes part in an edge and must not be marked unwired"
        );
    }
    let lonely = html
        .split("data-node=\"a/lonely\"")
        .next()
        .and_then(|h| h.rsplit("<g class=\"").next())
        .unwrap_or_default();
    assert!(
        lonely.contains("unwired"),
        "the cell with no edge at all has to carry the marker, got class {lonely:?}"
    );
    assert!(
        std::fs::read_to_string(root.join("render/client/surface.css"))
            .unwrap()
            .contains(".node.unwired"),
        "and the stylesheet has to have something to say about it"
    );
}
