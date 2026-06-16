//! HTML-Error-Pages für `/ui/*`. Spec-Klarstellung: bei Reads gibt es **kein
//! 422** — `422 Unprocessable Entity` ist semantisch POST-Mutation-only. UI-
//! Validierungs-Fehler für Read-Endpoints (z.B. invalides UUID-Format)
//! mappen auf 400 `Bad Request` + HTML-Page.
//!
//! 500 für serverseitige Probleme (Colony-Inbox down, ack-Drop) ist die
//! generische "etwas ist schief", ohne dem Operator Stacktraces zu zeigen.

use crate::ui::layout;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use maud::html;

/// Render `400 Bad Request` mit Detail-String.
pub(crate) fn render_400(detail: &str) -> Response {
    let content = html! {
        p class="error" { "400 Bad Request" }
        p { (detail) }
        p { a href="/ui/" { "Zurück zum Dashboard" } }
    };
    (
        StatusCode::BAD_REQUEST,
        Html(layout("400 Bad Request", content).into_string()),
    )
        .into_response()
}

/// Render `500 Internal Server Error` mit Detail-String.
pub(crate) fn render_500(detail: &str) -> Response {
    let content = html! {
        p class="error" { "500 Internal Server Error" }
        p { (detail) }
        p { a href="/ui/" { "Zurück zum Dashboard" } }
    };
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(layout("500 Internal Server Error", content).into_string()),
    )
        .into_response()
}
