//! Phase-12-X: blob storage for the attachments[] slot.
//! A concrete `DiskBlobStore` without a trait abstraction (variant b, directive).
//!
//! Layout (docs/meclaw-overview.md § Blob-Storage Z.1311):
//!   blobs/<uuid-v7>.<ext>            # Blob-Inhalt
//!   blobs/<uuid-v7>.<ext>.meta.json  # Sidecar (Commit-Marker)
//!
//! Write order: first the blob file (tmp→rename), then the sidecar (.meta.json)
//! as a commit marker via an atomic rename. Reader convention: a blob without a
//! sidecar
//! = ignorieren.
//!
//! Phase-12 scope: ONLY write_streaming + read_sidecar. read_body arrives in
//! Phase 13 (Cell-Konsumenten).

pub mod disk;
pub use disk::{BlobError, BlobSidecar, DiskBlobStore};
