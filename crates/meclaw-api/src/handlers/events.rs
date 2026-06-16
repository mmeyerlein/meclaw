//! Phase-14-Defer: /colony/events WebSocket-Broadcast braucht eigenen
//! Design-Pass (Broadcast-Mechanik, Slow-Consumer-Drop-Policy, Event-Schema)
//! und berührt den await-freien handle_cell_died-Korridor.
//! Spec: docs/meclaw-overview.md § API (HTTP), WebSocket /events ab Phase 14.

use axum::Json;
use axum::http::StatusCode;

/// GET /colony/events — gibt 501 Not Implemented zurück. Endpunkt ist bekannt
/// (Spec Z.406+1652), aber bewusst deferred. KEIN axum-404 — wir signalisieren
/// dem Client, dass das Feature existieren wird.
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
