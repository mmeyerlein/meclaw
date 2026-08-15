//! Phase-3a message: ten envelope fields per spec § Message model.
//!
//! Phase 3b adds Universal-Body-Format JSON-Schema validation; the body
//! itself remains `Body::Inline` in 3a.

use crate::body::Body;
use crate::path::Path;
use uuid::Uuid;

/// Default TTL for source messages. Hardcoded in 3a; configurable via
/// `colony.json::message_default_ttl` from Phase 4 onward (spec recommendation: 64).
pub const MESSAGE_DEFAULT_TTL: u32 = 64;

/// Routable message per spec § Message model. All ten envelope fields:
/// id, trace_id, parent_message_id, correlation_id, target, reply_to, ttl,
/// headers, body, created_at.
#[derive(Debug, Clone)]
pub struct Message {
    /// v7 UUID, time-sorted, set by colony for every new message.
    pub id: Uuid,
    /// Root message ID, constant across the entire trace.
    pub trace_id: Uuid,
    /// `None` on source messages; otherwise the ID of the consumed inbound message.
    pub parent_message_id: Option<Uuid>,
    /// Optional, for req/resp pairing (the cell sets it via the header slot, colony propagates).
    pub correlation_id: Option<Uuid>,
    /// Address (absolute, or relative before resolution).
    pub target: Path,
    /// `None` on source messages; otherwise the sender's absolute path.
    pub reply_to: Option<Path>,
    /// Routing-step-based hop counter, decremented per colony routing decision.
    pub ttl: u32,
    /// Routing metadata in two compartments (`context` persistent, `hop` cell output).
    pub headers: crate::Headers,
    /// Content.
    pub body: Body,
    /// Unix seconds (SystemTime → as_secs() as i64), not milliseconds.
    pub created_at: i64,
}

/// Unix seconds since the epoch (spec § Message model l. 788: i64, not ms).
pub fn now_unix_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX_EPOCH")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_default_ttl_is_64() {
        assert_eq!(MESSAGE_DEFAULT_TTL, 64);
    }

    #[test]
    fn now_unix_seconds_is_positive_and_recent() {
        let t = now_unix_seconds();
        assert!(t > 1_700_000_000, "must be after 2023-11-14");
        assert!(t < 4_000_000_000, "must be before 2096");
    }
}
