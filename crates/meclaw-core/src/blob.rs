//! Blob-Referenz für attachments[]-Slot (Phase 12). I/O-rein.
//! sha256 ist optional und wird in Phase 12 nicht berechnet
//! (docs/meclaw-overview.md § Blob-Storage, Sidecar-Schema Z.1334).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobRef {
    pub blob_id: Uuid,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}
