//! Phase-14 defer: the /colony/events WebSocket broadcast needs its own design
//! pass (broadcast mechanics, slow-consumer drop policy, event schema) and
//! touches the await-free handle_cell_died corridor.
//! Spec: docs/meclaw-overview.md § API (HTTP), WebSocket /events from phase 14.

use axum::Json;
use axum::http::StatusCode;

/// GET /colony/events — returns 501 Not Implemented. The endpoint is known
/// (spec l.406+1652) but deliberately deferred. NOT an axum 404 — we signal to
/// the client that the feature is going to exist.
pub async fn get_events() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "deferred",
            "phase": 14,
            "spec": "docs/meclaw-overview.md § API (HTTP), WebSocket /events ab Phase 14"
        })),
    )
}
