//! GET /colony/registry — Phase 12-B T8.1.
//!
//! Liest die in-memory Registry der Colony via `ColonyMsg::ReadRegistry` und
//! liefert eine JSON-Antwort `{"registry": [...]}`. Filter werden 1:1 aus dem
//! Query-String in die Read-Variante uebersetzt; `limit` wird via `clamp_limit`
//! (Default 100, Cap 1000) eingegrenzt.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use meclaw_core::Path;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query-String-Params fuer `GET /colony/registry`.
///
/// `type` ist ein reserviertes Wort in Rust; via `#[serde(rename)]` mappen.
#[derive(Debug, Deserialize)]
pub struct RegistryQuery {
    /// Exakter Pfad-Match (z.B. `?path=/main/llm`).
    pub path: Option<String>,
    /// Prefix-Match auf den Pfad-String (z.B. `?path_prefix=/main`).
    pub path_prefix: Option<String>,
    /// Cell-Type-Filter (z.B. `?type=llm`).
    #[serde(rename = "type")]
    pub cell_type: Option<String>,
    /// Phase-13.5 Lifecycle-3b T8 (F7): active-Filter (z.B. `?active=true`).
    /// `Some(true)` → nur aktive, `Some(false)` → nur inaktive, `None` → alle.
    pub active: Option<bool>,
    /// Hard cap auf zurueckgegebene Eintraege (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler fuer `GET /colony/registry`.
///
/// 503 wenn `colony.inbox.send` oder `ack_rx.await` failed (Colony down).
pub async fn get_registry(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<RegistryQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadRegistry {
        path: q.path.as_deref().map(Path::new),
        path_prefix: q.path_prefix.as_deref().map(Path::new),
        cell_type: q.cell_type,
        active: q.active,
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
    (StatusCode::OK, Json(json!({ "registry": reply.entries })))
}
