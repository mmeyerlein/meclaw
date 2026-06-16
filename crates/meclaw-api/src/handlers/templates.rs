//! GET /colony/templates — Phase 12-B T8.4 (Pure-Read).
//! POST /colony/templates/rescan — Phase 12-B T8.5 (Rescan-Trigger).
//!
//! Read nutzt `ColonyMsg::ReadTemplates`. Rescan reicht den `templates_root`
//! aus `ColonyHandle` an `ColonyMsg::RescanTemplates` weiter — der Pfad ist
//! beim CLI-Start fixiert, kein Request-Body noetig.

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

/// Query-Params fuer `GET /colony/templates`.
#[derive(Debug, Deserialize)]
pub struct TemplatesQuery {
    /// Optional: Filter auf Cell-Type aus `config.json`. Aktuell no-op
    /// (Phase-14-Backlog, siehe `TemplateEntryDto`-Doc).
    #[serde(rename = "type")]
    pub cell_type: Option<String>,
    /// Optional: exakter Match auf `template.json::name`.
    pub name: Option<String>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler fuer `GET /colony/templates`.
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

/// Handler fuer `POST /colony/templates/rescan` — Phase 12-B T8.5.
///
/// Wrappt `ColonyMsg::RescanTemplates`. Der `templates_root` kommt aus
/// `ColonyHandle.templates_root` (vom CLI-Start fixiert). Antwort:
/// `{"rescan": {"status": "ok"}}` — RescanTemplates ack ist `()`,
/// kein Body.
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
    if ack_rx.await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        );
    }
    (StatusCode::OK, Json(json!({ "rescan": {"status": "ok"} })))
}
