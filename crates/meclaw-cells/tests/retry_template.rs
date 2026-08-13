//! meclaw-os -- the retry template `retry@1` (night wave 3, track U, GH #3).
//!
//! A tool call can fail for reasons that go away on a second try. This template
//! is ONE `code` cell that routes a call towards its tool, watches the reply,
//! and re-emits the ORIGINAL call when the reply carries an error -- bounded by
//! the wiring, not by the cell. Three claims are pinned here:
//!
//! 1. THE CELL DOES NOT COUNT -- the attempt counter lives in `context.attempt`
//!    and is written exclusively by the retry EDGE
//!    (`set_context {"attempt": "int(context.attempt) + 1"}`); the bound is the
//!    edge condition (`int(context.attempt) < RETRY_MAX`). The colony half
//!    proves it: the flaky tool succeeds exactly when the counter has climbed
//!    high enough, and the give-up lane fires exactly at the cap.
//! 2. THE CALL SURVIVES IN CONTEXT -- a `code` cell has no state, so the call
//!    is parked on the call pass (`hop.call`, promoted by the wiring's
//!    `set_context {"call": "hop.call"}`) and rebuilt from `context.call` on an
//!    error reply. No cell.db, no topology knowledge.
//! 3. GIVING UP IS CLEAN -- at the cap the retry edge stops matching and the
//!    give-up edge hands the message to the error sink with the ORIGINAL
//!    `hop.error_code` and the attempt count on the hop. Nothing dead-letters.
//!
//! The script half runs the shipped `params.script_inline` against real stdin
//! documents; the colony half boots the shipped template file. Nothing is
//! mocked and no provider is called.

use std::io::Write;
use std::process::{Command, Stdio};

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::{boot, recv_bounded};
use tokio::sync::mpsc;

const RETRY_CONFIG: &str = "../../builder/templates/retry/config.json";

// ======================================================================= SCRIPT

