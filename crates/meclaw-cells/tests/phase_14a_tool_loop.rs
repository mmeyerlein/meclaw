//! Phase-14-A Tool-Loop-Mechanik — store-freie Single-Iteration-Topologie
//! `llm → dispatcher → tool → collector → CaptureCell`, end-to-end gegen das
//! eingefrorene Substrat. llm deterministisch via MockOpenAI; dispatcher/tool/
//! collector sind code-Cells (python3 inline). Positiver Beweis: /sink-Receipt.
//!
//! Diagnose-Slice (PROGRESS § Geplante Phasen, overview Z.1998 + Z.1122):
//! KEIN store, KEIN Thread-Rebuild, KEIN zweiter llm-Call — das ist 14-B.

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "topology_svg.rs"]
mod topology_svg;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use mock_openai::{MockOpenAI, canned_tool_calls};

/// Dispatcher: zerlegt den llm-Output in typisierte Tool-Call-Messages.
const DISPATCHER_PY: &str = r#"
import sys, json
data = json.load(sys.stdin)
turns = data.get("messages", [])
calls = [t for t in turns if t.get("type") == "tool_call"]
out = []
for c in calls:
    out.append({
        "header": {"msg_type": "tool_call", "tool_call_id": c.get("id", "")},
        "messages": [c],
    })
sys.stdout.write(json.dumps(out))
"#;

/// Tool-A: deterministisches Tool-Endpoint-Surrogat (atomisch-emittierend).
/// Liest den Tool-Call-Turn, emittiert genau einen `tool_result`-Turn mit
/// `header.msg_type="tool_result"`. Determiniert ("42"), keine externe I/O.
const TOOL_A_PY: &str = r#"
import sys, json
data = json.load(sys.stdin)
turns = data.get("messages", [])
call = next((t for t in turns if t.get("type") == "tool_call"), None)
cid = call.get("id", "") if call else ""
out = {
    "header": {"msg_type": "tool_result"},
    "messages": [{"origin": "tool", "type": "tool_result", "id": cid, "text": "42"}],
}
sys.stdout.write(json.dumps(out))
"#;

/// Eine code-Cell-config.json mit inline-python und optionalem multi_send.
fn code_config(script: &str, multi_send: bool) -> String {
    to_string_pretty(&json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": script,
            "external_timeout_ms": 10000
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}, "multi_send_capable": multi_send}
    }))
    .unwrap()
}

/// llm-config.json gegen den Mock.
fn llm_config(base_url: &str) -> String {
    to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": {"provider": "openai", "model": "gpt-4o", "api_key": "test-key", "base_url": base_url},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .unwrap()
}

/// Standard-Probe: ein user-Turn + tool-Schema unter `system.tools.*`, sodass
/// der Mock einen tool_calls-Response liefert (Muster phase_8_demo T28).
///
/// Sets intentionally **no `reply_to`**: the receipt travels over the FS
/// topology edge `dispatcher → /sink`, not via `reply_to`.
fn tool_request_probe() -> Message {
    let tool_schema = json!({
        "type": "function",
        "function": {"name": "calc", "description": "calc", "parameters": {"type": "object", "properties": {}}}
    });
    let tool_schema_str = meclaw_core::serde_json::to_string(&tool_schema).unwrap();
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({
            "system": {"tools": {"calc": {"text": tool_schema_str}}},
            "messages": [{"origin": "user", "type": "text", "text": "rechne 2+3"}]
        })))
        .build()
}

