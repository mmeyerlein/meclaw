//! GH #87: the `llm` cell as the first `attachments[]` consumer.
//!
//! Spec: `docs/meclaw-overview.en.md` § "`attachments[]` schema" — the owner of
//! attachment resolution is the consuming cell, which reads the blob on demand
//! at `handle()` time. These tests drive a real `LlmCell` against a real
//! temp-directory blob store and a `MockOpenAI`, and pin:
//!
//! * a declared image attachment reaches the provider as an image content part
//!   with the right MIME type and base64 payload,
//! * a non-image MIME and a missing blob are CELL-LEVEL errors naming the
//!   attachment id and the reason (never a delivery-boundary dead letter),
//! * a hanging store read costs the cell its own operation timeout (A), not the
//!   `message_timeout` backstop (B),
//! * a cell that does NOT declare consumption produces the pre-GH-#87 request
//!   byte for byte.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{AttachmentReader, ContractView, DbConn, DiskBlobStore};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::deadline;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// A PNG-ish payload. Content does not have to be a real image — the cell
/// forwards bytes, it does not decode them.
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

/// Contract view of a text-only cell — no attachment declaration.
fn text_only_contract() -> ContractView {
    let block: meclaw_core::ConsumesBlock =
        meclaw_core::serde_json::from_value(json!({"body": {"messages": {"type": "array"}}}))
            .unwrap();
    ContractView {
        consumes: Some(Arc::new(meclaw_core::CompiledConsumes::compile(&block))),
        ..ContractView::default()
    }
}

fn params_for(mock: &MockOpenAI) -> LlmParams {
    LlmParams::parse(&json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-gh87",
        "base_url": format!("{}/v1", mock.base_url),
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

/// Receive under the hang mark: a regression that emits nothing fails the
/// test with `HANG:`, it does not hang the suite. The mark is no verdict.
async fn recv_bounded(rx: &mut mpsc::Receiver<CellEmission>) -> CellEmission {
    deadline::hang("the cell emits its answer", rx.recv())
        .await
        .expect("the sink channel must stay open")
}

/// `handle()` under the hang mark: it returns on its own, or the test fails
/// with `HANG:` and the call it waited for.
async fn handle_bounded(
    cell: &mut LlmCell,
    msg: meclaw_core::Message,
    out: &OutputSink,
    db: &mut DbConn,
) {
    deadline::hang("handle() returns", cell.handle(msg, out, db)).await
}

fn message(body: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build()
}

// ───── the happy path: image attachment → image content part ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_image_attachment_becomes_an_image_content_part() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("I see a fox", "stop")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(PNG_BYTES, "image/png", Some("fox.png"))
        .await
        .unwrap();

    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store.clone()));
    assert!(reader.is_some(), "a declaring contract must yield a reader");
    let mut cell =
        LlmCell::new(params_for(&mock), reqwest::Client::new()).with_attachment_reader(reader);

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, _rx) = sink();
    handle_bounded(
        &mut cell,
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

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "the request must have reached the provider");
    let messages = snaps[0].messages().unwrap();
    let content = messages
        .last()
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .expect("the last user message carries a content array");
    assert_eq!(content[0], json!({"type": "text", "text": "what is this?"}));
    assert_eq!(content[1]["type"], "image_url");
    // base64("\x89PNG\r\n\x1a\nGH87") — the exact payload, prefixed with the
    // sidecar's MIME type.
    assert_eq!(
        content[1]["image_url"]["url"],
        json!("data:image/png;base64,iVBORw0KGgpHSDg3")
    );
}

// ───── failure modes: cell-level errors that name the attachment ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_image_attachment_is_a_named_cell_error() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("unused", "stop")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(b"%PDF-1.4".as_slice(), "application/pdf", Some("r.pdf"))
        .await
        .unwrap();

    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store));
    let mut cell =
        LlmCell::new(params_for(&mock), reqwest::Client::new()).with_attachment_reader(reader);

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    handle_bounded(
        &mut cell,
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
    assert!(
        mock.recorded_requests().await.is_empty(),
        "an unusable attachment must not reach the provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_blob_is_a_named_cell_error() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("unused", "stop")]).await;
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store));
    let mut cell =
        LlmCell::new(params_for(&mock), reqwest::Client::new()).with_attachment_reader(reader);

    let ghost = Uuid::now_v7();
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    handle_bounded(
        &mut cell,
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
    assert!(mock.recorded_requests().await.is_empty());
}

