//! GH #87 end-to-end: the `attachments[]` slot finally has a producer AND a
//! consumer in one running colony.
//!
//! Route: `POST /messages` (multipart) streams an image into the `DiskBlobStore`
//! and answers with the `BlobRef` — that is the one real producer,
//! `post_messages_multipart`. The client then sends the conversation turn plus
//! that `BlobRef` to a vision `llm` cell which declares
//! `consumes.body.attachments`. The cell resolves the blob through its read
//! handle at `handle()` time and the outbound provider request carries the
//! image as a content part.
//!
//! Spec: `docs/meclaw-overview.en.md` § "`attachments[]` schema" (the consuming
//! cell is the owner of the resolution) and § "Blob storage" (the sidecar is the
//! commit marker). One store serves both ends: the router and the colony share
//! the same `Arc<DiskBlobStore>` (`crates/meclaw-cli/src/lib.rs`).
//!
//! Mutation check: neutering the cell's read path (making
//! `LlmCell::resolve_image_attachments` return an empty vector) must turn
//! `multipart_upload_reaches_the_llm_cell_as_an_image_content_part` red.

use meclaw_cli::{Cli, StdioFormat, run_with_hooks};
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::net::SocketAddr;

/// A short PNG-ish payload; the cell forwards bytes, it does not decode them.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nGH87";
/// base64(PNG_BYTES) — the exact payload the provider must receive.
const PNG_BASE64: &str = "iVBORw0KGgpHSDg3";

fn canned_chat_completion(text: &str) -> MockResponse {
    MockResponse::ok_json(
        serde_json::json!({
            "id": "chatcmpl-gh87",
            "model": "gpt-4o-mock",
            "choices": [{"message": {"role": "assistant", "content": text},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        })
        .to_string()
        .as_bytes(),
    )
}

/// Colony tree: one root hive with a single vision `llm` cell at `/llm`.
///
/// The contract declares BOTH body slots it reads. Declaring is binding
/// (config.md § consumes), so the attachments-only body the multipart ingress
/// synthesizes for its own message is rejected at the delivery boundary and the
/// cell is never invoked with it — exactly the intent: the upload call stores
/// the blob, the follow-up call is the conversation.
fn write_tree(root: &std::path::Path, base_url: &str) {
    let llm_dir = root.join("llm");
    std::fs::create_dir_all(&llm_dir).unwrap();
    std::fs::write(root.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
    let cfg = serde_json::json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "test-key-gh87",
            "base_url": base_url,
            "attachment_timeout_ms": 5000
        },
        "contract": {
            "version": "0.1.0",
            "settings": {},
            "consumes": {"body": {
                "messages": {"type": "array"},
                "attachments": {"type": "array"}
            }}
        }
    });
    std::fs::write(
        llm_dir.join("config.json"),
        serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn cli_for(root: &std::path::Path, blobs: &std::path::Path, bind: SocketAddr) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some(bind),
        daemon: false,
        validate: false,
        strict: false,
        blobs: Some(blobs.into()),
        tokio_console: false,
        tokio_console_port: 6669,
        stdio_format: StdioFormat::Text,
    }
}

/// Hand-rolled multipart body (`reqwest` is configured here without the
/// `multipart` feature — same as the phase-12-X upload tests).
fn multipart_body(boundary: &str, target: &str, content: &[u8]) -> Vec<u8> {
    let prelude = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"target\"\r\n\r\n\
         {target}\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"attachment\"; filename=\"fox.png\"\r\n\
         Content-Type: image/png\r\n\r\n"
    );
    let mut body = prelude.into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multipart_upload_reaches_the_llm_cell_as_an_image_content_part() {
    let (mock_addr, _mock_join, captured) =
        start_mock_server_capturing(vec![canned_chat_completion("I see a fox")]).await;

    let td = tempfile::TempDir::new().unwrap();
    let root_dir = td.path().join("root");
    write_tree(&root_dir, &format!("http://{mock_addr}/v1"));
    let blob_td = tempfile::TempDir::new().unwrap();

    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let cli = cli_for(td.path(), blob_td.path(), bind);
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

    // ── 1. the producer: multipart upload → blob store → BlobRef ──────────
    let boundary = "----GH87Boundary";
    let resp = client
        .post(format!("http://{addr}/messages"))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(multipart_body(boundary, "/llm", PNG_BYTES))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let uploaded: serde_json::Value = resp.json().await.unwrap();
    let blob_ref = uploaded["attachments"]
        .as_array()
        .expect("the multipart response carries the BlobRef")
        .first()
        .cloned()
        .expect("exactly one uploaded file");
    assert_eq!(blob_ref["mime_type"], "image/png");
    assert_eq!(blob_ref["size_bytes"], PNG_BYTES.len() as u64);

    // ── 2. the consumer: conversation turn + that BlobRef → the llm cell ──
    let resp = client
        .post(format!("http://{addr}/messages"))
        .json(&serde_json::json!({
            "target": "/llm",
            "body": {
                "messages": [{"origin": "user", "type": "text", "text": "what is this?"}],
                "attachments": [blob_ref]
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    // ── 3. the receipt: the provider request carried the image ────────────
    let request = wait_for_provider_request(&captured).await;
    let messages = request["messages"].as_array().expect("messages[] on wire");
    let content = messages
        .last()
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .expect(
            "the last user message must carry a content array — an empty one \
             means the cell never resolved the attachment",
        );
    assert_eq!(
        content[0],
        serde_json::json!({"type": "text", "text": "what is this?"})
    );
    assert_eq!(
        content[1],
        serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/png;base64,{PNG_BASE64}")}
        }),
        "the uploaded blob must reach the provider as a base64 data URL"
    );

    let _ = shutdown_tx.send(());
    let _ = join.await;
}

/// Poll the mock's capture buffer until the llm cell's request arrives.
///
/// Generous 30 s failure-marker budget per the repo's convention (robust under
/// cargo's parallel load); the normal case resolves in milliseconds.
async fn wait_for_provider_request(
    captured: &std::sync::Arc<tokio::sync::Mutex<Vec<meclaw_testing::mock_http::CapturedRequest>>>,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        {
            let caps = captured.lock().await;
            if let Some(first) = caps.first() {
                return serde_json::from_slice(&first.body).expect("captured body is JSON");
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no provider request arrived — the llm cell never called out"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
