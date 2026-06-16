//! On-disk blob storage with sidecar-as-commit-marker semantics.
//! Spec: docs/meclaw-overview.md § Blob-Storage (Z.1311 / Z.1334).

use meclaw_core::BlobRef;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("blob not found: {0}")]
    NotFound(Uuid),
}

/// Sidecar metadata persisted alongside each blob as `<uuid>.<ext>.meta.json`.
///
/// `schema_version = 1` is the Phase-12 baseline. `sha256` is reserved for
/// later phases and omitted in JSON when `None` (serde-skip-if-None).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobSidecar {
    pub schema_version: u32,
    pub mime_type: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// ISO-8601-ish timestamp. Phase 12 uses unix-seconds-as-string to avoid
    /// pulling chrono into the dep-graph; upgrade to RFC3339 in Phase 13+ if
    /// a real timestamp library lands.
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// On-disk blob storage. Concrete struct, no trait abstraction (Variante b).
///
/// Phase-12-Backlog: `read_sidecar` performs a linear `read_dir` scan to find
/// the extension. Acceptable for development; revisit in Phase 14 if blob
/// directories grow large (e.g., shard by uuid-prefix subdirectories or keep
/// an index).
pub struct DiskBlobStore {
    root: PathBuf,
}

impl DiskBlobStore {
    /// Open or create the blob-store root directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, BlobError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Streams a body into `<root>/<uuid-v7>.<ext>` and writes the sidecar LAST
    /// as the commit marker. Reader-contract: blob without sidecar = incomplete
    /// = ignore.
    ///
    /// Order:
    ///   1. blob → `<uuid>.<ext>.tmp` → fsync → rename to `<uuid>.<ext>`
    ///   2. sidecar → `<uuid>.<ext>.meta.json.tmp` → fsync → rename to
    ///      `<uuid>.<ext>.meta.json` (commit marker)
    pub async fn write_streaming<R>(
        &self,
        mut body: R,
        mime_type: &str,
        filename: Option<&str>,
    ) -> Result<BlobRef, BlobError>
    where
        R: tokio::io::AsyncRead + Send + Unpin,
    {
        let blob_id = Uuid::now_v7();
        let ext = mime_to_ext(mime_type);
        let blob_path = self.root.join(format!("{blob_id}.{ext}"));
        let blob_tmp = self.root.join(format!("{blob_id}.{ext}.tmp"));
        let sidecar_path = self.root.join(format!("{blob_id}.{ext}.meta.json"));
        let sidecar_tmp = self.root.join(format!("{blob_id}.{ext}.meta.json.tmp"));

        // 1. Write blob to .tmp, flush, then rename(2) to final.
        let mut blob_file = tokio::fs::File::create(&blob_tmp).await?;
        let size_bytes = tokio::io::copy(&mut body, &mut blob_file).await?;
        blob_file.sync_all().await?;
        drop(blob_file);
        tokio::fs::rename(&blob_tmp, &blob_path).await?;

        // 2. Write sidecar to .tmp, flush, then rename(2) to final = commit marker.
        let created_at = unix_seconds_string();
        let sidecar = BlobSidecar {
            schema_version: 1,
            mime_type: mime_type.to_string(),
            size_bytes,
            sha256: None,
            created_at,
            filename: filename.map(String::from),
        };
        let sidecar_bytes = serde_json::to_vec(&sidecar)?;
        let mut sidecar_file = tokio::fs::File::create(&sidecar_tmp).await?;
        tokio::io::AsyncWriteExt::write_all(&mut sidecar_file, &sidecar_bytes).await?;
        sidecar_file.sync_all().await?;
        drop(sidecar_file);
        tokio::fs::rename(&sidecar_tmp, &sidecar_path).await?;

        Ok(BlobRef {
            blob_id,
            mime_type: mime_type.to_string(),
            filename: filename.map(String::from),
            size_bytes,
            sha256: None,
        })
    }

    /// Reads the sidecar of an existing blob.
    ///
    /// Performs a `read_dir` scan to find the file matching
    /// `<blob_id>.*.meta.json`. See struct doc for the Phase-14 backlog note.
    pub async fn read_sidecar(&self, blob_id: Uuid) -> Result<BlobSidecar, BlobError> {
        let mut entries = tokio::fs::read_dir(&self.root).await?;
        let prefix = blob_id.to_string();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(&prefix) && name_str.ends_with(".meta.json") {
                let bytes = tokio::fs::read(entry.path()).await?;
                return Ok(serde_json::from_slice(&bytes)?);
            }
        }
        Err(BlobError::NotFound(blob_id))
    }

    /// Reads an offloaded JSON body blob back into a `serde_json::Value`.
    ///
    /// Honours the reader-contract (spec Z.1362): a blob counts as existing iff
    /// its sidecar exists. [`read_sidecar`] returns [`BlobError::NotFound`] when
    /// the sidecar is absent, so an orphan blob (content file without a sidecar —
    /// e.g. a crash mid-write) is transparently treated as non-existent. The blob
    /// content extension is derived from the sidecar's authoritative MIME type.
    ///
    /// Phase-13.5 A8 only offloads `application/json` UBF bodies; the result is
    /// deserialised to a `Value` so the cell-delivery boundary can hand the cell a
    /// transparent inline body (spec Z.1363).
    pub async fn read_body(&self, blob_id: Uuid) -> Result<serde_json::Value, BlobError> {
        let sidecar = self.read_sidecar(blob_id).await?;
        let ext = mime_to_ext(&sidecar.mime_type);
        let blob_path = self.root.join(format!("{blob_id}.{ext}"));
        let bytes = tokio::fs::read(&blob_path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// MIME-type → file extension. Unknown → "bin".
fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "application/json" => "json",
        "application/octet-stream" => "bin",
        _ => "bin",
    }
}

/// Unix-seconds-since-epoch as decimal string. Phase-12 placeholder until a
/// real timestamp library (chrono / time) lands in the dep-graph.
fn unix_seconds_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_body_round_trips_json_value() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let value = serde_json::json!({"system": "s", "messages": [{"origin": "user"}]});
        let bytes = serde_json::to_vec(&value).unwrap();
        let blob_ref = store
            .write_streaming(bytes.as_slice(), "application/json", None)
            .await
            .unwrap();
        let got = store.read_body(blob_ref.blob_id).await.unwrap();
        assert_eq!(got, value);
    }

    #[tokio::test]
    async fn read_body_orphan_without_sidecar_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        // Place a blob content file WITHOUT a sidecar (orphan, crash mid-write).
        let id = Uuid::now_v7();
        std::fs::write(dir.path().join(format!("{id}.json")), b"{\"x\":1}").unwrap();
        let err = store.read_body(id).await.unwrap_err();
        assert!(
            matches!(err, BlobError::NotFound(_)),
            "orphan blob without sidecar must be non-existent (reader-contract Z.1362)"
        );
    }

    #[tokio::test]
    async fn read_body_unknown_uuid_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let err = store.read_body(Uuid::now_v7()).await.unwrap_err();
        assert!(matches!(err, BlobError::NotFound(_)));
    }
}
