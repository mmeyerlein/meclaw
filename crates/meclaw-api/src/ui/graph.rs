//! GET /ui/graph — Phase 12-D T23.
//!
//! Scope-filtered nodes+edges view. Wraps `ColonyMsg::ReadGraph` analogously to
//! the 12-B JSON handler. `?scope=<path>` (default `/`).
//!
//! Nodes as a `<ul>`, edges as a `<table>` (from / to / id).

use crate::ColonyHandle;
use crate::ui::layout;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{GraphEdgeDto, GraphNodeDto};
use meclaw_core::Path;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query params for `/ui/graph`.
#[derive(Debug, Deserialize, Default)]
pub struct GraphUiQuery {
    /// Scope prefix; default `/`.
    pub scope: Option<String>,
}

/// Handler `GET /ui/graph`.
pub async fn get_graph_ui(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<GraphUiQuery>,
) -> impl IntoResponse {
    let scope_str = q.scope.clone().unwrap_or_else(|| "/".to_string());
    let scope = Path::new(&scope_str);
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadGraph { scope, ack: ack_tx };
    if colony.inbox.send(msg).await.is_err() {
        return colony_unavailable();
    }
    let reply = match ack_rx.await {
        Ok(r) => r,
        Err(_) => return colony_unavailable(),
    };

    let content = html! {
        (render_scope_form(&scope_str))
        p { "Scope: " code { (reply.scope) } " · Graph-Version: " code { (reply.graph_version) } }

        h2 { "Nodes (" (reply.nodes.len()) ")" }
        (render_nodes(&reply.nodes))

        h2 { "Edges (" (reply.edges.len()) ")" }
        (render_edges(&reply.edges))
    };
    (StatusCode::OK, Html(layout("Graph", content).into_string())).into_response()
}

fn render_scope_form(scope: &str) -> Markup {
    html! {
        form method="get" action="/ui/graph" {
            label { "Scope: " input type="text" name="scope" value=(scope); }
            button type="submit" { "Filter" }
        }
    }
}

fn render_nodes(nodes: &[GraphNodeDto]) -> Markup {
    if nodes.is_empty() {
        return html! { p class="empty" { "Keine Nodes im Scope." } };
    }
    html! {
        ul {
            @for n in nodes {
                li { code { (n.path) } " — " (n.cell_type) }
            }
        }
    }
}

fn render_edges(edges: &[GraphEdgeDto]) -> Markup {
    if edges.is_empty() {
        return html! { p class="empty" { "Keine Edges im Scope." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Edge-ID" }
                    th { "From" }
                    th { "To" }
                }
            }
            tbody {
                @for e in edges {
                    tr {
                        td { code { (e.id) } }
                        td { code { (e.from) } }
                        td { code { (e.to) } }
                    }
                }
            }
        }
    }
}

fn colony_unavailable() -> axum::response::Response {
    let content = html! { p class="error" { "Colony unavailable." } };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Html(layout("Service Unavailable", content).into_string()),
    )
        .into_response()
}
