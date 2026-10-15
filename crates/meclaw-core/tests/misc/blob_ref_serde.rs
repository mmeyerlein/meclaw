//! Phase-12-X T15: BlobRef is serde-stable and I/O-free.
//! sha256 is Option (phase-12 spec l.1334: not computed in phase 12).

use meclaw_core::BlobRef;
use uuid::Uuid;

#[test]
fn blob_ref_serializes_with_optional_sha256_omitted() {
    let r = BlobRef {
        blob_id: Uuid::from_u128(1),
        mime_type: "application/pdf".into(),
        filename: Some("report.pdf".into()),
        size_bytes: 12345,
        sha256: None,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains("\"blob_id\""));
    assert!(s.contains("\"mime_type\":\"application/pdf\""));
    assert!(s.contains("\"filename\":\"report.pdf\""));
    assert!(s.contains("\"size_bytes\":12345"));
    // None → skip via #[serde(skip_serializing_if = "Option::is_none")]
    assert!(!s.contains("sha256"));
}

#[test]
fn blob_ref_round_trips_with_sha256_present() {
    let r = BlobRef {
        blob_id: Uuid::from_u128(2),
        mime_type: "text/plain".into(),
        filename: None,
        size_bytes: 100,
        sha256: Some("abc123".into()),
    };
    let s = serde_json::to_string(&r).unwrap();
    let parsed: BlobRef = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed.blob_id, r.blob_id);
    assert_eq!(parsed.mime_type, r.mime_type);
    assert_eq!(parsed.filename, r.filename);
    assert_eq!(parsed.size_bytes, r.size_bytes);
    assert_eq!(parsed.sha256, r.sha256);
}
