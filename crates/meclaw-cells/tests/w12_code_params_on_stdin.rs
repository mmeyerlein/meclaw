//! W12 Track PA (route A, sanctioned 2026-08-15) — a `code` script reads
//! its own `params` off stdin.
//!
//! Lives in `meclaw-cells/tests/` for the same reason as
//! `paket_7_contract_demo.rs`: it drives a real `CodeCellFactory` under a live
//! Colony topology, which is the only place that proves the wiring the
//! production path actually takes (`spawn_cell` → `with_stdin_params` →
//! `build_stdin_json`). A unit test on the cell struct could not — it would
//! attach the params itself and prove nothing about the factory.
//!
//! Two demos:
//!   (a) the script reads `params.window_size` and USES it — the value shows up
//!       in the body that reaches the sink;
//!   (c) the same cell's `api_key` param does NOT appear anywhere on stdin,
//!       proven by echoing the ENTIRE stdin document back through the wire.
//!
//! Anti-cascade (phase-6.5/7 demo discipline): `/sink` is a terminal
//! CaptureCell, spawned and resolved BEFORE any probe goes out. The sink
//! receipt is the positive signal.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30s-convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The script echoes its ENTIRE stdin document back as a body slot AND reads
/// one knob out of `params` to build a value with. Both demos run this one
/// script — what differs is what the template's `params` block contains.
///
/// Since #139 the document is three objects deep (`envelope`/`body`/`params`),
/// so the script reads its payload from `body` and its configuration from
/// `params` — the two no longer share a namespace.
const SCRIPT: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
n = doc["params"]["window_size"]
sys.stdout.write(json.dumps({
    "messages": [{"origin": "assistant", "type": "text", "text": "window=%d" % (n * 2)}],
    "stdin_echo": doc,
    "body_keys": sorted(d.keys())}))
"#;

async fn colony_with_sink(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factory: Arc<dyn CellFactory> = Arc::new(CodeCellFactory);
    let h = ColonyHandle::new_with_factories_at(td, vec![("code".to_string(), factory)]);

    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    (h, sink_rx)
}

/// Install a `code` template `name` running [`SCRIPT`] with `extra_params`
/// merged into its `params` block, then load it via `RescanTemplates`.
async fn install_code_template(
    td: &tempfile::TempDir,
    h: &ColonyHandle,
    name: &str,
    extra_params: Value,
) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();

    let mut params = json!({
        "runner": "python3",
        "script_inline": SCRIPT,
        "external_timeout_ms": 10000
    });
    for (k, v) in extra_params.as_object().expect("extra_params is an object") {
        params[k] = v.clone();
    }
    let config = json!({
        "cell": {"type": "code"},
        "params": params,
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

async fn add_code_node(h: &ColonyHandle, node: &str, template: &str) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": node, "template": template}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of /{node} from {template} must commit; got {outcome:?}"
    );
}

fn probe(node: &str) -> Message {
    MessageBuilder::new(Path::new(&format!("/{node}")))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

async fn recv(sink_rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("sink recv timeout: {what}"))
        .unwrap_or_else(|| panic!("sink channel closed: {what}"))
}

fn body_of(m: &Message) -> Value {
    match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("non-inline body at sink: {other:?}"),
    }
}

/// Run the probe once against a cell configured with `extra_params` and return
/// the body that reached the sink.
async fn run(extra_params: Value) -> Value {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;
    install_code_template(&td, &h, "knobbed", extra_params).await;
    add_code_node(&h, "knobbed", "knobbed").await;
    h.add_edge(Uuid::now_v7(), Path::new("/knobbed"), Path::new("/sink"))
        .await;

    h.send_from(Path::new("/"), probe("knobbed")).await;
    let m = recv(&mut sink_rx, "params-reading script").await;
    assert!(
        m.headers.hop.get("error_code").is_none(),
        "the script must not error out; headers: {:?}",
        m.headers
    );
    let body = body_of(&m);
    h.shutdown().await;
    body
}

// ── (a) The script reads a param and uses it ────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn script_reads_a_param_off_stdin_and_uses_it() {
    // The knob is per-INSTANCE configuration — the whole point of route A
    // (w11-kurator-receipt.md § L3): no `${VAR}` involved, the value lives in
    // this cell's `params` and nowhere else.
    let body = run(json!({"window_size": 7})).await;

    // Positive receipt: 7 could only have come from params, and 14 could only
    // have come from the script having COMPUTED with it.
    assert_eq!(
        body["messages"][0]["text"], "window=14",
        "the script must see and use params.window_size; body: {body}"
    );
    assert_eq!(
        body["stdin_echo"]["params"]["window_size"], 7,
        "params travel as a read-only copy: {body}"
    );
    // #139: the payload lives under `body`, and `params` is beside it rather
    // than inside it — a body slot can no longer be mistaken for a knob.
    assert_eq!(
        body["body_keys"],
        json!(["messages"]),
        "the body carries the message slots and nothing else: {body}"
    );
    assert!(
        body["stdin_echo"]["envelope"]["header"].is_object(),
        "the envelope is the third object: {body}"
    );
}

// ── (c) Secrets are withheld, plain configuration is not ────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secret_shaped_params_never_reach_stdin() {
    let body = run(json!({
        "window_size": 7,
        "api_key": "sk-LEAK-api-key",
        "bot_token": "LEAK-bot-token",
        "auth_ref": "env:LEAK",
        "upstream": {"base_url": "http://example.invalid", "secret": "LEAK-nested"},
        "author": "ada"
    }))
    .await;

    let params = &body["stdin_echo"]["params"];
    assert_eq!(params["window_size"], 7, "configuration travels: {params}");
    assert_eq!(
        params["author"], "ada",
        "`author` is NOT `auth` — a key that merely starts with the deny word keeps travelling: {params}"
    );
    assert_eq!(
        params["upstream"]["base_url"], "http://example.invalid",
        "the filter descends without emptying the object: {params}"
    );
    for withheld in ["api_key", "bot_token", "auth_ref"] {
        assert!(
            params.get(withheld).is_none(),
            "`{withheld}` must not be on stdin: {params}"
        );
    }
    assert!(
        params["upstream"].get("secret").is_none(),
        "the filter reaches into nested objects: {params}"
    );
    // The script's own source is substrate-owned and stays off the wire.
    assert!(
        params.get("script_inline").is_none(),
        "the script does not get its own source echoed back: {params}"
    );

    // Belt and braces: no secret ANYWHERE in the document that crossed the pipe.
    let whole = meclaw_core::serde_json::to_string(&body).unwrap();
    assert!(
        !whole.contains("LEAK"),
        "no secret may appear anywhere on stdin: {whole}"
    );
}
