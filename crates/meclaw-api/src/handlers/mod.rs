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
//! `trace`, `graph`, `mutations`, `rescan`, `ledger`.
//!
//! `ledger` (GH #267) is the one whose slot holds an **object** rather than a
//! list — it answers aggregates, so there are no `entries` to name.

pub mod dead_letters;
pub mod events;
pub mod graph;
pub mod health;
pub mod ledger;
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

/// Maximum length of an echoed correlation `tag`, in characters — the bound the
/// message door carries as `colony_dispatch::READ_TAG_MAX_CHARS`.
pub(crate) const READ_TAG_MAX_CHARS: usize = 64;

/// Clamp an echoed correlation `tag` to [`READ_TAG_MAX_CHARS`] characters.
///
/// Clamped is not dropped: a tag never touches the data, so shortening it
/// cannot change the answer, while an unbounded one is a growth hazard. The
/// doors that hand their query to `colony_dispatch` (`ledger`) get this for
/// free; `graph` and `registry` build their own JSON and clamp here.
pub(crate) fn clamp_read_tag(tag: Option<String>) -> Option<String> {
    tag.map(|t| t.chars().take(READ_TAG_MAX_CHARS).collect())
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
