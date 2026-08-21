//! GH #308 — the `builder-librarian` survives a hyphenated name, and marks the
//! briefing when it does not.
//!
//! Three things are pinned here, and all three were red when this file was
//! written:
//!
//! 1. **The query the `retrieve` cell builds is one FTS5 accepts.** Its
//!    tokeniser keeps the hyphen inside a token (`[A-Za-z_][A-Za-z0-9_-]{2,}`)
//!    and joined the terms unquoted, so `daily-digest OR cell` reached SQLite as
//!    a MATCH expression and came back `no such column: digest`. Every
//!    multi-word template name in the corpus this librarian indexes is
//!    hyphenated — `daily-digest`, `builder-hive`, `memory-hive`,
//!    `coder-pipeline` — so that was not an edge case, it was the normal path.
//!    The assertion runs the shipped query through the **real** store op against
//!    the **real** seed corpus: same DDL, same stemming tokeniser, same
//!    `op_search`.
//! 2. **A store error renders degraded AND marked.** The store answers a bad
//!    MATCH as a regular `tool_result` carrying BOTH `operation: search` and
//!    `error_code: sql_error` (`store/output.rs` writes `operation`
//!    unconditionally). The cell recognised phase B on `operation` first, so the
//!    error text was parsed as a result set, failed, and rendered
//!    `(no matching patterns)` with no marker — indistinguishable from an honest
//!    zero-hit, which is exactly what the template README forbids.
//! 3. **The other error shape does not dead-letter.** `emit_invalid_input` and
//!    `emit_query_timeout` write no `operation` at all, so the return edge
//!    `has(hop.operation) && hop.operation == 'search'` skipped them and the
//!    reply died as `no_route`. The edge is conditioned on context instead —
//!    the `canvy` / `coder-pipeline` pattern — and this evaluates the shipped
//!    condition with the real CEL engine to prove it.
//!
//! **R2b guard.** Every read is guarded by [`shipped_librarian`]: where the
//! template does not ship, these tests skip rather than fail on a dead
//! reference.

use meclaw_core::serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every file the hive is made of. The list is the guard AND the inventory.
const LIBRARIAN_FILES: &[&str] = &[
    "config.json",
    "template.json",
    "README.md",
    "retrieve/config.json",
    "store/config.json",
    "store/seed/docs.jsonl",
];

