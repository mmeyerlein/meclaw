//! meclaw-os -- the summarizer hive in a running colony (GH #100).
//!
//! The script-level pins live in `summarizer_prep.rs`. This file boots the
//! SHIPPED template into a colony and drives the collector's write-batch form
//! through it, with the writer llm talking to the mock OpenAI wire (never a
//! real provider): one batch in, exactly ONE `system.handover` update out --
//! and when the provider fails, exactly one `summary_error` instead of
//! silence. The wire itself is asserted too: the honesty instructions and the
//! session's turns must be what the model was actually asked with.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_error_status};
use support::{boot, recv_bounded};

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template and nothing else.
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
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../builder/templates/summarizer")
}

/// The template ships `${ctx.model}` (resolved at instantiation) and the
/// OpenRouter base URL. A test colony bootstraps from disk, so both are
/// patched to literals: the model to a name the mock echoes, the URL to the
/// mock server. Nothing else of the shipped config changes.
fn patch_writer(root: &std::path::Path, base_url: &str) {
    let p = root.join("main/sum/writer/config.json");
    let txt = std::fs::read_to_string(&p).unwrap();
    let mut v: Value = meclaw_core::serde_json::from_str(&txt).unwrap();
    v["params"]["base_url"] = json!(base_url);
    v["params"]["model"] = json!("gpt-4o-mock");
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// Stand-in for the collector: emits the c3 write-batch form -- messages[] the
/// whole day in order, the raw round rows in the top-level slot `rounds`, the
/// session and its sizes on the hop -- exactly what the close lane sends.
const CLOSED_DAY: &str = r#"
import sys, json
json.load(sys.stdin)
batch = {
  "header": {"route": "write", "session_id": "s1",
             "turn_count": "4", "round_count": "1"},
  "messages": [
    {"origin": "user", "type": "text", "text": "my editor is helix"},
    {"origin": "assistant", "type": "text", "text": "noted: helix"},
    {"origin": "user", "type": "text", "text": "and my shell is fish"},
    {"origin": "assistant", "type": "text", "text": "helix plus fish, got it"}],
  "rounds": [
    {"turn_id": "t1", "iter": 0, "role": "leg-window",
     "turn": {"turns": []}, "fired": 1}]
}
sys.stdout.write(json.dumps(batch))
"#;

fn probe_config() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": CLOSED_DAY,
                   "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {
                    "messages": {"type": "array", "required": true},
                    "rounds": {"type": "array", "required": true}
                },
                "hop": {
                    "route": {"type": "string", "values": ["write"], "required": true},
                    "session_id": {"type": "string", "required": true},
                    "turn_count": {"type": "string", "required": false},
                    "round_count": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for the collector's close lane.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The port wiring a parent draws around the summarizer: the collector's
/// write route into the batch lane, both exits into the sink.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./probe", "to": "./sum/prep",
         "condition": "hop.route == 'write'",
         "modifier": {"set_hop": {"route": "'in_batch'"}}},
        {"from": "./sum/prep", "to": "/sink",
         "condition": "hop.route == 'summary'"},
        {"from": "./sum/prep", "to": "/sink",
         "condition": "hop.route == 'summary_error'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    let main = json!(main_config());
    std::fs::create_dir_all(root.join("main/probe")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        meclaw_core::serde_json::to_string_pretty(&main).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("main/probe/config.json"),
        meclaw_core::serde_json::to_string_pretty(&probe_config()).unwrap(),
    )
    .unwrap();
    copy_cells(&template_dir(), &root.join("main/sum"));
    patch_writer(root, base_url);
}

fn close_request() -> Message {
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": "/close"}]}),
        ))
        .ttl(64)
        .build()
}

fn body_of(m: &Message) -> &Value {
    match &m.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("inline expected"),
    }
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_session_batch_becomes_exactly_one_handover_update() {
    let mock = MockOpenAI::start(vec![canned_chat_completion(
        "The user set up helix as their editor and fish as their shell.",
        "stop",
    )])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(close_request()).await;
    let got = recv_bounded(&mut sink_rx).await.expect("the summary");

    // ONE emission on route summary, and its body IS the system.handover
    // update the next generation's llm consumes without a provider call.
    assert_eq!(hop_of(&got, "route"), "summary");
    assert_eq!(hop_of(&got, "session_id"), "s1");
    let body = body_of(&got);
    assert_eq!(
        body["system"]["handover"]["text"],
        "The user set up helix as their editor and fish as their shell."
    );
    assert_eq!(hop_of(&got, "summary_chars"), "62");
    assert!(
        body.get("messages").is_none(),
        "a system update must not trigger an inference downstream: {body}"
    );

    // Exactly one: nothing follows the update.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), sink_rx.recv())
            .await
            .is_err(),
        "one batch, one emission"
    );

    // And the wire proves the prompt: the shipped instructions with the
    // honesty sentence went out as the system message, the session's turns
    // as the user document.
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "one close, one provider call");
    let msgs = reqs[0].messages().expect("wire messages");
    let system = msgs
        .iter()
        .find(|m| m["role"] == "system")
        .expect("system message on the wire");
    let sys_text = system["content"].as_str().unwrap_or_default();
    assert!(
        sys_text.contains("never invent"),
        "the honesty sentence reached the model: {sys_text}"
    );
    let user = msgs
        .iter()
        .find(|m| m["role"] == "user")
        .expect("user document on the wire");
    let user_text = user["content"].as_str().unwrap_or_default();
    assert!(
        user_text.contains("my editor is helix"),
        "the day itself reached the model: {user_text}"
    );
    assert!(
        user_text.contains("Session s1 closed with 4 turns."),
        "{user_text}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_provider_leaves_on_summary_error() {
    // The degradation ruling of GH #100: a failed call is handed to the
    // parent tree on its own route -- drain or alarm is the parent's call,
    // swallowing is nobody's.
    let mock = MockOpenAI::start(vec![canned_error_status(500)]).await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(close_request()).await;
    let got = recv_bounded(&mut sink_rx).await.expect("the error report");

    assert_eq!(hop_of(&got, "route"), "summary_error");
    assert_eq!(hop_of(&got, "session_id"), "s1");
    assert_eq!(hop_of(&got, "error_code"), "provider_error");
    let text = body_of(&got)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(text.contains("s1"), "the report names the session: {text}");

    // No half-summary sneaks out behind the error.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), sink_rx.recv())
            .await
            .is_err(),
        "an error is one report, not a report and a summary"
    );

    h.shutdown().await;
}