fn retry_script() -> String {
    let raw = std::fs::read_to_string(RETRY_CONFIG).expect("retry config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    v["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Runs the real script against a real stdin document and returns the emitted
/// messages.
fn emit(doc: Value) -> Vec<Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(retry_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "retry script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn call_turn(id: &str, args: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id, "text": args})
}

fn result_turn(id: &str, text: &str) -> Value {
    json!({"origin": "tool", "type": "tool_result", "id": id, "text": text})
}

/// A stdin document as the wiring delivers it: both header compartments plus
/// the body turns.
fn doc(context: Value, hop: Value, messages: Vec<Value>) -> Value {
    json!({"header": {"context": context, "hop": hop}, "messages": messages})
}

fn hop_str(msg: &Value, key: &str) -> String {
    msg["header"][key].as_str().unwrap_or_default().to_string()
}

#[test]
fn a_call_leaves_on_the_call_lane_and_parks_itself_on_the_hop() {
    let call = call_turn("c1", "{\"q\":\"alpha\"}");
    let out = emit(doc(json!({}), json!({}), vec![call.clone()]));

    assert_eq!(out.len(), 1, "exactly one call emission: {out:?}");
    assert_eq!(hop_str(&out[0], "route"), "call");
    assert_eq!(hop_str(&out[0], "tool_call_id"), "c1");
    assert_eq!(out[0]["messages"][0], call, "the call travels verbatim");
    // The parked copy: hop.call is the turn as JSON, so the wiring's
    // `set_context {"call": "hop.call"}` can carry it across the tool hop.
    let parked: Value =
        meclaw_core::serde_json::from_str(&hop_str(&out[0], "call")).expect("hop.call is JSON");
    assert_eq!(parked, call, "hop.call round-trips to the exact turn");
}

#[test]
fn an_error_reply_rebuilds_the_parked_call_on_the_retry_lane() {
    let call = call_turn("c1", "{\"q\":\"alpha\"}");
    let out = emit(doc(
        json!({"call": call.to_string(), "attempt": 1}),
        json!({"error_code": "io_error"}),
        vec![result_turn("c1", "boom")],
    ));

    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(hop_str(&out[0], "route"), "retry");
    assert_eq!(
        hop_str(&out[0], "error_code"),
        "io_error",
        "the original error code travels with the retry"
    );
    assert_eq!(hop_str(&out[0], "tool_call_id"), "c1");
    assert_eq!(
        out[0]["messages"][0], call,
        "the ORIGINAL call is rebuilt from context.call, not the error echo"
    );
}

#[test]
fn an_error_reply_without_a_parked_call_goes_to_the_error_lane() {
    // A retry is impossible without the parked call (mis-wired call edge, or a
    // stray error routed in): hand the error over instead of looping garbage.
    let reply = result_turn("c1", "boom");
    let out = emit(doc(
        json!({}),
        json!({"error_code": "io_error"}),
        vec![reply.clone()],
    ));

    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(hop_str(&out[0], "route"), "error");
    assert_eq!(hop_str(&out[0], "error_code"), "io_error");
    assert_eq!(
        out[0]["messages"][0], reply,
        "the error reply passes through"
    );
}

#[test]
fn a_corrupt_parked_call_goes_to_the_error_lane_instead_of_looping() {
    let out = emit(doc(
        json!({"call": "not json at all"}),
        json!({"error_code": "io_error"}),
        vec![result_turn("c1", "boom")],
    ));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(
        hop_str(&out[0], "route"),
        "error",
        "an unrebuildable call must never re-enter the tool: {out:?}"
    );
    assert_eq!(hop_str(&out[0], "error_code"), "io_error");
}

#[test]
fn a_success_reply_passes_through_on_the_ok_lane() {
    let reply = result_turn("c1", "42");
    let out = emit(doc(
        json!({"call": call_turn("c1", "{}").to_string(), "attempt": 2}),
        json!({"rows_affected": 1}),
        vec![reply.clone()],
    ));

    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(hop_str(&out[0], "route"), "ok");
    assert_eq!(hop_str(&out[0], "tool_call_id"), "c1");
    assert_eq!(out[0]["messages"][0], reply, "the result is passed through");
}

#[test]
fn a_bundle_is_refused_with_one_synthetic_result_per_call() {
    // This unit retries ONE call. A bundle belongs to the dispatcher; refusing
    // it loudly (with every id answered) beats parking an ambiguous call.
    let out = emit(doc(
        json!({}),
        json!({}),
        vec![call_turn("c1", "{}"), call_turn("c2", "{}")],
    ));

    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(hop_str(&out[0], "route"), "error");
    assert_eq!(hop_str(&out[0], "error_code"), "bundle_not_supported");
    let turns = out[0]["messages"].as_array().expect("messages");
    assert_eq!(turns.len(), 2, "every call id is answered: {turns:?}");
    assert_eq!(turns[0]["id"], "c1");
    assert_eq!(turns[1]["id"], "c2");
    assert_eq!(turns[0]["type"], "tool_result");
}

#[test]
fn a_turn_that_is_neither_call_nor_reply_is_terminal() {
    let out = emit(doc(
        json!({}),
        json!({}),
        vec![json!({"origin": "user", "type": "text", "text": "hello"})],
    ));
    assert!(
        out.is_empty(),
        "empty multi-send, terminal by design: {out:?}"
    );
}

// ======================================================================= COLONY

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the cell under test IS the template and nothing else.
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

fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../builder/templates/retry")
}

/// A deterministic flaky tool: it fails while `context.attempt` is below
/// `FLAKY_FAIL_BELOW` and succeeds after. Reading the counter is exactly what
/// makes the test measure the EDGE's counting -- the tool succeeds only if the
/// retry edge really incremented `context.attempt`.
const FLAKY_TOOL: &str = r#"
import sys, json
d = json.load(sys.stdin)
ctx = (d.get("header") or {}).get("context") or {}
try:
    attempt = int(ctx.get("attempt", 0))
except Exception:
    attempt = 0
m = (d.get("messages") or [{}])[0]
cid = str(m.get("id", ""))
if attempt < int("${FLAKY_FAIL_BELOW:-2}"):
    out = {"header": {"error_code": "flaky_error"},
           "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                         "text": "boom at attempt %d" % attempt}]}
else:
    out = {"header": {},
           "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                         "text": "ok at attempt %d" % attempt}]}
sys.stdout.write(json.dumps(out))
"#;

fn flaky_config() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": FLAKY_TOOL, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": false,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"error_code": {"type": "string", "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in: a tool that fails until context.attempt reaches FLAKY_FAIL_BELOW.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The wiring the README documents, verbatim: call lane parks the call and
/// seeds the counter, the reply lane is unconditional, the retry lane counts
/// and bounds, the give-up lane rewrites to the error route with the attempt
/// count on the hop. `/sink` stands in for both the ok sink and the error
/// sink, told apart by `hop.route`.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./retry", "to": "./tool",
         "condition": "has(hop.route) && hop.route == 'call'",
         "modifier": {"set_context": {"call": "hop.call", "attempt": "'0'"}}},
        {"from": "./tool", "to": "./retry"},
        {"from": "./retry", "to": "./tool",
         "condition": "has(hop.route) && hop.route == 'retry' && int(context.attempt) < ${RETRY_MAX:-3}",
         "modifier": {"set_context": {"attempt": "int(context.attempt) + 1"}}},
        {"from": "./retry", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'ok'",
         "modifier": {"delete_context": ["call", "attempt"]}},
        {"from": "./retry", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'retry' && int(context.attempt) >= ${RETRY_MAX:-3}",
         "modifier": {"set_hop": {"route": "'error'", "attempt": "int(context.attempt)"}}},
        {"from": "./retry", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"}
    ]}}})
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn build_tree(td: &tempfile::TempDir, env: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), env).unwrap();
    write(root, "main/config.json", &main_config());
    copy_cells(&template_dir(), &root.join("main/retry"));
    write(root, "main/tool/config.json", &flaky_config());
}

