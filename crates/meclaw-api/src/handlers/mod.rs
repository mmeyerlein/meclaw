//! HTTP handlers for `/colony/*` — phase 12-B task 8.
//!
//! Every handler follows the same pattern:
//! 1. axum extracts `State(Arc<ColonyHandle>)` + `Query<EndpointParams>`.
//! 2. Maps the query to a `ColonyMsg::Read*` variant with a `oneshot::channel()` for the ack.
//! 3. `colony.inbox.send(msg).await` → on error: 503 `{error: "colony unavailable"}`.
//! 4. `ack_rx.await` → on error: 503 likewise.
//! 5. Returns `(StatusCode::OK, Json({"<slot>": reply.entries}))`.
//!
//! Slot names per endpoint (spec l.410): `registry`, `dead_letters`, `templates`,
//! `trace`, `graph`, `mutations`, `rescan`.

pub mod dead_letters;
pub mod events;
pub mod graph;
pub mod health;
pub mod message_log;
pub mod messages;
pub mod mutations;
pub mod registry;
pub mod templates;
pub mod trace;

/// Read-limit clamp per spec l.412: default 100, hard cap 1000.
/// Applied in every GET read handler before the `ColonyMsg::Read*` is issued.
/// The inbox arms additionally clamp to `1..=1000` as defense in depth.
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
