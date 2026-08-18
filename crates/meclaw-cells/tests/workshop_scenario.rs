//! Track T (#104) — the WORKSHOP: every tool cell end to end in ONE colony.
//!
//! A deterministic `code`-cell brain (scripted tool_call bundles, no LLM)
//! drives a real coding task through the shipped `dispatcher@1` and
//! `collector` templates (both copied read-only, config.json for config.json):
//!
//!   round 0  file       writes a small Python project (module + test)
//!   round 1  bash       runs the test — RED (exit 1), sandboxed under Landlock
//!                       when the kernel offers it
//!   round 2  edit       patches the bug out (find_replace, 1 match)
//!   round 3  bash       runs the test again — GREEN (exit 0)
//!   round 4  web_fetch  pulls a fixture doc from a local mock server
//!            web_search queries a local mock endpoint (2 results) — parallel
//!   round 5  store      records the artifact in an artifacts table
//!   round 6  timer      schedules a follow-up (+3 s) as a tool call; the ack
//!                       closes the round, the FIRE lands independently
//!   round 7  mcp        stdio roundtrip against the line-JSON fixture child
//!   round 8  harness    delegates a task to the stub adapter; `accepted`
//!                       closes the round, `result` arrives on the origin lane
//!   round 9  stop       the brain reports what it OBSERVED
//!
//! The claims are asserted from the COLONY MESSAGE LOG (`colony.db`), not from
//! tool outputs alone: PLAIN order per round (the expectation set reaches the
//! collector before any tool message), the iteration counter 0..9 on the
//! brain seam, and the per-tool result headers (exit_code 1→0, http_status,
//! result_count, matches_changed, rows_affected, timer fire, mcp_tool,
//! harness result). The re-entry edge carries `restore_ttl` (GH #82), so ten
//! rounds run on the substrate default budget of 64.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::harness::HarnessCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, McpCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const MCP_FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");
const HARNESS_FIXTURE: &str = env!("CARGO_BIN_EXE_stream_json_harness_fixture");

// ------------------------------------------------------------------ the brain

/// The deterministic workshop driver. One fixed tool bundle per iteration —
/// measurable, replayable, LLM-free. `${WORKSHOP_WS}` and `${FETCH_URL}` are
/// resolved by the COLONY's `${VAR}` substitution at instantiation, exactly as
/// a template consumer would configure it.
const BRAIN: &str = r#"
import sys, json, datetime
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
ctx = (envelope.get("header") or {}).get("context") or {}
it = int(ctx.get("iter", 0) or 0)
ws = "${WORKSHOP_WS}"
fetch_url = "${FETCH_URL}"

def call(cid, tool, args):
    return {"origin": "assistant", "type": "tool_call", "id": cid,
            "text": json.dumps({"name": tool, "arguments": json.dumps(args)})}

CALC_BUGGY = "def add(a, b):\n    return a - b\n"
TEST = ("import calc, sys\n"
        "ok = calc.add(2, 3) == 5\n"
        "print('CALC_OK' if ok else 'CALC_FAILED')\n"
        "sys.exit(0 if ok else 1)\n")

if it == 0:
    msgs = [call("c0-1", "file", {"op": "write", "path": "calc.py", "content": CALC_BUGGY}),
            call("c0-2", "file", {"op": "write", "path": "test_calc.py", "content": TEST})]
elif it == 1:
    # -B: no bytecode cache. An agent edit can swap one char (same file size)
    # within the same second; CPython's pyc header stores whole-second mtime,
    # so a cached calc.pyc would survive the patch and round 3 would run the
    # STALE module. A real-world sharp edge, documented in the track receipt.
    msgs = [call("c1-1", "bash", {"command": "cd %s && python3 -B test_calc.py" % ws})]
elif it == 2:
    msgs = [call("c2-1", "edit", {"op": "find_replace", "path": "calc.py",
                                  "find": "return a - b", "replace": "return a + b"})]
