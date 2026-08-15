//! Phase-14-B Task 1 — Terminierungs-Guard (store-frei). Positiv-Test: korrekt
//! verdrahtete Loop-Kette terminiert sauber via finish_reason-Edge (exakt 2 Calls,
//! /sink final). The loop-back mechanism is canonically pinned in the 14-A addendum
//! (loop_back_mechanism_trace, Identity-Fallback em.target=reply_to=/llm, TTL-Deckel;
//! observability gap, roadmap U5) — NOT re-derived or reproduced here.
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

/// Dispatcher: reads the llm output turns; one message with `route=tool` to the
/// tool cell per `tool_call` turn. (Store-free: no c_asst/store path — that is
/// tasks 4+. This topology isolates ONLY the termination.)
const DISPATCHER_PY: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
turns = d.get("messages", [])
calls = [t for t in turns if t.get("type") == "tool_call"]
out = []
for c in calls:
    out.append({"header":{"route":"tool"}, "messages":[c]})
sys.stdout.write(json.dumps(out))
"#;

/// Tool A: a deterministic `tool_result` (text "42"), route=c_res set fresh.
const TOOL_A_PY: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
turns = d.get("messages", [])
call = next((t for t in turns if t.get("type") == "tool_call"), None)
cid = call.get("id","") if call else ""
out = {"header":{"route":"c_res"},
       "messages":[{"origin":"tool","type":"tool_result","id":cid,"text":"42"}]}
sys.stdout.write(json.dumps(out))
"#;

/// Collector (store-free): forwards the incoming tool_result turn on to /llm with
/// route=back. No store, no thread rebuild — just closing the loop.
const COLLECTOR_PY: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
out = {"header":{"route":"back"}, "messages": msgs}
sys.stdout.write(json.dumps(out))
"#;

/// One code-cell config.json with inline python and an optional multi_send.
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

/// llm config.json pointed at the mock.
fn llm_config(base_url: &str) -> String {
    to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": {"provider": "openai", "model": "gpt-4o", "api_key": "test-key", "base_url": base_url},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .unwrap()
}

/// Writes the correctly wired store-free loop tree:
/// `/llm → /tool-loop (Transit) → dispatcher → tool-a → collector → /llm` (Loop),
/// plus `/llm --(finish_reason != 'tool_calls')--> /sink` (Terminierung).
fn write_terminating_loop_tree(td: &std::path::Path, base_url: &str) {
    let tl = td.join("main/tool-loop");
    std::fs::create_dir_all(td.join("main/llm")).unwrap();
    std::fs::create_dir_all(tl.join("dispatcher")).unwrap();
    std::fs::create_dir_all(tl.join("tool-a")).unwrap();
    std::fs::create_dir_all(tl.join("collector")).unwrap();

    // Root hive /: /llm → /tool-loop (transit) on tool_calls; /llm → /sink on stop.
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

/// Boot without store: llm+code factories, the /sink CaptureCell before bootstrap. (Pattern: support::boot.)
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

/// User probe straight to /llm (no capture in this store-free topology), TTL explicit.
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

/// Bounded receipt (30 s, robust against cargo's parallel load).
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
        .expect("/sink MUST receive the final message");
    assert_eq!(
        fin.headers.hop["finish_reason"], "stop",
        "termination via the finish_reason edge"
    );
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 2, "exactly 2 calls — no unintended re-trigger");
    h.shutdown().await;
}
