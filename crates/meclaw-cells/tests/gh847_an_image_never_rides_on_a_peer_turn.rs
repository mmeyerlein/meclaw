//! GH #847 — an image attachment never rides on a peer turn.
//!
//! A peer turn goes out as role `user` (the only role that carries the other
//! side's words on both wires). Before this issue the image anchor was the LAST
//! wire message with role `user` (`attach_image_parts`, `attach_input_images`),
//! so in `[user + image, peer]` the agent's own person's picture would have
//! been handed to the stranger's turn. The anchor is the last UBF turn with
//! `origin: "user"` now; without one, the images still become one appended
//! user message of their own, as before.
//!
//! Measured at the seam: the request body the mock provider received.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "mock_responses.rs"]
mod mock_responses;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{AttachmentReader, ContractView, DbConn, DiskBlobStore};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use mock_responses::{MockResponses, canned_sse_text};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// base64: `iVBORw0KGgpHSDg3`.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nGH87";
const DATA_URL: &str = "data:image/png;base64,iVBORw0KGgpHSDg3";

fn declaring_contract() -> ContractView {
    let block: meclaw_core::ConsumesBlock = meclaw_core::serde_json::from_value(json!({
        "body": {"messages": {"type": "array"}, "attachments": {"type": "array"}}
    }))
    .unwrap();
    ContractView {
        consumes: Some(Arc::new(meclaw_core::CompiledConsumes::compile(&block))),
        ..ContractView::default()
    }
}

/// Drive one call carrying `messages` and one committed image attachment.
async fn drive(params: Value, messages: Value) {
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(PNG_BYTES, "image/png", Some("fox.png"))
        .await
        .unwrap();
    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store));
    let mut cell = LlmCell::new(LlmParams::parse(&params).unwrap(), reqwest::Client::new())
        .with_attachment_reader(reader);
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": messages,
            "attachments": [{
                "blob_id": blob_ref.blob_id.to_string(),
                "mime_type": "image/png",
                "size_bytes": blob_ref.size_bytes,
            }]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    let em = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("the cell must emit")
        .expect("the sink stays open");
    assert_eq!(
        em.content["header"]["finish_reason"], "stop",
        "{}",
        em.content
    );
}

fn user_then_peer() -> Value {
    json!([
        {"origin": "user", "type": "text", "text": "look at this"},
        {"origin": "peer", "type": "text", "text": "nice", "speaker_ref": "3a47fe3e"}
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_chat_completions_the_image_stays_with_the_agents_own_person() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    drive(
        json!({"provider": "openai", "model": "gpt-4o", "api_key": "sk-test",
            "base_url": format!("{}/v1", mock.base_url)}),
        user_then_peer(),
    )
    .await;
    let reqs = mock.recorded_requests().await;
    let messages = reqs[0].messages().unwrap();
    assert_eq!(messages.len(), 2, "no extra message: {messages:?}");
    assert_eq!(
        messages[0]["content"],
        json!([
            {"type": "text", "text": "look at this"},
            {"type": "image_url", "image_url": {"url": DATA_URL}}
        ]),
        "the image hangs on the user turn"
    );
    assert_eq!(
        messages[1],
        json!({"role": "user", "content": "[peer 3a47fe3e]\nnice"}),
        "and never on the peer turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_responses_the_image_stays_with_the_agents_own_person() {
    let mock = MockResponses::start(vec![canned_sse_text("ok", "gpt-5-mock")]).await;
    drive(
        json!({"provider": "openai", "model": "gpt-5", "api_key": "sk-test",
            "wire_dialect": "responses", "base_url": mock.base_url, "max_tokens": 32}),
        user_then_peer(),
    )
    .await;
    let reqs = mock.recorded().await;
    let input = reqs[0].body["input"].as_array().unwrap();
    assert_eq!(input.len(), 2, "no extra item: {input:?}");
    assert_eq!(
        input[0]["content"],
        json!([
            {"type": "input_text", "text": "look at this"},
            {"type": "input_image", "image_url": DATA_URL}
        ])
    );
    assert_eq!(
        input[1]["content"],
        json!([{"type": "input_text", "text": "[peer 3a47fe3e]\nnice"}])
    );
}

/// Only a peer turn: the images become an appended user message of their own,
/// never a part of the stranger's message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_user_turn_the_image_is_its_own_message() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    drive(
        json!({"provider": "openai", "model": "gpt-4o", "api_key": "sk-test",
            "base_url": format!("{}/v1", mock.base_url)}),
        json!([{"origin": "peer", "type": "text", "text": "nice"}]),
    )
    .await;
    let reqs = mock.recorded_requests().await;
    let messages = reqs[0].messages().unwrap();
    assert_eq!(
        messages,
        &vec![
            json!({"role": "user", "content": "[peer]\nnice"}),
            json!({"role": "user", "content": [{"type": "image_url", "image_url": {"url": DATA_URL}}]}),
        ]
    );
}
