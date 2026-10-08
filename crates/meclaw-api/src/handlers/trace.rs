//! GET /colony/trace — Phase 12-B T8.6.
//!
//! Wraps `ColonyMsg::ReadTrace` (spawn_blocking + SQLITE_OPEN_READ_ONLY).
//! The UUID fields (`trace_id`, `correlation_id`) are checked inline here;
//! a parse error yields 400 `{"error": "bad_query", "detail": "..."}`. T13
//! systematizes this later (a shared 400 helper); for T8 only these two UUID
//! fields.

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

/// Query params for `GET /colony/trace`.
#[derive(Debug, Deserialize)]
pub struct TraceQuery {
    /// Optional: filter on `trace_id` (UUID string).
    pub trace_id: Option<String>,
    /// Optional: filter on the `to_path` prefix.
    pub path_prefix: Option<String>,
    /// Optional: filter on `correlation_id` (UUID string).
    pub correlation_id: Option<String>,
    /// `?error=true` → only rows with an `error_code` in the headers JSON.
    pub error: Option<bool>,
    /// Optional: only rows with `created_at >= since` (Unix seconds).
    pub since: Option<i64>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
    /// GH #1099: the `log_seq` of the caller's previous reply. With
    /// `wait_ms`, the read is held until the colony log commits past it.
    pub after_seq: Option<u64>,
    /// GH #1099: how long the read may be held for `after_seq`, in ms;
    /// clamped to [`TRACE_WAIT_MAX_MS`]. Without `after_seq` the read is
    /// answered at once.
    pub wait_ms: Option<u64>,
}

/// GH #1099: the longest a waiting trace read is held, in ms. Half of the 60 s
/// idle timeout common reverse proxies apply, so a held request is answered
/// (with what the log holds, maybe unchanged) before a proxy in front of the
/// API cuts it; a caller that needs longer simply asks again with the
/// `log_seq` it got.
pub const TRACE_WAIT_MAX_MS: u64 = 30_000;

/// Handler for `GET /colony/trace`.
pub async fn get_trace(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<TraceQuery>,
) -> impl IntoResponse {
    // UUID parsing inline (T13 centralizes it).
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
        wait: q
            .after_seq
            .map(|after_seq| meclaw_colony::api_dto::TraceWait {
                after_seq,
                max: std::time::Duration::from_millis(
                    q.wait_ms.unwrap_or(0).min(TRACE_WAIT_MAX_MS),
                ),
            }),
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
    (
        StatusCode::OK,
        Json(json!({ "trace": reply.entries, "log_seq": reply.log_seq })),
    )
}
