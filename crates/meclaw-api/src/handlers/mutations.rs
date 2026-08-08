//! GET /colony/mutations — phase 12-B T8.8 (audit read).
//! POST /colony/mutations — phase 12-B T9 (submission, 200/422 mapping).
//!
//! GET is a pure read-only audit view on colony.db::mutation_log via
//! `ColonyMsg::ReadMutationsAudit`. POST passes the mutation diff body through as
//! `ColonyMsg::Mutation` and maps the `MutationOutcome` reply onto the HTTP
//! status: 200 on Committed, 422 on Rejected. Spec l.1660: the full `Rejected`
//! detail is preserved in the body's `mutation` slot.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query params for `GET /colony/mutations`.
#[derive(Debug, Deserialize)]
pub struct MutationsQuery {
    /// Optional: only rows with `created_at >= since` (Unix seconds).
    pub since: Option<i64>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler for `GET /colony/mutations` — audit read.
pub async fn get_mutations_audit(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<MutationsQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadMutationsAudit {
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
    (StatusCode::OK, Json(json!({ "mutations": reply.entries })))
}

/// Handler for `POST /colony/mutations` — submission.
///
/// Takes a mutation diff in the JSON body, forwards it as `ColonyMsg::Mutation`
/// to the colony inbox and maps the `MutationOutcome` reply onto the HTTP status:
/// - `Committed` → 200 with `{"mutation": {"outcome": "committed", "id": ...}}`.
/// - `Rejected`  → 422 with the full detail (outcome/id/error_code/details) in
///   the `mutation` slot. Spec l.1660.
///
/// `trace_id` is minted fresh here via `Uuid::now_v7` (HTTP starts a new trace);
/// `parent_message_id` is `Uuid::nil()` (no parent — the POST is the root of the
/// trace).
pub async fn post_mutation(
    State(colony): State<Arc<ColonyHandle>>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::Mutation {
        payload,
        reply_to: None,
        trace_id: meclaw_core::Uuid::now_v7(),
        parent_message_id: meclaw_core::Uuid::nil(),
        ack: ack_tx,
    };
    if colony.inbox.send(msg).await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        );
    }
    let outcome = match ack_rx.await {
        Ok(o) => o,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "colony unavailable" })),
            );
        }
    };
    match outcome {
        MutationOutcome::Committed { id } => (
            StatusCode::OK,
            Json(json!({ "mutation": { "outcome": "committed", "id": id } })),
        ),
        MutationOutcome::Rejected {
            id,
            error_code,
            details,
        } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "mutation": {
                    "outcome": "rejected",
                    "id": id,
                    "error_code": error_code,
                    "details": details,
                }
            })),
        ),
    }
}
