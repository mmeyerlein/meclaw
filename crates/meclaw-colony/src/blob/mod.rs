//! Phase-12-X: Blob-Storage für attachments[]-Slot.
//! Concrete `DiskBlobStore` ohne Trait-Abstraktion (Variante b, directive).
//!
//! Layout (docs/meclaw-overview.md § Blob-Storage Z.1311):
//!   blobs/<uuid-v7>.<ext>            # Blob-Inhalt
//!   blobs/<uuid-v7>.<ext>.meta.json  # Sidecar (Commit-Marker)
//!
//! Schreib-Reihenfolge: erst Blob-Datei (tmp→rename), dann Sidecar (.meta.json)
//! als Commit-Marker per atomarem rename. Reader-Konvention: Blob ohne Sidecar
//! = ignorieren.
//!
//! Phase-12-Scope: NUR write_streaming + read_sidecar. read_body kommt in
//! Phase 13 (Cell-Konsumenten).

pub mod disk;
pub use disk::{BlobError, BlobSidecar, DiskBlobStore};
