//! Phase-14 defer: the /colony/events WebSocket broadcast needs its own design
//! pass (broadcast mechanics, slow-consumer drop policy, event schema) and
//! touches the await-free handle_cell_died corridor.
//! Spec: docs/meclaw-overview.md § API (HTTP), WebSocket /events from phase 14.

use axum::Json;
use axum::http::StatusCode;

/// GET /colony/events — returns 501 Not Implemented. The endpoint is known
/// (spec l.406+1652) but deliberately deferred. NOT an axum 404 — we signal to
/// the client that the feature is going to exist.
///
/// The body says `{"error": "deferred"}` and stops there (W13 hardening). It
/// used to also ship an internal phase number and a path into the repo's own
/// `docs/` tree — coordinates that mean nothing to an HTTP client, cannot be
/// followed from outside, and would have to be maintained against a numbering
/// scheme the public surface does not otherwise expose. The one fact a caller
/// can act on is that the endpoint exists and does not answer yet.
pub async fn get_events() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({ "error": "deferred" })),
    )
}
