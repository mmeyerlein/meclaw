//! HTTP-Handler für `/colony/*` — Phase 12-B Task 8.
//!
//! Jeder Handler folgt dem gleichen Pattern:
//! 1. axum extracts `State(Arc<ColonyHandle>)` + `Query<EndpointParams>`.
//! 2. Maps Query → `ColonyMsg::Read*` Variante mit `oneshot::channel()` für ack.
//! 3. `colony.inbox.send(msg).await` → bei Fehler: 503 `{error: "colony unavailable"}`.
//! 4. `ack_rx.await` → bei Fehler: 503 dito.
//! 5. Returnt `(StatusCode::OK, Json({"<slot>": reply.entries}))`.
//!
//! Slot-Namen pro Endpoint (Spec Z.410): `registry`, `dead_letters`, `templates`,
//! `trace`, `graph`, `mutations`, `rescan`.

pub mod dead_letters;
pub mod events;
pub mod graph;
pub mod messages;
pub mod mutations;
pub mod registry;
pub mod templates;
pub mod trace;

/// Read-Limit-Clamp gemäß Spec Z.412: Default 100, Hard-Cap 1000.
/// Wird in jedem GET-Read-Handler angewendet bevor der `ColonyMsg::Read*`
/// abgesetzt wird. Die Inbox-Arms clampen zusätzlich `1..=1000` als Defense-
/// in-Depth.
pub(crate) fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(100).clamp(1, 1000)
}

#[cfg(test)]
mod tests {
    use super::clamp_limit;

    #[test]
    fn clamp_limit_default_is_100() {
        assert_eq!(clamp_limit(None), 100);
    }

    #[test]
    fn clamp_limit_caps_at_1000() {
        assert_eq!(clamp_limit(Some(10_000)), 1000);
    }

    #[test]
    fn clamp_limit_floor_is_1() {
        assert_eq!(clamp_limit(Some(0)), 1);
    }

    #[test]
    fn clamp_limit_passes_through_in_range() {
        assert_eq!(clamp_limit(Some(42)), 42);
    }
}
