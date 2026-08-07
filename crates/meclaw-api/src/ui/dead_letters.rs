//! GET /ui/dead_letters — Phase 12-D T24.
//!
//! Pure-Read der in-memory Dead-Letter-Queue via
//! `ColonyMsg::ReadDeadLetters`. Render als HTML-Tabelle mit
//! `error_code` + Pfaden. Kein Drain-Knopf (Mutation-Forms sind
//! per Anti-Scope verboten).

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use crate::ui::layout;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::DeadLetterDto;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query-Params fuer `/ui/dead_letters`.
#[derive(Debug, Deserialize, Default)]
pub struct DeadLettersUiQuery {
    /// Filter auf `error_code`.
    pub error_code: Option<String>,
    /// Hard-Cap (Default 100, Max 1000).
    pub limit: Option<usize>,
}

/// Handler `GET /ui/dead_letters`.
pub async fn get_dead_letters_ui(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<DeadLettersUiQuery>,
) -> impl IntoResponse {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadDeadLetters {
        since: None,
        error_code: q.error_code.clone(),
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
        h2 { "Dead Letters (" (reply.entries.len()) ")" }
        (render_table(&reply.entries))
    };
    (
        StatusCode::OK,
        Html(layout("Dead Letters", content).into_string()),
    )
        .into_response()
}

fn render_filter_form(q: &DeadLettersUiQuery) -> Markup {
    let error_code = q.error_code.clone().unwrap_or_default();
    let limit = q.limit.map(|n| n.to_string()).unwrap_or_default();
    html! {
        form method="get" action="/ui/dead_letters" {
            label { "Error-Code: " input type="text" name="error_code" value=(error_code); }
            label { "Limit: " input type="text" name="limit" value=(limit); }
            button type="submit" { "Filter" }
        }
    }
}

fn render_table(entries: &[DeadLetterDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "Keine Dead Letters." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Error-Code" }
                    th { "Sender" }
                    th { "Original Target" }
                    th { "Resolved Target" }
                    th { "Original" }
                }
            }
            tbody {
                @for e in entries {
                    tr {
                        td class="error" { (e.error_code) }
                        td { code { (e.sender_path) } }
                        td { code { (e.original_target) } }
                        td { code { (e.resolved_target) } }
                        // P1: pivot into the message browser. Message-exact when
                        // the persisted envelope carried an id, trace-exact
                        // otherwise — never a dead end.
                        td {
                            @match &e.message_id {
                                Some(id) => a href=(format!("/ui/message?id={id}")) { "Message" },
                                None => a href=(format!("/ui/messages?trace_id={}", e.trace_id)) { "Trace" },
                            }
                        }
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
