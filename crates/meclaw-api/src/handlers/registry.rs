//! GET /colony/registry — Phase 12-B T8.1.
//!
//! Reads the colony's in-memory registry via `ColonyMsg::ReadRegistry` and
//! returns a JSON response `{"registry": [...]}`. Filters are translated 1:1 from
//! the query string into the read variant; `limit` is bounded via `clamp_limit`
//! (default 100, cap 1000).

use crate::ColonyHandle;
use crate::handlers::{clamp_limit, clamp_read_tag};
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

/// Query-string params for `GET /colony/registry`.
///
/// `type` is a reserved word in Rust; mapped via `#[serde(rename)]`.
#[derive(Debug, Deserialize)]
pub struct RegistryQuery {
    /// Exact path match (e.g. `?path=/main/llm`).
    pub path: Option<String>,
    /// Prefix match on the path string (e.g. `?path_prefix=/main`).
    pub path_prefix: Option<String>,
    /// Cell-type filter (e.g. `?type=llm`).
    #[serde(rename = "type")]
    pub cell_type: Option<String>,
    /// Phase-13.5 lifecycle-3b T8 (F7): active filter (e.g. `?active=true`).
    /// `Some(true)` → active only, `Some(false)` → inactive only, `None` → all.
    pub active: Option<bool>,
    /// Hard cap on returned entries (default 100, max 1000).
    pub limit: Option<usize>,
    /// Opaque caller correlation token, echoed verbatim beside the list
    /// (max 64 chars).
    pub tag: Option<String>,
}

/// Handler for `GET /colony/registry`.
///
/// 503 when `colony.inbox.send` or `ack_rx.await` fails (colony down).
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
    let mut out = json!({ "registry": reply.entries });
    if let (Some(t), Some(obj)) = (clamp_read_tag(q.tag), out.as_object_mut()) {
        obj.insert("tag".into(), json!(t));
    }
    (StatusCode::OK, Json(out))
}