elif it == 3:
    msgs = [call("c3-1", "bash", {"command": "cd %s && python3 -B test_calc.py" % ws})]
elif it == 4:
    msgs = [call("c4-1", "web_fetch", {"url": fetch_url}),
            call("c4-2", "web_search", {"query": "workshop patterns"})]
elif it == 5:
    msgs = [call("c5-1", "store", {"operation": "insert", "table": "artifacts",
                                   "row": {"step": "suite-green", "detail": "calc.py patched"}})]
elif it == 6:
    # +3 s, not +1 s: `at` is written with SECOND precision, so `strftime`
    # throws the sub-second part away and the real lead time is
    # `delta - frac(now)`. At +1 s that lead is uniform in (0, 1] s, and the
    # op still has to travel brain -> dispatcher -> timer and be INSERTed
    # before the cell takes its active snapshot. `load_active_filter_past`
    # drops a one-shot whose `at` is already `<= now` at snapshot time, so a
    # lead that ran out in flight means the schedule silently never fires and
    # the sink waits forever. +3 s leaves a lead of (2, 3] s -- two orders of
    # magnitude over the observed hop cost, also under a loaded cargo run.
    at = (datetime.datetime.now(datetime.timezone.utc)
          + datetime.timedelta(seconds=3)).strftime("%Y-%m-%dT%H:%M:%SZ")
    msgs = [call("c6-1", "timer", {"op": "add",
                                   "schedule_id": "0198aaaa-aaaa-7aaa-aaaa-aaaaaaaaaaaa",
                                   "schedule_name": "workshop-followup", "at": at,
                                   "emit_to": "/sink",
                                   "emit_body": {"messages": [{"origin": "user", "type": "text",
                                                               "text": "follow-up"}]},
                                   "emit_headers": {"msg_type": "workshop_tick"}})]
elif it == 7:
    msgs = [call("c7-1", "mcp", {"name": "echo", "arguments": {"text": "workshop-mcp"}})]
elif it == 8:
    msgs = [call("c8-1", "coder", {"name": "start_task",
                                   "arguments": {"task_id": "t-ws-1",
                                                 "prompt": "polish the module",
                                                 "workspace": "wt-1"}})]
else:
    # Only tool RESULTS count: the tool_call turns carry the test source in
    # their write args, which contains both marker strings.
    results = [m for m in d.get("messages", []) if m.get("type") == "tool_result"]
    joined = "\n".join(str(m.get("text", "")) for m in results)
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text",
                         "text": "workshop-complete|results_seen=%d|red_seen=%s|green_seen=%s" % (
                             len(results),
                             "yes" if "CALC_FAILED" in joined else "no",
                             "yes" if "CALC_OK" in joined else "no")}]}
    sys.stdout.write(json.dumps(out))
    sys.exit(0)
sys.stdout.write(json.dumps({"header": {"finish_reason": "tool_calls"}, "messages": msgs}))
"#;

/// Turns the inbound message into the collector's `turn` lane (port pattern
/// from `collector_colony.rs`).
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn"}, "messages": d.get("messages", [])}))
"#;

// ------------------------------------------------------------- tree building

