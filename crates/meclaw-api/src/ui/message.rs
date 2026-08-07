//! `GET /ui/message?id=…` — P1 message browser, detail view.
//!
//! One message, fully inspectable: envelope fields, both header compartments
//! apart (`context` vs `hop`), the complete payload, and the pivots that make
//! the log walkable — trace, parent, children, correlation, `reply_to`, and the
//! registry entries behind `from_path`/`to_path`.
//!
//! Blob-kind bodies are resolved only on `?blob=1`. Opening a message must not
//! silently pull an arbitrarily large body off disk.
//!
//! Read-only: two reads, no writes, no forms.

use crate::ColonyHandle;
use crate::handlers::message_log::DEFAULT_SCAN_BUDGET;
use crate::ui::layout;
use crate::ui::message_render::{pretty_json, render_headers_split};
use crate::ui::messages::format_ts;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_colony::blob::DiskBlobStore;
use meclaw_core::Uuid;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Children listed inline on the detail page before deferring to the list view.
const CHILDREN_LIMIT: usize = 100;

/// Query params for `/ui/message`.
#[derive(Debug, Deserialize, Default)]
pub struct MessageUiQuery {
    /// The message id (UUID string).
    pub id: Option<String>,
    /// `?blob=1` resolves a `blob`-kind body through the blob store.
    pub blob: Option<u8>,
}

/// Handler `GET /ui/message`.
pub async fn get_message_ui(
    State(colony): State<Arc<ColonyHandle>>,
    State(blob_store): State<Arc<DiskBlobStore>>,
    Query(q): Query<MessageUiQuery>,
) -> impl IntoResponse {
    let Some(raw_id) = q.id.as_deref().filter(|s| !s.is_empty()) else {
        let content = html! {
            p { "Keine Message-ID angegeben. " a href="/ui/messages" { "Zur Message-Liste" } "." }
        };
        return (
            StatusCode::OK,
            Html(layout("Message", content).into_string()),
        )
            .into_response();
    };
    if Uuid::parse_str(raw_id).is_err() {
        return crate::ui::errors::render_400("id is not a valid UUID");
    }

    // Read 1 — the message itself (PRIMARY KEY lookup).
    let message = match read_messages(
        &colony,
        MessageLogFilter {
            id: Some(raw_id.to_string()),
            limit: 1,
            scan_budget: DEFAULT_SCAN_BUDGET,
            ..Default::default()
        },
    )
    .await
    {
        Ok(mut rows) => rows.pop(),
        Err(()) => return crate::ui::errors::render_500("colony unavailable"),
    };
    let Some(msg) = message else {
        let content = html! {
            p class="empty" { "Keine Message gefunden für " code { (raw_id) } "." }
            p { a href="/ui/messages" { "Zur Message-Liste" } }
        };
        return (
            StatusCode::OK,
            Html(layout("Message", content).into_string()),
        )
            .into_response();
    };

    // Read 2 — direct children (indexed on parent_message_id).
    let children = match read_messages(
        &colony,
        MessageLogFilter {
            parent_message_id: Some(msg.id.clone()),
            limit: CHILDREN_LIMIT,
            scan_budget: DEFAULT_SCAN_BUDGET,
            ..Default::default()
        },
    )
    .await
    {
        Ok(rows) => rows,
        Err(()) => return crate::ui::errors::render_500("colony unavailable"),
    };

    let blob_section = if msg.body_kind == "blob" {
        Some(render_blob_section(&msg, q.blob == Some(1), &blob_store).await)
    } else {
        None
    };

    let content = html! {
        p { a href="/ui/messages" { "← Message-Liste" } }
        (render_envelope(&msg))
        h2 { "Headers" }
        (render_headers_split(&msg.headers_json))
        h2 { "Body" }
        (render_body(&msg))
        @if let Some(section) = &blob_section { (section) }
        h2 { "Pivots" }
        (render_pivots(&msg))
        h2 { "Kinder (" (children.len()) ")" }
        (render_children(&msg, &children))
    };
    (
        StatusCode::OK,
        Html(layout("Message", content).into_string()),
    )
        .into_response()
}

/// Issue one `ReadMessages` request. `Err(())` == colony unavailable.
async fn read_messages(
    colony: &ColonyHandle,
    filter: MessageLogFilter,
) -> Result<Vec<MessageLogDto>, ()> {
    let (ack_tx, ack_rx) = oneshot::channel();
    if colony
        .inbox
        .send(ColonyMsg::ReadMessages {
            filter,
            ack: ack_tx,
        })
        .await
        .is_err()
    {
        return Err(());
    }
    ack_rx.await.map(|r| r.entries).map_err(|_| ())
}