/// Bounded /sink-Receipt (30s, robust gegen cargo-parallel-Last).
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Boot-Boilerplate: Factories (llm+code), `/sink`- und `/graphsink`-CaptureCells
/// vor Bootstrap, bootstrap_from_filesystem. Gibt `(ColonyHandle, sink_rx,
/// graph_rx)` zurück. `/graphsink` ist das `reply_to`-Ziel der
/// `/colony/graph`-Reads (Workstream B, Live-Graph-Quelle für die Topologie-SVGs).
async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let code_f: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![
            ("llm".to_string(), llm_f.clone()),
            ("code".to_string(), code_f.clone()),
        ],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let (graph_tx, graph_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/graphsink"), move || {
        CaptureCell::new(graph_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    registry.insert("code".to_string(), code_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx, graph_rx)
}

/// Poll-then-drain the DLQ until an entry with `resolved_target == target`
/// appears (bounded). Dead-letters land in Colony's ring buffer asynchronously
/// after the cascade drains, so a single drain can race the routing loop — this
/// mirrors the `phase_13_5_a6_demo` poll-then-drain pattern. Returns ALL drained
/// entries (the snapshot at the moment the wanted target first showed up, or at
/// the deadline), so the caller can also assert on co-resident DLQ entries
/// (trennschärfe).
async fn drain_until_target(h: &ColonyHandle, target: &str) -> Vec<meclaw_colony::DeadLetter> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = h.drain_dead_letters().await;
        let hit = snapshot
            .iter()
            .any(|d| d.resolved_target.as_str() == target);
        if hit || std::time::Instant::now() >= deadline {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Find the single DLQ entry whose `resolved_target` matches `target`.
fn dlq_entry_for<'a>(
    snapshot: &'a [meclaw_colony::DeadLetter],
    target: &str,
) -> &'a meclaw_colony::DeadLetter {
    let matches: Vec<_> = snapshot
        .iter()
        .filter(|d| d.resolved_target.as_str() == target)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one DLQ entry for resolved_target={target}; snapshot={snapshot:?}"
    );
    matches[0]
}

/// Kopiert den eingecheckten `examples/14a-tool-loop/main/`-Baum nach `<td>/main`
/// und überschreibt den llm-Placeholder-`base_url` auf den Mock. Quelle ist der
/// COMMITTED Baum (Single Source of Truth der Demo-Topologie) — Laufzeit-State
/// landet im `TempDir`, der eingecheckte Baum bleibt sauber. `base_url` ist der
/// echte Override-Punkt (am Code via `LlmParams` geerdet).
fn copy_example_tree(td: &std::path::Path, base_url: &str) {
    let src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/14a-tool-loop/main");
    copy_dir_recursive(&src, &td.join("main"));
    std::fs::write(td.join("main/llm/config.json"), llm_config(base_url)).unwrap();
}

