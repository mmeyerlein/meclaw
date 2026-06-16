//! GET /colony/graph — Phase 12-B T8.7.
//!
//! Liefert eine scope-gefilterte Nodes+Edges-Snapshot via `ColonyMsg::ReadGraph`.
//! `?scope=<path>` (default `/`). Reply ist nicht ein Array — sondern das volle
//! Schema `{scope, graph_version, nodes, edges}` (Spec Z.410).

use crate::ColonyHandle;
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

/// Query-Params fuer `GET /colony/graph`.
#[derive(Debug, Deserialize)]
pub struct GraphQuery {
    /// Scope-Prefix; default `/` (alles).
    pub scope: Option<String>,
}

/// Handler fuer `GET /colony/graph`.
pub async fn get_graph(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    let scope = Path::new(q.scope.as_deref().unwrap_or("/"));
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadGraph { scope, ack: ack_tx };
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
        Json(json!({
            "graph": {
                "scope": reply.scope,
                "graph_version": reply.graph_version,
                "nodes": reply.nodes,
                "edges": reply.edges,
            }
        })),
    )
}
