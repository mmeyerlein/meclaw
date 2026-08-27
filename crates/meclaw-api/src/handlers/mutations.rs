//! GET /colony/mutations — phase 12-B T8.8 (audit read).
//! POST /colony/mutations — phase 12-B T9 (submission, 200/422 mapping).
//!
//! GET is a pure read-only audit view on colony.db::mutation_log via
//! `ColonyMsg::ReadMutationsAudit`. POST passes the mutation diff body through as
//! `ColonyMsg::MutationDoor` and maps the verdict onto the HTTP status: 200 on
//! committed, 422 otherwise. Spec l.1660: the full `Rejected` detail is
//! preserved in the body's `mutation` slot. Since GH #422 the same door also
//! takes a MANIFEST body and answers in its own `manifest` slot.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::{ColonyMsg, mutation_door_reply};
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
/// Takes a mutation body in the JSON body, forwards it VERBATIM as
/// `ColonyMsg::MutationDoor` to the colony inbox, and maps the verdict onto the
/// HTTP status. Two body forms arrive here and the handler tells neither apart
/// — the colony does that, in one place, so the message door and this one
/// cannot drift (GH #422):
///
/// - single form, `Committed` → 200, `{"mutation": {"outcome": "committed", "id": …}}`;
/// - single form, `Rejected`  → 422, the full detail in the `mutation` slot (spec l.1660);
/// - manifest, `committed`    → 200, `{"manifest": {"outcome": "committed", "applied": k, "ids": […]}}`;
/// - manifest, `rejected`     → 422, the same slot plus `failed_at` / `remaining`;
/// - a body that meant to be a manifest and could not be read → 422, `error_code: "schema"`.
///
/// The reply JSON is rendered by `meclaw_colony::mutation_door_reply`, the same
/// renderer the EDA door uses — so a caller gets the same document whichever
/// way the body arrived. GH #293 still holds: `violations` is not on this wire.
///
/// `trace_id` is minted fresh here via `Uuid::now_v7` (HTTP starts a new trace);
/// `parent_message_id` is `Uuid::nil()` (no parent — the POST is the root of the
/// trace).
pub async fn post_mutation(
    State(colony): State<Arc<ColonyHandle>>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::MutationDoor {
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
    let status = if outcome.is_committed() {
        StatusCode::OK
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    (status, Json(mutation_door_reply(&outcome)))
}
