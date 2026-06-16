//! Phase-14-B Task 1 — Terminierungs-Guard (store-frei). Positiv-Test: korrekt
//! verdrahtete Loop-Kette terminiert sauber via finish_reason-Edge (exakt 2 Calls,
//! /sink-Final). Der Loop-Back-Mechanismus ist kanonisch gepinnt im 14-A-Nachtrag
//! (loop_back_mechanism_trace, Identity-Fallback em.target=reply_to=/llm, TTL-Deckel;
//! Observability-Gap roadmap U5) — HIER nicht neu hergeleitet, nicht reproduziert.
#[path = "mock_openai.rs"]
mod mock_openai;

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

use mock_openai::{MockOpenAI, canned_chat_completion, canned_tool_calls};

/// Dispatcher: liest die llm-Output-Turns; je `tool_call`-Turn eine Message
/// mit `route=tool` an die Tool-Cell. (Store-frei: kein c_asst/store-Pfad —
/// der ist Tasks 4+. Diese Topologie isoliert NUR die Terminierung.)
const DISPATCHER_PY: &str = r#"
import sys, json
d = json.load(sys.stdin)
turns = d.get("messages", [])
calls = [t for t in turns if t.get("type") == "tool_call"]
out = []
for c in calls:
    out.append({"header":{"route":"tool"}, "messages":[c]})
sys.stdout.write(json.dumps(out))
"#;

/// Tool-A: deterministisches `tool_result` (text "42"), route=c_res frisch gesetzt.
const TOOL_A_PY: &str = r#"
import sys, json
d = json.load(sys.stdin)
turns = d.get("messages", [])
call = next((t for t in turns if t.get("type") == "tool_call"), None)
cid = call.get("id","") if call else ""
out = {"header":{"route":"c_res"},
       "messages":[{"origin":"tool","type":"tool_result","id":cid,"text":"42"}]}
sys.stdout.write(json.dumps(out))
"#;

/// Collector (store-frei): reicht den eingehenden tool_result-Turn weiter an /llm,
/// route=back. Kein store, kein Thread-Rebuild — nur Loop-Schluss.
const COLLECTOR_PY: &str = r#"
import sys, json
d = json.load(sys.stdin)
msgs = d.get("messages", [])
out = {"header":{"route":"back"}, "messages": msgs}
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

/// Schreibt den korrekt verdrahteten store-freien Loop-Baum:
/// `/llm → /tool-loop (Transit) → dispatcher → tool-a → collector → /llm` (Loop),
/// plus `/llm --(finish_reason != 'tool_calls')--> /sink` (Terminierung).
fn write_terminating_loop_tree(td: &std::path::Path, base_url: &str) {
    let tl = td.join("main/tool-loop");
    std::fs::create_dir_all(td.join("main/llm")).unwrap();
    std::fs::create_dir_all(tl.join("dispatcher")).unwrap();
    std::fs::create_dir_all(tl.join("tool-a")).unwrap();
    std::fs::create_dir_all(tl.join("collector")).unwrap();

    // Root-Hive /: /llm → /tool-loop (Transit) bei tool_calls; /llm → /sink bei stop.
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./llm","to":"./tool-loop","condition":"hop.finish_reason == 'tool_calls'"},
            {"from":"./llm","to":"/sink","condition":"hop.finish_reason != 'tool_calls'"}
        ]}}}"#,
    )
    .unwrap();

    // Hive /tool-loop: Transit → dispatcher → tool-a → collector → /llm.
    std::fs::write(
        tl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./dispatcher","condition":"hop.finish_reason == 'tool_calls'"},
            {"from":"./dispatcher","to":"./tool-a","condition":"hop.route == 'tool'"},
            {"from":"./tool-a","to":"./collector","condition":"hop.route == 'c_res'"},
            {"from":"./collector","to":"/llm","condition":"hop.route == 'back'"}
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
    std::fs::write(
        tl.join("collector/config.json"),
        code_config(COLLECTOR_PY, false),
    )
    .unwrap();
}

/// Boot ohne store: llm+code-Factories, /sink-CaptureCell vor Bootstrap. (Muster: support::boot.)
async fn boot_loopback(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let code_f: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![
            ("llm".into(), llm_f.clone()),
            ("code".into(), code_f.clone()),
        ],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut reg = CellFactoryRegistry::new();
    reg.insert("llm".into(), llm_f);
    reg.insert("code".into(), code_f);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .unwrap();
    (h, sink_rx)
}

/// user-Probe direkt an /llm (kein Capture in dieser store-freien Topologie), TTL explizit.
fn user_probe_llm(turn_id: &str, ttl: u32) -> Message {
    let tool_schema = meclaw_core::serde_json::to_string(&json!({
        "type":"function","function":{"name":"calc","description":"calc",
        "parameters":{"type":"object","properties":{}}}}))
    .unwrap();
    let mut headers = meclaw_core::serde_json::Map::new();
    headers.insert("turn_id".into(), json!(turn_id));
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({
            "system":{"tools":{"calc":{"text":tool_schema}}},
            "messages":[{"origin":"user","type":"text","text":"rechne 2+3"}]})))
        .context(headers)
        .ttl(ttl)
        .build()
}

/// Bounded Receipt (30 s, robust gegen cargo-parallel-Last).
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminating_edge_stops_loop_exactly_at_finish_reason() {
    // Call 1 → tool_calls (Loop), Call 2 → stop (Terminierung).
    let mock = MockOpenAI::start(vec![
        canned_tool_calls(vec![("c1", "calc", r#"{}"#)]),
        canned_chat_completion("fertig", "stop"),
    ])
    .await;
    let base_url = format!("{}/v1", mock.base_url);
    let td = tempfile::TempDir::new().unwrap();
    write_terminating_loop_tree(td.path(), &base_url);
    let (h, mut sink_rx) = boot_loopback(&td).await;
    h.send(user_probe_llm("t1", 16)).await;
    let fin = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink MUSS die Final-Message empfangen");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "Terminierung über finish_reason-Edge"
    );
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        2,
        "exakt 2 Calls — kein unbeabsichtigter Re-Trigger"
    );
    h.shutdown().await;
}
