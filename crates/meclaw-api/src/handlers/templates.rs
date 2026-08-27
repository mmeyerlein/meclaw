//! GET /colony/templates — phase 12-B T8.4 (pure read).
//! POST /colony/templates/rescan — phase 12-B T8.5 (rescan trigger).
//!
//! The read uses `ColonyMsg::ReadTemplates`. The rescan forwards the
//! `templates_root` from `ColonyHandle` to `ColonyMsg::RescanTemplates` — the
//! path is fixed at CLI start, so no request body is needed.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query params for `GET /colony/templates`.
#[derive(Debug, Deserialize)]
pub struct TemplatesQuery {
    /// Optional: exact match on the cell type declared in the template's
    /// `config.json` (W13 hardening — this used to be accepted and ignored).
    /// An unknown type yields an empty list, not an error.
    #[serde(rename = "type")]
    pub cell_type: Option<String>,
    /// Optional: exact match on `template.json::name`.
    pub name: Option<String>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler for `GET /colony/templates`.
pub async fn get_templates(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<TemplatesQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadTemplates {
        cell_type: q.cell_type,
        name: q.name,
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
    (StatusCode::OK, Json(json!({ "templates": reply.entries })))
}

/// Handler for `POST /colony/templates/rescan` — phase 12-B T8.5.
///
/// Wraps `ColonyMsg::RescanTemplates`. The `templates_root` comes from
/// `ColonyHandle.templates_root` (fixed at CLI start).
///
/// GH #440: the ack carries the scan outcome, so this door answers `200` with
/// `{"rescan": {"status": "ok"}}` or `422` with
/// `{"rescan": {"status": "error", "error": "<the scanner's own words>"}}`.
/// Until then the ack was `()` and there was exactly one return value, which
/// said `ok` even for a scan that had aborted on a duplicate name.
pub async fn post_rescan(State(colony): State<Arc<ColonyHandle>>) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::RescanTemplates {
        templates_root: colony.templates_root.clone(),
        ack: ack_tx,
    };
    if colony.inbox.send(msg).await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        );
    }
    match ack_rx.await {
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        ),
        Ok(Ok(())) => (StatusCode::OK, Json(json!({ "rescan": {"status": "ok"} }))),
        // GH #440: an aborted scan is not an `ok`. 422 like a rejected
        // mutation — the request was well formed, the tree was not.
        Ok(Err(error)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "rescan": {"status": "error", "error": error} })),
        ),
    }
}
