//! GH #94: `attachments[]` consumption on the `responses` wire dialect.
//!
//! GH #87 built attachment consumption for `chat_completions` (`image_url`
//! content parts). The Responses API carries images as `input_image` items
//! inside the typed `input[]` (pinned reference: `openai/codex` @ `266c6920`,
//! `protocol/src/models.rs:716-734`, `ContentItem::InputImage` — `image_url`
//! is a plain data-URL **string** on this wire, not the chat-completions
//! object form). Until now a non-empty `attachments[]` on
//! `wire_dialect: "responses"` was a loud `invalid_input` reject; these tests
//! redefine that pin: the reject falls away, replaced by the translation, with
//! the SAME failure taxonomy as the chat-completions path:
//!
//! * a declared image attachment reaches the provider as an `input_image`
//!   item on the last user message of `input[]`,
//! * without a user turn the images become one appended user message item,
//! * non-image MIME and missing blob are CELL-LEVEL errors (`invalid_input`)
//!   naming the attachment id and the reason,
//! * an elapsed `attachment_timeout_ms` is `timeout` (operation timeout A,
//!   not the message_timeout backstop B).

#[path = "mock_responses.rs"]
mod mock_responses;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{AttachmentReader, ContractView, DbConn, DiskBlobStore};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_responses::{MockResponses, canned_sse_text};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Same PNG-ish payload as the GH-#87 suite — the cell forwards bytes, it does
/// not decode them. base64: `iVBORw0KGgpHSDg3`.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nGH87";

/// Contract view of a cell that declares `consumes.body.attachments`.
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

/// Params for the metered (api_key) Responses lane, pointed at the mock.
fn params_for(mock: &MockResponses) -> LlmParams {
    LlmParams::parse(&json!({
        "provider": "openai",
        "model": "gpt-5",
        "api_key": "test-key-gh94",
        "wire_dialect": "responses",
        "base_url": mock.base_url,
        "max_tokens": 32,
    }))
    .unwrap()
}

fn sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel(8);
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

async fn cell_db(td: &TempDir) -> DbConn {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    DbConn::wrap(conn, None)
}

/// Bounded receive (30 s failure-marker budget, repo convention).
async fn recv_bounded(rx: &mut mpsc::Receiver<CellEmission>) -> CellEmission {
    tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("the cell must emit a message, not stay silent")
        .expect("the sink channel must stay open")
}

fn message(body: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build()
}

/// Rig: blob store with one committed blob + a declaring responses-cell.
async fn cell_with_store(mock: &MockResponses, store: Arc<DiskBlobStore>) -> LlmCell {
    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store));
    assert!(reader.is_some(), "a declaring contract must yield a reader");
    LlmCell::new(params_for(mock), reqwest::Client::new()).with_attachment_reader(reader)
}

// ───── the happy path: image attachment → input_image item ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_image_attachment_becomes_an_input_image_item() {
    let mock = MockResponses::start(vec![canned_sse_text("I see a fox", "gpt-5-mock")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(PNG_BYTES, "image/png", Some("fox.png"))
        .await
        .unwrap();
    let mut cell = cell_with_store(&mock, store).await;

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    cell.handle(
        message(json!({
            "messages": [{"origin": "user", "type": "text", "text": "what is this?"}],
            "attachments": [{
                "blob_id": blob_ref.blob_id.to_string(),
                "mime_type": "image/png",
                "filename": "fox.png",
                "size_bytes": blob_ref.size_bytes,
            }]
        })),
        &out,
        &mut db,
    )
    .await;

    let reqs = mock.recorded().await;
    assert_eq!(reqs.len(), 1, "the request must have reached the provider");
    assert_eq!(reqs[0].path, "/responses");
    let input = reqs[0].body["input"]
        .as_array()
        .expect("responses body carries a typed input[]");
    let user_msg = input
        .iter()
        .rev()
        .find(|i| i["type"] == "message" && i["role"] == "user")
        .expect("the last user message carries the image");
    let content = user_msg["content"].as_array().unwrap();
    assert_eq!(
        content[0],
        json!({"type": "input_text", "text": "what is this?"})
    );
    // The pinned wire form: a typed input_image item whose image_url is a
    // plain base64-data-URL STRING (ContentItem::InputImage{image_url:String}),
    // carrying the sidecar's MIME type — same data URL as chat-completions.
    assert_eq!(
        content[1],
        json!({
            "type": "input_image",
            "image_url": "data:image/png;base64,iVBORw0KGgpHSDg3"
        })
    );
    // The cell answered normally — no error emission.
    let em = recv_bounded(&mut rx).await;
    assert_eq!(em.content["header"]["finish_reason"], "stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachment_without_user_turn_becomes_its_own_user_message() {
    let mock = MockResponses::start(vec![canned_sse_text("noted", "gpt-5-mock")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(PNG_BYTES, "image/png", None)
        .await
        .unwrap();
    let mut cell = cell_with_store(&mock, store).await;

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, _rx) = sink();
    cell.handle(
        message(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": "earlier answer"}],
            "attachments": [{
                "blob_id": blob_ref.blob_id.to_string(),
                "mime_type": "image/png",
                "size_bytes": blob_ref.size_bytes,
            }]
        })),
        &out,
        &mut db,
    )
    .await;

    let reqs = mock.recorded().await;
    assert_eq!(reqs.len(), 1);
    let input = reqs[0].body["input"].as_array().unwrap();
    // An attachment always reaches the model as user input: appended as its
    // own user message item after the assistant turn.
    let last = input.last().unwrap();
    assert_eq!(last["type"], "message");
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["type"], "input_image");
    assert_eq!(
        last["content"][0]["image_url"],
        "data:image/png;base64,iVBORw0KGgpHSDg3"
    );
}

