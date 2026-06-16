//! Phase-3a message: ten envelope fields per spec § Message-Modell.
//!
//! Phase 3b adds Universal-Body-Format JSON-Schema validation; the body
//! itself remains `Body::Inline` in 3a.

use crate::body::Body;
use crate::path::Path;
use uuid::Uuid;

/// Default TTL for source messages. Hardcoded in 3a; configurable via
/// `colony.json::message_default_ttl` from Phase 4 onward (spec recommendation: 64).
pub const MESSAGE_DEFAULT_TTL: u32 = 64;

/// Routable message per spec § Message-Modell. All ten envelope fields:
/// id, trace_id, parent_message_id, correlation_id, target, reply_to, ttl,
/// headers, body, created_at.
#[derive(Debug, Clone)]
pub struct Message {
    /// v7 UUID, zeitsortiert, gesetzt von Colony bei jeder neuen Message.
    pub id: Uuid,
    /// Root-Message-ID, konstant über den gesamten Trace.
    pub trace_id: Uuid,
    /// `None` bei Source-Messages; sonst ID der konsumierten Eingangs-Message.
    pub parent_message_id: Option<Uuid>,
    /// Optional, für req/resp-Paarung (Cell setzt via header-Slot, Colony propagiert).
    pub correlation_id: Option<Uuid>,
    /// Adresse (absolut oder relativ-vor-Resolution).
    pub target: Path,
    /// `None` bei Source-Messages; sonst absoluter Pfad des Senders.
    pub reply_to: Option<Path>,
    /// Routing-Step-basierter Hop-Counter, dekrementiert pro Colony-Routing-Entscheidung.
    pub ttl: u32,
    /// Routing-Metadaten in zwei Fächern (`context` persistent, `hop` Cell-Output).
    pub headers: crate::Headers,
    /// Inhalt.
    pub body: Body,
    /// Unix-Sekunden (SystemTime → as_secs() as i64), nicht Millisekunden.
    pub created_at: i64,
}

/// Unix-Sekunden seit Epoche (spec § Message-Modell Z. 788 — i64, nicht ms).
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
