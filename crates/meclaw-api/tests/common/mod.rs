//! Shared helpers for `meclaw-api` integration tests.
//!
//! Phase-12-X T17: `build_router` requires `Arc<DiskBlobStore>` as second param.
//! Tests that don't care about uploads use [`test_blob_store`] which creates a
//! short-lived store in a `TempDir`. The caller MUST keep the returned
//! `TempDir` alive for the duration of the test (Drop removes the dir).

#![allow(dead_code)]

use meclaw_colony::blob::DiskBlobStore;
use std::sync::Arc;

/// Returns a fresh `DiskBlobStore` rooted in a new `TempDir`.
///
/// The `TempDir` must be kept alive by the test — drop it at the end of the
/// scope. Typical use:
///
/// ```ignore
/// let (blob_store, _blob_td) = common::test_blob_store();
/// let app = build_router(api_colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
/// ```
pub fn test_blob_store() -> (Arc<DiskBlobStore>, tempfile::TempDir) {
    let td = tempfile::TempDir::new().expect("create blob TempDir");
    let store = DiskBlobStore::new(td.path()).expect("create blob store");
    (Arc::new(store), td)
}
