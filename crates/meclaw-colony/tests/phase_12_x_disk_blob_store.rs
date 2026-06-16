//! Phase-12-X T16: DiskBlobStore::write_streaming creates blob file + sidecar,
//! rename-Order: blob first, sidecar as commit-marker LAST.

use meclaw_colony::blob::DiskBlobStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_streaming_creates_blob_and_sidecar_with_correct_meta() {
    let td = tempfile::TempDir::new().unwrap();
    let store = DiskBlobStore::new(td.path()).unwrap();

    let payload = b"hello world".to_vec();
    let payload_cursor = std::io::Cursor::new(payload.clone());
    let blob_ref = store
        .write_streaming(payload_cursor, "text/plain", Some("hello.txt"))
        .await
        .unwrap();

    assert_eq!(blob_ref.mime_type, "text/plain");
    assert_eq!(blob_ref.filename, Some("hello.txt".into()));
    assert_eq!(blob_ref.size_bytes, 11);
    assert!(blob_ref.sha256.is_none());

    // Verify on-disk layout
    let blob_path = td.path().join(format!("{}.txt", blob_ref.blob_id));
    let sidecar_path = td
        .path()
        .join(format!("{}.txt.meta.json", blob_ref.blob_id));
    assert!(blob_path.exists());
    assert!(sidecar_path.exists());

    let blob_bytes = tokio::fs::read(&blob_path).await.unwrap();
    assert_eq!(blob_bytes, b"hello world");

    let sidecar_json: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&sidecar_path).await.unwrap()).unwrap();
    assert_eq!(sidecar_json["schema_version"], 1);
    assert_eq!(sidecar_json["mime_type"], "text/plain");
    assert_eq!(sidecar_json["size_bytes"], 11);
    assert_eq!(sidecar_json["filename"], "hello.txt");
    assert!(sidecar_json["created_at"].is_string());
    // sha256 NOT in JSON (Phase 12 doesn't compute it; serde-skip-if-None)
    assert!(sidecar_json.get("sha256").is_none() || sidecar_json["sha256"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_sidecar_returns_metadata() {
    let td = tempfile::TempDir::new().unwrap();
    let store = DiskBlobStore::new(td.path()).unwrap();

    let payload = b"data".to_vec();
    let blob_ref = store
        .write_streaming(
            std::io::Cursor::new(payload),
            "application/octet-stream",
            None, // no filename
        )
        .await
        .unwrap();

    let sidecar = store.read_sidecar(blob_ref.blob_id).await.unwrap();
    assert_eq!(sidecar.mime_type, "application/octet-stream");
    assert_eq!(sidecar.size_bytes, 4);
    assert!(sidecar.filename.is_none());
}