/// Rekursiver Verzeichnis-Kopierer (Test-Code).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatcher_decomposes_llm_tool_call() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = tempfile::TempDir::new().unwrap();
    let disp = td.path().join("main/tool-loop/dispatcher");
    std::fs::create_dir_all(&disp).unwrap();
    std::fs::create_dir_all(td.path().join("main/llm")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./llm","to":"./tool-loop/dispatcher","condition":"hop.finish_reason == 'tool_calls'"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(td.path().join("main/tool-loop")).unwrap();
    std::fs::write(
        td.path().join("main/tool-loop/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./dispatcher","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/llm/config.json"),
        llm_config(&base_url),
    )
    .unwrap();
    std::fs::write(disp.join("config.json"), code_config(DISPATCHER_PY, true)).unwrap();

    let (h, mut sink_rx, mut graph_rx) = boot(&td).await;
    h.send(tool_request_probe()).await;

    let em = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive dispatcher output");
    assert_eq!(em.target, Path::new("/sink"));
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    };
    assert_eq!(
        em.headers.hop["msg_type"], "tool_call",
        "dispatcher must tag msg_type=tool_call"
    );
    assert_eq!(body["messages"][0]["type"], "tool_call");
    assert_eq!(body["messages"][0]["id"], "call-xyz");
    topology_svg::write_topology_diagram(&h, &mut graph_rx, "dispatcher_decomposes_llm_tool_call")
        .await;
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_transit_routes_llm_output_to_dispatcher() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = tempfile::TempDir::new().unwrap();
    let disp = td.path().join("main/tool-loop/dispatcher");
    std::fs::create_dir_all(&disp).unwrap();
    std::fs::create_dir_all(td.path().join("main/llm")).unwrap();
    // Root-Hive /: Edge /llm → /tool-loop (HIVE-PFAD als Target → Transit).
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./llm","to":"./tool-loop","condition":"hop.finish_reason == 'tool_calls'"}
        ]}}}"#,
    )
    .unwrap();
    // Hive /tool-loop. Hive-Out-Edge from="." routet den Transit weiter zum Dispatcher.
    std::fs::write(
        td.path().join("main/tool-loop/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./dispatcher","condition":"hop.finish_reason == 'tool_calls'"},
            {"from":"./dispatcher","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/llm/config.json"),
        llm_config(&base_url),
    )
    .unwrap();
    std::fs::write(disp.join("config.json"), code_config(DISPATCHER_PY, true)).unwrap();

    let (h, mut sink_rx, mut graph_rx) = boot(&td).await;
    h.send(tool_request_probe()).await;

    let em = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive after hive-transit → dispatcher");
    assert_eq!(em.headers.hop["msg_type"], "tool_call");
    topology_svg::write_topology_diagram(
        &h,
        &mut graph_rx,
        "hive_transit_routes_llm_output_to_dispatcher",
    )
    .await;
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cel_edge_routes_tool_call_to_tool() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = tempfile::TempDir::new().unwrap();
    let tl = td.path().join("main/tool-loop");
    std::fs::create_dir_all(tl.join("dispatcher")).unwrap();
    std::fs::create_dir_all(tl.join("tool-a")).unwrap();
    std::fs::create_dir_all(td.path().join("main/llm")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./llm","to":"./tool-loop","condition":"hop.finish_reason == 'tool_calls'"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        tl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./dispatcher","condition":"hop.finish_reason == 'tool_calls'"},
            {"from":"./dispatcher","to":"./tool-a","condition":"hop.msg_type == 'tool_call'"},
            {"from":"./tool-a","to":"/sink"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/llm/config.json"),
        llm_config(&base_url),
    )
    .unwrap();
    std::fs::write(
        tl.join("dispatcher/config.json"),
        code_config(DISPATCHER_PY, true),
    )
    .unwrap();
    std::fs::write(tl.join("tool-a/config.json"), code_config(TOOL_A_PY, false)).unwrap();

    let (h, mut sink_rx, mut graph_rx) = boot(&td).await;
    h.send(tool_request_probe()).await;

    let em = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive tool_result");
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    };
    assert_eq!(em.headers.hop["msg_type"], "tool_result");
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(body["messages"][0]["text"], "42");
    assert_eq!(
        body["messages"][0]["id"], "call-xyz",
        "tool_call_id must survive into tool_result"
    );
    topology_svg::write_topology_diagram(&h, &mut graph_rx, "cel_edge_routes_tool_call_to_tool")
        .await;
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_loop_end_to_end_reaches_collector() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = tempfile::TempDir::new().unwrap();
    copy_example_tree(td.path(), &base_url);

    let (h, mut sink_rx, mut graph_rx) = boot(&td).await;
    h.send(tool_request_probe()).await;

    let em = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive collector output end-to-end");
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    };
    assert_eq!(
        em.headers.hop["msg_type"], "collected",
        "final hop must be the collector"
    );
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(body["messages"][0]["text"], "42");

    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        1,
        "14-A is single-iteration — exactly one llm call"
    );
    let g = topology_svg::write_topology_diagram(
        &h,
        &mut graph_rx,
        "tool_loop_end_to_end_reaches_collector",
    )
    .await;
    topology_svg::emit_14a_example_diagram_if_requested(&g);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_no_route_distinct_from_unresolved_path() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_example_tree(td.path(), &base_url);
    let (h, _sink_rx, mut graph_rx) = boot(&td).await;

    // (1) Erreichbare Hive /tool-loop, aber Header matcht KEINE Out-Edge
    //     (finish_reason=stop ≠ 'tool_calls', msg_type fehlt) → hive_no_route.
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("finish_reason".into(), json!("stop"));
    let probe = MessageBuilder::new(Path::new("/tool-loop"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"x"}]}),
        ))
        .hop(headers)
        .ttl(8)
        .build();
    h.send(probe).await;
    let snap = drain_until_target(&h, "/tool-loop").await;
    assert_eq!(
        dlq_entry_for(&snap, "/tool-loop").reason.as_code(),
        "hive_no_route"
    );

    // (2) Nicht-existenter Pfad → unresolved_path.
    let bogus = MessageBuilder::new(Path::new("/tool-loop/nope"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"x"}]}),
        ))
        .ttl(8)
        .build();
    h.send(bogus).await;
    let snap2 = drain_until_target(&h, "/tool-loop/nope").await;
    assert_eq!(
        dlq_entry_for(&snap2, "/tool-loop/nope").reason.as_code(),
        "unresolved_path"
    );

    topology_svg::write_topology_diagram(
        &h,
        &mut graph_rx,
        "hive_no_route_distinct_from_unresolved_path",
    )
    .await;
    h.shutdown().await;
}