fn shipped_librarian() -> Option<PathBuf> {
    let root = templates_root().join("builder-librarian");
    for rel in LIBRARIAN_FILES {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn read_json(p: &Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The `retrieve` cell's shipped script with `${…}` resolved the way bootstrap
/// resolves it: `substitute_env_only` over an empty environment, so every
/// placeholder falls to its documented default. Running the raw bytes would not
/// even parse as Python.
fn retrieve_script(root: &Path) -> String {
    let cfg = read_json(&root.join("retrieve/config.json"));
    let env: HashMap<String, String> = HashMap::new();
    let params = meclaw_colony::mutation::substitute::substitute_env_only(&cfg["params"], &env)
        .expect("retrieve params must substitute");
    params["script_inline"]
        .as_str()
        .expect("script_inline must be a string")
        .to_string()
}

/// Run the shipped script exactly as the `code` cell runs it: `params.runner`
/// with the script via `-c`, the stdin document in the three-key wire shape.
fn run_retrieve(root: &Path, stdin_doc: Value) -> Value {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let cfg = read_json(&root.join("retrieve/config.json"));
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = retrieve_script(root);

    let mut child = Command::new(runner)
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the shipped runner");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_doc.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "retrieve exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "retrieve stdout is not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn stdin_doc(body: Value, hop: Value, context: Value) -> Value {
    json!({
        "envelope": {
            "header": { "context": context, "hop": hop },
            "target": "/librarian/retrieve",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64
        },
        "body": body,
        "params": {}
    })
}

fn user_request(text: &str) -> Value {
    json!({ "messages": [{ "origin": "user", "type": "text", "id": "", "text": text }] })
}

/// The corpus as the `store` cell would hold it: the shipped `params.schema`
/// and `params.fts`, through the shipped DDL and seed loader, on a connection
/// carrying the store's own extensions and stemming tokeniser. Anything less
/// faithful would prove a query against a table nobody ships.
fn corpus_conn(root: &Path) -> rusqlite::Connection {
    let cfg = read_json(&root.join("store/config.json"));
    let schema: BTreeMap<String, BTreeMap<String, String>> =
        meclaw_core::serde_json::from_value(cfg["params"]["schema"].clone()).unwrap();
    let fts: BTreeMap<String, Vec<String>> =
        meclaw_core::serde_json::from_value(cfg["params"]["fts"].clone()).unwrap();

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    meclaw_cells::store::query::install_connection_extensions(&conn).unwrap();
    meclaw_cells::store::ddl::apply_schema_ddl(&conn, &schema).unwrap();
    meclaw_cells::store::ddl::apply_fts_ddl(&conn, &fts, &BTreeMap::new()).unwrap();
    meclaw_cells::store::seed::load_seed_if_present(&conn, &root.join("store"), &schema)
        .expect("the shipped seed corpus must load");
    conn
}

/// The op the `retrieve` cell asks the store to run, out of its phase-A output.
fn emitted_op(out: &Value) -> Value {
    let text = out["messages"][0]["text"]
        .as_str()
        .expect("phase A emits a tool_call turn carrying the op as text");
    meclaw_core::serde_json::from_str(text).expect("the op travels as JSON in `text`")
}

/// 1 — the hyphen. A request naming a shipped template must reach the corpus.
#[test]
fn a_hyphenated_template_name_reaches_the_corpus() {
    let Some(root) = shipped_librarian() else {
        return;
    };
    let out = run_retrieve(
        &root,
        stdin_doc(
            user_request("build me a daily-digest that posts to telegram"),
            json!({}),
            json!({}),
        ),
    );
    assert_eq!(out["header"]["route"], "lsearch");
    let op = emitted_op(&out);

    let conn = corpus_conn(&root);
    let outcome =
        meclaw_cells::store::ops::dispatch(&conn, &op).expect("search args must be well formed");
    assert_eq!(
        outcome.error_code, None,
        "the shipped query must be an FTS5 expression the store accepts, got {:?}: {:?}\nmatch was: {}",
        outcome.error_code, outcome.error_text, op["match"]
    );
    let rows = outcome.payload.as_array().expect("search returns rows");
    assert!(
        !rows.is_empty(),
        "asking for `daily-digest` must return at least one chunk; match was: {}",
        op["match"]
    );
    assert!(
        rows.iter().any(|r| {
            r["text"]
                .as_str()
                .unwrap_or_default()
                .contains("daily-digest")
                || r["source"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("daily-digest")
                || r["section"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("daily-digest")
        }),
        "the named template must be among the hits, got: {:?}",
        rows.iter()
            .map(|r| (r["source"].clone(), r["section"].clone()))
            .collect::<Vec<_>>()
    );
}

/// 2 — a store error is DEGRADED AND MARKED, never an honest-looking zero-hit.
///
/// The reply carries both headers, because that is what `store/output.rs`
/// writes: `operation` unconditionally, `error_code` on top.
#[test]
fn a_store_error_renders_a_marked_briefing() {
    let Some(root) = shipped_librarian() else {
        return;
    };
    let out = run_retrieve(
        &root,
        stdin_doc(
            json!({ "messages": [{
                "origin": "tool", "type": "tool_result", "id": "lib1",
                "text": "no such column: digest" }] }),
            json!({ "operation": "search", "error_code": "sql_error", "rows_affected": 0 }),
            json!({ "orig_request": "build me a daily-digest" }),
        ),
    );
    assert_eq!(
        out["header"]["route"], "brief",
        "a failure still comes back on `brief`"
    );
    assert_eq!(
        out["header"]["degraded"], true,
        "a store error must be MARKED degraded, not rendered as a zero-hit: {out}"
    );
    let rendered = out["messages"][1]["text"].as_str().unwrap_or_default();
    assert!(
        rendered.contains("sql_error"),
        "the brief must name the reason, got {rendered:?}"
    );
    assert_ne!(
        rendered, "(no matching patterns)",
        "that text is the honest zero-hit and must never stand in for a failure"
    );
    assert_eq!(
        out["messages"][0]["text"], "build me a daily-digest",
        "the original question rides along even in the degraded briefing"
    );
}

/// 2b — the honest zero-hit is still honest. The marker must not fire on a
/// clean search that simply found nothing.
#[test]
fn an_empty_result_set_is_not_marked_degraded() {
    let Some(root) = shipped_librarian() else {
        return;
    };
    let out = run_retrieve(
        &root,
        stdin_doc(
            json!({ "messages": [{
                "origin": "tool", "type": "tool_result", "id": "lib1", "text": "[]" }] }),
            json!({ "operation": "search", "rows_affected": 0 }),
            json!({ "orig_request": "something nobody wrote about" }),
        ),
    );
    assert_eq!(out["header"]["route"], "brief");
    assert_eq!(out["header"]["hits"], 0);
    assert!(
        out["header"].get("degraded").is_none() || out["header"]["degraded"] == false,
        "a clean search that found nothing is not a degradation: {out}"
    );
    assert_eq!(out["messages"][1]["text"], "(no matching patterns)");
}

/// 3 — the `invalid_input` shape routes home instead of dead-lettering.
///
/// `emit_invalid_input` / `emit_query_timeout` write no `operation`, so the old
/// `has(hop.operation) && hop.operation == 'search'` return edge skipped and the
/// reply died `no_route`. The shipped condition is evaluated here with the real
/// CEL engine, against the context the hive's own outbound edge stamps.
#[test]
fn the_return_edge_carries_every_reply_the_store_can_send() {
    let Some(root) = shipped_librarian() else {
        return;
    };
    let hive = read_json(&root.join("config.json"));
    let edges = hive["params"]["graph"]["edges"].as_array().unwrap();

    let out_edge = edges
        .iter()
        .find(|e| e["from"] == "./retrieve" && e["to"] == "./store")
        .expect("the query edge must exist");
    let back_edge = edges
        .iter()
        .find(|e| e["from"] == "./store" && e["to"] == "./retrieve")
        .expect("the return edge must exist");

    // The context the outbound edge stamps is what the return edge reads. The
    // `hop.*` promotions need a live hop; the literal markers — the ones the
    // `canvy` pattern is built on — are exactly the ones a return edge can
    // condition on, and they are reproduced here verbatim from the shipped
    // modifier.
    let set_context = out_edge["modifier"]["set_context"]
        .as_object()
        .expect("the outbound edge must promote into context");
    let mut context = meclaw_core::serde_json::Map::new();
    let mut literals = 0usize;
    for (k, expr) in set_context {
        let expr = expr.as_str().expect("a set_context value is a CEL string");
        match expr.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            Some(lit) => {
                literals += 1;
                context.insert(k.clone(), Value::String(lit.to_string()));
            }
            // `hop.orig` — the question, promoted so phase B can hand it on.
            None => {
                context.insert(k.clone(), Value::String("build me a daily-digest".into()));
            }
        }
    }
    assert!(
        literals > 0,
        "the outbound edge must stamp a literal marker into context for the return \
         edge to condition on (the canvy/coder-pipeline pattern), got {set_context:?}"
    );

    let cond = meclaw_colony::cel_eval::parse_condition(back_edge["condition"].as_str().unwrap())
        .expect("the return condition must compile");

    // Every reply shape the store can produce, per `store/cell.rs` and
    // `store/output.rs`.
    let replies: [(&str, Value); 4] = [
        (
            "search ok",
            json!({ "operation": "search", "rows_affected": 3, "duration_ms": 1 }),
        ),
        (
            "sql_error",
            json!({ "operation": "search", "rows_affected": 0, "error_code": "sql_error" }),
        ),
        (
            "invalid_input",
            json!({ "finish_reason": "error", "error_code": "invalid_input", "duration_ms": 1 }),
        ),
        (
            "query_timeout",
            json!({ "finish_reason": "error", "error_code": "query_timeout", "duration_ms": 1 }),
        ),
    ];
    for (name, hop) in replies {
        let hop = hop.as_object().unwrap().clone();
        let verdict = meclaw_colony::cel_eval::evaluate_condition(&cond, &context, &hop);
        assert!(
            matches!(verdict, Ok(true)),
            "the `{name}` reply must route back to ./retrieve, not dead-letter as no_route \
             (condition {:?} said {:?})",
            back_edge["condition"],
            verdict.map_err(|e| e.to_string())
        );
    }
}
