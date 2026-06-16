//! GET /ui/registry — Phase 12-D T22.
//!
//! Server-rendered Registry-Tabelle mit GET-Filter-Form. Wrappt
//! `ColonyMsg::ReadRegistry` (gleich wie Phase-12-B JSON-Handler).
//!
//! Filter im Form: `path_prefix`, `type`, `limit`. Eingegebene Werte
//! erscheinen als `value=`-Defaults wieder im Form (Round-Trip).

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use crate::ui::layout;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::RegistryEntryDto;
use meclaw_core::Path;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query-Params fuer `/ui/registry`. Spiegelt 1:1 die JSON-Variante.
#[derive(Debug, Deserialize, Default)]
pub struct RegistryUiQuery {
    /// Prefix-Match auf den Pfad.
    pub path_prefix: Option<String>,
    /// Cell-Type-Filter; `type` ist Rust-Keyword, daher `rename`.
    #[serde(rename = "type")]
    pub cell_type: Option<String>,
    /// Hard-Cap (Default 100, Max 1000).
    pub limit: Option<usize>,
}

/// Handler `GET /ui/registry`.
pub async fn get_registry_ui(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<RegistryUiQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadRegistry {
        path: None,
        path_prefix: q.path_prefix.as_deref().map(Path::new),
        cell_type: q.cell_type.clone(),
        active: None,
        limit: clamp_limit(q.limit),
        ack: ack_tx,
    };
    if colony.inbox.send(msg).await.is_err() {
        return colony_unavailable();
    }
    let reply = match ack_rx.await {
        Ok(r) => r,
        Err(_) => return colony_unavailable(),
    };

    let content = html! {
        (render_filter_form(&q))
        h2 { "Cells (" (reply.entries.len()) ")" }
        (render_table(&reply.entries))
    };
    (
        StatusCode::OK,
        Html(layout("Registry", content).into_string()),
    )
        .into_response()
}

fn render_filter_form(q: &RegistryUiQuery) -> Markup {
    let path_prefix = q.path_prefix.clone().unwrap_or_default();
    let cell_type = q.cell_type.clone().unwrap_or_default();
    let limit = q.limit.map(|n| n.to_string()).unwrap_or_default();
    html! {
        form method="get" action="/ui/registry" {
            label { "Path-Prefix: " input type="text" name="path_prefix" value=(path_prefix); }
            label { "Type: " input type="text" name="type" value=(cell_type); }
            label { "Limit: " input type="text" name="limit" value=(limit); }
            button type="submit" { "Filter" }
        }
    }
}

fn render_table(entries: &[RegistryEntryDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "Keine Cells gefunden." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Path" }
                    th { "Type" }
                    th { "Cell-ID" }
                }
            }
            tbody {
                @for e in entries {
                    tr {
                        td { code { (e.path) } }
                        td { (e.cell_type) }
                        td { code { (e.cell_id) } }
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