/// Task-6: prove the full `parent_message_id` trace chain lands in the central
/// message-log (`colony.db`) — exactly the data `/ui/trace` + `/colony/trace`
/// read. This is trace **data completeness** at the substrate level, NOT the
/// HTTP/UI layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_loop_trace_is_complete() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_example_tree(td.path(), &base_url);
    let (h, mut sink_rx, mut graph_rx) = boot(&td).await;
    h.send(tool_request_probe()).await;

    let em = recv_bounded(&mut sink_rx).await.expect("/sink receipt");
    let trace_id = em.trace_id;

    // Barrier: the full trace chain has 6 message-log rows (empirically pinned
    // from the first red run: 5 cell-output hops + the inbound /llm probe).
    let db_path = h.tempdir_path().join("colony.db");
    meclaw_testing::wait::wait_for_message_log_count(
        &db_path,
        &trace_id.to_string(),
        6,
        Duration::from_secs(5),
    )
    .await;

    // Chain-assert: every key target of the tool-loop must appear in the central
    // message-log under this trace_id. Column `to_path` per colony-schema
    // (crates/meclaw-colony/src/persist/schema.rs:66).
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT to_path FROM message_log WHERE trace_id = ?1")
        .unwrap();
    let targets: Vec<String> = stmt
        .query_map([trace_id.to_string()], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for expect in [
        "/llm",
        "/tool-loop/dispatcher",
        "/tool-loop/tool-a",
        "/tool-loop/collector",
        "/sink",
    ] {
        assert!(
            targets.iter().any(|t| t == expect),
            "trace must include {expect}; got {targets:?}"
        );
    }

    topology_svg::write_topology_diagram(&h, &mut graph_rx, "tool_loop_trace_is_complete").await;
    h.shutdown().await;
}

/// Workstream B (14-A-Nachtrag): die Quelle des Topologie-Bilds ist der LIVE
/// gebootete Graph. Liest `/colony/graph` für `/` + `/tool-loop` (gemerged) und
/// beweist, dass das Substrat die Transit- UND CEL-Edges der Demo-Topologie
/// wirklich geladen hat. Read unvollständig → BEFUND.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_graph_read_returns_tool_loop_edges() {
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    copy_example_tree(td.path(), &base_url);
    let (h, _sink_rx, mut graph_rx) = boot(&td).await;

    let g = topology_svg::read_live_graph(&h, &mut graph_rx, &["/", "/tool-loop"]).await;

    let has_edge = |from: &str, to: &str| g.edges.iter().any(|e| e.from == from && e.to == to);
    // Hive-Transit-Edge (Root-Hive) + CEL-Edge (innerhalb /tool-loop).
    assert!(
        has_edge("/llm", "/tool-loop"),
        "live graph must carry the hive-transit edge /llm → /tool-loop; got {:?}",
        g.edges.iter().map(|e| (&e.from, &e.to)).collect::<Vec<_>>()
    );
    assert!(
        has_edge("/tool-loop", "/tool-loop/dispatcher"),
        "live graph must carry the hive out-edge . → ./dispatcher"
    );
    assert!(
        has_edge("/tool-loop/dispatcher", "/tool-loop/tool-a"),
        "live graph must carry the CEL edge dispatcher → tool-a"
    );
    assert!(
        has_edge("/tool-loop/collector", "/sink"),
        "live graph must carry the terminal edge collector → /sink"
    );
    // CEL-condition-Source kommt mit (Edge-Label-Quelle).
    let transit = g
        .edges
        .iter()
        .find(|e| e.from == "/llm" && e.to == "/tool-loop")
        .unwrap();
    assert_eq!(
        transit.condition.as_deref(),
        Some("hop.finish_reason == 'tool_calls'"),
        "edge condition source must survive into the graph read"
    );
    // /tool-loop ist als Hive klassifiziert (Edge-Endpunkt ohne Registry-Node).
    assert!(
        g.hives.iter().any(|p| p == "/tool-loop"),
        "/tool-loop must classify as hive (no registry node); hives={:?}",
        g.hives
    );

    h.shutdown().await;
}

