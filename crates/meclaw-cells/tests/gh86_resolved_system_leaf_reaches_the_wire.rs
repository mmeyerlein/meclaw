//! GH #86 — the llm half: a resolved `system` leaf reaches the provider.
//!
//! Before this issue the `llm` cell rejected a `{text_id}` leaf outright
//! (`TranslateError::BlobUnsupported`), because nothing resolved that pointer
//! class. The substrate resolves it at the delivery boundary now, so a leaf that
//! travelled as a pointer arrives here as `{"text": …}`, indistinguishable from
//! one that was always inline. These tests replace the rejection pins: the
//! resolved form is not merely accepted, it reaches the wire.
//!
//! The two halves of the chain are pinned at their own levels:
//!
//! * that the boundary PRODUCES this shape from a pointer —
//!   `meclaw-colony`'s `gh86_system_tree_pointer_resolution.rs` (end-to-end
//!   through a colony) and the `blob::pointers` unit tests;
//! * that this shape reaches the provider — here, against a real `LlmCell` and
//!   a mock OpenAI endpoint.
//!
//! The body each test sends is exactly what the boundary hands the cell.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

/// Drive one inference with `body` through a real `LlmCell` against `mock`, and
/// return what landed on the wire.
async fn drive(mock: &MockOpenAI, body: Value) -> OpenAiRequestSnapshot {
    let td = TempDir::new().unwrap();
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
        "system_order": ["identity"],
    }))
    .expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    rx.recv().await.expect("the cell must emit something");
    let mut snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call per inference");
    snaps.remove(0)
}

/// The leaf that travelled as a pointer joins the system prompt, in tree order,
/// next to the leaf that was always inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resolved_leaf_joins_the_provider_system_message() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let snap = drive(
        &mock,
        json!({
            "system": {"identity": {
                // Was `{"text_id": "<uuid>"}` on the wire into the colony.
                "body": {"text": "the long persona"},
                "soul": {"text": "inline"},
            }},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        }),
    )
    .await;

    let messages = snap.messages().expect("request must carry messages[]");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages[0]["content"], "the long persona\n\ninline",
        "the resolved leaf must reach the provider, not be dropped: {messages:?}"
    );
    assert_eq!(messages[1]["role"], "user");
}

/// A `system.tools.*` leaf is the sharpest case: the resolver must not skip the
/// tools sub-slot, because `extract_tools` demands a `text` field and a leaf
/// left as a pointer would fail the whole inference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resolved_tools_leaf_becomes_a_provider_native_tool() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let snap = drive(
        &mock,
        json!({
            "system": {"tools": {
                // Was `{"text_id": "<uuid>"}` on the wire into the colony.
                "calculator": {"text": "{\"type\":\"function\",\"function\":{\"name\":\"calc\"}}"}
            }},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        }),
    )
    .await;

    let tools = snap.tools().expect("request must carry tools[]");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "calc");
    // The tools sub-slot stays out of the prompt string, as it always has.
    let messages = snap.messages().expect("request must carry messages[]");
    assert_eq!(
        messages[0]["role"], "user",
        "tools must not leak into the system message: {messages:?}"
    );
}
