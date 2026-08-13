//! GH #87: the read handle a declared `attachments[]` consumer receives.
//!
//! Spec: `docs/meclaw-overview.md` § `attachments[]`-Schema (English:
//! `docs/meclaw-overview.en.md` § "`attachments[]` schema"). The owner of
//! `attachments[]` resolution is the **consuming cell**: the substrate resolves
//! only pointers whose target is a body document (`messages_id`/`text_id`); an
//! attachment is a file of arbitrary type and size and never gets inlined into
//! the JSON body. What the cell was missing is a way to read it at all — this
//! module is that way.
//!
//! Shape of the wiring, and why this one:
//!
//! * **No eleventh `CellFactory` parameter.** The blob store already rides into
//!   `spawn_cell` (it is the handle the cell-delivery boundary holds anyway,
//!   see the GH #19 receipt on `DiskBlobStore::with_max_recursion_depth`), and
//!   the contract already rides in as [`ContractView`]. The handle is therefore
//!   a *function of two things the factory already has*, not new plumbing.
//! * **Read-only by construction.** [`AttachmentReader`] wraps the store but
//!   exposes exactly one operation, [`AttachmentReader::read`]. A cell cannot
//!   write, delete or enumerate blobs through it.
//! * **Declaration-gated.** [`AttachmentReader::for_contract`] returns `None`
//!   unless the cell declares `consumes.body.attachments`. A cell that does not
//!   declare consumption gets no handle and behaves exactly as before.
//! * **Every read carries an operation timeout** (`CLAUDE.md` hard rule 12,
//!   spec § Timeouts, concept A): the caller passes the deadline from its own
//!   params, and `cell.message_timeout` stays the backstop it is meant to be.

use super::disk::{BlobError, DiskBlobStore};
use crate::ContractView;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

/// The body slot whose declaration hands a cell the reader.
const ATTACHMENTS_SLOT: &str = "attachments";

/// One attachment, read from the blob store.
#[derive(Debug, Clone)]
pub struct AttachmentBytes {
    /// Raw file content.
    pub bytes: Vec<u8>,
    /// Authoritative MIME type from the sidecar (NOT the one the message
    /// claimed — the sidecar is what the store committed).
    pub mime_type: String,
}

/// Why an attachment read did not produce bytes.
///
/// All three are **cell-level** conditions: the consuming cell turns them into
/// a regular error message and finishes `handle()` normally. None of them is a
/// delivery-boundary dead letter — the message was delivered correctly, it is
/// the attachment behind it that is unreadable.
#[derive(Debug, Error)]
pub enum AttachmentReadError {
    /// The operation timeout elapsed before the store answered (concept A).
    #[error("attachment {0}: read timed out after {1} ms")]
    Timeout(Uuid, u128),
    /// No committed blob for this id (missing, or content without a sidecar).
    #[error("attachment {0}: blob not found")]
    NotFound(Uuid),
    /// The store failed for any other reason (I/O, malformed sidecar).
    #[error("attachment {0}: {1}")]
    Store(Uuid, String),
}

/// Read-only handle on the colony's blob store, held by a cell that declared
/// `consumes.body.attachments`.
///
/// Cheap to clone (one `Arc`), which is what the factory's wake/respawn
/// closures need.
#[derive(Clone)]
pub struct AttachmentReader {
    store: Arc<DiskBlobStore>,
}

impl std::fmt::Debug for AttachmentReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AttachmentReader")
    }
}

impl AttachmentReader {
    /// Hand out a reader iff the cell declared `consumes.body.attachments`
    /// AND a store is wired.
    ///
    /// `None` on either miss, and the two misses are deliberately
    /// indistinguishable to the cell: a cell without a handle does not consume
    /// attachments, whatever the reason. Declaring is binding (config.md
    /// § `consumes`), so declaration is read off the compiled required-key
    /// projection.
    pub fn for_contract(
        contract: &ContractView,
        store: Option<Arc<DiskBlobStore>>,
    ) -> Option<Self> {
        let declared = contract
            .consumes
            .as_ref()
            .is_some_and(|c| c.declares_body(ATTACHMENTS_SLOT));
        if !declared {
            return None;
        }
        store.map(|store| Self { store })
    }

