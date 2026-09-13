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
pub struct DiskBlobStore {
    root: PathBuf,
    max_recursion_depth: u32,
}

/// Substrate default for `blob_max_recursion_depth` (spec § Blob references
/// are universal). Mirrors [`crate::ColonyConfig`]'s default so a store built
/// without an explicit override behaves like a colony without a `colony.json`.
pub const DEFAULT_BLOB_MAX_RECURSION_DEPTH: u32 = 64;

impl DiskBlobStore {
    /// Open or create the blob-store root directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, BlobError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            max_recursion_depth: DEFAULT_BLOB_MAX_RECURSION_DEPTH,
        })
    }

    /// GH #19: set the hard bound for recursive in-message pointer resolution.
    ///
    /// The bound rides on the store rather than on `spawn_cell`, because the
    /// store handle is ALREADY threaded to exactly the one place that resolves
    /// pointers (the cell-delivery boundary) — and adding an eleventh parameter
    /// to the `CellFactory` trait would touch every cell factory in the
    /// workspace to move one number. Wired from `colony.json
    /// blob_max_recursion_depth` where the store is constructed.
    #[must_use]
    pub fn with_max_recursion_depth(mut self, depth: u32) -> Self {
        self.max_recursion_depth = depth;
        self
    }

    /// The configured bound for recursive in-message pointer resolution.
    pub fn max_recursion_depth(&self) -> u32 {
        self.max_recursion_depth
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
    /// GH #683: the file is asked for by name, one probe per extension in
    /// [`SIDECAR_EXTS`], most common first. The directory scan this replaced
    /// read every entry of `blobs/` for every blob a page resolved — measured
    /// 1558 entries times 100 blobs per page — and it would open any stray
    /// file that merely shared the id's prefix. A miss on every name is
    /// [`BlobError::NotFound`], as before.
    pub async fn read_sidecar(&self, blob_id: Uuid) -> Result<BlobSidecar, BlobError> {
        for ext in SIDECAR_EXTS {
            let path = self.root.join(format!("{blob_id}.{ext}.meta.json"));
            match tokio::fs::read(&path).await {
                Ok(bytes) => return Ok(serde_json::from_slice(&bytes)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
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
        let (bytes, _sidecar) = self.read_bytes(blob_id).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Reads a blob's raw content plus its sidecar (GH #87).
    ///
    /// The read path for `attachments[]`: an attachment is a file of arbitrary
    /// type, so the consumer needs the bytes and the authoritative MIME type,
    /// not a deserialised `Value`. Honours the same reader-contract as
    /// [`read_body`](Self::read_body) — the sidecar is the commit marker, so an
    /// orphan content file (crash mid-write) reads as
    /// [`BlobError::NotFound`].
    pub async fn read_bytes(&self, blob_id: Uuid) -> Result<(Vec<u8>, BlobSidecar), BlobError> {
        let sidecar = self.read_sidecar(blob_id).await?;
        let ext = mime_to_ext(&sidecar.mime_type);
        let blob_path = self.root.join(format!("{blob_id}.{ext}"));
        let bytes = tokio::fs::read(&blob_path).await?;
        Ok((bytes, sidecar))
    }
}

/// Every extension `mime_to_ext` can produce. A sidecar is named
/// `<blob_id>.<ext>.meta.json`, and the set of `<ext>` is closed, so the file is
/// found by asking for it rather than by reading the directory it lives in.
/// Ordered by frequency: the UBF offload writes `json`, everything unknown is
/// `bin`.
const SIDECAR_EXTS: [&str; 6] = ["json", "bin", "txt", "png", "jpg", "pdf"];

/// MIME-type → file extension. Unknown → "bin". Every arm's result is a member
/// of [`SIDECAR_EXTS`]; a new arm here is a new entry there.
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

    // ── GH #87: raw-bytes read for the attachments[] consumer ──────────────

    #[tokio::test]
    async fn read_bytes_round_trips_content_and_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let png = b"\x89PNG\r\n\x1a\nnot-really-a-png";
        let blob_ref = store
            .write_streaming(png.as_slice(), "image/png", Some("shot.png"))
            .await
            .unwrap();
        let (bytes, sidecar) = store.read_bytes(blob_ref.blob_id).await.unwrap();
        assert_eq!(bytes, png);
        assert_eq!(sidecar.mime_type, "image/png");
        assert_eq!(sidecar.filename.as_deref(), Some("shot.png"));
    }

    #[tokio::test]
    async fn read_bytes_orphan_without_sidecar_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let id = Uuid::now_v7();
        std::fs::write(dir.path().join(format!("{id}.png")), b"orphan").unwrap();
        let err = store.read_bytes(id).await.unwrap_err();
        assert!(
            matches!(err, BlobError::NotFound(_)),
            "orphan blob without sidecar must be non-existent (reader-contract Z.1362)"
        );
    }

    #[tokio::test]
    async fn read_bytes_unknown_uuid_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let err = store.read_bytes(Uuid::now_v7()).await.unwrap_err();
        assert!(matches!(err, BlobError::NotFound(_)));
    }

    // ── GH #683: a sidecar is found by name, not by reading the directory ───

    /// Every MIME type the store can write comes back through `read_sidecar`.
    /// The set of extensions is closed (`mime_to_ext`), so every one of them
    /// is a name the reader can ask for.
    #[tokio::test]
    async fn every_written_mime_type_is_found_by_its_sidecar_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        for mime in [
            "application/json",
            "application/octet-stream",
            "text/plain",
            "image/png",
            "image/jpeg",
            "application/pdf",
            "video/unknown-goes-to-bin",
        ] {
            let blob_ref = store
                .write_streaming(b"payload".as_slice(), mime, None)
                .await
                .unwrap();
            let sidecar = store.read_sidecar(blob_ref.blob_id).await.unwrap();
            assert_eq!(sidecar.mime_type, mime, "the sidecar of {mime} is found");
        }
    }

    /// The reader-contract is the sidecar, not the content: a sidecar whose
    /// content file is gone is still found by `read_sidecar` (and `read_bytes`
    /// then fails on the content, not on existence).
    #[tokio::test]
    async fn a_sidecar_without_its_content_file_is_still_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let blob_ref = store
            .write_streaming(b"{}".as_slice(), "application/json", None)
            .await
            .unwrap();
        std::fs::remove_file(dir.path().join(format!("{}.json", blob_ref.blob_id))).unwrap();
        let sidecar = store.read_sidecar(blob_ref.blob_id).await.unwrap();
        assert_eq!(sidecar.mime_type, "application/json");
    }

    /// A file that merely starts with the blob id and ends in `.meta.json` is
    /// not a sidecar. The directory scan this replaced would have opened it
    /// and failed on its content; asking for the sidecar by name never sees
    /// it, and the blob is what it is: absent.
    #[tokio::test]
    async fn a_stray_file_beside_the_blob_id_is_not_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let id = Uuid::now_v7();
        std::fs::write(
            dir.path().join(format!("{id}.stray.meta.json")),
            b"not a sidecar",
        )
        .unwrap();
        let err = store.read_sidecar(id).await.unwrap_err();
        assert!(
            matches!(err, BlobError::NotFound(_)),
            "a stray file is no sidecar; got {err:?}"
        );
    }

    /// A stray file with the same prefix beside a REAL blob does not get in
    /// the way of the real one, whatever order the directory lists them in.
    #[tokio::test]
    async fn a_stray_file_does_not_shadow_a_real_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path()).unwrap();
        let blob_ref = store
            .write_streaming(b"{\"x\":1}".as_slice(), "application/json", None)
            .await
            .unwrap();
        let id = blob_ref.blob_id;
        std::fs::write(
            dir.path().join(format!("{id}.aaa.meta.json")),
            b"not a sidecar",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(format!("{id}.zzz.meta.json")),
            b"not a sidecar",
        )
        .unwrap();
        let got = store.read_body(id).await.unwrap();
        assert_eq!(got, serde_json::json!({"x": 1}));
    }
}