fn call_message() -> Message {
    MessageBuilder::new(Path::new("/retry"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{\"q\":\"alpha\"}"}
        ]})))
        .ttl(64)
        .build()
}

fn hop_of<'a>(m: &'a Message, key: &str) -> Option<&'a Value> {
    m.headers.hop.get(key)
}

fn first_text(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or("").to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn assert_dlq_empty(root: &std::path::Path) {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count");
    assert_eq!(n, 0, "nothing may dead-letter in a wired retry unit");
}

/// A short bounded receive for "nothing more arrives" assertions. The 30 s of
/// `recv_bounded` are a failure marker; this one is a semantic discriminator
/// and stays tight on purpose (a retry round is two hops of local python).
async fn recv_bounded_short(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .ok()
        .flatten()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flaky_tool_succeeds_after_edge_counted_retries() {
    let td = tempfile::TempDir::new().unwrap();
    // Defaults: RETRY_MAX 3, the tool fails below attempt 2. The loop must run
    // initial + 2 retries and end in the ok sink -- succeeding ONLY because the
    // retry edge incremented context.attempt (the cell never counts).
    build_tree(&td, "");
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(call_message()).await;
    let m = recv_bounded(&mut sink_rx).await.expect("the ok result");

    assert_eq!(
        hop_of(&m, "route"),
        Some(&json!("ok")),
        "{:?}",
        m.headers.hop
    );
    assert_eq!(
        first_text(&m),
        "ok at attempt 2",
        "the counter climbed on the retry edge to exactly 2"
    );
    assert!(
        recv_bounded_short(&mut sink_rx).await.is_none(),
        "one result, nothing else"
    );
    assert_dlq_empty(td.path());

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_permanently_broken_tool_ends_in_the_error_sink_at_the_cap() {
    let td = tempfile::TempDir::new().unwrap();
    // The tool never succeeds: after RETRY_MAX (default 3) retries the retry
    // edge stops matching and the give-up edge fires -- original error code
    // and attempt count on the hop, nothing in the DLQ.
    build_tree(&td, "FLAKY_FAIL_BELOW=99\n");
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(call_message()).await;
    let m = recv_bounded(&mut sink_rx)
        .await
        .expect("the give-up handover");

    assert_eq!(
        hop_of(&m, "route"),
        Some(&json!("error")),
        "{:?}",
        m.headers.hop
    );
    assert_eq!(
        hop_of(&m, "error_code"),
        Some(&json!("flaky_error")),
        "the ORIGINAL tool error code travels to the sink"
    );
    assert_eq!(
        hop_of(&m, "attempt"),
        Some(&json!(3)),
        "the attempt count is on the hop"
    );
    // The body is the ORIGINAL call (the give-up edge re-routes the retry
    // emission): the sink learns WHICH call failed, WHY (hop.error_code) and
    // after HOW MANY attempts (hop.attempt).
    assert_eq!(
        first_text(&m),
        "{\"q\":\"alpha\"}",
        "the failed call travels to the sink"
    );
    assert!(
        recv_bounded_short(&mut sink_rx).await.is_none(),
        "the loop is bounded: exactly one handover"
    );
    assert_dlq_empty(td.path());

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_max_is_a_knob_on_the_edge_not_in_the_cell() {
    let td = tempfile::TempDir::new().unwrap();
    // RETRY_MAX=1: one retry, then give-up at attempt 1. The knob lives in the
    // edge condition (boot-substituted); the cell is byte-identical.
    build_tree(&td, "RETRY_MAX=1\nFLAKY_FAIL_BELOW=99\n");
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(call_message()).await;
    let m = recv_bounded(&mut sink_rx)
        .await
        .expect("the give-up handover");

    assert_eq!(
        hop_of(&m, "route"),
        Some(&json!("error")),
        "{:?}",
        m.headers.hop
    );
    assert_eq!(
        hop_of(&m, "attempt"),
        Some(&json!(1)),
        "cap reached after ONE retry"
    );
    assert_eq!(
        first_text(&m),
        "{\"q\":\"alpha\"}",
        "the failed call travels to the sink"
    );
    assert_dlq_empty(td.path());

    h.shutdown().await;
}
