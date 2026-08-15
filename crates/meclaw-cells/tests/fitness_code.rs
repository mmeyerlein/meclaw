//! Track T (#104) — fitness battery for the `code` cell's script interface.
//!
//! The failure modes have their own suite (`code_failure_modes.rs`); this
//! battery pins the POSITIVE contract a deterministic tool-loop brain stands
//! on (`docs/cell-types.md` § code):
//!
//! - stdin is a three-object document — `envelope` (both header compartments
//!   and the substrate's own fields), `body` (the message slots) and a
//!   read-only, secret-filtered `params` copy — with exactly those three keys
//!   at the top level, all of them always present (`params` is `{}` when the
//!   cell was built without one);
//! - stdout becomes the emission: `header` → hop, the rest → body;
//! - the process metadata headers (`exit_code`/`duration_ms`/`had_stderr`)
//!   are cell-owned and override script attempts to fake them;
//! - multi-send: a JSON array fans out into N ordered emissions, an object
//!   under `multi_send_capable` behaves as an array of one.

use meclaw_cells::code::{CodeCell, CodeParams};
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

fn cell_with(script: &str, multi_send: bool) -> CodeCell {
    let params = CodeParams::parse(&json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 10000
    }))
    .expect("params parse");
    CodeCell::new(params, multi_send, None, false)
}

async fn run(script: &str, multi_send: bool, body: Value, context: Value) -> Vec<CellEmission> {
    let (otx, mut orx) = mpsc::channel(16);
    let sink = OutputSink::new(
        otx,
        Path::new("/code"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let mut ctx_map = meclaw_core::serde_json::Map::new();
    if let Some(o) = context.as_object() {
        for (k, v) in o {
            ctx_map.insert(k.clone(), v.clone());
        }
    }
    let msg = MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(body))
        .context(ctx_map)
        .reply_to(Path::new("/sink"))
        .build();
    cell_with(script, multi_send).handle(msg, &sink).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(em) = orx.recv().await {
        out.push(em);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stdin_carries_body_headers_envelope_and_a_filtered_params_copy() {
    // The script echoes its ENTIRE stdin document back as a body slot, so the
    // assertion reads exactly what the cell put on the wire.
    let script = r#"
import sys, json
d = json.load(sys.stdin)
sys.stdout.write(json.dumps({"messages": [], "echo": d, "keys": sorted(d.keys())}))
"#;
    let ems = run(
        script,
        false,
        json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}],
               "custom_slot": {"a": 1}}),
        json!({"iter": "3", "session_id": "s1"}),
    )
    .await;
    assert_eq!(ems.len(), 1);
    let echo = &ems[0].content["echo"];

    // #139: the top level is closed by construction — three objects, no more.
    assert_eq!(
        ems[0].content["keys"],
        json!(["body", "envelope", "params"]),
        "the stdin document has exactly three top-level keys: {echo}"
    );

    // Body slots travel, and they travel INSIDE `body` — a script reads them
    // there instead of subtracting a hard-coded envelope key list.
    assert_eq!(echo["body"]["messages"][0]["text"], "hi");
    assert_eq!(echo["body"]["custom_slot"]["a"], 1);
    assert_eq!(
        echo["body"]
            .as_object()
            .map(meclaw_core::serde_json::Map::len),
        Some(2),
        "no envelope field leaks into the body: {echo}"
    );
    // Both header compartments travel, inside the envelope.
    assert_eq!(echo["envelope"]["header"]["context"]["iter"], "3");
    assert_eq!(echo["envelope"]["header"]["context"]["session_id"], "s1");
    assert!(echo["envelope"]["header"]["hop"].is_object());
    // The rest of the envelope travels with it.
    assert_eq!(echo["envelope"]["target"], "/code");
    assert_eq!(echo["envelope"]["reply_to"], "/sink");
    assert!(echo["envelope"]["trace_id"].is_string());
    assert_eq!(echo["envelope"]["ttl"], 64);
    // W12 route A (sanctioned 2026-08-15): the params travel as a read-only
    // copy. `cell_with` builds the cell WITHOUT `with_stdin_params`, which is
    // the pre-W12 shape — the field is present and empty, never missing.
    assert_eq!(
        echo["params"],
        json!({}),
        "the params field is always there, `{{}}` when nothing was attached: {echo}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stdout_header_becomes_hop_and_the_rest_becomes_body() {
    let script = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"route": "fire", "my_metric": 7},
    "messages": [{"origin": "assistant", "type": "text", "text": "built"}],
    "artifacts": ["a.py"]}))
"#;
    let ems = run(script, false, json!({"messages": []}), json!({})).await;
    assert_eq!(ems.len(), 1);
    let c = &ems[0].content;
    assert_eq!(c["header"]["route"], "fire");
    assert_eq!(c["header"]["my_metric"], 7);
    assert_eq!(c["messages"][0]["text"], "built");
    assert_eq!(
        c["artifacts"][0], "a.py",
        "own top-level slots pass through"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn process_metadata_headers_are_cell_owned_and_unforgeable() {
    // The script LIES about its own exit metadata; the cell overrides.
    let script = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"exit_code": 99, "had_stderr": True, "duration_ms": 424242},
    "messages": []}))
"#;
    let ems = run(script, false, json!({"messages": []}), json!({})).await;
    let h = &ems[0].content["header"];
    assert_eq!(h["exit_code"], 0, "the real exit code wins");
    assert_eq!(h["had_stderr"], false, "the real stderr flag wins");
    assert_ne!(h["duration_ms"], 424242, "the real duration wins");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_send_array_fans_out_in_order() {
    let script = r#"
import sys, json
json.load(sys.stdin)
out = [{"header": {"route": "calls"}, "messages": [{"origin": "assistant", "type": "text", "text": "first"}]},
       {"header": {"route": "tool", "tool_name": "bash"}, "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{}"}]},
       {"header": {"route": "tool", "tool_name": "file"}, "messages": [{"origin": "assistant", "type": "tool_call", "id": "c2", "text": "{}"}]}]
sys.stdout.write(json.dumps(out))
"#;
    let ems = run(script, true, json!({"messages": []}), json!({})).await;
    assert_eq!(ems.len(), 3, "one emission per array element");
    assert_eq!(ems[0].content["header"]["route"], "calls");
    assert_eq!(ems[1].content["header"]["tool_name"], "bash");
    assert_eq!(ems[2].content["header"]["tool_name"], "file");
    assert_eq!(
        ems[0].content["messages"][0]["text"], "first",
        "array order is emission order"
    );
    // Every element carries the cell-owned metadata headers.
    for em in &ems {
        assert_eq!(em.content["header"]["exit_code"], 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_object_under_multi_send_is_an_array_of_one() {
    let script = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps({"header": {"route": "solo"}, "messages": []}))
"#;
    let ems = run(script, true, json!({"messages": []}), json!({})).await;
    assert_eq!(ems.len(), 1);
    assert_eq!(ems[0].content["header"]["route"], "solo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_array_is_the_terminal_park_zero_emissions() {
    // The tool-loop idiom: a collector that loses the guard race emits an
    // empty multi-send and stops. Zero emissions, no error.
    let script = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;
    let ems = run(script, true, json!({"messages": []}), json!({})).await;
    assert!(ems.is_empty(), "empty multi-send is terminal by design");
}
