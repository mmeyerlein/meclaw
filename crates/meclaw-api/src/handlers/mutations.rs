//! GET /colony/mutations — Phase 12-B T8.8 (Audit-Read).
//! POST /colony/mutations — Phase 12-B T9 (Submission, 200/422 mapping).
//!
//! GET ist ein pure read-only Audit-View auf colony.db::mutation_log via
//! `ColonyMsg::ReadMutationsAudit`. POST reicht den Mutation-Diff-Body in
//! `ColonyMsg::Mutation` durch und mapt die `MutationOutcome`-Antwort auf
//! HTTP-Status: 200 bei Committed, 422 bei Rejected. Spec Z.1660: die volle
//! `Rejected`-Detail bleibt im `mutation`-Slot des Bodies erhalten.

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

/// Query-Params fuer `GET /colony/mutations`.
#[derive(Debug, Deserialize)]
pub struct MutationsQuery {
    /// Optional: nur Rows mit `created_at >= since` (Unix-Sek).
    pub since: Option<i64>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler fuer `GET /colony/mutations` — Audit-Read.
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

/// Handler fuer `POST /colony/mutations` — Submission.
///
/// Nimmt einen Mutation-Diff im JSON-Body entgegen, leitet ihn als
/// `ColonyMsg::Mutation` an die Colony-Inbox weiter und mapt die
/// `MutationOutcome`-Antwort auf HTTP-Status:
/// - `Committed` → 200 mit `{"mutation": {"outcome": "committed", "id": ...}}`.
/// - `Rejected`  → 422 mit voller Detail (outcome/id/error_code/details) im
///   `mutation`-Slot. Spec Z.1660.
///
/// `trace_id` wird hier frisch via `Uuid::now_v7` erzeugt (HTTP startet einen
/// neuen Trace); `parent_message_id` ist `Uuid::nil()` (kein Parent — der
/// POST ist der Root des Traces).
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
