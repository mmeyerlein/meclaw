//! GET /colony/dead_letters — phase 12-B T8.2 (pure read).
//! DELETE /colony/dead_letters — phase 12-B T8.3 (drain).
//!
//! The pure read uses `ColonyMsg::ReadDeadLetters` (in-memory queue,
//! non-destructive). The drain uses the existing `ColonyMsg::DrainDeadLetters` —
//! no new variant needed. Both endpoints return the JSON slot `dead_letters`.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::DeadLetterDto;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query params for `GET /colony/dead_letters`.
#[derive(Debug, Deserialize)]
pub struct DeadLettersQuery {
    /// Optional: only entries with `created_at >= since` (Unix seconds).
    /// Currently a no-op — dead letters carry no timestamp (phase-14 backlog).
    pub since: Option<i64>,
    /// Optional: exact match on the canonical `error_code` string.
    pub error_code: Option<String>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler for `GET /colony/dead_letters` (pure read).
pub async fn get_dead_letters(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<DeadLettersQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadDeadLetters {
        since: q.since,
        error_code: q.error_code,
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
    (
        StatusCode::OK,
        Json(json!({ "dead_letters": reply.entries })),
    )
}

/// Handler for `DELETE /colony/dead_letters` (drain via `ColonyMsg::DrainDeadLetters`).
///
/// Returns the drained entries as the JSON slot `dead_letters`, mapped onto
/// `DeadLetterDto` (same schema as the pure read). Status 200 even when the
/// queue was empty — idempotence.
pub async fn delete_dead_letters(State(colony): State<Arc<ColonyHandle>>) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    if colony
        .inbox
        .send(ColonyMsg::DrainDeadLetters { ack: ack_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        );
    }
    let drained = match ack_rx.await {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "colony unavailable" })),
            );
        }
    };
    let entries: Vec<DeadLetterDto> = drained
        .into_iter()
        .map(|dl| DeadLetterDto {
            sender_path: dl.sender_path.as_str().to_string(),
            original_target: dl.original_target.as_str().to_string(),
            resolved_target: dl.resolved_target.as_str().to_string(),
            error_code: dl.reason.as_code().to_string(),
            trace_id: dl.message.trace_id.to_string(),
            created_at: dl.message.created_at,
            // P1: on the drain path the full envelope is in hand, so the id is
            // always available — no `message_json` reparse needed.
            message_id: Some(dl.message.id.to_string()),
        })
        .collect();
    (StatusCode::OK, Json(json!({ "dead_letters": entries })))
}