// ───── failure taxonomy: identical to the chat-completions path ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_image_attachment_is_a_named_cell_error() {
    let mock = MockResponses::start(vec![canned_sse_text("unused", "gpt-5-mock")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(b"%PDF-1.4".as_slice(), "application/pdf", Some("r.pdf"))
        .await
        .unwrap();
    let mut cell = cell_with_store(&mock, store).await;

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    cell.handle(
        message(json!({
            "messages": [{"origin": "user", "type": "text", "text": "read this"}],
            "attachments": [{
                "blob_id": blob_ref.blob_id.to_string(),
                "mime_type": "application/pdf",
                "size_bytes": blob_ref.size_bytes,
            }]
        })),
        &out,
        &mut db,
    )
    .await;

    let em = recv_bounded(&mut rx).await;
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "invalid_input");
    let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&blob_ref.blob_id.to_string()),
        "the detail must name the attachment: {detail}"
    );
    assert!(
        detail.contains("application/pdf"),
        "the detail must name the reason: {detail}"
    );
    // Gate-1: the original conversation travels with the error.
    assert_eq!(em.content["messages"][0]["text"], "read this");
    assert_eq!(
        mock.call_count().await,
        0,
        "an unusable attachment must not reach the provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_blob_is_a_named_cell_error() {
    let mock = MockResponses::start(vec![canned_sse_text("unused", "gpt-5-mock")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let mut cell = cell_with_store(&mock, store).await;

    let ghost = Uuid::now_v7();
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    cell.handle(
        message(json!({
            "messages": [{"origin": "user", "type": "text", "text": "look"}],
            "attachments": [{
                "blob_id": ghost.to_string(), "mime_type": "image/png", "size_bytes": 4
            }]
        })),
        &out,
        &mut db,
    )
    .await;

    let em = recv_bounded(&mut rx).await;
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(em.content["header"]["error_code"], "invalid_input");
    let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&ghost.to_string()) && detail.contains("not found"),
        "the detail must name the attachment and the reason: {detail}"
    );
    assert_eq!(mock.call_count().await, 0);
}

/// The operation timeout (A) fires as a regular error message — same FIFO rig
/// as the GH-#87 suite: the blob content path is a pipe nobody writes to.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hanging_blob_read_hits_the_operation_timeout_not_the_backstop() {
    let mock = MockResponses::start(vec![canned_sse_text("unused", "gpt-5-mock")]).await;
    let blob_dir = TempDir::new().unwrap();
    let id = Uuid::now_v7();
    let sidecar = json!({
        "schema_version": 1, "mime_type": "image/png", "size_bytes": 4, "created_at": "0"
    });
    std::fs::write(
        blob_dir.path().join(format!("{id}.png.meta.json")),
        meclaw_core::serde_json::to_vec(&sidecar).unwrap(),
    )
    .unwrap();
    let fifo = blob_dir.path().join(format!("{id}.png"));
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs")
            .success()
    );

    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store));
    let mut params = params_for(&mock);
    params.attachment_timeout_ms = 150;
    let mut cell = LlmCell::new(params, reqwest::Client::new()).with_attachment_reader(reader);

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    let started = std::time::Instant::now();
    cell.handle(
        message(json!({
            "messages": [{"origin": "user", "type": "text", "text": "look"}],
            "attachments": [{
                "blob_id": id.to_string(), "mime_type": "image/png", "size_bytes": 4
            }]
        })),
        &out,
        &mut db,
    )
    .await;
    let elapsed = started.elapsed();
    // Release the cell's blocked open(2) — from a throwaway thread, because a
    // writer-open on a FIFO blocks until a reader appears: if a regression ever
    // stops the cell from reading the blob on this dialect (the pre-GH-#94
    // reject did exactly that), an inline open would hang the suite instead of
    // letting the assertions below fail loudly.
    let fifo_release = fifo.clone();
    std::thread::spawn(move || {
        let _ = std::fs::OpenOptions::new().write(true).open(fifo_release);
    });

    let em = recv_bounded(&mut rx).await;
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(
        em.content["header"]["error_code"], "timeout",
        "an elapsed operation timeout is the `timeout` code of the closed enum"
    );
    let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&id.to_string()) && detail.contains("timed out"),
        "the detail must name the attachment and the reason: {detail}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "handle() must return on its own operation timeout, took {elapsed:?}"
    );
    assert_eq!(mock.call_count().await, 0);
}
