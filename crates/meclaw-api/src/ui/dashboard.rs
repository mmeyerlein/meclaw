//! GET /ui/ — Phase 12-D T21 Dashboard.
//!
//! Aggregiert drei `ColonyMsg::Read*`-Reads (Registry, DeadLetters,
//! Trace mit `only_error=true`) und rendert sie als drei HTML-Sections.
//!
//! **Explizit kein konsistenter Snapshot** (Spec Z.468): die drei Reads
//! laufen sequentiell, zwischen ihnen darf der Colony-State weiterlaufen.
//! Disclaimer rendert direkt in der Seite.

use crate::ColonyHandle;
use crate::ui::layout;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{DeadLetterDto, MessageLogDto, RegistryEntryDto};
use std::sync::Arc;
use tokio::sync::oneshot;

/// Dashboard-Handler. Bei Colony-Unavailable: 503 + minimaler HTML-Error.
pub async fn get_dashboard(State(colony): State<Arc<ColonyHandle>>) -> impl IntoResponse {
    // 1) Registry — alle Cells (Default-Limit 100).
    let registry = match read_registry(&colony).await {
        Ok(r) => r,
        Err(()) => return colony_unavailable(),
    };
    // 2) Dead Letters — non-destruktiv.
    let dead_letters = match read_dead_letters(&colony).await {
        Ok(r) => r,
        Err(()) => return colony_unavailable(),
    };
    // 3) Recent Errors — Trace mit only_error=true.
    let errors = match read_errors(&colony).await {
        Ok(r) => r,
        Err(()) => return colony_unavailable(),
    };

    let content = html! {
        p {
            "Aggregierte Sicht aus drei Reads. "
            strong { "Hinweis: " }
            "Dies ist " strong { "kein konsistenter Snapshot" } " — "
            "die drei Reads laufen sequentiell, zwischen ihnen darf der "
            "Colony-State weiterlaufen."
        }

        h2 { "Cells (" (registry.len()) ")" }
        (render_registry_section(&registry))

        h2 { "Dead Letters (" (dead_letters.len()) ")" }
        (render_dead_letters_section(&dead_letters))

        h2 { "Recent Errors (" (errors.len()) ")" }
        (render_errors_section(&errors))
    };

    (
        StatusCode::OK,
        Html(layout("Dashboard", content).into_string()),
    )
        .into_response()
}

fn render_registry_section(entries: &[RegistryEntryDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "Keine Cells registriert." } };
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

fn render_dead_letters_section(entries: &[DeadLetterDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "Keine Dead Letters." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Sender" }
                    th { "Original Target" }
                    th { "Resolved Target" }
                    th { "Error" }
                }
            }
            tbody {
                @for e in entries {
                    tr {
                        td { code { (e.sender_path) } }
                        td { code { (e.original_target) } }
                        td { code { (e.resolved_target) } }
                        td class="error" { (e.error_code) }
                    }
                }
            }
        }
    }
}

fn render_errors_section(entries: &[MessageLogDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "Keine Error-Hops im Trace-Log." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Message-ID" }
                    th { "From" }
                    th { "To" }
                    th { "Headers" }
                }
            }
            tbody {
                @for e in entries {
                    tr {
                        td { code { (e.id) } }
                        td { code { (e.from_path) } }
                        td { code { (e.to_path) } }
                        td class="error" { (truncate(&e.headers_json, 200)) }
                    }
                }
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

async fn read_registry(colony: &Arc<ColonyHandle>) -> Result<Vec<RegistryEntryDto>, ()> {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadRegistry {
        path: None,
        path_prefix: None,
        cell_type: None,
        active: None,
        limit: 100,
        ack: ack_tx,
    };
    colony.inbox.send(msg).await.map_err(|_| ())?;
    let reply = ack_rx.await.map_err(|_| ())?;
    Ok(reply.entries)
}

async fn read_dead_letters(colony: &Arc<ColonyHandle>) -> Result<Vec<DeadLetterDto>, ()> {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadDeadLetters {
        since: None,
        error_code: None,
        limit: 100,
        ack: ack_tx,
    };
    colony.inbox.send(msg).await.map_err(|_| ())?;
    let reply = ack_rx.await.map_err(|_| ())?;
    Ok(reply.entries)
}

async fn read_errors(colony: &Arc<ColonyHandle>) -> Result<Vec<MessageLogDto>, ()> {
    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadTrace {
        trace_id: None,
        path_prefix: None,
        correlation_id: None,
        only_error: true,
        since: None,
        limit: 100,
        ack: ack_tx,
    };
    colony.inbox.send(msg).await.map_err(|_| ())?;
    let reply = ack_rx.await.map_err(|_| ())?;
    Ok(reply.entries)
}

fn colony_unavailable() -> axum::response::Response {
    let content = html! { p class="error" { "Colony unavailable." } };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Html(layout("Service Unavailable", content).into_string()),
    )
        .into_response()
}