fn render_envelope(msg: &MessageLogDto) -> Markup {
    let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".into());
    html! {
        table {
            tbody {
                tr { th { "id" } td { code { (msg.id) } } }
                tr { th { "created_at" } td { (format_ts(msg.created_at)) " UTC (" (msg.created_at) ")" } }
                tr { th { "from_path" } td { code { (msg.from_path) } } }
                tr { th { "to_path" } td { code { (msg.to_path) } } }
                tr { th { "ttl" } td { (msg.ttl) } }
                tr { th { "body_kind" } td { (msg.body_kind) } }
                tr { th { "trace_id" } td { code { (msg.trace_id) } } }
                tr { th { "parent_message_id" } td { code { (opt(&msg.parent_message_id)) } } }
                tr { th { "correlation_id" } td { code { (opt(&msg.correlation_id)) } } }
                tr { th { "reply_to" } td { code { (opt(&msg.reply_to)) } } }
            }
        }
    }
}

fn render_body(msg: &MessageLogDto) -> Markup {
    match msg.body_payload.as_deref() {
        None => html! { p class="empty" { "Kein Body." } },
        Some(payload) if msg.body_kind == "blob" => html! {
            p { "Blob-Body. Blob-ID: " code { (payload) } }
        },
        Some(payload) => match pretty_json(payload) {
            Some(pretty) => html! { pre { (pretty) } },
            None => html! {
                p class="empty" { "Payload ist kein gültiges JSON — Rohwert:" }
                pre { (payload) }
            },
        },
    }
}

/// Lazy blob resolution: a link when not requested, the resolved body when it is.
/// Errors report their class only — never the store path or partial content.
async fn render_blob_section(
    msg: &MessageLogDto,
    resolve: bool,
    blob_store: &DiskBlobStore,
) -> Markup {
    if !resolve {
        return html! {
            p {
                a href=(format!("/ui/message?id={}&blob=1", msg.id)) {
                    "Blob-Body laden"
                }
                " (wird nicht automatisch geladen)"
            }
        };
    }
    let Some(raw) = msg.body_payload.as_deref() else {
        return html! { p class="error" { "Blob-Body nicht auflösbar: missing_blob_id" } };
    };
    let Ok(blob_id) = Uuid::parse_str(raw) else {
        return html! { p class="error" { "Blob-Body nicht auflösbar: malformed_blob_id" } };
    };
    match blob_store.read_body(blob_id).await {
        Ok(body) => {
            let pretty = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
            html! {
                h3 { "Blob-Body" }
                pre { (pretty) }
            }
        }
        Err(_) => html! { p class="error" { "Blob-Body nicht auflösbar: blob_unreadable" } },
    }
}

fn render_pivots(msg: &MessageLogDto) -> Markup {
    html! {
        ul {
            li {
                "Trace: "
                a href=(format!("/ui/trace?trace_id={}", msg.trace_id)) { "Hop-Baum" }
                " · "
                a href=(format!("/ui/messages?trace_id={}", msg.trace_id)) { "als Liste" }
            }
            @if let Some(parent) = &msg.parent_message_id {
                li { "Parent: " a href=(format!("/ui/message?id={parent}")) { code { (parent) } } }
            } @else {
                li class="empty" { "Parent: — (Source-Message)" }
            }
            li {
                "Kinder: "
                a href=(format!("/ui/messages?parent_message_id={}", msg.id)) { "alle anzeigen" }
            }
            @if let Some(corr) = &msg.correlation_id {
                li {
                    "Correlation: "
                    a href=(format!("/ui/messages?correlation_id={corr}")) { code { (corr) } }
                }
            } @else {
                li class="empty" { "Correlation: —" }
            }
            @if let Some(reply_to) = &msg.reply_to {
                li {
                    "reply_to: "
                    a href=(format!("/ui/messages?to_path_prefix={reply_to}")) { code { (reply_to) } }
                }
            } @else {
                li class="empty" { "reply_to: —" }
            }
            li {
                "Registry: "
                a href=(format!("/ui/registry?path_prefix={}", msg.from_path)) { "from " code { (msg.from_path) } }
                " · "
                a href=(format!("/ui/registry?path_prefix={}", msg.to_path)) { "to " code { (msg.to_path) } }
            }
        }
    }
}

fn render_children(msg: &MessageLogDto, children: &[MessageLogDto]) -> Markup {
    if children.is_empty() {
        return html! { p class="empty" { "Keine Folge-Messages." } };
    }
    html! {
        ul {
            @for child in children {
                li {
                    a href=(format!("/ui/message?id={}", child.id)) { code { (child.to_path) } }
                    " · " (format_ts(child.created_at)) " UTC"
                }
            }
        }
        @if children.len() == CHILDREN_LIMIT {
            p class="error" {
                "Nur die ersten " (CHILDREN_LIMIT) " Kinder gezeigt — "
                a href=(format!("/ui/messages?parent_message_id={}", msg.id)) { "vollständige Liste" }
            }
        }
    }
}