/// Schreibt einen FEHLKONFIGURIERTEN Tool-Loop-Tree: identisch zur Demo, aber die
/// Dispatcher-Out-Edge trägt eine CEL-`condition`, die NIE matcht (`msg_type ==
/// 'WRONG'` statt `'tool_call'`). Damit matcht die Dispatcher-Emission KEINE
/// Out-Edge → das Substrat fällt auf die implizite Identity-Decision (`em.target`)
/// zurück (colony.rs `if matched.is_empty()`). `em.target` der code-Cell =
/// inbound `reply_to` (cell.rs `reply_target`), und das ist via Transit-Erhalt
/// `/llm` → Loop-Back-Inferenz.
fn write_misconfigured_no_match_tree(td: &std::path::Path, base_url: &str) {
    let tl = td.join("main/tool-loop");
    std::fs::create_dir_all(tl.join("dispatcher")).unwrap();
    std::fs::create_dir_all(tl.join("tool-a")).unwrap();
    std::fs::create_dir_all(td.join("main/llm")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./llm","to":"./tool-loop","condition":"hop.finish_reason == 'tool_calls'"}
        ]}}}"#,
    )
    .unwrap();
    // MISCONFIG: dispatcher → tool-a Condition matcht NIE (Builder-Tippfehler).
    std::fs::write(
        tl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./dispatcher","condition":"hop.finish_reason == 'tool_calls'"},
            {"from":"./dispatcher","to":"./tool-a","condition":"hop.msg_type == 'WRONG'"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(td.join("main/llm/config.json"), llm_config(base_url)).unwrap();
    std::fs::write(
        tl.join("dispatcher/config.json"),
        code_config(DISPATCHER_PY, true),
    )
    .unwrap();
    std::fs::write(tl.join("tool-a/config.json"), code_config(TOOL_A_PY, false)).unwrap();
}

/// Workstream C (14-A-Nachtrag): EMPIRISCHE Beobachtung des Loop-Back. Pinnt den
/// EXAKTEN Mechanismus, ersetzt das „wahrscheinlich" der 14-A-Diagnose-Notiz.
/// Mehrere canned `tool_calls`-Responses (der Loop-Back-Call darf nicht am Mock
/// verhungern), kleine TTL (Deckel). Beobachtet: `mock.recorded_requests()` —
/// Anzahl + Inhalt jedes `/llm`-Requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_back_mechanism_trace() {
    // 8 canned tool_calls: jeder llm-Call liefert wieder finish_reason=tool_calls
    // → Root-Edge matcht erneut → der Loop ist selbsttragend bis zur TTL.
    let canned: Vec<_> = (0..8)
        .map(|i| {
            let id = format!("call-{i}");
            canned_tool_calls(vec![(
                Box::leak(id.into_boxed_str()),
                "calc",
                r#"{"x":2,"y":3}"#,
            )])
        })
        .collect();
    let mock = MockOpenAI::start(canned).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = tempfile::TempDir::new().unwrap();
    write_misconfigured_no_match_tree(td.path(), &base_url);
    let (h, _sink_rx, _graph_rx) = boot(&td).await;

    // Probe mit kleiner TTL als Deckel.
    let tool_schema = json!({
        "type": "function",
        "function": {"name": "calc", "description": "calc", "parameters": {"type": "object", "properties": {}}}
    });
    let tool_schema_str = meclaw_core::serde_json::to_string(&tool_schema).unwrap();
    let probe = MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({
            "system": {"tools": {"calc": {"text": tool_schema_str}}},
            "messages": [{"origin": "user", "type": "text", "text": "rechne 2+3"}]
        })))
        .ttl(12)
        .build();
    h.send(probe).await;

    // W2 (Ruling A1): the misconfigured `dispatcher → tool-a` edge (condition
    // never matches) no longer identity-loops the dispatcher emission back into
    // the tree. The dispatcher's unmatched emission `no_route`-dead-letters and
    // the chain TERMINATES — there is no loop-back (that identity-driven loop is
    // exactly the class A1 eliminates). Bounded wait for the terminating
    // no_route DLQ.
    let mut no_route_seen = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        let snap = h.drain_dead_letters().await;
        if snap.iter().any(|d| d.reason.as_code() == "no_route") {
            no_route_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let snaps = mock.recorded_requests().await;
    eprintln!("NO-LOOP-TRACE: llm calls = {}", snaps.len());
    eprintln!("NO-LOOP-TRACE: no_route DLQ seen = {no_route_seen}");

    // (1) NO loop-back under A1: the chain terminates at the misconfigured
    //     dispatcher edge, so there is exactly ONE llm inference (the green
    //     path call), not a self-sustaining loop.
    assert_eq!(
        snaps.len(),
        1,
        "no identity loop-back under A1: exactly one llm call expected; got {}",
        snaps.len()
    );
    // (2) The dispatcher emission matched no out-edge → it terminates as a
    //     no_route DLQ (the loop class is dead-lettered, not looped).
    assert!(
        no_route_seen,
        "the misconfigured dispatcher emission must terminate as a no_route DLQ"
    );

    h.shutdown().await;
}
