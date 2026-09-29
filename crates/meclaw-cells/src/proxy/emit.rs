//! Phase-10-C: emit helpers for `ProxyCell`. (1) `build_user_turn_content`
//! constructs the UBF body for outbound (T11). (2) `emit_inbound_error` emits
//! error replies for `missing_chat_id`/`send_failed`/`missing_assistant_turn`
//! (T13, W5/W6/W12).

use meclaw_core::{CellOutput, JsonValue, Message, OutputSink, serde_json::json};

/// Builds the UBF content for a user-source emission. Headers per spec
/// cell-types.md l.370: `chat_id`, `user_id`, `platform: "telegram"`, optionally
/// `message_id`. Body = `messages[]` with one user turn (`origin: "user"`,
/// `type: "text"`, `text: <what the user typed>`).
pub fn build_user_turn_content(
    chat_id: i64,
    user_id: Option<i64>,
    message_id: Option<i64>,
    text: &str,
) -> JsonValue {
    let mut header = meclaw_core::serde_json::Map::new();
    header.insert("chat_id".into(), json!(chat_id));
    if let Some(uid) = user_id {
        header.insert("user_id".into(), json!(uid));
    }
    header.insert("platform".into(), json!("telegram"));
    if let Some(mid) = message_id {
        header.insert("message_id".into(), json!(mid));
    }
    json!({
        "header": header,
        "messages": [
            { "origin": "user", "type": "text", "text": text }
        ]
    })
}

/// GH #907: the UBF content of a document turn. Same header as a text turn,
/// and the turn carries the caption as its text (empty allowed). With the
/// document committed to the colony blob store (`stored = Ok(blob)`) the body
/// gains ONE `attachments[]` entry -- `{blob_id, mime_type, filename,
/// size_bytes, sha256}` -- and the header says `has_file: "1"`, the one hop
/// key the member lifts into context to send the turn to its file space
/// instead of straight to the assistant (the body is never visible to an edge
/// condition). The document rides as a blob reference, never as bytes (GH
/// #907, R-FJ-1): a resolved turn is parked in hop and context by a member's
/// firewall, and hop and context are never offloaded to a blob, so bytes in
/// the turn were written several times over into the message log. Without a
/// stored document (`stored = Err(why)`) the turn still goes out -- a document
/// is never swallowed -- carrying the caption and one sentence that names the
/// file and why it is not there.
pub fn build_document_turn_content(
    chat_id: i64,
    user_id: Option<i64>,
    message_id: Option<i64>,
    caption: &str,
    name: &str,
    stored: Result<&meclaw_core::BlobRef, &str>,
) -> JsonValue {
    let mut v = build_user_turn_content(chat_id, user_id, message_id, caption);
    let why = match stored {
        Ok(blob) => {
            v["header"]["has_file"] = json!("1");
            v["attachments"] = json!([{
                "blob_id": blob.blob_id.to_string(),
                "mime_type": blob.mime_type,
                "filename": blob.filename.as_deref().unwrap_or(name),
                "size_bytes": blob.size_bytes,
                "sha256": blob.sha256.as_deref().unwrap_or(""),
            }]);
            return v;
        }
        Err(why) => why,
    };
    let line = format!("[file \"{name}\" could not be stored: {why}]");
    let text = if caption.is_empty() {
        line
    } else {
        format!("{caption}\n{line}")
    };
    v["messages"][0]["text"] = json!(text);
    v
}

/// GH #907: why a fetched document has no bytes to store, as the fixed code
/// the fallback line names; `None` when there are bytes.
pub fn document_failure(content: &crate::proxy::io::DocumentContent) -> Option<String> {
    use crate::proxy::io::DocumentContent;
    match content {
        DocumentContent::Bytes(_) => None,
        DocumentContent::TooLarge => Some("document too large".to_string()),
        DocumentContent::Failed(code) => Some(code.clone()),
        DocumentContent::NotFetched => Some("not_fetched".to_string()),
    }
}

/// GH #907: commits a document's bytes to the colony blob store under
/// `timeout` (an operation timeout, hard rule 12) and returns its reference
/// with the lowercase-hex SHA-256 of the bytes filled in. A failure is one
/// fixed code for the fallback line: `no_blob_store` (the cell was spawned
/// without a store), `timeout`, `blob_write_failed` -- never the store's own
/// error text, which names a path on the host.
///
/// Takes the bytes by value: the SHA-256 over up to `max_document_bytes`
/// (20 MiB) runs in `spawn_blocking` (AGENTS.md rule 13) -- inline it held a
/// current-thread worker for 282 ms in the debug build
/// (`hashing_a_twenty_mib_document_leaves_the_worker_free`, OR-FJ-82).
pub async fn store_document(
    store: Option<&meclaw_colony::DiskBlobStore>,
    bytes: Vec<u8>,
    mime: &str,
    name: &str,
    timeout: std::time::Duration,
) -> Result<meclaw_core::BlobRef, &'static str> {
    use sha2::{Digest, Sha256};
    let Some(store) = store else {
        return Err("no_blob_store");
    };
    let write = store.write_streaming(bytes.as_slice(), mime, Some(name));
    let mut blob = match tokio::time::timeout(timeout, write).await {
        Err(_) => return Err("timeout"),
        Ok(Err(_)) => return Err("blob_write_failed"),
        Ok(Ok(blob)) => blob,
    };
    let hex = tokio::task::spawn_blocking(move || {
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    })
    .await
    .map_err(|_| "blob_write_failed")?;
    blob.sha256 = Some(hex);
    Ok(blob)
}

/// Phase-10-C T13: error reply for the inbound failure paths (W5/W6/W12, spec
/// cell-types.md l.374, commit `1dc081a`). Target = `msg.reply_to`.
/// Phase-16 W2 (ruling A1): with `reply_to` absent, `/colony/dead_letters` (the
/// READ endpoint) is NO longer the emission target — the error reply travels as a
/// normal emission to the cell's own `msg.target`; matching no out-edge, it ends
/// up loudly as `no_route` in the DLQ. Non-conversational origin: `messages[]` is
/// empty — NO user/assistant turn. The pure-sink discipline is preserved because
/// the error reply is not a conversation turn.
pub async fn emit_inbound_error(sink: &OutputSink, msg: &Message, error_code: &str, detail: &str) {
    let target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
    let content = json!({
        "header": {
            "error_code": error_code,
            "msg_type":   "proxy_inbound_error",
        },
        "messages": [],
        "meta": { "detail": detail },
    });
    let _ = sink.push(CellOutput { target, content }).await;
}
