//! Blob reference for the attachments[] slot (phase 12). I/O-free.
//! sha256 is optional and is not computed in phase 12
//! (docs/meclaw-overview.md § Blob storage, sidecar schema l.1334).

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
