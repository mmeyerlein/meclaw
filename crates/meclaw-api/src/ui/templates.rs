//! GET /ui/templates — Phase 12-D T26.
//!
//! Server-rendered templates table (template_id, name, version,
//! filesystem_path). Filter form for name + type. Wraps
//! `ColonyMsg::ReadTemplates` analogously to the 12-B JSON handler. The `type`
//! filter is a no-op today (phase-14 backlog, see the `TemplateEntryDto` docs) —
//! the UI exposes it anyway so the operator is not surprised once it takes
//! effect.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use crate::ui::layout;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::TemplateEntryDto;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query params for `/ui/templates`.
#[derive(Debug, Deserialize, Default)]
pub struct TemplatesUiQuery {
    /// Exact match on `template.json::name`.
    pub name: Option<String>,
    /// Cell-type filter (a no-op today, see the module docs).
    #[serde(rename = "type")]
    pub cell_type: Option<String>,
    /// Hard cap (default 100, max 1000).
    pub limit: Option<usize>,
}

/// Handler `GET /ui/templates`.
pub async fn get_templates_ui(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<TemplatesUiQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadTemplates {
        cell_type: q.cell_type.clone(),
        name: q.name.clone(),
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
        h2 { "Templates (" (reply.entries.len()) ")" }
        (render_table(&reply.entries))
    };
    (
        StatusCode::OK,
        Html(layout("Templates", content).into_string()),
    )
        .into_response()
}

fn render_filter_form(q: &TemplatesUiQuery) -> Markup {
    let name = q.name.clone().unwrap_or_default();
    let cell_type = q.cell_type.clone().unwrap_or_default();
    let limit = q.limit.map(|n| n.to_string()).unwrap_or_default();
    html! {
        form method="get" action="/ui/templates" {
            label { "Name: " input type="text" name="name" value=(name); }
            label { "Type: " input type="text" name="type" value=(cell_type); }
            label { "Limit: " input type="text" name="limit" value=(limit); }
            button type="submit" { "Filter" }
        }
    }
}

fn render_table(entries: &[TemplateEntryDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "No templates registered." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Name" }
                    th { "Version" }
                    th { "Template ID" }
                    th { "Filesystem path" }
                    th { "Author" }
                }
            }
            tbody {
                @for e in entries {
                    tr {
                        td { (e.name) }
                        td { (e.version.clone().unwrap_or_default()) }
                        td { code { (e.template_id) } }
                        td { code { (e.filesystem_path) } }
                        td { (e.author.clone().unwrap_or_default()) }
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