/// The shipped templates, copied config.json for config.json — the cells
/// under test ARE the templates (pattern: `dispatcher_template.rs`).
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn template_dir(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(name)
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// Retunes collector knobs in an already-copied instance. Since
/// `collector@1.2.0` they are params of `./assemble`, not colony-global `.env`
/// keys, so a tree sets them in the copy it owns.
fn tune_collector(root: &std::path::Path, rel: &str, knobs: &[(&str, &str)]) {
    let p = root.join(rel);
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let params = v["params"].as_object_mut().expect("params object");
    for (k, val) in knobs {
        assert!(params.contains_key(*k), "no such collector param: {k}");
        params.insert((*k).to_string(), json!(val));
    }
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// A `code` cell config with the contract the substrate validates against
/// (pattern: `collector_colony.rs`).
fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({});
    if !routes.is_empty() {
        hop["route"] = json!({"type": "string", "values": routes, "required": false});
    }
    if let Some(obj) = extra_hop.as_object() {
        for (k, v) in obj {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Workshop scenario stand-in cell.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A minimal tool-cell config: real type, real params, generic contract.
fn tool_cell(cell_type: &str, params: Value, long_running: bool) -> Value {
    let mut cell = json!({"type": cell_type});
    if long_running {
        cell["timeout"] = json!(-1);
    }
    json!({
        "cell": cell,
        "params": params,
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": true}}},
            "consumes": {"body": {"messages": {"type": "array", "required": true}}}
        },
        "description": {
            "purpose": "Workshop scenario tool endpoint.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// All port wiring of the workshop. Lane names travel on `hop.route`; the
/// tool lanes discriminate on `hop.tool_name` exactly as the dispatcher
/// README prescribes. The re-entry seam carries the iteration promotion AND
/// `restore_ttl` (GH #82).
fn main_config() -> Value {
    let in_tool = json!({"set_hop": {"route": "'in_tool'"}});
    let edges = vec![
        // Ingress: probe → collector turn lane.
        json!({"from": "./probe", "to": "./collect/assemble",
               "condition": "has(hop.route) && hop.route == 'turn'",
               "modifier": {"set_hop": {"route": "'in_turn'"}}}),
        // THE seam: collector → brain, iteration promoted, budget restored.
        json!({"from": "./collect/assemble", "to": "./brain",
               "condition": "has(hop.route) && hop.route == 'brain'",
               "modifier": {"set_context": {"turn_id": "hop.turn_id",
                                            "session_id": "hop.session_id",
                                            "iter": "hop.iter"},
                            "restore_ttl": true}}),
        // Brain answers: bundles to the dispatcher, the final turn back into
        // the window via in_answer, the collector's answer route to the sink.
        json!({"from": "./brain", "to": "./dispatcher",
               "condition": "has(hop.finish_reason) && hop.finish_reason == 'tool_calls'"}),
        json!({"from": "./brain", "to": "./collect/assemble",
               "condition": "has(hop.finish_reason) && hop.finish_reason == 'stop'",
               "modifier": {"set_hop": {"route": "'in_answer'"}}}),
        json!({"from": "./collect/assemble", "to": "/sink",
               "condition": "has(hop.route) && hop.route == 'answer'"}),
        // Dispatcher lanes: expectation set and synthetic errors to the
        // fan-in, tools by NAME (the cell knows no topology).
        json!({"from": "./dispatcher", "to": "./collect/assemble",
               "condition": "has(hop.route) && hop.route == 'calls'",
               "modifier": {"set_hop": {"route": "'in_calls'"}}}),
        json!({"from": "./dispatcher", "to": "./collect/assemble",
               "condition": "has(hop.route) && hop.route == 'result'",
               "modifier": &in_tool}),
        json!({"from": "./dispatcher", "to": "./shell",
               "condition": "has(hop.tool_name) && hop.tool_name == 'bash'"}),
        json!({"from": "./dispatcher", "to": "./fs",
               "condition": "has(hop.tool_name) && hop.tool_name == 'file'"}),
        json!({"from": "./dispatcher", "to": "./patch",
               "condition": "has(hop.tool_name) && hop.tool_name == 'edit'"}),
        json!({"from": "./dispatcher", "to": "./reader",
               "condition": "has(hop.tool_name) && hop.tool_name == 'web_fetch'"}),
        json!({"from": "./dispatcher", "to": "./search",
               "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'"}),
        json!({"from": "./dispatcher", "to": "./artifacts",
               "condition": "has(hop.tool_name) && hop.tool_name == 'store'"}),
        json!({"from": "./dispatcher", "to": "./remind",
               "condition": "has(hop.tool_name) && hop.tool_name == 'timer'"}),
        json!({"from": "./dispatcher", "to": "./bridge",
               "condition": "has(hop.tool_name) && hop.tool_name == 'mcp'"}),
        json!({"from": "./dispatcher", "to": "./coder",
               "condition": "has(hop.tool_name) && hop.tool_name == 'coder'"}),
        // Fan-in: every tool result back into the collector.
        json!({"from": "./shell", "to": "./collect/assemble",
               "condition": "has(hop.operation) && hop.operation == 'bash'",
               "modifier": &in_tool}),
        json!({"from": "./fs", "to": "./collect/assemble",
               "condition": "has(hop.operation)", "modifier": &in_tool}),
        json!({"from": "./patch", "to": "./collect/assemble",
               "condition": "has(hop.operation)", "modifier": &in_tool}),
        json!({"from": "./reader", "to": "./collect/assemble",
               "condition": "has(hop.operation) && hop.operation == 'web_fetch'",
               "modifier": &in_tool}),
        json!({"from": "./search", "to": "./collect/assemble",
               "condition": "has(hop.operation) && hop.operation == 'web_search'",
               "modifier": &in_tool}),
        json!({"from": "./artifacts", "to": "./collect/assemble",
               "condition": "has(hop.operation)", "modifier": &in_tool}),
        // Timer: the op ack closes the round; the FIRE goes to the sink; the
        // error lane is drained (template README discipline).
        json!({"from": "./remind", "to": "./collect/assemble",
               "condition": "has(hop.msg_type) && hop.msg_type == 'timer_op_ack'",
               "modifier": &in_tool}),
        json!({"from": "./remind", "to": "/sink",
               "condition": "has(hop.msg_type) && hop.msg_type == 'workshop_tick'"}),
        json!({"from": "./remind", "to": "/park",
               "condition": "has(hop.msg_type) && hop.msg_type == 'timer_op_error'"}),
        // MCP: every emission carries mcp_tool.
        json!({"from": "./bridge", "to": "./collect/assemble",
               "condition": "has(hop.mcp_tool)", "modifier": &in_tool}),
        // Harness: accepted closes the round, result reaches the sink, the
        // rest (progress/…) is drained to /park.
        json!({"from": "./coder", "to": "./collect/assemble",
               "condition": "has(hop.harness_event) && hop.harness_event == 'accepted'",
               "modifier": &in_tool}),
        json!({"from": "./coder", "to": "/sink",
               "condition": "has(hop.harness_event) && hop.harness_event == 'result'"}),
        json!({"from": "./coder", "to": "/park",
               "condition": "has(hop.harness_event) && hop.harness_event != 'accepted' \
                             && hop.harness_event != 'result'"}),
    ];
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

struct Workshop {
    h: ColonyHandle,
    sink_rx: mpsc::Receiver<Message>,
    _park_rx: mpsc::Receiver<Message>,
    td: tempfile::TempDir,
    _fetch_srv: tokio::task::JoinHandle<()>,
    _search_srv: tokio::task::JoinHandle<()>,
}

async fn boot_workshop() -> Workshop {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();

    // The coding workspace and the harness worktree.
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let worktrees = root.join("worktrees");
    std::fs::create_dir_all(worktrees.join("wt-1")).unwrap();

    // Local fixture servers: one document, one search endpoint (2 hits).
    let (fetch_addr, fetch_srv) =
        start_mock_server(MockResponse::ok(b"WORKSHOP-FIXTURE-DOC unit testing howto")).await;
    let search_body = json!({"results": [
        {"title": "Workshop patterns", "url": "http://a", "snippet": "s1"},
        {"title": "More patterns", "url": "http://b", "snippet": "s2"}
    ]})
    .to_string();
    let (search_addr, search_srv) =
        start_mock_server(MockResponse::ok_json(search_body.as_bytes())).await;

    // `${VAR}` config for the brain and the tool cells. The collector's own
    // knobs are NOT here any more -- they are params of the instance and are
    // set below, on the copy this tree owns.
    std::fs::write(
        root.join(".env"),
        format!(
            "WORKSHOP_WS={}\nFETCH_URL=http://{}/doc\nMCP_CMD={}\nHARNESS_CMD={}\n",
            ws.display(),
            fetch_addr,
            MCP_FIXTURE,
            HARNESS_FIXTURE
        ),
    )
    .unwrap();

    // The tree: templates copied read-only, tool cells with real params.
    write(root, "main/config.json", &main_config());
    copy_cells(&template_dir("dispatcher"), &root.join("main/dispatcher"));
    copy_cells(&template_dir("collector"), &root.join("main/collect"));
    // The workshop turn takes ten brain entries, so the seam cap gets headroom;
    // the round slate must carry every result of the turn for the final report.
    tune_collector(
        root,
        "main/collect/assemble/config.json",
        &[
            ("max_iter", "16"),
            ("round_bytes", "200000"),
            ("tool_chars", "8000"),
        ],
    );
    write(
        root,
        "main/probe/config.json",
        &code_cell(PROBE, &["turn"], json!({})),
    );
    let finish = json!({"finish_reason": {"type": "string",
                                          "values": ["stop", "tool_calls"], "required": true}});
    write(
        root,
        "main/brain/config.json",
        &code_cell(BRAIN, &[], finish),
    );
    write(
        root,
        "main/shell/config.json",
        &tool_cell(
            "bash",
            json!({"max_concurrency": 2, "external_timeout_ms": 30000}),
            false,
        ),
    );
    write(
        root,
        "main/fs/config.json",
        &tool_cell("file", json!({"base_path": ws.to_str().unwrap()}), false),
    );
    write(
        root,
        "main/patch/config.json",
        &tool_cell("edit", json!({"base_path": ws.to_str().unwrap()}), false),
    );
    write(
        root,
        "main/reader/config.json",
        &tool_cell(
            "web_fetch",
            // GH #117: round 4 pulls a fixture doc from a LOCAL mock server,
            // which the shipped default refuses. Documented opt-out.
            json!({"max_concurrency": 2, "external_timeout_ms": 10000, "max_bytes": 32768,
                   "allow_private_networks": true}),
            false,
        ),
    );
    write(
        root,
        "main/search/config.json",
        &tool_cell(
            "web_search",
            json!({"endpoint": format!("http://{search_addr}/search"),
                   "max_concurrency": 2, "external_timeout_ms": 10000}),
            false,
        ),
    );
    write(
        root,
        "main/artifacts/config.json",
        &tool_cell(
            "store",
            json!({"schema": {"artifacts": {"step": "text", "detail": "text"}}}),
            false,
        ),
    );
    write(
        root,
        "main/remind/config.json",
        &tool_cell("timer", json!({}), true),
    );
    write(
        root,
        "main/bridge/config.json",
        &tool_cell(
            "mcp",
            json!({"transport": "stdio", "command": "${MCP_CMD}", "args": ["mcp"],
                   "external_timeout_ms": 5000, "query_timeout_ms": 1000, "kill_grace_ms": 500}),
            true,
        ),
    );
    write(
        root,
        "main/coder/config.json",
        &tool_cell(
            "harness",
            json!({"adapter": "claude-code", "command": "${HARNESS_CMD}", "emit_to": "/sink",
                   "workspace_root": worktrees.to_str().unwrap(),
                   "startup_timeout_ms": 5000, "external_timeout_ms": 5000,
                   "query_timeout_ms": 1000, "kill_grace_ms": 500}),
            true,
        ),
    );

    // Boot: every tool factory registered; /sink + /park BEFORE bootstrap
    // (anti-cascade).
    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        ("code".into(), Arc::new(CodeCellFactory)),
        ("store".into(), Arc::new(StoreCellFactory)),
        ("bash".into(), Arc::new(BashCellFactory)),
        ("file".into(), Arc::new(FileCellFactory)),
        ("edit".into(), Arc::new(EditCellFactory)),
        ("web_fetch".into(), Arc::new(WebFetchCellFactory)),
        ("web_search".into(), Arc::new(WebSearchCellFactory)),
        ("timer".into(), Arc::new(TimerCellFactory)),
        ("mcp".into(), Arc::new(McpCellFactory)),
        ("harness".into(), Arc::new(HarnessCellFactory)),
    ];
    let h = ColonyHandle::new_with_factories_at(&td, factories.clone());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (k, f) in factories {
        registry.insert(k, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");

    Workshop {
        h,
        sink_rx,
        _park_rx: park_rx,
        td,
        _fetch_srv: fetch_srv,
        _search_srv: search_srv,
    }
}

// ----------------------------------------------------------------- log query

/// One message_log row, insertion-ordered.
struct LogRow {
    rowid: i64,
    parent: Option<String>,
    from_path: String,
    to_path: String,
    headers: Value,
}

fn read_log(db: &std::path::Path) -> Vec<LogRow> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT rowid, parent_message_id, from_path, to_path, headers \
             FROM message_log ORDER BY rowid",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(LogRow {
            rowid: r.get(0)?,
            parent: r.get(1)?,
            from_path: r.get(2)?,
            to_path: r.get(3)?,
            headers: meclaw_core::serde_json::from_str(&r.get::<_, String>(4)?)
                .unwrap_or(Value::Null),
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn hop<'a>(row: &'a LogRow, key: &str) -> &'a Value {
    &row.headers["hop"][key]
}

/// Poll-bounded: the log writer is asynchronous, so the assertions wait for
/// the row set to become complete (pattern: `support_14b::await_body_kind`).
async fn read_log_until<F: Fn(&[LogRow]) -> bool>(db: &std::path::Path, done: F) -> Vec<LogRow> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = read_log(db);
        if done(&rows) {
            return rows;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "message_log never reached the expected state ({} rows)",
            rows.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Bounded sink receive. On timeout the whole colony record (message log +
/// dead letters) is dumped, so a stalled round names its stall point.
async fn recv_sink(rx: &mut mpsc::Receiver<Message>, db: &std::path::Path) -> Message {
    match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
        Ok(Some(m)) => m,
        other => {
            eprintln!("=== message_log at stall ===");
            for r in read_log(db) {
                eprintln!(
                    "  #{} {} -> {}  hop={}",
                    r.rowid, r.from_path, r.to_path, r.headers["hop"]
                );
            }
            eprintln!("=== dead_letters at stall ===");
            if let Ok(conn) = rusqlite::Connection::open(db) {
                let mut stmt = conn
                    .prepare("SELECT sender_path, original_target, error_code FROM dead_letters")
                    .unwrap();
                let dl = stmt
                    .query_map([], |r| {
                        Ok(format!(
                            "  {} -> {}  [{}]",
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?
                        ))
                    })
                    .unwrap();
                for line in dl.flatten() {
                    eprintln!("{line}");
                }
            }
            panic!("no sink receipt within 60s: {other:?}");
        }
    }
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

// ------------------------------------------------------------------ the test

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_workshop_drives_every_tool_cell_through_one_coding_task() {
    let w = boot_workshop().await;
    let Workshop {
        h, mut sink_rx, td, ..
    } = w;

    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert("session_id".into(), json!("ws-1"));
    h.send(
        MessageBuilder::new(Path::new("/probe"))
            .body(Body::Inline(json!({"messages": [{
                "origin": "user", "type": "text", "text": "run the workshop"
            }]})))
            .context(ctx)
            .ttl(64)
            .build(),
    )
    .await;

    // Three independent arrivals prove the surface: the final answer, the
    // timer fire, the harness result. Order between them is scheduling.
    let db_for_dump = td.path().join("colony.db");
    let (mut answer, mut tick, mut result) = (None, None, None);
    while answer.is_none() || tick.is_none() || result.is_none() {
        let m = recv_sink(&mut sink_rx, &db_for_dump).await;
        if m.headers.hop.get("msg_type") == Some(&json!("workshop_tick")) {
            tick = Some(m);
        } else if m.headers.hop.get("harness_event") == Some(&json!("result")) {
            result = Some(m);
        } else if m.headers.hop.get("route") == Some(&json!("answer")) {
            answer = Some(m);
        } else {
            panic!("unexpected sink receipt: {:?}", m.headers.hop);
        }
    }

    // --- The brain's own report: red seen, green seen, every result carried.
    let answer = answer.unwrap();
    assert_eq!(
        text_of(&answer),
        "workshop-complete|results_seen=11|red_seen=yes|green_seen=yes",
        "the final context carried the WHOLE workshop"
    );

    // --- The timer fire is a real schedule firing, not an echo.
    let tick = tick.unwrap();
    assert_eq!(tick.headers.hop["schedule_name"], "workshop-followup");
    assert!(tick.headers.hop.get("fired_at").is_some());
    assert_eq!(text_of(&tick), "follow-up");

    // --- The harness result names its outcome and workspace.
    let result = result.unwrap();
    assert_eq!(result.headers.hop["status"], "ok");
    assert_eq!(result.headers.hop["task_id"], "t-ws-1");

    // --- Filesystem truth: the project exists and the bug is OUT.
    assert_eq!(
        std::fs::read_to_string(td.path().join("workspace/calc.py")).unwrap(),
        "def add(a, b):\n    return a + b\n",
        "file wrote it, edit patched it"
    );

    // ================= MESSAGE-LOG ASSERTIONS (the round-trip itself) ======
    let db = td.path().join("colony.db");
    let rows = read_log_until(&db, |rows| {
        rows.iter().filter(|r| r.to_path == "/brain").count() >= 10
            && rows
                .iter()
                .any(|r| r.to_path == "/sink" && hop(r, "msg_type") == "workshop_tick")
            && rows
                .iter()
                .any(|r| r.to_path == "/sink" && hop(r, "harness_event") == "result")
    })
    .await;

    // (1) The iteration counter on the seam: ten brain entries, 0..9, in order.
    let iters: Vec<String> = rows
        .iter()
        .filter(|r| r.to_path == "/brain")
        .map(|r| {
            r.headers["context"]["iter"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        iters,
        (0..10).map(|i| i.to_string()).collect::<Vec<_>>(),
        "ten seam crossings, the edge owns the counter"
    );

    // (2) PLAIN order per round: within every dispatcher fan-out (grouped by
    // the consumed parent message) the expectation set reaches the collector
    // BEFORE any tool message leaves.
    let split_rows: Vec<&LogRow> = rows
        .iter()
        .filter(|r| r.from_path == "/dispatcher")
        .collect();
    assert!(!split_rows.is_empty(), "the dispatcher routed the rounds");
    let mut bundles: std::collections::BTreeMap<String, Vec<&LogRow>> = Default::default();
    for r in &split_rows {
        bundles
            .entry(r.parent.clone().unwrap_or_default())
            .or_default()
            .push(r);
    }
    assert_eq!(bundles.len(), 9, "nine tool rounds crossed the dispatcher");
    for (parent, group) in &bundles {
        let calls_rowid = group
            .iter()
            .find(|r| r.to_path == "/collect/assemble" && hop(r, "route") == "in_calls")
            .unwrap_or_else(|| panic!("round {parent}: no expectation-set delivery"))
            .rowid;
        for r in group {
            if r.to_path != "/collect/assemble" {
                assert!(
                    calls_rowid < r.rowid,
                    "round {parent}: tool message (rowid {}) overtook the expectation set \
                     (rowid {calls_rowid})",
                    r.rowid
                );
            }
        }
    }

    // (3) bash: red then green, as data on the hop.
    let shell_results: Vec<&LogRow> = rows
        .iter()
        .filter(|r| r.from_path == "/shell" && r.to_path == "/collect/assemble")
        .collect();
    let shell_hops: Vec<Value> = shell_results
        .iter()
        .map(|r| r.headers["hop"].clone())
        .collect();
    assert_eq!(shell_results.len(), 2, "two test runs: {shell_hops:?}");
    assert_eq!(
        *hop(shell_results[0], "exit_code"),
        json!(1),
        "first RED: {shell_hops:?}"
    );
    assert_eq!(
        *hop(shell_results[1], "exit_code"),
        json!(0),
        "then GREEN: {shell_hops:?}"
    );

    // (4) file: both project writes.
    let fs_results: Vec<&LogRow> = rows
        .iter()
        .filter(|r| r.from_path == "/fs" && r.to_path == "/collect/assemble")
        .collect();
    assert_eq!(fs_results.len(), 2, "module + test written");
    for r in &fs_results {
        assert_eq!(*hop(r, "operation"), json!("write"));
        assert!(hop(r, "error_code").is_null());
    }

    // (5) edit: exactly one match changed.
    let patch = rows
        .iter()
        .find(|r| r.from_path == "/patch" && r.to_path == "/collect/assemble")
        .expect("the edit round");
    assert_eq!(*hop(patch, "operation"), json!("find_replace"));
    assert_eq!(*hop(patch, "matches_changed"), json!(1));

    // (6) web_fetch 200 / web_search result_count 2.
    let fetch = rows
        .iter()
        .find(|r| r.from_path == "/reader" && r.to_path == "/collect/assemble")
        .expect("the fetch round");
    assert_eq!(*hop(fetch, "http_status"), json!(200));
    let search = rows
        .iter()
        .find(|r| r.from_path == "/search" && r.to_path == "/collect/assemble")
        .expect("the search round");
    assert_eq!(*hop(search, "result_count"), json!(2));

    // (7) store: the artifact row landed.
    let artifact = rows
        .iter()
        .find(|r| r.from_path == "/artifacts" && r.to_path == "/collect/assemble")
        .expect("the store round");
    assert_eq!(*hop(artifact, "operation"), json!("insert"));
    assert_eq!(*hop(artifact, "rows_affected"), json!(1));

    // (8) timer: the ack closed the round, the fire reached the sink.
    assert!(
        rows.iter().any(|r| r.from_path == "/remind"
            && r.to_path == "/collect/assemble"
            && hop(r, "msg_type") == "timer_op_ack"),
        "the timer ack fanned back in"
    );
    // The fire is an ORIGIN emission (fresh trace) — the log records origin
    // sources as `@external`, so the match is target + header, not from_path.
    assert!(
        rows.iter()
            .any(|r| r.to_path == "/sink" && hop(r, "msg_type") == "workshop_tick"),
        "the schedule fired into the sink"
    );

    // (9) mcp: the stdio roundtrip answered on the tool lane.
    let mcp = rows
        .iter()
        .find(|r| r.from_path == "/bridge" && r.to_path == "/collect/assemble")
        .expect("the mcp round");
    assert_eq!(*hop(mcp, "mcp_tool"), json!("echo"));
    assert!(hop(mcp, "error_code").is_null());

    // (10) harness: accepted closed the round, the result went out fresh.
    assert!(
        rows.iter().any(|r| r.from_path == "/coder"
            && r.to_path == "/collect/assemble"
            && hop(r, "harness_event") == "accepted"),
        "the harness accepted receipt fanned back in"
    );
    // The result is an ORIGIN emission like the timer fire (`@external`).
    let hr = rows
        .iter()
        .find(|r| r.to_path == "/sink" && hop(r, "harness_event") == "result")
        .expect("the harness result");
    assert_eq!(*hop(hr, "status"), json!("ok"));

    // The store cell's own artifact table holds the record (durable truth).
    let art_db = td.path().join("main/artifacts/cell.db");
    let conn = rusqlite::Connection::open(&art_db).unwrap();
    let (step, detail): (String, String) = conn
        .query_row("SELECT step, detail FROM artifacts", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(step, "suite-green");
    assert_eq!(detail, "calc.py patched");

    h.shutdown().await;
}
