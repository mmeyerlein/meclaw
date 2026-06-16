//! GET /colony/trace — Phase 12-B T8.6.
//!
//! Wrappt `ColonyMsg::ReadTrace` (spawn_blocking + SQLITE_OPEN_READ_ONLY).
//! UUID-Fields (`trace_id`, `correlation_id`) werden hier inline geprueft;
//! Parse-Fehler → 400 `{"error": "bad_query", "detail": "..."}`. T13
//! systematisiert das spaeter (gemeinsamer 400-Helper); fuer T8 nur die
//! beiden UUID-Felder.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use meclaw_core::{Path, Uuid};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query-Params fuer `GET /colony/trace`.
#[derive(Debug, Deserialize)]
pub struct TraceQuery {
    /// Optional: Filter auf `trace_id` (UUID-String).
    pub trace_id: Option<String>,
    /// Optional: Filter auf `to_path`-Prefix.
    pub path_prefix: Option<String>,
    /// Optional: Filter auf `correlation_id` (UUID-String).
    pub correlation_id: Option<String>,
    /// `?error=true` → nur Rows mit `error_code` im Headers-JSON.
    pub error: Option<bool>,
    /// Optional: nur Rows mit `created_at >= since` (Unix-Sek).
    pub since: Option<i64>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler fuer `GET /colony/trace`.
pub async fn get_trace(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<TraceQuery>,
) -> impl IntoResponse {
    // UUID-Parsing inline (T13 zentralisiert).
    let trace_id = match q.trace_id.as_deref().map(Uuid::parse_str) {
        Some(Ok(u)) => Some(u),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "bad_query", "detail": "trace_id is not a valid UUID" })),
            );
        }
        None => None,
    };
    let correlation_id = match q.correlation_id.as_deref().map(Uuid::parse_str) {
        Some(Ok(u)) => Some(u),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "bad_query",
                    "detail": "correlation_id is not a valid UUID"
                })),
            );
        }
        None => None,
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadTrace {
        trace_id,
        path_prefix: q.path_prefix.as_deref().map(Path::new),
        correlation_id,
        only_error: q.error.unwrap_or(false),
        since: q.since,
        limit: clamp_limit(q.limit),
        ack: ack_tx,
    };
    if colony.inbox.send(msg).await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        );
    }
    let reply = match ack_rx.await {
        Ok(r) => r,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "colony unavailable" })),
            );
        }
    };
    (StatusCode::OK, Json(json!({ "trace": reply.entries })))
}