    /// Read one attachment, bounded by `timeout` (operation timeout, concept A).
    ///
    /// The whole store operation — sidecar lookup and content read — sits
    /// inside one `tokio::time::timeout`, so a store that never answers costs
    /// the cell `timeout`, not `cell.message_timeout` plus a restart.
    pub async fn read(
        &self,
        blob_id: Uuid,
        timeout: Duration,
    ) -> Result<AttachmentBytes, AttachmentReadError> {
        match tokio::time::timeout(timeout, self.store.read_bytes(blob_id)).await {
            Err(_elapsed) => Err(AttachmentReadError::Timeout(blob_id, timeout.as_millis())),
            Ok(Err(BlobError::NotFound(id))) => Err(AttachmentReadError::NotFound(id)),
            Ok(Err(e)) => Err(AttachmentReadError::Store(blob_id, e.to_string())),
            Ok(Ok((bytes, sidecar))) => Ok(AttachmentBytes {
                bytes,
                mime_type: sidecar.mime_type,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{CompiledConsumes, ConsumesBlock};

    fn contract_with_consumes(json: serde_json::Value) -> ContractView {
        let block: ConsumesBlock = serde_json::from_value(json).unwrap();
        let compiled = CompiledConsumes::compile(&block);
        ContractView {
            consumes: Some(Arc::new(compiled)),
            ..ContractView::default()
        }
    }

    fn store(dir: &std::path::Path) -> Arc<DiskBlobStore> {
        Arc::new(DiskBlobStore::new(dir).unwrap())
    }

    #[test]
    fn declared_consumption_yields_a_handle() {
        let dir = tempfile::tempdir().unwrap();
        let contract =
            contract_with_consumes(serde_json::json!({"body": {"attachments": {"type": "array"}}}));
        assert!(AttachmentReader::for_contract(&contract, Some(store(dir.path()))).is_some());
    }

    #[test]
    fn undeclared_consumption_yields_no_handle() {
        let dir = tempfile::tempdir().unwrap();
        let contract =
            contract_with_consumes(serde_json::json!({"body": {"messages": {"type": "array"}}}));
        assert!(AttachmentReader::for_contract(&contract, Some(store(dir.path()))).is_none());
    }

    #[test]
    fn empty_contract_yields_no_handle() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            AttachmentReader::for_contract(&ContractView::default(), Some(store(dir.path())))
                .is_none()
        );
    }

    #[test]
    fn declared_consumption_without_a_store_yields_no_handle() {
        let contract =
            contract_with_consumes(serde_json::json!({"body": {"attachments": {"type": "array"}}}));
        assert!(AttachmentReader::for_contract(&contract, None).is_none());
    }

    #[tokio::test]
    async fn read_returns_bytes_and_sidecar_mime() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let png = b"\x89PNG\r\n\x1a\npayload";
        let blob_ref = s
            .write_streaming(png.as_slice(), "image/png", None)
            .await
            .unwrap();
        let reader = AttachmentReader { store: s };
        let got = reader
            .read(blob_ref.blob_id, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(got.bytes, png);
        assert_eq!(got.mime_type, "image/png");
    }

    /// A store read that never answers must cost the cell its own operation
    /// timeout (concept A), NOT `cell.message_timeout` plus a restart
    /// (concept B). The hang is real: the blob content path is a FIFO with no
    /// writer, so `open(2)` blocks indefinitely.
    ///
    /// The writer end is opened right after the read returns and before the
    /// assertion — that releases the blocked `open(2)` so runtime teardown
    /// (which waits for blocking tasks) cannot hang, whatever the assertion
    /// does. No timing discriminator is involved: the alternative to the
    /// 100 ms timeout is "forever".
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn read_of_a_hanging_store_hits_the_operation_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::now_v7();
        // Committed sidecar (image/png → content extension `png`) …
        let sidecar = serde_json::json!({
            "schema_version": 1, "mime_type": "image/png",
            "size_bytes": 4, "created_at": "0"
        });
        std::fs::write(
            dir.path().join(format!("{id}.png.meta.json")),
            serde_json::to_vec(&sidecar).unwrap(),
        )
        .unwrap();
        // … whose content path is a reader-blocking FIFO.
        let fifo = dir.path().join(format!("{id}.png"));
        let mkfifo = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs");
        assert!(mkfifo.success(), "mkfifo failed");

        let reader = AttachmentReader {
            store: store(dir.path()),
        };
        let started = std::time::Instant::now();
        let result = reader.read(id, Duration::from_millis(100)).await;
        let elapsed = started.elapsed();

        // Release the blocked open(2) before asserting.
        drop(std::fs::OpenOptions::new().write(true).open(&fifo));

        match result {
            Err(AttachmentReadError::Timeout(got, ms)) => {
                assert_eq!(got, id);
                assert_eq!(ms, 100);
            }
            other => panic!("expected an operation timeout, got {other:?}"),
        }
        assert!(
            elapsed < Duration::from_secs(20),
            "the operation timeout must return long before any message_timeout \
             backstop could apply, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn read_of_an_unknown_blob_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let reader = AttachmentReader {
            store: store(dir.path()),
        };
        let id = Uuid::now_v7();
        let err = reader.read(id, Duration::from_secs(5)).await.unwrap_err();
        assert!(matches!(err, AttachmentReadError::NotFound(got) if got == id));
        assert!(err.to_string().contains(&id.to_string()), "{err}");
    }
}