/// Lets every reader of the FIFO go -- now, later, and when an assertion
/// panics. A blocking-pool thread stuck in `open(2)` holds the runtime's
/// teardown forever, and a plain writer `open(2)` blocks itself when no reader
/// ever came: the A-timeout may fire while the store still reads the sidecar,
/// and the FIFO is then never opened at all.
#[cfg(unix)]
struct FifoRelease(std::path::PathBuf);

#[cfg(unix)]
impl Drop for FifoRelease {
    fn drop(&mut self) {
        // O_RDWR on a FIFO does not block on Linux and counts as a writer: a
        // reader blocked in open(2) returns and reads end-of-file once it closes.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.0);
        // A read that starts later finds an empty plain file, never the FIFO.
        let plain = self.0.with_extension("released");
        let _ = std::fs::write(&plain, b"");
        let _ = std::fs::rename(&plain, &self.0);
        drop(held);
    }
}

/// The operation timeout (A) is the one that fires, and it fires as a regular
/// error message — `handle()` returns normally, so the substrate's
/// `message_timeout` backstop (B) never sees a hung cell and never restarts it.
///
/// The hang is real: the blob content path is a FIFO nobody writes to. The
/// proof is the error the cell answers with: the `timeout` code and a detail
/// that names the attachment and the cell's own A of 150 ms. A cell without
/// its own bound would never return, and the hang mark on `handle()` names
/// that -- the mark is no verdict on speed. Time runs real here: the hung
/// read sits in a blocking-pool thread, which freezes a paused clock.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hanging_blob_read_hits_the_operation_timeout_not_the_backstop() {
    const A_MS: u64 = 150;
    let mock = MockOpenAI::start(vec![canned_chat_completion("unused", "stop")]).await;
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
    let _release = FifoRelease(fifo.clone());

    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let reader = AttachmentReader::for_contract(&declaring_contract(), Some(store));
    // The cell's own A; no backstop guards a cell driven directly, so the
    // discriminator is "an answer naming A vs never".
    let mut raw = params_for(&mock);
    raw.attachment_timeout_ms = A_MS;
    let mut cell = LlmCell::new(raw, reqwest::Client::new()).with_attachment_reader(reader);

    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td).await;
    let (out, mut rx) = sink();
    handle_bounded(
        &mut cell,
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

    let em = recv_bounded(&mut rx).await;
    assert_eq!(em.content["header"]["finish_reason"], "error");
    assert_eq!(
        em.content["header"]["error_code"], "timeout",
        "an elapsed operation timeout is the `timeout` code of the closed enum"
    );
    let detail = em.content["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&id.to_string()) && detail.contains(&format!("timed out after {A_MS} ms")),
        "the detail must name the attachment and the cell's own operation timeout: {detail}"
    );
}

// ───── no declaration: byte-identical to pre-GH-#87 ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn undeclared_cell_ignores_attachments_byte_identically() {
    let blob_dir = TempDir::new().unwrap();
    let store = Arc::new(DiskBlobStore::new(blob_dir.path()).unwrap());
    let blob_ref = store
        .write_streaming(PNG_BYTES, "image/png", None)
        .await
        .unwrap();
    // A text-only contract yields no reader at all.
    assert!(
        AttachmentReader::for_contract(&text_only_contract(), Some(store)).is_none(),
        "a cell without the declaration must not receive a handle"
    );

    let turn = json!({"origin": "user", "type": "text", "text": "what is this?"});
    let with_slot = json!({
        "messages": [turn.clone()],
        "attachments": [{
            "blob_id": blob_ref.blob_id.to_string(),
            "mime_type": "image/png",
            "size_bytes": blob_ref.size_bytes,
        }]
    });
    let without_slot = json!({"messages": [turn]});

    let mut bodies = Vec::new();
    for body in [with_slot, without_slot] {
        let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
        let mut cell = LlmCell::new(params_for(&mock), reqwest::Client::new());
        let td = TempDir::new().unwrap();
        let mut db = cell_db(&td).await;
        let (out, _rx) = sink();
        handle_bounded(&mut cell, message(body), &out, &mut db).await;
        let snaps = mock.recorded_requests().await;
        assert_eq!(snaps.len(), 1);
        bodies.push(meclaw_core::serde_json::to_string(&snaps[0].body).unwrap());
    }
    assert_eq!(
        bodies[0], bodies[1],
        "an undeclared cell must produce the same request with and without the \
         attachments slot — the slot travels past it untouched"
    );
}
