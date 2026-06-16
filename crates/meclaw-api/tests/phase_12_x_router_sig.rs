//! Phase-12-X T17: build_router accepts blob_store as second param.
//! Sanity-test that compiles only when the new signature is in place.

use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn build_router_accepts_blob_store_param() {
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: tx,
        templates_root: std::path::PathBuf::new(),
    });
    let td = tempfile::TempDir::new().unwrap();
    let blob_store = Arc::new(meclaw_colony::blob::DiskBlobStore::new(td.path()).unwrap());
    let _router =
        meclaw_api::router::build_router(colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
}
